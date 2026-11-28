//! Disqro's view of Waveform Native's shared evidence store.
//!
//! The engine keys evidence by source identity and revision. Disqro maps
//! each validated `Source` (path, file stamp, native object, rate, frames)
//! to an exact engine `SourceKey` and keeps its playback generation fence for
//! in-flight work: a result computed for an older generation is abandoned.
use crate::{
    cache::Cache,
    timeline::Source,
    waveform_source::{self, Evidence as RequestEvidence, Reader},
};
#[cfg(test)]
use std::ops::Range;
#[cfg(feature = "qualification")]
use std::{
    cell::RefCell,
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    sync::OnceLock,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};
use waveform::{
    FrameRange, SampleBlock, SourceKey, planning,
    store::{self, Budget, DecodeLatency, EvidenceStore, Request, Reservation},
};

pub use waveform::store::Coverage;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    source: Arc<Source>,
    request: Request,
}

pub struct Work {
    reservation: Reservation,
    key: Key,
}

pub enum Tile {
    Evidence(Arc<waveform::EvidencePage>),
    Samples(Arc<SampleBlock>),
}

impl Key {
    fn new(source: &Arc<Source>, request: Request) -> Self {
        Self {
            source: source.clone(),
            request,
        }
    }

    #[cfg(test)]
    pub fn samples(source: &Arc<Source>, range: Range<usize>) -> Vec<Self> {
        if !source.available {
            return vec![];
        }
        let Ok(range) = FrameRange::new(range.start as u64, range.end.max(range.start) as u64)
        else {
            return vec![];
        };
        planning::sample_blocks(source.frames as u64, range)
            .into_iter()
            .map(|block| Self::new(source, Request::Samples(block)))
            .collect()
    }

    #[cfg(test)]
    pub fn geometry(
        source: &Arc<Source>,
        playable: Range<usize>,
        range: Range<usize>,
        pixels: f64,
        seconds: f64,
    ) -> Vec<Self> {
        let (Ok(rate), Ok(segment)) = (
            waveform::SampleRate::new(source.rate),
            FrameRange::new(playable.start as u64, playable.end as u64),
        ) else {
            return vec![];
        };
        if !source.available || segment.is_empty() {
            return vec![];
        }
        planning::detail_requests(rate, segment, range, pixels, seconds)
            .into_iter()
            .map(|mut request| {
                request.plan = request
                    .plan
                    .with_channel_power(waveform_source::CHANNEL_POWER);
                Self::new(source, Request::Evidence(request))
            })
            .collect()
    }

    pub(crate) fn load(
        &self,
        cache: Option<&Cache>,
        reader: &mut Option<Reader>,
        current: &impl Fn() -> bool,
    ) -> anyhow::Result<Tile> {
        Ok(match self.request {
            Request::Evidence(request) => {
                let evidence = RequestEvidence {
                    playable: request.segment.start as usize..request.segment.end as usize,
                    plan: request.plan,
                };
                Tile::Evidence(Arc::new(waveform_source::load(
                    &self.source,
                    &evidence,
                    cache,
                    reader,
                    current,
                )?))
            }
            Request::Samples(block) => {
                let range = block.start as usize..block.end as usize;
                let frames = waveform_source::samples(&self.source, range, reader, current)?;
                Tile::Samples(Arc::new(SampleBlock::new(
                    block.start,
                    frames.into_iter().map(|f| [f.left, f.right]).collect(),
                )?))
            }
        })
    }
}

impl Work {
    pub fn reservation(&self) -> Reservation {
        self.reservation
    }

    pub fn load(
        &self,
        cache: Option<&Cache>,
        reader: &mut Option<Reader>,
        current: &impl Fn() -> bool,
    ) -> anyhow::Result<Tile> {
        self.key.load(cache, reader, current)
    }
}

/// Evidence for one view, in the engine's representation.
pub type Evidence = store::Evidence;

/// The engine store plus Disqro's playback generation fence.
pub struct Store {
    revision: u64,
    evidence: EvidenceStore,
    sources: HashMap<PathBuf, (SourceKey, Arc<Source>)>,
    next_source: u64,
    validated: HashMap<PathBuf, Arc<Source>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            revision: 0,
            evidence: EvidenceStore::new(Budget::default()),
            sources: HashMap::new(),
            next_source: 0,
            validated: HashMap::new(),
        }
    }
}

impl Store {
    /// Starts a new playback generation. Evidence of still-current sources
    /// stays resident; in-flight work of the old generation is abandoned.
    pub fn reset(&mut self, revision: u64) {
        self.revision = revision;
        self.validated.clear();
        self.evidence.clear_views();
        self.collect_sources();
    }

    /// Publish identities resolved by the background program reader. View
    /// submissions only compare in-memory identities, including after eviction.
    pub fn set_sources<'a>(
        &mut self,
        revision: u64,
        sources: impl IntoIterator<Item = &'a Arc<Source>>,
    ) {
        if self.revision == revision {
            self.validated = sources
                .into_iter()
                .map(|s| (s.path.clone(), s.clone()))
                .collect();
        }
    }

    pub fn next(&mut self, revision: u64) -> Option<Work> {
        if self.revision != revision {
            return None;
        }
        let reservation = self.evidence.schedule()?;
        let key = reservation.key();
        let (_, source) = self
            .sources
            .values()
            .find(|(id, _)| *id == key.source)
            .expect("demand retains its source registration");
        Some(Work {
            reservation,
            key: Key::new(source, key.request),
        })
    }

    pub fn is_current(&self, revision: u64, work: &Work) -> bool {
        self.revision == revision && self.evidence.is_current(work.reservation)
    }

    pub fn abandon(&mut self, work: Work) {
        self.evidence.abandon(work.reservation);
        self.collect_sources();
    }

    pub fn publish(&mut self, revision: u64, work: Work, value: Option<Tile>) -> bool {
        if !self.is_current(revision, &work) {
            self.abandon(work);
            return false;
        }
        let tile = value.map(|tile| match tile {
            Tile::Evidence(page) => store::Tile::Evidence(page),
            Tile::Samples(block) => store::Tile::Samples(block),
        });
        let published = self.evidence.publish(work.reservation, tile);
        self.collect_sources();
        published
    }

    fn register(&mut self, source: &Arc<Source>) -> Option<SourceKey> {
        let current = self.validated.get(&source.path)?;
        if !Arc::ptr_eq(current, source) && current != source {
            return None;
        }
        let previous = self.sources.get(&source.path);
        if let Some((key, held)) = previous
            && (Arc::ptr_eq(held, source) || held == source)
        {
            return Some(*key);
        }
        let key = if let Some((previous, _)) = previous {
            let next = previous
                .revision
                .0
                .checked_add(1)
                .expect("source revision exhausted");
            let key = SourceKey::new(previous.id.0, next);
            self.evidence.retire(*previous);
            key
        } else {
            self.next_source = self
                .next_source
                .checked_add(1)
                .expect("source identity exhausted");
            SourceKey::new(self.next_source, 1)
        };
        self.sources
            .insert(source.path.clone(), (key, source.clone()));
        Some(key)
    }

    #[cfg(test)]
    fn engine_key(&mut self, key: &Key) -> Option<store::Key> {
        Some(store::Key {
            source: self.register(&key.source)?,
            request: key.request,
        })
    }

    #[cfg(test)]
    fn set_view(
        &mut self,
        id: u64,
        keys: &[Key],
        prefetch: &[Key],
        preview: Option<FrameRange>,
    ) -> (Evidence, bool) {
        let mut unavailable_samples = 0;
        let engine: Vec<_> = keys
            .iter()
            .filter_map(|key| {
                let registered = self.engine_key(key);
                if registered.is_none() && matches!(key.request, Request::Samples(_)) {
                    unavailable_samples += 1;
                }
                registered
            })
            .collect();
        let ahead: Vec<_> = prefetch
            .iter()
            .filter_map(|key| self.engine_key(key))
            .collect();
        let (mut evidence, changed) =
            self.evidence
                .request(store::ViewId(id), &engine, &ahead, preview);
        evidence.coverage.unavailable += keys.len() - engine.len();
        evidence.coverage.missing_samples += unavailable_samples;
        self.collect_sources();
        (evidence, changed)
    }

    fn collect_sources(&mut self) {
        self.sources
            .retain(|_, (key, _)| self.evidence.retains_source(*key));
    }
}

pub struct View {
    id: u64,
    shared: Arc<Mutex<Store>>,
    wake: mpsc::SyncSender<()>,
    #[cfg(feature = "qualification")]
    trace: RefCell<ResidencyTrace>,
}

#[cfg(feature = "qualification")]
#[derive(Default)]
struct ResidencyTrace {
    requested: HashSet<store::Key>,
    consumed: HashSet<store::Key>,
}

#[cfg(feature = "qualification")]
pub fn trace_key(key: &store::Key) -> u64 {
    let mut hash = DefaultHasher::new();
    key.hash(&mut hash);
    hash.finish()
}

impl View {
    pub fn new(id: u64, shared: Arc<Mutex<Store>>, wake: mpsc::SyncSender<()>) -> Self {
        Self {
            id,
            shared,
            wake,
            #[cfg(feature = "qualification")]
            trace: RefCell::default(),
        }
    }

    #[cfg(test)]
    pub fn tiles(&self, revision: u64, keys: Vec<Key>, preview: Option<Range<usize>>) -> Evidence {
        self.tiles_ahead(revision, keys, vec![], preview)
    }

    #[cfg(test)]
    pub fn tiles_ahead(
        &self,
        revision: u64,
        keys: Vec<Key>,
        prefetch: Vec<Key>,
        preview: Option<Range<usize>>,
    ) -> Evidence {
        let mut store = self.shared.lock().unwrap();
        if store.revision != revision {
            return Evidence::default();
        }
        let preview =
            preview.and_then(|range| FrameRange::new(range.start as u64, range.end as u64).ok());
        let (evidence, changed) = store.set_view(self.id, &keys, &prefetch, preview);
        if changed {
            let _ = self.wake.try_send(());
        }
        evidence
    }

    pub fn residency(
        &self,
        revision: u64,
        source: &Arc<Source>,
        preview: Option<FrameRange>,
        plan: impl FnOnce(DecodeLatency) -> planning::ResidencyPlan,
    ) -> Evidence {
        let mut store = self.shared.lock().unwrap();
        if store.revision != revision {
            return Evidence::default();
        }
        let id = store::ViewId(self.id);
        let plan = plan(store.evidence.decode_latency())
            .with_channel_power(waveform_source::CHANNEL_POWER);
        let Some(source) = store.register(source) else {
            store.evidence.clear_view(id);
            store.collect_sources();
            let mut evidence = Evidence::default();
            evidence.coverage.unavailable = plan.required.len();
            evidence.coverage.missing_samples = plan
                .required
                .iter()
                .filter(|request| matches!(request, Request::Samples(_)))
                .count();
            return evidence;
        };
        let key = |request| store::Key { source, request };
        let required: Vec<_> = plan.required.into_iter().map(key).collect();
        let ahead: Vec<_> = plan.ahead.into_iter().map(|item| item.map(key)).collect();
        #[cfg(feature = "qualification")]
        let ahead = {
            static REACTIVE: OnceLock<bool> = OnceLock::new();
            if *REACTIVE.get_or_init(|| std::env::var_os("DISQRO_RESIDENCY_REACTIVE").is_some()) {
                Vec::new()
            } else {
                ahead
            }
        };
        let (evidence, changed) = store.evidence.request_timed(id, &required, &ahead, preview);
        if let Some(timestamp) = crate::diagnostics::begin_stage() {
            let at = Some(timestamp);
            #[cfg(feature = "qualification")]
            {
                let mut trace = self.trace.borrow_mut();
                trace.requested.retain(|key| {
                    required.contains(key) || ahead.iter().any(|item| item.key == *key)
                });
                trace
                    .consumed
                    .retain(|key| required.contains(key) && store.evidence.is_exact(key));
                for (key, speculative) in required
                    .iter()
                    .map(|key| (key, false))
                    .chain(ahead.iter().map(|item| (&item.key, true)))
                {
                    if trace.requested.insert(*key) {
                        crate::diagnostics::stage(
                            at,
                            "wave_evidence_requested",
                            trace_key(key),
                            (self.id << 1) | u64::from(speculative),
                        );
                    }
                    if !speculative && store.evidence.is_exact(key) && trace.consumed.insert(*key) {
                        crate::diagnostics::stage(
                            at,
                            "wave_evidence_consumed",
                            trace_key(key),
                            self.id,
                        );
                    }
                }
            }
            let visible = store.evidence.visible_residency(&required);
            for (stage, count) in [
                ("wave_residency_geometry", visible.geometry),
                ("wave_residency_spectral", visible.spectral),
                ("wave_residency_pcm", visible.pcm),
            ] {
                crate::diagnostics::stage(
                    at,
                    stage,
                    count.exact as u64,
                    (self.id << 32) | count.missing as u64,
                );
            }
            if changed {
                crate::diagnostics::stage(at, "wave_evidence_demand", self.id, ahead.len() as u64);
                crate::diagnostics::stage(
                    at,
                    "wave_evidence_bytes",
                    store.evidence.resident_bytes() as u64,
                    store.evidence.reserved_bytes() as u64,
                );
            }
        }
        if changed {
            store.collect_sources();
        }
        drop(store);
        if changed {
            let _ = self.wake.try_send(());
        }
        evidence
    }

    pub fn clear(&self) {
        #[cfg(feature = "qualification")]
        {
            let mut trace = self.trace.borrow_mut();
            trace.requested.clear();
            trace.consumed.clear();
        }
        let mut store = self.shared.lock().unwrap();
        if store.evidence.clear_view(store::ViewId(self.id)) {
            store.collect_sources();
            drop(store);
            let _ = self.wake.try_send(());
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Revision;

    fn source(path: PathBuf, contents: &[u8]) -> Arc<Source> {
        std::fs::write(&path, contents).unwrap();
        let revision = Revision::read(&path).unwrap();
        Arc::new(Source {
            path,
            stamp: revision.stamp,
            object: revision.object,
            rate: 48_000,
            frames: 16_384,
            available: true,
        })
    }

    fn measured(first: u64) -> Option<Tile> {
        Some(Tile::Samples(Arc::new(
            SampleBlock::new(first, vec![[0.0; 2]; 8192]).unwrap(),
        )))
    }

    #[test]
    fn normal_replacement_retires_residency_and_rejects_late_work_without_disturbing_another_source()
     {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replace.wav");
        let original = source(path.clone(), b"old");
        let other = source(dir.path().join("other.wav"), b"another source");
        let store = Arc::new(Mutex::new(Store::default()));
        let (wake, _) = mpsc::sync_channel(1);
        let left = View::new(0, store.clone(), wake.clone());
        let right = View::new(1, store.clone(), wake);
        store.lock().unwrap().set_sources(0, [&original, &other]);
        let first = Key::samples(&original, 0..8192);
        let second = Key::samples(&other, 0..8192);
        left.tiles(0, first.clone(), None);
        right.tiles(0, second.clone(), None);
        let retired = {
            let mut store = store.lock().unwrap();
            let old = store.next(0).unwrap();
            let key = old.reservation.key().source;
            assert!(store.publish(0, old, measured(0)));
            let unrelated = store.next(0).unwrap();
            assert!(store.publish(0, unrelated, measured(0)));
            key
        };
        let ahead = Key::samples(&original, 8192..16_384);
        let coverage = left.tiles_ahead(0, first.clone(), ahead, None).coverage;
        assert!(coverage.complete());
        assert_eq!(coverage.exact, 1);
        assert_eq!(coverage.missing_samples, 0);
        let late = store.lock().unwrap().next(0).unwrap();
        assert_eq!(late.reservation.key().source, retired);
        assert_eq!(store.lock().unwrap().evidence.reserved_bytes(), 65_536);
        let weak = Arc::downgrade(&original);
        std::fs::remove_file(&path).unwrap();
        let replacement = source(path.clone(), b"replacement stamp and native object");
        store.lock().unwrap().set_sources(0, [&replacement, &other]);
        let wanted = Key::samples(&replacement, 0..8192);
        assert_eq!(left.tiles(0, wanted.clone(), None).coverage.pending, 1);
        {
            let mut store = store.lock().unwrap();
            assert!(!store.evidence.is_source_current(retired));
            assert!(!store.evidence.retains_source(retired));
            assert_eq!(store.evidence.reserved_bytes(), 0);
            assert_eq!(store.evidence.resident_bytes(), 65_536);
            assert!(!store.publish(0, late, measured(8192)));
            let fresh = store.next(0).unwrap();
            assert_ne!(fresh.reservation.key().source, retired);
            assert!(store.publish(0, fresh, measured(0)));
        }
        assert_eq!(right.tiles(0, second, None).coverage.exact, 1);
        let rejected_ahead = Key::samples(&original, 8192..16_384);
        let coverage = left.tiles_ahead(0, first, rejected_ahead, None).coverage;
        assert_eq!(coverage.unavailable, 1);
        assert_eq!(coverage.missing_samples, 1);
        assert_eq!(left.tiles(0, wanted, None).coverage.exact, 1);
        drop(original);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn identities_follow_active_and_idle_evidence_and_are_never_reused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store {
            evidence: EvidenceStore::new(Budget {
                idle_entries: 2,
                ..Budget::default()
            }),
            ..Store::default()
        }));
        let (wake, _) = mpsc::sync_channel(1);
        let view = View::new(0, store.clone(), wake);
        let mut weak_sources = Vec::new();
        let mut previous = None;
        for index in 0..24 {
            let source = source(dir.path().join(format!("{index}.wav")), b"sample");
            store.lock().unwrap().set_sources(0, [&source]);
            weak_sources.push(Arc::downgrade(&source));
            view.tiles(0, Key::samples(&source, 0..8192), None);
            {
                let mut store = store.lock().unwrap();
                let work = store.next(0).unwrap();
                let id = work.reservation.key().source.id;
                assert!(previous.is_none_or(|previous| previous < id));
                previous = Some(id);
                assert!(store.publish(0, work, measured(0)));
            }
            view.clear();
        }
        assert_eq!(store.lock().unwrap().sources.len(), 2);
        assert_eq!(
            weak_sources
                .iter()
                .filter(|source| source.upgrade().is_some())
                .count(),
            2
        );
        let replacement = source(dir.path().join("0.wav"), b"rebuilt source");
        store.lock().unwrap().set_sources(0, [&replacement]);
        view.tiles(0, Key::samples(&replacement, 0..8192), None);
        let mut store = store.lock().unwrap();
        let old_generation = store.next(0).unwrap();
        assert!(old_generation.reservation.key().source.id > previous.unwrap());
        store.reset(1);
        assert!(!store.publish(0, old_generation, measured(0)));
        assert_eq!(store.evidence.reserved_bytes(), 0);
    }
}

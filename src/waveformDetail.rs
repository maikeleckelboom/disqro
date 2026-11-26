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
use std::{
    collections::HashMap,
    ops::Range,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};
use waveform::{
    FrameRange, SampleBlock, SampleRate, SourceKey, planning,
    store::{self, Budget, EvidenceStore, Request, Reservation},
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

    pub fn geometry(
        source: &Arc<Source>,
        playable: Range<usize>,
        range: Range<usize>,
        pixels: f64,
        seconds: f64,
    ) -> Vec<Self> {
        let (Ok(rate), Ok(segment)) = (
            SampleRate::new(source.rate),
            FrameRange::new(playable.start as u64, playable.end as u64),
        ) else {
            return vec![];
        };
        if !source.available || segment.is_empty() {
            return vec![];
        }
        planning::detail_requests(rate, segment, range, pixels, seconds)
            .into_iter()
            .map(|request| Self::new(source, Request::Evidence(request)))
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
}

impl Default for Store {
    fn default() -> Self {
        Self {
            revision: 0,
            evidence: EvidenceStore::new(Budget::default()),
            sources: HashMap::new(),
            next_source: 0,
        }
    }
}

impl Store {
    /// Starts a new playback generation. Evidence of still-current sources
    /// stays resident; in-flight work of the old generation is abandoned.
    pub fn reset(&mut self, revision: u64) {
        self.revision = revision;
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
            key: Key {
                source: source.clone(),
                request: key.request,
            },
        })
    }

    pub fn is_current(&self, revision: u64, work: &Work) -> bool {
        self.revision == revision && self.evidence.is_current(work.reservation)
    }

    pub fn publish(&mut self, revision: u64, work: Work, value: Option<Tile>) -> bool {
        if !self.is_current(revision, &work) {
            self.evidence.abandon(work.reservation);
            self.collect_sources();
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
        let previous = self.sources.get(&source.path);
        if let Some((key, held)) = previous
            && (Arc::ptr_eq(held, source) || held == source)
        {
            return Some(*key);
        }
        // An older view must not re-register a replaced native file or stamp.
        if !source.unchanged() {
            return None;
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

    fn engine_keys(&mut self, keys: &[Key]) -> Vec<store::Key> {
        keys.iter()
            .filter_map(|key| {
                Some(store::Key {
                    source: self.register(&key.source)?,
                    request: key.request,
                })
            })
            .collect()
    }

    fn set_view(
        &mut self,
        id: u64,
        keys: &[Key],
        prefetch: &[Key],
        preview: Option<FrameRange>,
    ) -> (Evidence, bool) {
        let engine = self.engine_keys(keys);
        let ahead = self.engine_keys(prefetch);
        let (mut evidence, changed) =
            self.evidence
                .request(store::ViewId(id), &engine, &ahead, preview);
        evidence.coverage.unavailable += keys.len() - engine.len();
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
}

impl View {
    pub fn new(id: u64, shared: Arc<Mutex<Store>>, wake: mpsc::SyncSender<()>) -> Self {
        Self { id, shared, wake }
    }

    #[cfg(test)]
    pub fn tiles(&self, revision: u64, keys: Vec<Key>, preview: Option<Range<usize>>) -> Evidence {
        self.tiles_ahead(revision, keys, vec![], preview)
    }

    /// `prefetch` is decoded ahead of need and is not part of the coverage.
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

    pub fn clear(&self) {
        if self
            .shared
            .lock()
            .unwrap()
            .set_view(self.id, &[], &[], None)
            .1
        {
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
        left.tiles(0, Key::samples(&original, 0..16_384), None);
        let late = store.lock().unwrap().next(0).unwrap();
        assert_eq!(late.reservation.key().source, retired);
        assert_eq!(store.lock().unwrap().evidence.reserved_bytes(), 65_536);
        let weak = Arc::downgrade(&original);
        std::fs::remove_file(&path).unwrap();
        let replacement = source(path.clone(), b"replacement stamp and native object");
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
        assert_eq!(left.tiles(0, first, None).coverage.unavailable, 1);
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
        view.tiles(0, Key::samples(&replacement, 0..8192), None);
        let mut store = store.lock().unwrap();
        let old_generation = store.next(0).unwrap();
        assert!(old_generation.reservation.key().source.id > previous.unwrap());
        store.reset(1);
        assert!(!store.publish(0, old_generation, measured(0)));
        assert_eq!(store.evidence.reserved_bytes(), 0);
    }
}

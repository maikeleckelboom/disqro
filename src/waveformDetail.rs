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
    sync::{Arc, Mutex, OnceLock, mpsc},
};
use waveform::{
    FrameRange, SampleBlock, SampleRate, SourceKey, planning,
    store::{self, Budget, EvidenceStore, Request},
};

pub use waveform::store::Coverage;

/// Exact mapping between Disqro sources and engine source keys. Each path
/// keeps one id; every distinct stamp of it gets a new revision.
#[derive(Default)]
struct Interner {
    keys: HashMap<Arc<Source>, SourceKey>,
    sources: HashMap<SourceKey, Arc<Source>>,
    ids: HashMap<std::path::PathBuf, (u64, u64)>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(Mutex::default)
}

/// The engine key of `source`, and whether it replaced an older revision.
fn source_key(source: &Arc<Source>) -> (SourceKey, Option<SourceKey>) {
    let mut interner = interner().lock().unwrap();
    if let Some(&key) = interner.keys.get(source) {
        return (key, None);
    }
    let next_id = interner.ids.len() as u64 + 1;
    let entry = interner
        .ids
        .entry(source.path.clone())
        .or_insert((next_id, 0));
    entry.1 += 1;
    let key = SourceKey::new(entry.0, entry.1);
    let previous = (entry.1 > 1).then(|| SourceKey::new(entry.0, entry.1 - 1));
    interner.keys.insert(source.clone(), key);
    interner.sources.insert(key, source.clone());
    (key, previous)
}

fn source_of(key: SourceKey) -> Option<Arc<Source>> {
    interner().lock().unwrap().sources.get(&key).cloned()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    source: Arc<Source>,
    key: store::Key,
}

pub enum Tile {
    Evidence(Arc<waveform::EvidencePage>),
    Samples(Arc<SampleBlock>),
}

impl Key {
    fn new(source: &Arc<Source>, request: Request) -> Self {
        let (key, _) = source_key(source);
        Self {
            source: source.clone(),
            key: store::Key {
                source: key,
                request,
            },
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

    pub fn load(
        &self,
        cache: Option<&Cache>,
        reader: &mut Option<Reader>,
        current: &impl Fn() -> bool,
    ) -> anyhow::Result<Tile> {
        Ok(match self.key.request {
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
                Tile::Samples(Arc::new(SampleBlock {
                    first: block.start,
                    frames: frames.into_iter().map(|f| [f.left, f.right]).collect(),
                }))
            }
        })
    }
}

/// Evidence for one view, in the engine's representation.
pub type Evidence = store::Evidence;

/// The engine store plus Disqro's playback generation fence.
pub struct Store {
    revision: u64,
    evidence: EvidenceStore,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            revision: 0,
            evidence: EvidenceStore::new(Budget::default()),
        }
    }
}

impl Store {
    /// Starts a new playback generation. Evidence of still-current sources
    /// stays resident; in-flight work of the old generation is abandoned.
    pub fn reset(&mut self, revision: u64) {
        self.revision = revision;
    }

    pub fn next(&mut self, revision: u64) -> Option<Key> {
        if self.revision != revision {
            return None;
        }
        let key = self.evidence.schedule()?;
        let Some(source) = source_of(key.source) else {
            self.evidence.retire(key.source);
            return None;
        };
        Some(Key { source, key })
    }

    pub fn publish(&mut self, revision: u64, key: Key, value: Option<Tile>) -> bool {
        if self.revision != revision {
            self.evidence.abandon(&key.key);
            return false;
        }
        let tile = value.map(|tile| match tile {
            Tile::Evidence(page) => store::Tile::Evidence(page),
            Tile::Samples(block) => store::Tile::Samples(block),
        });
        self.evidence.publish(key.key, tile)
    }

    fn set_view(&mut self, id: u64, keys: &[Key]) -> bool {
        for key in keys {
            if let (_, Some(previous)) = source_key(&key.source) {
                self.evidence.retire(previous);
            }
        }
        self.evidence
            .set_view(store::ViewId(id), keys.iter().map(|key| key.key).collect())
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

    pub fn tiles(&self, revision: u64, keys: Vec<Key>, preview: Option<Range<usize>>) -> Evidence {
        let mut store = self.shared.lock().unwrap();
        if store.revision != revision {
            return Evidence::default();
        }
        let engine: Vec<_> = keys.iter().map(|key| key.key).collect();
        let preview =
            preview.and_then(|range| FrameRange::new(range.start as u64, range.end as u64).ok());
        let evidence = store.evidence.evidence(&engine, preview);
        if store.set_view(self.id, &keys) {
            let _ = self.wake.try_send(());
        }
        evidence
    }

    pub fn clear(&self) {
        if self.shared.lock().unwrap().set_view(self.id, &[]) {
            let _ = self.wake.try_send(());
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.clear();
    }
}

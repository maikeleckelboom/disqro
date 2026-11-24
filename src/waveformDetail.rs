//! Bounded source tiles shared by visible waveform consumers.
use crate::{
    cache::Cache,
    timeline::Source,
    waveform::{self, Measure, Reader},
};
use hashlink::LinkedHashMap;
use kira::Frame;
use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
    sync::{Arc, Mutex, mpsc},
};

const SAMPLES: usize = 8192;
const RESIDENT: usize = 32;
pub const VIEW_TILES: usize = 8;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Resolution {
    Cells,
    Samples,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    source: Arc<Source>,
    resolution: Resolution,
    number: usize,
}

pub enum Tile {
    Cells { first: usize, cells: Vec<Measure> },
    Samples { first: usize, samples: Vec<Frame> },
}

impl Key {
    pub fn covering(
        source: &Arc<Source>,
        range: Range<usize>,
        resolution: Resolution,
    ) -> Vec<Self> {
        let end = range.end.min(source.frames);
        if range.start >= end || !source.available {
            return vec![];
        }
        let (first, last) = match resolution {
            Resolution::Cells => (
                (waveform::ceil_cell(range.start + 1, source.rate) - 1) / waveform::PAGE,
                (waveform::ceil_cell(end, source.rate) - 1) / waveform::PAGE,
            ),
            Resolution::Samples => (range.start / SAMPLES, (end - 1) / SAMPLES),
        };
        (first..=last)
            .take(VIEW_TILES)
            .map(|number| Self {
                source: source.clone(),
                resolution,
                number,
            })
            .collect()
    }

    pub fn load(
        &self,
        cache: Option<&Cache>,
        reader: &mut Option<Reader>,
        current: &impl Fn() -> bool,
    ) -> anyhow::Result<Tile> {
        Ok(match self.resolution {
            Resolution::Cells => Tile::Cells {
                first: self.number * waveform::PAGE,
                cells: waveform::load(&self.source, self.number, cache, reader, current)?,
            },
            Resolution::Samples => {
                let first = self.number * SAMPLES;
                let end = (first + SAMPLES).min(self.source.frames);
                Tile::Samples {
                    first,
                    samples: waveform::samples(&self.source, first..end, reader, current)?,
                }
            }
        })
    }
}

#[derive(Default)]
pub struct Store {
    revision: u64,
    views: BTreeMap<u64, Vec<Key>>,
    entries: LinkedHashMap<Key, Option<Arc<Tile>>>,
}

impl Store {
    pub fn reset(&mut self, revision: u64) {
        self.revision = revision;
        self.views.clear();
        self.entries.clear();
    }

    fn wanted(&self) -> impl Iterator<Item = &Key> {
        let mut seen = HashSet::new();
        // Give each view one tile before considering its next one.
        (0..VIEW_TILES * 2)
            .flat_map(|index| self.views.values().filter_map(move |keys| keys.get(index)))
            .filter(move |&key| seen.insert(key))
            .take(RESIDENT)
    }

    pub fn next(&self, revision: u64) -> Option<Key> {
        if self.revision != revision {
            return None;
        }
        self.wanted()
            .find(|&key| !self.entries.contains_key(key))
            .cloned()
    }

    pub fn publish(&mut self, revision: u64, key: Key, value: Option<Tile>) -> bool {
        if self.revision != revision || !self.wanted().any(|wanted| *wanted == key) {
            return false;
        }
        if self.entries.len() == RESIDENT {
            let evicted = {
                let wanted: HashSet<_> = self.wanted().collect();
                self.entries
                    .keys()
                    .find(|key| !wanted.contains(key))
                    .cloned()
            };
            if let Some(evicted) = evicted {
                self.entries.remove(&evicted);
            } else {
                return false;
            }
        }
        self.entries.insert(key, value.map(Arc::new));
        true
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

    pub fn tiles(&self, revision: u64, keys: Vec<Key>) -> Vec<Arc<Tile>> {
        let mut store = self.shared.lock().unwrap();
        if store.revision != revision {
            return vec![];
        }
        let mut tiles: Vec<_> = keys
            .iter()
            .filter_map(|key| store.entries.get(key)?.clone())
            .collect();
        tiles.sort_unstable_by_key(|tile| match tile.as_ref() {
            Tile::Cells { first, .. } | Tile::Samples { first, .. } => *first,
        });
        if store.views.get(&self.id) != Some(&keys) {
            if keys.is_empty() {
                store.views.remove(&self.id);
            } else {
                store.views.insert(self.id, keys);
            }
            let _ = self.wake.try_send(());
        }
        tiles
    }

    pub fn clear(&self) {
        if self.shared.lock().unwrap().views.remove(&self.id).is_some() {
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

    fn source() -> Arc<Source> {
        Arc::new(Source {
            path: "fixture.wav".into(),
            stamp: crate::cache::Stamp {
                bytes: 12,
                modified: 9,
            },
            object: None,
            rate: 48000,
            frames: 2_000_000,
            available: true,
        })
    }

    #[test]
    fn views_share_tiles_and_ignore_superseded_publication() {
        let shared = Arc::new(Mutex::new(Store::default()));
        let (wake, _) = mpsc::sync_channel(1);
        let (first, second) = (
            View::new(0, shared.clone(), wake.clone()),
            View::new(1, shared.clone(), wake),
        );
        let keys = Key::covering(&source(), 0..SAMPLES, Resolution::Samples);
        shared.lock().unwrap().reset(1);
        assert!(first.tiles(1, keys.clone()).is_empty());
        assert!(second.tiles(1, keys.clone()).is_empty());
        let key = shared.lock().unwrap().next(1).unwrap();
        assert!(shared.lock().unwrap().publish(
            1,
            key.clone(),
            Some(Tile::Samples {
                first: 0,
                samples: vec![Frame::new(1., -1.)]
            })
        ));
        assert!(shared.lock().unwrap().next(1).is_none());
        let first = first.tiles(1, keys.clone());
        let second = second.tiles(1, keys);
        assert!(Arc::ptr_eq(&first[0], &second[0]));
        let mut store = shared.lock().unwrap();
        store.reset(2);
        assert!(!store.publish(1, key, None));
        assert!(store.entries.is_empty());
    }

    #[test]
    fn inactive_tiles_are_evicted_without_displacing_a_visible_tile() {
        let shared = Arc::new(Mutex::new(Store::default()));
        let (wake, _) = mpsc::sync_channel(1);
        let (first, second) = (
            View::new(0, shared.clone(), wake.clone()),
            View::new(1, shared.clone(), wake),
        );
        let source = source();
        let visible = Key::covering(&source, 0..SAMPLES, Resolution::Samples);
        first.tiles(0, visible.clone());
        for number in 0..RESIDENT * 2 {
            let start = number * SAMPLES;
            second.tiles(
                0,
                Key::covering(&source, start..start + SAMPLES, Resolution::Samples),
            );
            let mut store = shared.lock().unwrap();
            let key = store.next(0).unwrap();
            assert!(store.publish(0, key, None));
            assert!(store.entries.contains_key(&visible[0]));
            assert!(store.entries.len() <= RESIDENT);
        }
    }
}

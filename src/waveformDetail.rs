//! Bounded source tiles shared by visible waveform consumers.
use crate::{
    cache::Cache,
    timeline::Source,
    waveform::{self, Evidence, Reader},
    waveform_port_analysis::{EVIDENCE_FRAMES, LEAF, Plan, Spectrum},
    waveform_render::Page,
};
use hashlink::LinkedHashMap;
use kira::Frame;
use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
    sync::{Arc, Mutex, mpsc},
};

const SAMPLES: usize = 8192;
const RESIDENT: usize = 64;
const VIEW_TILES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Request {
    Samples(usize),
    Evidence(Evidence),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    source: Arc<Source>,
    request: Request,
}

pub enum Tile {
    Evidence(Arc<Page>),
    Samples { first: usize, samples: Vec<Frame> },
}

impl Key {
    pub fn samples(source: &Arc<Source>, range: Range<usize>) -> Vec<Self> {
        let range = range.start.min(source.frames)..range.end.min(source.frames);
        if range.is_empty() || !source.available {
            return vec![];
        }
        (range.start / SAMPLES..=(range.end - 1) / SAMPLES)
            .take(8)
            .map(|number| Self {
                source: source.clone(),
                request: Request::Samples(number),
            })
            .collect()
    }

    pub fn geometry(
        source: &Arc<Source>,
        playable: Range<usize>,
        range: Range<usize>,
        pixels: f64,
        seconds: f64,
    ) -> Vec<Self> {
        let range = range.start.min(playable.len())..range.end.min(playable.len());
        if range.is_empty() || !source.available {
            return vec![];
        }
        let fitted = matches!(source.rate, 44_100 | 48_000);
        let close = (8.0 / seconds).log2().clamp(0.0, 1.0);
        let columns = ((pixels * 10.0).clamp(128.0, 16384.0) * (1.0 + 3.0 * close))
            .ceil()
            .min(49152.0);
        let tier = ((seconds * f64::from(source.rate) / (columns - 1.0))
            .ceil()
            .max(1.0) as usize)
            .next_power_of_two();
        let spectrum = Spectrum::available(source.rate, fitted);
        let mut keys: Vec<_> = Self::pages(
            source,
            &playable,
            range.clone(),
            Plan {
                start_frame: 0,
                end_frame: 0,
                frames_per_column: 256,
                leaf_frames: Some(LEAF),
                octave: spectrum,
                overview: false,
                fallback_energy: spectrum.is_none(),
                preview: false,
            },
            EVIDENCE_FRAMES,
        )
        .collect();
        if tier < LEAF {
            keys.extend(Self::pages(
                source,
                &playable,
                range,
                Plan {
                    start_frame: 0,
                    end_frame: 0,
                    frames_per_column: tier,
                    leaf_frames: Some(tier),
                    octave: None,
                    overview: false,
                    fallback_energy: false,
                    preview: false,
                },
                tier * 8192,
            ));
        }
        keys
    }

    fn pages(
        source: &Arc<Source>,
        playable: &Range<usize>,
        range: Range<usize>,
        plan: Plan,
        size: usize,
    ) -> impl Iterator<Item = Self> {
        let first = range.start / size;
        let last = (range.end - 1) / size;
        let centre = (first + last) / 2;
        (0..=last - first)
            .map(move |offset| {
                if offset % 2 == 0 {
                    centre - offset / 2
                } else {
                    centre + offset.div_ceil(2)
                }
            })
            .filter(move |tile| (first..=last).contains(tile))
            .take(VIEW_TILES)
            .map(move |tile| Self {
                source: source.clone(),
                request: Request::Evidence(Evidence {
                    playable: playable.clone(),
                    plan: Plan {
                        start_frame: tile * size,
                        end_frame: ((tile + 1) * size).min(playable.len()),
                        ..plan
                    },
                }),
            })
    }

    pub fn load(
        &self,
        cache: Option<&Cache>,
        reader: &mut Option<Reader>,
        current: &impl Fn() -> bool,
    ) -> anyhow::Result<Tile> {
        Ok(match &self.request {
            Request::Evidence(request) => Tile::Evidence(Arc::new(waveform::load(
                &self.source,
                request,
                cache,
                reader,
                current,
            )?)),
            Request::Samples(number) => {
                let first = number * SAMPLES;
                let end = (first + SAMPLES).min(self.source.frames);
                Tile::Samples {
                    first,
                    samples: waveform::samples(&self.source, first..end, reader, current)?,
                }
            }
        })
    }

    fn complements(&self, requested: &Self) -> bool {
        let (Request::Evidence(held), Request::Evidence(wanted)) =
            (&self.request, &requested.request)
        else {
            return false;
        };
        self.source == requested.source
            && held.playable == wanted.playable
            && held.plan.fallback_energy == wanted.plan.fallback_energy
            && held.plan.leaf_frames.is_some() == wanted.plan.leaf_frames.is_some()
            && held.plan.start_frame < wanted.plan.end_frame
            && wanted.plan.start_frame < held.plan.end_frame
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
        if self.revision != revision || self.entries.contains_key(&key) {
            return false;
        }
        let wanted = self.wanted().any(|wanted| *wanted == key);
        if !wanted && value.is_none() {
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
        wanted
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
        tiles.extend(store.entries.iter().filter_map(|(held, tile)| {
            (!keys.contains(held) && keys.iter().any(|wanted| held.complements(wanted)))
                .then(|| tile.clone())
                .flatten()
        }));
        tiles.sort_unstable_by_key(|tile| match tile.as_ref() {
            Tile::Evidence(page) => (page.start_frame, page.frames_per_column),
            Tile::Samples { first, .. } => (*first, 0),
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
        let keys = Key::samples(&source(), 0..SAMPLES);
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
        let visible = Key::samples(&source, 0..SAMPLES);
        first.tiles(0, visible.clone());
        for number in 0..RESIDENT * 2 {
            let start = number * SAMPLES;
            second.tiles(0, Key::samples(&source, start..start + SAMPLES));
            let mut store = shared.lock().unwrap();
            let key = store.next(0).unwrap();
            assert!(store.publish(0, key, None));
            assert!(store.entries.contains_key(&visible[0]));
            assert!(store.entries.len() <= RESIDENT);
        }
    }
}

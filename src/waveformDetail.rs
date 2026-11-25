//! Bounded source tiles shared by visible waveform consumers.
use crate::{
    cache::Cache,
    timeline::Source,
    waveform::{self, Evidence as RequestEvidence, Reader},
    waveform_port_analysis::{EVIDENCE_FRAMES, LEAF, MAX_LEAF, Plan, Spectrum},
    waveform_render::Page,
};
use hashlink::LinkedHashMap;
use kira::Frame;
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    sync::{Arc, Mutex, mpsc},
};

const SAMPLES: usize = 8192;
const LEAVES_PER_TILE: usize = EVIDENCE_FRAMES / LEAF;
const IDLE_BYTES: usize = 8 * 1024 * 1024;
const IDLE_ENTRIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Request {
    Samples(usize),
    Evidence(RequestEvidence),
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

impl Tile {
    fn bytes(&self) -> usize {
        match self {
            Self::Evidence(page) => page.bytes(),
            Self::Samples { samples, .. } => std::mem::size_of_val(samples.as_slice()),
        }
    }
}

impl Key {
    fn frames(&self) -> Range<usize> {
        match &self.request {
            Request::Samples(number) => {
                number * SAMPLES..((number + 1) * SAMPLES).min(self.source.frames)
            }
            Request::Evidence(e) => {
                e.playable.start + e.plan.start_frame..e.playable.start + e.plan.end_frame
            }
        }
    }

    pub fn samples(source: &Arc<Source>, range: Range<usize>) -> Vec<Self> {
        let range = range.start.min(source.frames)..range.end.min(source.frames);
        if range.is_empty() || !source.available {
            return vec![];
        }
        (range.start / SAMPLES..=(range.end - 1) / SAMPLES)
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
        let visible_frames = (seconds * f64::from(source.rate)).ceil() as usize;
        let close = (8.0 / seconds).log2().clamp(0.0, 1.0);
        let columns = ((pixels * 10.0).clamp(128.0, 16384.0) * (1.0 + 3.0 * close))
            .ceil()
            .min(49152.0);
        let tier = visible_frames
            .div_ceil((columns as usize).saturating_sub(1).max(1))
            .clamp(1, MAX_LEAF)
            .next_power_of_two();
        let leaf = tier.max(LEAF);
        let spectrum = Spectrum::available(source.rate, fitted);
        // Keep fitted Stroke colour at its authored hop size. The other rates
        // pool measured power when the span would exceed the colour budget.
        let colour = if fitted {
            256
        } else {
            range.len().div_ceil(24_576).max(256).next_power_of_two()
        };
        let size = (leaf * LEAVES_PER_TILE).min(EVIDENCE_FRAMES * 4);
        let mut keys: Vec<_> = Self::pages(
            source,
            &playable,
            range.clone(),
            Plan {
                start_frame: 0,
                end_frame: 0,
                frames_per_column: colour,
                leaf_frames: Some(leaf),
                octave: spectrum,
                overview: false,
                fallback_energy: spectrum.is_none(),
                preview: false,
            },
            size,
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
                tier * LEAVES_PER_TILE,
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
            .map(move |tile| Self {
                source: source.clone(),
                request: Request::Evidence(RequestEvidence {
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
            && held.plan.start_frame < wanted.plan.end_frame
            && wanted.plan.start_frame < held.plan.end_frame
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub exact: usize,
    pub approximate: usize,
    pub pending: usize,
    pub unavailable: usize,
}

#[derive(Default)]
pub struct Evidence {
    pub tiles: Vec<Arc<Tile>>,
    pub coverage: Coverage,
}

#[derive(Default)]
pub struct Store {
    revision: u64,
    views: BTreeMap<u64, Vec<Key>>,
    after: u64,
    pins: HashMap<Key, usize>,
    entries: LinkedHashMap<Key, Option<Arc<Tile>>>,
}

impl Store {
    pub fn reset(&mut self, revision: u64) {
        *self = Self {
            revision,
            ..Self::default()
        };
    }

    pub fn next(&mut self, revision: u64) -> Option<Key> {
        if self.revision != revision {
            return None;
        }
        use std::ops::Bound::{Excluded, Unbounded};
        let (id, key) = self
            .views
            .range((Excluded(self.after), Unbounded))
            .chain(self.views.range(..=self.after))
            .find_map(|(&id, keys)| {
                Some((
                    id,
                    keys.iter().find(|key| !self.entries.contains_key(*key))?,
                ))
            })?;
        self.after = id;
        Some(key.clone())
    }

    pub fn publish(&mut self, revision: u64, key: Key, value: Option<Tile>) -> bool {
        if self.revision != revision || self.entries.contains_key(&key) {
            return false;
        }
        let wanted = self.pins.contains_key(&key);
        if !wanted && value.is_none() {
            return false;
        }
        self.entries.insert(key, value.map(Arc::new));
        self.trim();
        wanted
    }

    fn trim(&mut self) {
        let mut idle = 0;
        let mut bytes = 0;
        let mut expired = Vec::new();
        for (key, tile) in self.entries.iter().rev() {
            if !self.pins.contains_key(key) {
                idle += 1;
                bytes += tile.as_ref().map_or(0, |tile| tile.bytes());
                if idle > IDLE_ENTRIES || bytes > IDLE_BYTES {
                    expired.push(key.clone());
                }
            }
        }
        for key in expired {
            self.entries.remove(&key);
        }
    }

    fn set_view(&mut self, id: u64, keys: Vec<Key>) -> bool {
        if self.views.get(&id).map_or(&[][..], Vec::as_slice) == keys.as_slice() {
            return false;
        }
        for key in &keys {
            *self.pins.entry(key.clone()).or_default() += 1;
            self.entries.to_back(key);
        }
        if let Some(previous) = self.views.remove(&id) {
            for key in &previous {
                let pins = self.pins.get_mut(key).unwrap();
                *pins -= 1;
                if *pins == 0 {
                    self.pins.remove(key);
                    if self.entries.get(key).is_some_and(Option::is_none) {
                        self.entries.remove(key);
                    }
                }
            }
        }
        if !keys.is_empty() {
            self.views.insert(id, keys);
        }
        self.trim();
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

    pub fn tiles(&self, revision: u64, keys: Vec<Key>, preview: Option<Range<usize>>) -> Evidence {
        let mut store = self.shared.lock().unwrap();
        if store.revision != revision {
            return Evidence::default();
        }
        let mut evidence = Evidence::default();
        let mut missing = Vec::new();
        for key in &keys {
            match store.entries.get(key) {
                Some(Some(tile)) => {
                    evidence.tiles.push(tile.clone());
                    evidence.coverage.exact += 1;
                    continue;
                }
                Some(None) => evidence.coverage.unavailable += 1,
                None => evidence.coverage.pending += 1,
            }
            missing.push(key);
        }
        // Keep useful prior tiers while a new tier arrives. Cap the extra
        // projection work regardless of how many other panels hold evidence.
        let alternatives: Vec<_> = store
            .entries
            .iter()
            .rev()
            .take(if missing.is_empty() {
                0
            } else {
                store.entries.len()
            })
            .filter_map(|(key, tile)| Some((key, tile.as_ref()?)))
            .filter(|(held, _)| {
                !keys.contains(held) && missing.iter().any(|wanted| held.complements(wanted))
            })
            .take(8)
            .collect();
        evidence.coverage.approximate = missing
            .iter()
            .filter(|wanted| {
                preview.as_ref().is_some_and(|preview| {
                    let frames = wanted.frames();
                    frames.start < preview.end && preview.start < frames.end
                }) || alternatives
                    .iter()
                    .any(|(held, _)| held.complements(wanted))
            })
            .count();
        evidence
            .tiles
            .extend(alternatives.into_iter().map(|(_, tile)| tile.clone()));
        evidence
            .tiles
            .sort_unstable_by_key(|tile| match tile.as_ref() {
                Tile::Evidence(page) => (page.start_frame, page.frames_per_column),
                Tile::Samples { first, .. } => (*first, 0),
            });
        if store.set_view(self.id, keys) {
            let _ = self.wake.try_send(());
        }
        evidence
    }

    pub fn clear(&self) {
        if self.shared.lock().unwrap().set_view(self.id, vec![]) {
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
        assert_eq!(first.tiles(1, keys.clone(), None).coverage.pending, 1);
        assert_eq!(
            second
                .tiles(1, keys.clone(), Some(0..SAMPLES))
                .coverage
                .approximate,
            1
        );
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
        let first = first.tiles(1, keys.clone(), None);
        let second = second.tiles(1, keys, None);
        assert_eq!(first.coverage.exact, 1);
        assert!(Arc::ptr_eq(&first.tiles[0], &second.tiles[0]));
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
        first.tiles(0, visible.clone(), None);
        for number in 0..source.frames / SAMPLES {
            let start = number * SAMPLES;
            second.tiles(0, Key::samples(&source, start..start + SAMPLES), None);
            let mut store = shared.lock().unwrap();
            let key = store.next(0).unwrap();
            assert!(store.publish(0, key, None));
            assert!(store.entries.contains_key(&visible[0]));
            assert!(store.entries.len() <= IDLE_ENTRIES + 2);
        }
    }

    #[test]
    fn wide_views_cover_all_supported_rates_and_unaligned_ranges() {
        use crate::waveform_render::SUPPORTED_OCTAVE_RATES;
        for rate in SUPPORTED_OCTAVE_RATES.into_iter().chain([8400, 384_000]) {
            let source = Arc::new(Source {
                rate,
                frames: rate as usize * 347 + 3907,
                ..Arc::try_unwrap(source()).unwrap()
            });
            let region = 117..source.frames - 309;
            let span = rate as usize * 120;
            for width in [460., 1920., 5120., 16_384.] {
                for visible in [
                    0..span,
                    123_293..123_293 + span,
                    region.len() - span..region.len(),
                ] {
                    let keys = Key::geometry(&source, region.clone(), visible.clone(), width, 120.);
                    let mut tiles: Vec<_> = keys
                        .iter()
                        .filter_map(|key| match &key.request {
                            Request::Evidence(e) => Some(e),
                            _ => None,
                        })
                        .collect();
                    tiles.sort_by_key(|e| e.plan.start_frame);
                    assert!(tiles.first().unwrap().plan.start_frame <= visible.start);
                    assert!(tiles.last().unwrap().plan.end_frame >= visible.end);
                    assert!(
                        tiles
                            .windows(2)
                            .all(|pair| pair[0].plan.end_frame == pair[1].plan.start_frame)
                    );
                    assert!(
                        tiles
                            .iter()
                            .all(|e| e.playable == region && e.plan.leaf_frames.unwrap() >= LEAF)
                    );
                    if rate >= 96_000 {
                        assert!(tiles.iter().all(|e| e.plan.leaf_frames.unwrap() > LEAF));
                    }
                }
            }
        }
    }

    #[test]
    fn every_visible_panel_can_finish_while_another_panel_moves() {
        let mut source = Arc::try_unwrap(source()).unwrap();
        source.rate = 192_000;
        source.frames = source.rate as usize * 1600;
        let source = Arc::new(source);
        let mut store = Store::default();
        // More distinct pinned requests than the former process-wide limit.
        for panel in 0..8 {
            let start = panel as usize * source.rate as usize * 145;
            store.set_view(
                panel,
                Key::geometry(
                    &source,
                    0..source.frames,
                    start..start + source.rate as usize * 120,
                    5120.,
                    120.,
                ),
            );
        }
        let visible = store.pins.len();
        assert!(visible > 64);
        for step in 0..visible * 2 {
            let start = (step + 31_000) * SAMPLES;
            store.set_view(8, Key::samples(&source, start..start + SAMPLES));
            let key = store.next(0).unwrap();
            assert!(store.publish(0, key, Some(Tile::Evidence(Arc::default()))));
        }
        assert!(
            store
                .views
                .range(..8)
                .flat_map(|(_, keys)| keys)
                .all(|key| store.entries.contains_key(key))
        );
        for id in 0..=8 {
            store.set_view(id, vec![]);
        }
        assert!(store.entries.len() <= IDLE_ENTRIES);
    }
}

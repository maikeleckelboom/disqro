//! Time is counted in captured mix frames. A column is the calibrated peak
//! line power in each frequency cell, sampled at a quarter-window hop.
use crate::spectrum::{Mode, Range, Speed, Transform, choices};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};

pub const ROWS: usize = 256;
pub const TILE: usize = 32;

choices!(View { Heatmap => "Spectrogram", Waterfall => "Waterfall" });
choices!(Mapping { Log => "Logarithmic", Linear => "Linear" });
choices!(WindowLength { Balanced => "Balanced · 43 ms", Short => "Transient · 21 ms", Long => "Tonal · 85 ms" });
choices!(Span { Normal => "30 seconds", Short => "15 seconds", Long => "60 seconds" });
choices!(Palette { Ember => "Ember", Marine => "Marine" });
choices!(Floor { Normal => "−84 dBFS", Deep => "−108 dBFS", Focus => "−60 dBFS" });
choices!(Ceiling { Full => "0 dBFS", Soft => "−12 dBFS", Quiet => "−24 dBFS" });
impl Span {
    pub fn seconds(self) -> u32 {
        match self {
            Self::Normal => 30,
            Self::Short => 15,
            Self::Long => 60,
        }
    }
}
impl Floor {
    pub fn db(self) -> f32 {
        match self {
            Self::Normal => -84.,
            Self::Deep => -108.,
            Self::Focus => -60.,
        }
    }
}
impl Ceiling {
    pub fn db(self) -> f32 {
        match self {
            Self::Full => 0.,
            Self::Soft => -12.,
            Self::Quiet => -24.,
        }
    }
}
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Analysis {
    pub mode: Mode,
    pub range: Range,
    pub mapping: Mapping,
    pub resolution: WindowLength,
}
impl WindowLength {
    pub fn len(self, rate: u32) -> usize {
        let reference = match self {
            Self::Short => 1024.,
            Self::Balanced => 2048.,
            Self::Long => 4096.,
        };
        1 << ((rate as f32 * (reference / 48000.)).log2().round() as u32).clamp(9, 15)
    }
}
impl Mapping {
    pub fn fraction(self, hz: f32, high: f32) -> f32 {
        match self {
            Self::Linear => (hz - 20.) / (high - 20.),
            Self::Log => (hz / 20.).ln() / (high / 20.).ln(),
        }
    }
    fn frequency(self, t: f32, high: f32) -> f32 {
        match self {
            Self::Linear => 20. + (high - 20.) * t,
            Self::Log => 20. * (high / 20.).powf(t),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(flatten)]
    pub analysis: Analysis,
    pub view: View,
    pub span: Span,
    pub palette: Palette,
    pub floor: Floor,
    pub ceiling: Ceiling,
    pub speed: Speed,
}

// Half-dB steps, from −120 through +7.5 dBFS. Zero includes digital silence.
pub fn db(code: u8) -> f32 {
    f32::from(code) * 0.5 - 120.
}
fn encode(power: f64) -> u8 {
    (20. * power.log10() + 240.).round() as u8
}

struct Cell {
    first: usize,
    last: usize,
    blend: f64,
}
pub struct Scale {
    cells: Vec<Cell>,
    mode: Mode,
}
impl Scale {
    pub fn new(a: Analysis, rate: u32, n: usize) -> Self {
        let high = a.range.high(rate);
        let bin = |i: f32| {
            (a.mapping.frequency(i / ROWS as f32, high) * n as f32 / rate as f32)
                .clamp(0., (n / 2) as f32)
        };
        let cells = (0..ROWS)
            .map(|i| {
                let lo = bin(i as f32).ceil() as usize;
                let hi = bin((i + 1) as f32).floor() as usize;
                if hi >= lo {
                    Cell {
                        first: lo,
                        last: hi,
                        blend: -1.,
                    }
                } else {
                    let mid = bin(i as f32 + 0.5);
                    Cell {
                        first: mid as usize,
                        last: (mid as usize + 1).min(n / 2),
                        blend: f64::from(mid.fract()),
                    }
                }
            })
            .collect();
        Self {
            cells,
            mode: a.mode,
        }
    }
    pub fn column(&self, t: &Transform) -> [u8; ROWS] {
        std::array::from_fn(|i| {
            let c = &self.cells[i];
            let mut p = t.power(c.first, self.mode);
            if c.blend >= 0. {
                p += (t.power(c.last, self.mode) - p) * c.blend;
            } else {
                for k in c.first + 1..=c.last {
                    p = p.max(t.power(k, self.mode));
                }
            }
            encode(p)
        })
    }
}
#[derive(Clone)]
pub struct Tile {
    /// Center sample of the first STFT window.
    pub center: u64,
    pub columns: Vec<[u8; ROWS]>,
}
#[derive(Clone)]
pub struct History {
    pub tiles: VecDeque<Arc<Tile>>,
    pub rate: u32,
    pub hop: u64,
    pub end: u64,
}
impl History {
    pub fn new(rate: u32, n: usize) -> Self {
        Self {
            tiles: VecDeque::new(),
            rate,
            hop: (n / 4) as u64,
            end: 0,
        }
    }
    pub fn push(&mut self, center: u64, column: [u8; ROWS]) {
        if self.tiles.back().is_none_or(|t| t.columns.len() == TILE) {
            self.tiles.push_back(Arc::new(Tile {
                center,
                columns: Vec::with_capacity(TILE),
            }));
        }
        Arc::make_mut(self.tiles.back_mut().unwrap())
            .columns
            .push(column);
        self.end = center + self.hop;
        let start = self.end.saturating_sub(60 * u64::from(self.rate));
        while self
            .tiles
            .front()
            .is_some_and(|t| t.center + TILE as u64 * self.hop <= start)
        {
            self.tiles.pop_front();
        }
    }
    pub fn column_at(&self, sample: u64) -> Option<&[u8; ROWS]> {
        let index = sample.checked_sub(self.tiles.front()?.center)? / self.hop;
        self.tiles
            .get(index as usize / TILE)?
            .columns
            .get(index as usize % TILE)
    }
}

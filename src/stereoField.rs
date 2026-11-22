//! Paired PCM occupancy independent of viewport size. Sample time controls phosphor
//! decay, including during digital silence. Neither coordinate is synthesized.
use crate::spectrum::{Speed, Stereo, choices};
use kira::Frame;
use serde::{Deserialize, Serialize};
const DENSITY_SIDE: usize = 2048;
const TRACE_SIDE: usize = 1024;
const HALF_INTENSITY: f32 = 2.5;

choices!(Axes { MidSide => "Mid / side", XY => "Left / right (XY)" });
choices!(Ink { Density => "Point density", Trace => "Vector trace" });
choices!(Persistence { Normal => "240 ms", Short => "80 ms", Long => "720 ms" });
choices!(Gain { Unity => "1×", Double => "2×", Quad => "4×" });
impl Persistence {
    fn half_life(self) -> f32 {
        match self {
            Self::Short => 0.08,
            Self::Normal => 0.24,
            Self::Long => 0.72,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Analysis {
    pub axes: Axes,
    pub ink: Ink,
    pub persistence: Persistence,
    pub gain: Gain,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(flatten)]
    pub analysis: Analysis,
    pub speed: Speed,
    pub statistics: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            analysis: Analysis::default(),
            speed: Speed::default(),
            statistics: true,
        }
    }
}
impl Axes {
    pub fn coordinates(self, f: Frame) -> (f32, f32) {
        match self {
            Self::XY => (f.left, f.right),
            Self::MidSide => ((f.left - f.right) * 0.5, (f.left + f.right) * 0.5),
        }
    }
}
pub struct Reading {
    pub side: usize,
    // Inclusive bounds of the visible cells in the analysis grid.
    bounds: [usize; 4],
    occupied: Vec<(u32, u8)>,
    pub stereo: Stereo,
}

pub struct Raster {
    pub bounds: [usize; 4],
    pub intensity: Vec<u8>,
}

impl Reading {
    pub fn raster(&self, side: usize) -> Option<Raster> {
        if self.occupied.is_empty() {
            return None;
        }

        // Map cell centers into display pixels. Retain the peak where several
        // cells share one pixel so thin traces survive in small panes.
        let shift = self.side.trailing_zeros();
        let pixel = |cell: usize| ((2 * cell + 1) * side) >> (shift + 1);
        let [x, y, right, bottom] = self.bounds;
        let (left, top) = (pixel(x).saturating_sub(1), pixel(y).saturating_sub(1));
        let (right, bottom) = ((pixel(right) + 2).min(side), (pixel(bottom) + 2).min(side));
        let width = right - left;
        let mut intensity = vec![0; width * (bottom - top)];
        for &(cell, value) in &self.occupied {
            let cell = cell as usize;
            let x = pixel(cell & (self.side - 1)) - left;
            let y = pixel(cell >> shift) - top;
            let index = y * width + x;
            intensity[index] = intensity[index].max(value);
        }
        Some(Raster {
            bounds: [left, top, right, bottom],
            intensity,
        })
    }
}

pub struct Field {
    analysis: Analysis,
    rate: u32,
    side: usize,
    cells: Vec<f32>,
    active: Vec<u32>,
    scale: f32,
    previous: Option<(f32, f32)>,
    pub settled: bool,
    silent_frames: usize,
}
impl Field {
    pub fn new(analysis: Analysis, rate: u32) -> Self {
        let side = match analysis.ink {
            Ink::Density => DENSITY_SIDE,
            Ink::Trace => TRACE_SIDE,
        };
        Self {
            analysis,
            rate,
            side,
            cells: vec![0.; side * side],
            active: Vec::new(),
            scale: 1.,
            previous: None,
            settled: false,
            silent_frames: 0,
        }
    }
    fn add(&mut self, cell: usize, weight: f32) {
        if weight > 0. {
            if self.cells[cell] == 0. {
                self.active.push(cell as u32);
            }
            self.cells[cell] += weight;
        }
    }

    fn deposit(&mut self, x: f32, y: f32, weight: f32) {
        let (i, j) = (x as usize, y as usize);
        let (fx, fy) = (x - i as f32, y - j as f32);
        let cell = j * self.side + i;
        self.add(cell, weight * (1. - fx) * (1. - fy));
        self.add(cell + 1, weight * fx * (1. - fy));
        self.add(cell + self.side, weight * (1. - fx) * fy);
        self.add(cell + self.side + 1, weight * fx * fy);
    }
    pub fn push(&mut self, frames: &[Frame]) {
        if self.settled && frames.iter().all(|f| *f == Frame::ZERO) {
            return;
        }
        let half = self.analysis.persistence.half_life();
        self.scale *= (-((frames.len() as f32) / (self.rate as f32 * half))).exp2();
        if self.scale < 0.125 {
            // Release cells far below the first visible intensity level.
            self.active.retain(|&index| {
                let cell = &mut self.cells[index as usize];
                *cell *= self.scale;
                if *cell < 0.0005 {
                    *cell = 0.;
                }
                *cell > 0.
            });
            self.scale = 1.;
        }
        let gain = match self.analysis.gain {
            Gain::Unity => 1.,
            Gain::Double => 2.,
            Gain::Quad => 4.,
        };
        let weight = 48000. / self.rate as f32 / self.scale;
        let k = (self.side - 2) as f32 * 0.5;
        for &f in frames {
            let (x, y) = self.analysis.axes.coordinates(f);
            if f == Frame::ZERO {
                self.silent_frames += 1;
                self.previous = None;
                continue;
            }
            self.silent_frames = 0;
            let (x, y) = (x * gain, y * gain);
            // Off-scale and invalid samples leave the viewport; clamping them
            // to the border would invent a high-density signal on its edge.
            if !(x.abs() <= 1. && y.abs() <= 1.) {
                self.previous = None;
                continue;
            }
            let (x, y) = ((1. + x) * k + 0.5, (1. - y) * k + 0.5);
            if self.analysis.ink == Ink::Trace
                && let Some((a, b)) = self.previous
            {
                let steps = ((x - a).abs().max((y - b).abs()).ceil() as usize).clamp(1, 32);
                let weight = weight / steps as f32;
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    self.deposit(a + (x - a) * t, b + (y - b) * t, weight);
                }
            } else {
                self.deposit(x, y, weight);
            }
            self.previous = Some((x, y));
        }
        self.settled = self.silent_frames as f32 > self.rate as f32 * half * 18.;
        if self.settled {
            for cell in self.active.drain(..) {
                self.cells[cell as usize] = 0.;
            }
        }
    }
    pub fn reading(&self, stereo: Stereo) -> Reading {
        // Bounded monotone density response, normalized for the decay window.
        let scale = self.scale * (0.24 / self.analysis.persistence.half_life());
        let (mut left, mut top, mut right, mut bottom) = (self.side, self.side, 0, 0);
        let shift = self.side.trailing_zeros();
        let occupied = self
            .active
            .iter()
            .filter_map(|&cell| {
                let hits = self.cells[cell as usize] * scale;
                if hits < HALF_INTENSITY / 509. {
                    return None;
                }
                let intensity = (255. * hits / (hits + HALF_INTENSITY) + 0.5) as u8;
                let (x, y) = (cell as usize & (self.side - 1), cell as usize >> shift);
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
                Some((cell, intensity))
            })
            .collect();
        Reading {
            side: self.side,
            occupied,
            stereo,
            bounds: [left, top, right, bottom],
        }
    }
}

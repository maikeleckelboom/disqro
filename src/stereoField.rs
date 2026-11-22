//! Paired PCM occupancy in a fixed square. Sample time controls phosphor
//! decay, including during digital silence. Neither coordinate is synthesized.
use crate::spectrum::{Speed, Stereo, choices};
use kira::Frame;
use serde::{Deserialize, Serialize};
pub const SIDE: usize = 256;

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
    pub bounds: Option<[usize; 4]>,
    pub intensity: Vec<u8>,
    pub stereo: Stereo,
}
pub struct Field {
    analysis: Analysis,
    rate: u32,
    cells: Vec<f32>,
    scale: f32,
    previous: Option<(f32, f32)>,
    pub settled: bool,
    silent_frames: usize,
}
impl Field {
    pub fn new(analysis: Analysis, rate: u32) -> Self {
        Self {
            analysis,
            rate,
            cells: vec![0.; SIDE * SIDE],
            scale: 1.,
            previous: None,
            settled: false,
            silent_frames: 0,
        }
    }
    fn deposit(&mut self, x: f32, y: f32, weight: f32) {
        let (i, j) = (x as usize, y as usize);
        let (fx, fy) = (x - i as f32, y - j as f32);
        let cell = j * SIDE + i;
        self.cells[cell] += weight * (1. - fx) * (1. - fy);
        self.cells[cell + 1] += weight * fx * (1. - fy);
        self.cells[cell + SIDE] += weight * (1. - fx) * fy;
        self.cells[cell + SIDE + 1] += weight * fx * fy;
    }
    pub fn push(&mut self, frames: &[Frame]) {
        if self.settled && frames.iter().all(|f| *f == Frame::ZERO) {
            return;
        }
        let half = self.analysis.persistence.half_life();
        self.scale *= (-((frames.len() as f32) / (self.rate as f32 * half))).exp2();
        if self.scale < 0.125 {
            for c in &mut self.cells {
                *c *= self.scale;
            }
            self.scale = 1.;
        }
        let gain = match self.analysis.gain {
            Gain::Unity => 1.,
            Gain::Double => 2.,
            Gain::Quad => 4.,
        };
        let weight = 48000. / self.rate as f32 / self.scale;
        let k = (SIDE - 2) as f32 * 0.5;
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
            let (x, y) = ((1. + x) * k, (1. - y) * k);
            if self.analysis.ink == Ink::Trace
                && let Some((a, b)) = self.previous
            {
                let steps = ((x - a).abs().max((y - b).abs()).ceil() as usize).clamp(1, 32);
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    self.deposit(a + (x - a) * t, b + (y - b) * t, weight / steps as f32);
                }
            } else {
                self.deposit(x, y, weight);
            }
            self.previous = Some((x, y));
        }
        self.settled = self.silent_frames as f32 > self.rate as f32 * half * 18.;
        if self.settled {
            self.cells.fill(0.);
        }
    }
    pub fn reading(&self, stereo: Stereo) -> Reading {
        // Bounded monotone density response, normalized for the decay window.
        let scale = self.scale * (0.24 / self.analysis.persistence.half_life());
        let intensity: Vec<u8> = self
            .cells
            .iter()
            .map(|v| {
                let hits = v * scale;
                (255. * hits / (hits + 2.5)).round() as u8
            })
            .collect();
        let (mut x, mut y, mut right, mut bottom) = (SIDE, SIDE, 0, 0);
        for (row, line) in intensity.as_chunks::<SIDE>().0.iter().enumerate() {
            if let Some(first) = line.iter().position(|v| *v != 0) {
                x = x.min(first);
                right = right.max(1 + line.iter().rposition(|v| *v != 0).unwrap());
                y = y.min(row);
                bottom = row + 1;
            }
        }
        let bounds = (right > x).then_some([x, y, right, bottom]);
        Reading {
            intensity,
            stereo,
            bounds,
        }
    }
}

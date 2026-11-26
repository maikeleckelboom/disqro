//! Paired PCM occupancy at the largest live subscriber's physical plot size. Sample time controls phosphor
//! decay, including during digital silence. Neither coordinate is synthesized.
use crate::spectrum::{Speed, Stereo, choices};
use kira::Frame;
use serde::{Deserialize, Serialize};
pub const MAX_SIDE: usize = 2049;
const BAND_HEIGHT: usize = 64;

// Logarithmic high-density shoulder, with a softer toe for isolated samples.
// Use the exponent and six mantissa bits to avoid a logarithm per active cell.
fn intensity_table(ink: Ink) -> &'static [u8] {
    fn table(ink: Ink) -> Box<[u8]> {
        (0..16384)
            .map(|i| {
                let mut hits = f32::from_bits(i << 17);
                // A fast scope sweep spends only a fraction of a sample at each
                // pixel. Lift just this toe, preserving dwell and segment ordering.
                if ink == Ink::Trace && hits < 1. {
                    hits = hits.sqrt();
                }
                let octaves = hits.ln_1p() / std::f32::consts::LN_2;
                let value = 0.86 * (hits / (hits + 2.2)) + 0.14 * (octaves / (octaves + 6.));
                (255. * value).round() as u8
            })
            .collect()
    }
    static DENSITY: std::sync::LazyLock<Box<[u8]>> =
        std::sync::LazyLock::new(|| table(Ink::Density));
    static TRACE: std::sync::LazyLock<Box<[u8]>> = std::sync::LazyLock::new(|| table(Ink::Trace));
    match ink {
        Ink::Density => &DENSITY,
        Ink::Trace => &TRACE,
    }
}

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
    pub bands: Vec<Raster>,
    pub stereo: Stereo,
}

#[derive(PartialEq)]
pub struct Raster {
    // Half-open pixel bounds in the presentation grid.
    pub bounds: [usize; 4],
    pub intensity: Vec<u8>,
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
    pub fn new(analysis: Analysis, rate: u32, side: usize) -> Self {
        let side = side.clamp(3, MAX_SIDE);
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
        // An isolated full-scale transient may traverse the entire plot. Bound
        // total line work for pathological alternating/Nyquist-frequency PCM.
        let mut line_budget = frames.len() * 64;
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
                let steps = ((x - a).abs().max((y - b).abs()).ceil() as usize).max(1);
                if steps > line_budget {
                    self.deposit(x, y, weight);
                    self.previous = Some((x, y));
                    continue;
                }
                line_budget -= steps;
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
    pub fn side(&self) -> usize {
        self.side
    }

    pub fn resize(&mut self, side: usize) {
        let side = side.clamp(3, MAX_SIDE);
        if self.side == side {
            return;
        }
        let mut next = Self::new(self.analysis, self.rate, side);
        let ratio = (next.side - 2) as f32 / (self.side - 2) as f32;
        let last = next.side as f32 - 1.001;
        let project = |value: f32| ((value - 0.5) * ratio + 0.5).clamp(0., last);
        for &cell in &self.active {
            let i = cell as usize;
            next.deposit(
                project((i % self.side) as f32),
                project((i / self.side) as f32),
                self.cells[i],
            );
        }
        next.scale = self.scale;
        next.previous = self.previous.map(|(x, y)| (project(x), project(y)));
        next.settled = self.settled;
        next.silent_frames = self.silent_frames;
        *self = next;
    }

    pub fn reading(&self, stereo: Stereo, side: usize) -> Reading {
        let side = side.clamp(3, MAX_SIDE);
        // A one-dimensional sweep spends half as long in a pixel when its
        // diameter doubles. Compensate that linear change without making a
        // diffuse two-dimensional distribution as bright as a narrow line.
        let exposure = (side as f32 / 769.).clamp(0.5, 2.5);
        let scale = self.scale * exposure * (0.24 / self.analysis.persistence.half_life());
        let table = intensity_table(self.analysis.ink);
        let intensity = |mass: f32| table[(mass.to_bits() >> 17) as usize];
        let mut extents = [[side, side, 0, 0]; MAX_SIDE.div_ceil(BAND_HEIGHT)];
        let bounds = &mut extents[..side.div_ceil(BAND_HEIGHT)];
        let pixel = |i: usize| ((2 * i + 1) * side / (2 * self.side)).min(side - 1);
        let xy = |cell: usize| (pixel(cell % self.side), pixel(cell / self.side));
        let native = side == self.side;
        for &index in &self.active {
            let i = index as usize;
            if native && intensity(self.cells[i] * scale) == 0 {
                continue;
            }
            let (x, y) = if native { (i % side, i / side) } else { xy(i) };
            let b = &mut bounds[y / BAND_HEIGHT];
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x + 1);
            b[3] = b[3].max(y + 1);
        }
        // Join neighboring bands when one atlas insertion costs little extra
        // empty area. Diagonals retain narrow strips and mono uses one sprite.
        let mut bands: Vec<Raster> = Vec::new();
        let mut band_at = [0; MAX_SIDE.div_ceil(BAND_HEIGHT)];
        for (row, &b) in bounds.iter().enumerate() {
            if b[2] == 0 {
                continue;
            }
            let area = |[x, y, r, d]: [usize; 4]| (r - x) * (d - y);
            if let Some(last) = bands.last_mut() {
                let a = last.bounds;
                let union = [a[0].min(b[0]), a[1], a[2].max(b[2]), b[3]];
                if area(union) <= (area(a) + area(b)) * 9 / 8 + 256 {
                    last.bounds = union;
                    band_at[row] = bands.len() - 1;
                    continue;
                }
            }
            band_at[row] = bands.len();
            bands.push(Raster {
                bounds: b,
                intensity: Vec::new(),
            });
        }
        for band in &mut bands {
            let [x, y, r, d] = band.bounds;
            band.intensity.resize((r - x) * (d - y), 0);
        }
        let mut mass: Vec<Vec<f32>> = if native {
            Vec::new()
        } else {
            bands.iter().map(|b| vec![0.; b.intensity.len()]).collect()
        };
        for &index in &self.active {
            let i = index as usize;
            let hits = self.cells[i] * scale;
            let code = intensity(hits);
            if native && code == 0 {
                continue;
            }
            let (x, y) = if native { (i % side, i / side) } else { xy(i) };
            let slot = band_at[y / BAND_HEIGHT];
            let band = &mut bands[slot];
            let [left, top, right, _] = band.bounds;
            let offset = (y - top) * (right - left) + x - left;
            if native {
                band.intensity[offset] = code;
            } else {
                mass[slot][offset] += hits;
            }
        }
        for (band, hits) in bands.iter_mut().zip(mass) {
            for (out, hits) in band.intensity.iter_mut().zip(hits) {
                *out = intensity(hits);
            }
        }
        Reading {
            side,
            bands,
            stereo,
        }
    }
}

#[cfg(test)]
mod tests;

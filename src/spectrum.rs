//! Analysis of contiguous stereo mix frames. FFT line levels use sine-peak
//! calibration; integrated bands use window-energy normalization (mean square).
use kira::Frame;
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex32};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const POINTS: usize = 256;
pub const MAX_LEN: usize = 32768;
pub const FLOOR: f32 = -90.;
pub const BANDS: [(f32, f32, &str); 6] = [
    (20., 60., "Sub"),
    (60., 250., "Bass"),
    (250., 500., "Low mid"),
    (500., 2000., "Mid"),
    (2000., 6000., "High mid"),
    (6000., 20000., "High"),
];

macro_rules! choices {
    ($name:ident { $first:ident => $default:literal $(, $variant:ident => $label:literal)* }) => {
        #[derive(Debug,Clone,Copy,Default,Eq,PartialEq,Hash,Serialize,Deserialize)]
        pub enum $name { #[default] $first, $($variant),* }
        impl $name {
            pub const ALL: &[Self] = &[Self::$first, $(Self::$variant),*];
            pub fn label(self) -> &'static str {
                match self { Self::$first => $default, $(Self::$variant => $label),* }
            }
        }
    };
}
pub(crate) use choices;
choices!(Mode { Stereo => "Stereo power", Left => "Left", Right => "Right", Mid => "Mid", Side => "Side" });
choices!(Style { Trace => "Spectrum trace", Bands => "Band energy" });
choices!(Resolution { Standard => "Standard · 170 ms", Fast => "Fast · 85 ms", Fine => "Fine · 340 ms" });
choices!(Smoothing { Raw => "None", Twelfth => "1/12 octave", Third => "1/3 octave" });
choices!(Tilt { Flat => "Flat", Three => "+3 dB/oct", Six => "+6 dB/oct" });
choices!(Hold { Short => "1 second", Off => "Off", Long => "2 seconds" });
choices!(Release { Normal => "36 dB/s", Slow => "18 dB/s", Fast => "72 dB/s" });
choices!(Range { Full => "20 Hz – 20 kHz", Bass => "20 Hz – 500 Hz" });
choices!(Speed { Normal => "30 Hz", Slow => "15 Hz", Fast => "60 Hz" });

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    pub style: Style,
    pub resolution: Resolution,
    pub smoothing: Smoothing,
    pub tilt: Tilt,
    pub hold: Hold,
    pub release: Release,
    pub range: Range,
    pub speed: Speed,
}
impl Resolution {
    pub fn len(self, rate: u32) -> usize {
        let seconds = match self {
            Self::Fast => 0.085,
            Self::Standard => 0.17,
            Self::Fine => 0.34,
        };
        1 << ((rate as f32 * seconds).log2().round() as u32).clamp(10, 15)
    }
}
impl Speed {
    pub fn hz(self) -> u32 {
        match self {
            Self::Slow => 15,
            Self::Normal => 30,
            Self::Fast => 60,
        }
    }
}
impl Range {
    pub fn high(self, rate: u32) -> f32 {
        (rate as f32 * 0.5).min(match self {
            Self::Full => 20000.,
            Self::Bass => 500.,
        })
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Stereo {
    /// Unweighted mean-square L, R, (L+R)/2, (L-R)/2.
    pub energy: [f64; 4],
    pub correlation: Option<f64>,
}
impl Stereo {
    /// Zero-lag normalized cross-power and unweighted energies over the
    /// supplied observation window. The phase measurement needs no transform.
    pub fn measure(frames: &[Frame]) -> Self {
        if frames.is_empty() {
            return Self::default();
        }
        let (mut ll, mut rr, mut cross) = (0., 0., 0.);
        for f in frames {
            let (l, r) = (f64::from(finite(f.left)), f64::from(finite(f.right)));
            ll += l * l;
            rr += r * r;
            cross += l * r;
        }
        let inv = 1. / frames.len() as f64;
        Self {
            energy: [
                ll * inv,
                rr * inv,
                ((ll + rr + 2. * cross) * 0.25 * inv).max(0.),
                ((ll + rr - 2. * cross) * 0.25 * inv).max(0.),
            ],
            correlation: (ll > 0. && rr > 0.).then(|| (cross / (ll * rr).sqrt()).clamp(-1., 1.)),
        }
    }
    pub fn mono_retained(self) -> Option<f64> {
        let [_, _, mid, side] = self.energy;
        (mid + side > 0.).then(|| mid / (mid + side))
    }
    pub fn opposed(self) -> bool {
        self.correlation.is_some_and(|r| r < 0.)
    }
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0. }
}

pub struct Transform {
    plan: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    left: Vec<Complex32>,
    right: Vec<Complex32>,
    scratch: Vec<Complex32>,
    coherent: f64,
    energy_scale: f64,
    // L, R, M, S squared peak-equivalent amplitudes.
    bins: Vec<[f64; 4]>,
    stereo: Option<Stereo>,
}

impl Transform {
    pub fn new(n: usize) -> Self {
        let plan = RealFftPlanner::new().plan_fft_forward(n);
        let window: Vec<_> = (0..n)
            .map(|i| {
                let phase = std::f64::consts::TAU * i as f64 / n as f64;
                (0.42 - 0.5 * phase.cos() + 0.08 * (2. * phase).cos()) as f32
            })
            .collect();
        let sum: f64 = window.iter().map(|&w| f64::from(w)).sum();
        let squares: f64 = window.iter().map(|&w| f64::from(w).powi(2)).sum();
        Self {
            input: plan.make_input_vec(),
            left: plan.make_output_vec(),
            right: plan.make_output_vec(),
            scratch: plan.make_scratch_vec(),
            plan,
            window,
            coherent: 1. / sum.powi(2),
            energy_scale: sum.powi(2) / (n as f64 * squares),
            bins: vec![[0.; 4]; n / 2 + 1],
            stereo: None,
        }
    }
    pub fn stereo(&mut self, frames: &[Frame]) -> Stereo {
        *self.stereo.get_or_insert_with(|| Stereo::measure(frames))
    }
    pub fn measure(&mut self, frames: &[Frame]) {
        let n = self.input.len();
        assert_eq!(frames.len(), n);
        self.stereo = None;
        for ((x, w), f) in self.input.iter_mut().zip(&self.window).zip(frames) {
            *x = finite(f.left) * w;
        }
        self.plan
            .process_with_scratch(&mut self.input, &mut self.left, &mut self.scratch)
            .expect("planned left FFT");
        for ((x, w), f) in self.input.iter_mut().zip(&self.window).zip(frames) {
            *x = finite(f.right) * w;
        }
        self.plan
            .process_with_scratch(&mut self.input, &mut self.right, &mut self.scratch)
            .expect("planned right FFT");
        for (k, ((bin, l), r)) in self
            .bins
            .iter_mut()
            .zip(&self.left)
            .zip(&self.right)
            .enumerate()
        {
            let scale = if k == 0 || k == n / 2 {
                self.coherent
            } else {
                4. * self.coherent
            };
            let l = realfft::num_complex::Complex64::new(l.re.into(), l.im.into());
            let r = realfft::num_complex::Complex64::new(r.re.into(), r.im.into());
            *bin = [
                l.norm_sqr() * scale,
                r.norm_sqr() * scale,
                (l + r).norm_sqr() * scale * 0.25,
                (l - r).norm_sqr() * scale * 0.25,
            ];
        }
    }
    pub(crate) fn power(&self, k: usize, mode: Mode) -> f64 {
        let [l, r, m, s] = self.bins[k];
        match mode {
            Mode::Stereo => (l + r) * 0.5,
            Mode::Left => l,
            Mode::Right => r,
            Mode::Mid => m,
            Mode::Side => s,
        }
    }
}

struct Cell {
    lo: f64,
    hi: f64,
    center: f64,
    tilt: f32,
}

/// Precomputed log coordinates, in FFT bins. The same projection survives
/// resize; only the GPUI mapping to logical pixels changes.
pub struct Projection {
    cells: Vec<Cell>,
    smoothed: bool,
    settings: Settings,
}
impl Projection {
    pub fn len(&self) -> usize {
        self.cells.len()
    }
    pub fn new(rate: u32, n: usize, settings: Settings) -> Self {
        let high = settings.range.high(rate);
        let scale = n as f64 / f64::from(rate);
        let oct = match settings.smoothing {
            Smoothing::Raw => 0.,
            Smoothing::Twelfth => 1. / 12.,
            Smoothing::Third => 1. / 3.,
        };
        let tilt = match settings.tilt {
            Tilt::Flat => 0.,
            Tilt::Three => 3.,
            Tilt::Six => 6.,
        };
        let mut cells = Vec::new();
        if settings.style == Style::Bands {
            for (lo, hi, _) in BANDS {
                if lo >= high {
                    break;
                }
                cells.push(Cell {
                    lo: f64::from(lo) * scale,
                    hi: f64::from(hi.min(high)) * scale,
                    center: 0.,
                    tilt: 0.,
                });
            }
        } else if high > 20. {
            let ratio = (f64::from(high) / 20.).powf(1. / (POINTS - 1) as f64);
            let half_width = ratio.sqrt().max(2f64.powf(oct / 2.));
            for i in 0..POINTS {
                let hz = 20. * ratio.powi(i as i32);
                cells.push(Cell {
                    lo: (hz / half_width).max(20.) * scale,
                    hi: (hz * half_width).min(f64::from(high)) * scale,
                    center: hz * scale,
                    tilt: tilt * (hz as f32 / 1000.).log2(),
                });
            }
        }
        Self {
            cells,
            settings,
            smoothed: oct > 0.,
        }
    }
    pub fn targets(&self, t: &Transform, out: &mut [f32; POINTS]) -> usize {
        let last = t.bins.len() - 1;
        let power = |k| t.power(k, self.settings.mode);
        let at = |x: f64| {
            let x = x.clamp(0., last as f64);
            let k = x.floor() as usize;
            power(k) + (power((k + 1).min(last)) - power(k)) * x.fract()
        };
        for (target, c) in out.iter_mut().zip(&self.cells) {
            let band = self.settings.style == Style::Bands;
            let value = if band || (self.smoothed && c.hi - c.lo >= 1.) {
                let mut sum = 0.;
                // A bin owns the interval halfway to either neighbor.
                for k in
                    (c.lo - 0.5).ceil().max(0.) as usize..=((c.hi + 0.5).floor() as usize).min(last)
                {
                    let edge = k == 0 || k == last;
                    let width = (c.hi.min(k as f64 + 0.5) - c.lo.max(k as f64 - 0.5)).max(0.);
                    let factor = if band {
                        (if edge { 2. } else { 0.5 }) * t.energy_scale
                    } else {
                        1.
                    };
                    sum += power(k) * width * factor;
                }
                if band { sum } else { sum / (c.hi - c.lo) }
            } else if c.hi - c.lo < 1. {
                at(c.center)
            } else {
                let mut peak = at(c.lo).max(at(c.hi));
                for k in c.lo.ceil() as usize..=(c.hi.floor() as usize).min(last) {
                    peak = peak.max(power(k));
                }
                peak
            };
            *target = ((10. * value.log10()) as f32 + c.tilt).max(FLOOR);
        }
        self.cells.len()
    }
}

pub struct Ballistics {
    pub levels: [f32; POINTS],
    pub peaks: [f32; POINTS],
    hold_left: [f32; POINTS],
}
impl Default for Ballistics {
    fn default() -> Self {
        Self {
            levels: [FLOOR; POINTS],
            peaks: [FLOOR; POINTS],
            hold_left: [0.; POINTS],
        }
    }
}
impl Ballistics {
    pub fn step(&mut self, targets: &[f32], dt: f32, settings: Settings) {
        let attack = 1. - (-dt / 0.012).exp();
        let fall = match settings.release {
            Release::Slow => 18.,
            Release::Normal => 36.,
            Release::Fast => 72.,
        };
        let hold = match settings.hold {
            Hold::Off => 0.,
            Hold::Short => 1.,
            Hold::Long => 2.,
        };
        for (i, &target) in targets.iter().enumerate() {
            let level = &mut self.levels[i];
            *level = if target > *level {
                *level + (target - *level) * attack
            } else {
                target.max(*level - fall * dt)
            };
            if settings.hold == Hold::Off || *level >= self.peaks[i] {
                self.peaks[i] = *level;
                self.hold_left[i] = hold;
            } else {
                let decay = (dt - self.hold_left[i]).max(0.);
                self.hold_left[i] = (self.hold_left[i] - dt).max(0.);
                self.peaks[i] = (*level).max(self.peaks[i] - fall * decay);
            }
        }
    }
}

#[cfg(test)]
mod tests;

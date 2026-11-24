//! Dekzer's Hann512 Stroke and fixed-grid Blackman512 overview measurements.
//! Time and power remain source facts. The renderer owns display colour.

use std::{collections::VecDeque, f64::consts::PI, fmt, sync::OnceLock};

pub const OCTAVE_BANDS: usize = 8;
pub const SOURCE_HOP: u64 = 256;
pub const ANALYSIS_RATE: u32 = 44_100;
/// Bounded, common decoder rates for the fixed-analysis-rate candidate.
use crate::waveform_render::SUPPORTED_OCTAVE_RATES;
/// Qualified 44.1/48 kHz input requirement: cell start +420 exclusive source frames.
/// Thus feed 164 frames after a full cell end, unless actual EOF is reached.
pub const CELL_LOOKAHEAD: u64 = 420;
/// Qualified 44.1/48 kHz context before the first global cell start.
/// Added rates derive their bounded context below; 200 ms decoder preroll covers all.
pub const CELL_PREROLL: u64 = 296;
const WINDOW: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OctaveDescriptor {
    /// Half-open coverage in source frames, clipped to the represented interval.
    pub start_frame: u64,
    pub end_frame: u64,
    /// Power normalized so an interior amplitude-.5 sine has total near one.
    /// Pool these values linearly; do not average already encoded RGB.
    pub energy: [f64; OCTAVE_BANDS],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OctaveError {
    UnsupportedRate,
    InvalidRange,
    InsufficientPreroll,
    NonFiniteSample,
    Finished,
}

impl fmt::Display for OctaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedRate => "unsupported source rate for the selected octave analysis",
            Self::InvalidRange => "invalid octave source interval",
            Self::InsufficientPreroll => "octave decode context starts after required preroll",
            Self::NonFiniteSample => "octave input contains a non-finite sample",
            Self::Finished => "octave analysis has already finished",
        })
    }
}

impl std::error::Error for OctaveError {}

pub struct OctaveAnalyzer {
    rate: u32,
    stroke: bool,
    represented_start: u64,
    represented_end: u64,
    cell: u64,
    received: u64,
    buffer_start: u64,
    mono: VecDeque<f64>,
    window: [f64; WINDOW],
    power_scale: f64,
    // scipy.signal.resample_poly default: symmetric Kaiser beta5, DC normalized,
    // multiplied by the reduced upsampling factor. At 48k this remains exactly
    // firwin(3201,1/160)*147. Direct phases remove the zero-phase FIR delay.
    resample_filter: Vec<f64>,
    up: u32,
    down: u32,
    preroll: u64,
    lookahead: u64,
    finished: bool,
}

impl OctaveAnalyzer {
    pub fn supports_rate(rate: u32, stroke: bool) -> bool {
        if stroke {
            matches!(rate, 44_100 | 48_000)
        } else {
            SUPPORTED_OCTAVE_RATES.contains(&rate)
        }
    }

    pub fn new(
        sample_rate: u32,
        decode_start: u64,
        represented_start: u64,
        represented_end: u64,
        stroke: bool,
    ) -> Result<Self, OctaveError> {
        if !Self::supports_rate(sample_rate, stroke) {
            return Err(OctaveError::UnsupportedRate);
        }
        // Bound integer-to-float coordinates well below loss of half-frame detail.
        if decode_start > represented_start
            || represented_start >= represented_end
            || represented_end > (1_u64 << 40)
        {
            return Err(OctaveError::InvalidRange);
        }
        let cell = represented_start / SOURCE_HOP * SOURCE_HOP;
        // Keep the qualified rates' exact admission and completion behavior.
        // At added rates, bound a 512-sample analysis window plus the Kaiser
        // FIR's ten source/analysis periods on either side. The full-cell
        // centre can be 128 source frames after the cell's start.
        let (preroll, lookahead) = if matches!(sample_rate, 44_100 | 48_000) {
            (CELL_PREROLL, CELL_LOOKAHEAD)
        } else {
            let before = CELL_PREROLL
                .max((268_u64 * u64::from(sample_rate)).div_ceil(u64::from(ANALYSIS_RATE)) + 16);
            (before, SOURCE_HOP / 2 + before + 2)
        };
        if decode_start > cell.saturating_sub(preroll) {
            return Err(OctaveError::InsufficientPreroll);
        }
        let window = std::array::from_fn(|n| {
            let phase = 2.0 * PI * n as f64 / (WINDOW - 1) as f64;
            if stroke {
                0.5 - 0.5 * phase.cos()
            } else {
                0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos()
            }
        });
        let power_scale = 16.0 / (WINDOW as f64 * window.iter().map(|v| v * v).sum::<f64>());
        let mut a = ANALYSIS_RATE;
        let mut b = sample_rate;
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let up = ANALYSIS_RATE / a;
        let down = sample_rate / a;
        Ok(Self {
            rate: sample_rate,
            stroke,
            represented_start,
            represented_end,
            cell,
            received: decode_start,
            buffer_start: decode_start,
            mono: VecDeque::with_capacity((preroll + lookahead) as usize),
            window,
            power_scale,
            resample_filter: if sample_rate != ANALYSIS_RATE && !stroke {
                polyphase_filter(up, down)
            } else {
                Vec::new()
            },
            up,
            down,
            preroll,
            lookahead,
            finished: false,
        })
    }

    /// Contiguous stereo frames beginning at `decode_start`. Mono decoding must
    /// duplicate its sample into both lanes. Reject a non-finite batch atomically.
    /// Input after all represented cells complete is accepted but not retained.
    pub fn push(
        &mut self,
        frames: &[[f32; 2]],
        mut emit: impl FnMut(OctaveDescriptor),
    ) -> Result<(), OctaveError> {
        if self.finished {
            return Err(OctaveError::Finished);
        }
        if frames.iter().flatten().any(|x| !x.is_finite()) {
            return Err(OctaveError::NonFiniteSample);
        }
        if self
            .received
            .checked_add(frames.len() as u64)
            .is_none_or(|end| end > (1_u64 << 40))
        {
            return Err(OctaveError::InvalidRange);
        }
        for frame in frames {
            if self.cell < self.represented_end {
                self.mono
                    .push_back((f64::from(frame[0]) + f64::from(frame[1])) * 0.5);
            }
            self.received += 1;
            if self.cell < self.represented_end && self.received >= self.cell + self.lookahead {
                emit(self.descriptor(None));
                self.advance();
            }
            self.trim();
        }
        Ok(())
    }

    /// Call only when the contiguous decode stream ends. `received` is actual
    /// EOF, or a decode stop after sufficient lookahead has emitted every cell.
    /// An early decode stop must not be misreported as actual source EOF.
    pub fn finish(&mut self, mut emit: impl FnMut(OctaveDescriptor)) -> Result<(), OctaveError> {
        if self.finished {
            return Err(OctaveError::Finished);
        }
        while self.cell < self.represented_end && self.cell < self.received {
            if self.received > self.represented_start {
                emit(self.descriptor(Some(self.received)));
            }
            self.advance();
        }
        self.mono.clear();
        self.finished = true;
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.cell >= self.represented_end || self.finished
    }

    fn advance(&mut self) {
        self.cell += SOURCE_HOP;
        self.trim();
    }

    fn trim(&mut self) {
        let keep = if self.cell >= self.represented_end {
            self.received
        } else {
            self.cell.saturating_sub(self.preroll).min(self.received)
        };
        while self.buffer_start < keep {
            self.mono.pop_front();
            self.buffer_start += 1;
        }
    }

    fn descriptor(&self, eof: Option<u64>) -> OctaveDescriptor {
        let end = (self.cell + SOURCE_HOP).min(eof.unwrap_or(u64::MAX));
        let centre = (self.cell as f64 + end as f64) * 0.5;
        let grid_centre = if self.stroke {
            self.cell as f64
        } else {
            centre * f64::from(ANALYSIS_RATE) / f64::from(self.rate)
        };
        let first = (grid_centre - 255.5).floor() as i64;
        let samples =
            std::array::from_fn(|j| self.grid_sample(first + j as i64, eof) * self.window[j]);
        let (real, imag) = fft(samples);
        let edges = [1, 3, 5, 9, 17, 33, 65, 129, 257];
        let energy = std::array::from_fn(|band| {
            (edges[band]..edges[band + 1])
                .map(|i| real[i] * real[i] + imag[i] * imag[i])
                .sum::<f64>()
                * self.power_scale
        });
        OctaveDescriptor {
            start_frame: self.cell.max(self.represented_start),
            end_frame: end.min(self.represented_end),
            energy,
        }
    }

    fn grid_sample(&self, index: i64, eof: Option<u64>) -> f64 {
        if self.stroke {
            return self.source(index, eof);
        }
        // scipy resample_poly returns exactly ceil(source_frames *up/down)
        // samples. Its FIR tail beyond that finite output is not source data.
        let output_len = eof.map(|n| (n * u64::from(self.up)).div_ceil(u64::from(self.down)));
        if index < 0 || output_len.is_some_and(|n| index as u64 >= n) {
            return 0.0;
        }
        self.resampled(index, eof)
    }

    fn resampled(&self, index: i64, eof: Option<u64>) -> f64 {
        if self.rate == ANALYSIS_RATE {
            return self.source(index, eof);
        }
        let up = i64::from(self.up);
        let half = i64::from(self.up.max(self.down)) * 10;
        let at = index * i64::from(self.down);
        let first = (at - half).div_euclid(up);
        let last = (at + half).div_euclid(up);
        (first..=last)
            .filter_map(|n| {
                let tap = at - n * up + half;
                self.resample_filter
                    .get(tap as usize)
                    .map(|h| self.source(n, eof) * h)
            })
            .sum()
    }

    fn source(&self, index: i64, eof: Option<u64>) -> f64 {
        if index < 0 || eof.is_some_and(|end| index as u64 >= end) {
            return 0.0;
        }
        let offset = (index as u64).checked_sub(self.buffer_start);
        let sample = offset.and_then(|i| self.mono.get(i as usize)).copied();
        debug_assert!(
            sample.is_some(),
            "required octave context missing: index {index}, buffer {}..{}, eof {eof:?}",
            self.buffer_start,
            self.received
        );
        sample.unwrap_or(0.0)
    }
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..40 {
        term *= x * x / (4.0 * f64::from(k * k));
        sum += term;
    }
    sum
}

fn polyphase_filter(up: u32, down: u32) -> Vec<f64> {
    let denominator = bessel_i0(5.0);
    let factor = up.max(down);
    let half = factor as i32 * 10;
    let mut taps: Vec<f64> = (-half..=half)
        .map(|i| {
            let x = f64::from(i) / f64::from(factor);
            let sinc = if i == 0 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            let radius = f64::from(i) / f64::from(half);
            sinc / f64::from(factor) * bessel_i0(5.0 * (1.0 - radius * radius).max(0.0).sqrt())
                / denominator
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap *= f64::from(up) / sum;
    }
    taps
}

// Fixed radix-2 transform: no planning, heap scratch or external FFT dependency.
fn fft(mut real: [f64; WINDOW]) -> ([f64; WINDOW], [f64; WINDOW]) {
    // Every window uses the same 511 rotations. Cache the exact original
    // expression once rather than recomputing trigonometry for a whole CD.
    static ROTATIONS: OnceLock<[(f64, f64); WINDOW - 1]> = OnceLock::new();
    let rotations = ROTATIONS.get_or_init(|| {
        let mut values = [(0.0, 0.0); WINDOW - 1];
        let mut width = 2;
        while width <= WINDOW {
            let half = width / 2;
            for j in 0..half {
                let angle = -2.0 * PI * j as f64 / width as f64;
                values[half - 1 + j] = angle.sin_cos();
            }
            width *= 2;
        }
        values
    });
    let mut imag = [0.0; WINDOW];
    for i in 0..WINDOW {
        let reversed = i.reverse_bits() >> (usize::BITS - 9);
        if i < reversed {
            real.swap(i, reversed);
        }
    }
    let mut width = 2;
    while width <= WINDOW {
        let half = width / 2;
        for j in 0..half {
            let (s, c) = rotations[half - 1 + j];
            for start in (0..WINDOW).step_by(width) {
                let a = start + j;
                let b = a + half;
                let tr = c * real[b] - s * imag[b];
                let ti = s * real[b] + c * imag[b];
                real[b] = real[a] - tr;
                imag[b] = imag[a] - ti;
                real[a] += tr;
                imag[a] += ti;
            }
        }
        width *= 2;
    }
    (real, imag)
}

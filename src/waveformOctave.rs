//! Dekzer's Hann512 Stroke and fixed-grid Blackman512 overview measurements.
//! Time and power remain source facts. The renderer owns display colour.

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex64};
use std::{
    collections::VecDeque,
    f64::consts::PI,
    fmt,
    sync::{Arc, OnceLock},
};

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
    fft: Arc<dyn RealToComplex<f64>>,
    bins: Vec<Complex64>,
    scratch: Vec<Complex64>,
    window: [f64; WINDOW],
    power_scale: f64,
    // scipy.signal.resample_poly default: symmetric Kaiser beta5, DC normalized,
    // multiplied by the reduced upsampling factor. At 48k this remains exactly
    // firwin(3201,1/160)*147. Direct phases remove the zero-phase FIR delay.
    resample_filter: Vec<f64>,
    resampled_window: Option<(i64, [f64; WINDOW])>,
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
        static TRANSFORM: OnceLock<Arc<dyn RealToComplex<f64>>> = OnceLock::new();
        let fft = TRANSFORM.get_or_init(|| RealFftPlanner::new().plan_fft_forward(WINDOW));
        Ok(Self {
            rate: sample_rate,
            stroke,
            represented_start,
            represented_end,
            cell,
            received: decode_start,
            buffer_start: decode_start,
            mono: VecDeque::with_capacity((preroll + lookahead) as usize),
            fft: fft.clone(),
            bins: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            window,
            power_scale,
            resample_filter: if sample_rate != ANALYSIS_RATE && !stroke {
                polyphase_filter(up, down)
            } else {
                Vec::new()
            },
            resampled_window: None,
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
        let mut pending = frames;
        while !pending.is_empty() && self.cell < self.represented_end {
            let next = self.cell + self.lookahead;
            let count = (next - self.received).min(pending.len() as u64) as usize;
            self.mono.extend(
                pending[..count]
                    .iter()
                    .map(|f| (f64::from(f[0]) + f64::from(f[1])) * 0.5),
            );
            self.received += count as u64;
            pending = &pending[count..];
            if self.received == next {
                emit(self.descriptor(None));
                self.advance();
            }
        }
        self.received += pending.len() as u64;
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
        self.mono.drain(..(keep - self.buffer_start) as usize);
        self.buffer_start = keep;
    }

    fn descriptor(&mut self, eof: Option<u64>) -> OctaveDescriptor {
        let end = (self.cell + SOURCE_HOP).min(eof.unwrap_or(u64::MAX));
        let centre = (self.cell as f64 + end as f64) * 0.5;
        let grid_centre = if self.stroke {
            self.cell as f64
        } else {
            centre * f64::from(ANALYSIS_RATE) / f64::from(self.rate)
        };
        let first = (grid_centre - 255.5).floor() as i64;
        let mut samples = [0.0; WINDOW];
        if self.resample_filter.is_empty() {
            let start = first.max(self.buffer_start as i64);
            let end = (first + WINDOW as i64).min(self.received as i64);
            if end > start {
                let source = (start as u64 - self.buffer_start) as usize
                    ..(end as u64 - self.buffer_start) as usize;
                let target = (start - first) as usize..(end - first) as usize;
                for (value, input) in samples[target].iter_mut().zip(self.mono.range(source)) {
                    *value = *input;
                }
            }
        } else {
            // Consecutive 512-sample windows overlap at every supported rate
            // above 22.05 kHz. A grid sample has the same Kaiser support in
            // both windows, including when the last window reaches EOF.
            let reused = if let Some((before, previous)) = &self.resampled_window {
                let advance = (first - before).clamp(0, WINDOW as i64) as usize;
                let reused = WINDOW - advance;
                samples[..reused].copy_from_slice(&previous[advance..]);
                reused
            } else {
                0
            };
            for (j, value) in samples.iter_mut().enumerate().skip(reused) {
                *value = self.grid_sample(first + j as i64, eof);
            }
            self.resampled_window = Some((first, samples));
        }
        for (sample, weight) in samples.iter_mut().zip(self.window) {
            *sample *= weight;
        }
        self.fft
            .process_with_scratch(&mut samples, &mut self.bins, &mut self.scratch)
            .expect("fixed 512-point real transform and matching scratch");
        let edges = [1, 3, 5, 9, 17, 33, 65, 129, 257];
        let energy = std::array::from_fn(|band| {
            self.bins[edges[band]..edges[band + 1]]
                .iter()
                .map(Complex64::norm_sqr)
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
        // scipy resample_poly returns exactly ceil(source_frames *up/down)
        // samples. Its FIR tail beyond that finite output is not source data.
        let output_len = eof.map(|n| (n * u64::from(self.up)).div_ceil(u64::from(self.down)));
        if index < 0 || output_len.is_some_and(|n| index as u64 >= n) {
            return 0.0;
        }
        self.resampled(index, eof)
    }

    fn resampled(&self, index: i64, eof: Option<u64>) -> f64 {
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

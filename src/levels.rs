//! Frame-counted stereo statistics. All storage is fixed at construction.
//! One bucket spans ~25 ms of output. Four buckets measure RMS/sample peak;
//! forty retain a one-second sample peak even if a reader misses a publication.
use kira::Frame;

const RMS_BUCKETS: usize = 4;
const HOLD_BUCKETS: usize = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Levels {
    pub rms: [f32; 2],
    pub peak: [f32; 2],
    pub held: [f32; 2],
}

#[derive(Clone, Copy, Default)]
struct Bucket {
    squares: [f64; 2],
    peaks: [f32; 2],
}

pub(super) struct Measurement {
    bucket_frames: usize,
    filled: usize,
    current: Bucket,
    recent: [Bucket; RMS_BUCKETS],
    peaks: [[f32; 2]; HOLD_BUCKETS],
    next: usize,
    warm: usize,
}

impl Measurement {
    pub fn new(rate: u32) -> Self {
        Self {
            bucket_frames: ((u64::from(rate) + 20) / 40).max(1) as usize,
            current: Bucket::default(),
            recent: [Bucket::default(); RMS_BUCKETS],
            peaks: [[0.; 2]; HOLD_BUCKETS],
            filled: 0,
            next: 0,
            warm: 0,
        }
    }

    pub fn observe(&mut self, mut input: &[Frame], mut publish: impl FnMut(Levels)) {
        while !input.is_empty() {
            let count = input.len().min(self.bucket_frames - self.filled);
            let (chunk, rest) = input.split_at(count);
            for frame in chunk {
                for (ch, value) in [frame.left, frame.right].into_iter().enumerate() {
                    // Invalid samples contribute silence to this channel's statistics.
                    if value.is_finite() {
                        self.current.peaks[ch] = self.current.peaks[ch].max(value.abs());
                        self.current.squares[ch] += f64::from(value) * f64::from(value);
                    }
                }
            }
            self.filled += count;
            input = rest;
            if self.filled == self.bucket_frames {
                publish(self.finish_bucket());
            }
        }
    }

    fn finish_bucket(&mut self) -> Levels {
        self.peaks[self.next] = self.current.peaks;
        self.recent[self.next % RMS_BUCKETS] = std::mem::take(&mut self.current);
        self.next = (self.next + 1) % HOLD_BUCKETS;
        self.warm = (self.warm + 1).min(RMS_BUCKETS);
        self.filled = 0;
        let frames = (self.warm * self.bucket_frames) as f64;
        let mut levels = Levels::default();
        for ch in 0..2 {
            let sum: f64 = self.recent.iter().map(|b| b.squares[ch]).sum();
            levels.rms[ch] = (sum / frames).sqrt() as f32;
            levels.peak[ch] = self.recent.iter().map(|b| b.peaks[ch]).fold(0., f32::max);
            levels.held[ch] = self.peaks.iter().map(|p| p[ch]).fold(0., f32::max);
        }
        levels
    }
}

/// Linear amplitude relative to digital full scale. Zero maps to −∞.
pub fn dbfs(amplitude: f32) -> f32 {
    20. * amplitude.log10()
}

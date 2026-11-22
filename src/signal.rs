//! Read-only observation on Kira's main track, before main gain and output clamp.
use crate::levels::{Levels, Measurement};
use kira::{
    Frame,
    effect::{Effect, EffectBuilder},
    info::Info,
};
use std::sync::{
    Arc,
    atomic::{
        AtomicU64, AtomicUsize,
        Ordering::{Relaxed, SeqCst},
    },
};

#[derive(Clone, Default)]
pub struct SignalMonitor(Arc<Shared>);

#[derive(Default)]
struct Shared {
    readers: AtomicUsize,
    epoch: AtomicU64,
    latest: Publication,
}

// One audio writer. SeqCst gives both version checks and all payload accesses
// a single order. A reader overlapping publication can skip this refresh.
#[derive(Default)]
struct Publication {
    version: AtomicU64,
    epoch: AtomicU64,
    rms: AtomicU64,
    peak: AtomicU64,
    held: AtomicU64,
}

fn pack([l, r]: [f32; 2]) -> u64 {
    u64::from(l.to_bits()) | (u64::from(r.to_bits()) << 32)
}
fn unpack(bits: u64) -> [f32; 2] {
    [
        f32::from_bits(bits as u32),
        f32::from_bits((bits >> 32) as u32),
    ]
}

impl Publication {
    fn write(&self, epoch: u64, levels: Levels) {
        let version = self.version.load(SeqCst);
        self.version.store(version.wrapping_add(1), SeqCst);
        self.rms.store(pack(levels.rms), SeqCst);
        self.peak.store(pack(levels.peak), SeqCst);
        self.held.store(pack(levels.held), SeqCst);
        self.epoch.store(epoch, SeqCst);
        self.version.store(version.wrapping_add(2), SeqCst);
    }

    fn read(&self, epoch: u64) -> Option<Levels> {
        let version = self.version.load(SeqCst);
        if !version.is_multiple_of(2) || self.epoch.load(SeqCst) != epoch {
            return None;
        }
        let levels = Levels {
            rms: unpack(self.rms.load(SeqCst)),
            peak: unpack(self.peak.load(SeqCst)),
            held: unpack(self.held.load(SeqCst)),
        };
        (self.version.load(SeqCst) == version).then_some(levels)
    }
}

impl SignalMonitor {
    pub fn subscribe(&self) -> LevelReader {
        if self.0.readers.fetch_add(1, SeqCst) == 0 {
            self.reset();
        }
        LevelReader(self.0.clone())
    }

    /// Start a fresh measurement for a replacement voice or first subscriber.
    pub fn reset(&self) {
        self.0.epoch.fetch_add(1, SeqCst);
    }
}

pub struct LevelReader(Arc<Shared>);
impl LevelReader {
    pub fn epoch(&self) -> u64 {
        self.0.epoch.load(SeqCst)
    }
    pub fn latest(&self) -> Option<Levels> {
        let shared = &self.0;
        shared.latest.read(shared.epoch.load(SeqCst))
    }
}
impl Drop for LevelReader {
    fn drop(&mut self) {
        self.0.readers.fetch_sub(1, SeqCst);
    }
}

impl EffectBuilder for SignalMonitor {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, ()) {
        (
            Box::new(SignalTap {
                shared: self.0,
                rate: 0,
                epoch: 0,
                meter: Measurement::new(0),
            }),
            (),
        )
    }
}

struct SignalTap {
    shared: Arc<Shared>,
    rate: u32,
    epoch: u64,
    meter: Measurement,
}

impl Effect for SignalTap {
    fn init(&mut self, rate: u32, _: usize) {
        self.on_change_sample_rate(rate);
    }
    fn on_change_sample_rate(&mut self, rate: u32) {
        self.rate = rate;
        self.epoch = self.shared.epoch.fetch_add(1, SeqCst).wrapping_add(1);
        self.meter = Measurement::new(rate);
        self.shared.latest.write(self.epoch, Levels::default());
    }
    fn process(&mut self, input: &mut [Frame], _: f64, _: &Info) {
        self.observe(input);
    }
}

impl SignalTap {
    // Only this boundary sees Kira's frames. Future reductions receive a shared slice.
    fn observe(&mut self, input: &[Frame]) {
        if self.shared.readers.load(Relaxed) == 0 {
            return;
        }
        let epoch = self.shared.epoch.load(SeqCst);
        if self.epoch != epoch {
            self.meter = Measurement::new(self.rate);
            self.epoch = epoch;
        }
        self.meter
            .observe(input, |levels| self.shared.latest.write(epoch, levels));
    }
}

#[cfg(test)]
#[path = "signalTests.rs"]
mod tests;

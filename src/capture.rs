//! Stereo observations cross the callback boundary in complete, sequence-stamped blocks.
//! Only effect construction and the analysis worker access the consumer mutex.
use kira::Frame;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc, Mutex,
    atomic::{
        AtomicU64, AtomicUsize,
        Ordering::{AcqRel, Acquire, Relaxed, Release},
    },
};

pub const BLOCK: usize = 128;
const SLOTS: usize = 256;

pub struct Block {
    pub epoch: u64,
    pub start: u64,
    pub rate: u32,
    pub frames: [Frame; BLOCK],
}

#[derive(Default)]
pub struct Capture {
    readers: AtomicUsize,
    epoch: AtomicU64,
    latest_end: AtomicU64,
    consumer: Mutex<Option<Consumer<Block>>>,
}

pub struct Demand(Arc<Capture>);
impl Drop for Demand {
    fn drop(&mut self) {
        self.0.readers.fetch_sub(1, AcqRel);
    }
}

impl Capture {
    pub fn writer(&self) -> Writer {
        let (producer, reader) = RingBuffer::new(SLOTS);
        *self.consumer.lock().unwrap() = Some(reader);
        Writer {
            producer,
            epoch: 0,
            next: 0,
            partial: [Frame::ZERO; BLOCK],
            filled: 0,
        }
    }
    pub fn subscribe(self: &Arc<Self>) -> Demand {
        if self.readers.fetch_add(1, AcqRel) == 0 {
            self.reset();
        }
        Demand(self.clone())
    }
    pub fn reset(&self) {
        self.epoch.fetch_add(1, AcqRel);
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Acquire)
    }

    /// A stalled worker gets at most one ringful per pass. Missed blocks are
    /// visible as gaps even when the producer couldn't enqueue a newer block.
    pub fn drain(&self, mut take: impl FnMut(Block)) -> u64 {
        if let Some(rx) = self.consumer.lock().unwrap().as_mut() {
            for _ in 0..rx.slots() {
                if let Ok(block) = rx.pop() {
                    take(block);
                }
            }
        }
        self.latest_end.load(Acquire)
    }
}

pub struct Writer {
    producer: Producer<Block>,
    epoch: u64,
    next: u64,
    partial: [Frame; BLOCK],
    filled: usize,
}

impl Writer {
    pub fn observe(&mut self, capture: &Capture, rate: u32, mut frames: &[Frame]) {
        if capture.readers.load(Relaxed) == 0 {
            return;
        }
        let epoch = capture.epoch();
        if epoch != self.epoch {
            self.filled = 0;
            self.epoch = epoch;
        }
        while !frames.is_empty() {
            let n = frames.len().min(BLOCK - self.filled);
            self.partial[self.filled..self.filled + n].copy_from_slice(&frames[..n]);
            frames = &frames[n..];
            self.filled += n;
            if self.filled == BLOCK {
                let accepted = self
                    .producer
                    .push(Block {
                        epoch,
                        start: self.next,
                        rate,
                        frames: self.partial,
                    })
                    .is_ok();
                self.next += BLOCK as u64;
                self.filled = 0;
                capture.latest_end.store(self.next, Release);
                if let Some(d) = crate::diagnostics::get() {
                    let counter = if accepted {
                        &d.capture_frames
                    } else {
                        &d.capture_drops
                    };
                    counter.fetch_add(BLOCK as u64, Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_restart_and_partial_blocks_preserve_stereo_and_report_gaps() {
        let capture = Arc::new(Capture::default());
        let mut writer = capture.writer();
        let chunk = [Frame::new(0.25, -0.75); BLOCK];
        writer.observe(&capture, 48000, &chunk);
        assert_eq!(capture.drain(|_| panic!()), 0);
        let first = capture.subscribe();
        for _ in 0..SLOTS + 3 {
            writer.observe(&capture, 48000, &chunk);
        }
        let mut count = 0;
        let end = capture.drain(|b| {
            assert_eq!(b.start, count * BLOCK as u64);
            assert_eq!(b.frames, chunk);
            count += 1;
        });
        assert_eq!(count as usize, SLOTS);
        assert_eq!(end, ((SLOTS + 3) * BLOCK) as u64);
        let epoch = capture.epoch();
        let second = capture.subscribe();
        drop(first);
        writer.observe(&capture, 48000, &chunk[..9]);
        assert_eq!(capture.epoch(), epoch);
        capture.reset();
        writer.observe(&capture, 96000, &[Frame::ZERO; BLOCK]);
        let mut seen = false;
        capture.drain(|b| {
            seen = true;
            assert_eq!((b.start, b.epoch, b.rate), (end, capture.epoch(), 96000));
            assert_eq!(b.frames, [Frame::ZERO; BLOCK]);
        });
        assert!(seen);
        drop(second);
        let old = capture.epoch();
        let _new = capture.subscribe();
        assert_ne!(old, capture.epoch());
    }
}

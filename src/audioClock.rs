//! A bounded view of rendered source time, independent of transport polling.
use std::{
    sync::atomic::{AtomicU64, Ordering::SeqCst},
    time::Instant,
};

const HISTORY: usize = 8;
const JITTER_NS: u64 = 2_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Block {
    id: u64,
    observed: u64,
    start: u64,
    end: u64,
    from: f64,
    to: f64,
}

#[derive(Debug, Default)]
struct Slot {
    sequence: AtomicU64,
    words: [AtomicU64; 6],
}

impl Slot {
    fn write(&self, block: Block) {
        // One renderer writes each voice. Ordering all fields with the two
        // sequence changes prevents a reader from pairing a cursor and time
        // from different publications, even when the slot is reused.
        self.sequence.fetch_add(1, SeqCst);
        for (word, value) in self.words.iter().zip([
            block.id,
            block.observed,
            block.start,
            block.end,
            block.from.to_bits(),
            block.to.to_bits(),
        ]) {
            word.store(value, SeqCst);
        }
        self.sequence.fetch_add(1, SeqCst);
    }

    fn read(&self, id: u64) -> Option<Block> {
        let begin = self.sequence.load(SeqCst);
        if !begin.is_multiple_of(2) {
            return None;
        }
        let [found, observed, start, end, from, to] =
            self.words.each_ref().map(|word| word.load(SeqCst));
        (found == id && begin == self.sequence.load(SeqCst)).then(|| Block {
            id,
            observed,
            start,
            end,
            from: f64::from_bits(from),
            to: f64::from_bits(to),
        })
    }
}

#[derive(Debug)]
pub(super) struct Clock {
    epoch: Instant,
    initial: f64,
    latest: AtomicU64,
    blocks: [Slot; HISTORY],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Position {
    pub frame: f64,
    pub advancing: bool,
    pub observation_age_ns: u64,
}

impl Clock {
    pub(super) fn new(initial: usize) -> Self {
        Self {
            epoch: Instant::now(),
            initial: initial as f64,
            latest: AtomicU64::new(0),
            blocks: std::array::from_fn(|_| Slot::default()),
        }
    }

    fn at(&self, time: Instant) -> u64 {
        time.saturating_duration_since(self.epoch).as_nanos() as u64
    }

    fn slot(&self, id: u64) -> &Slot {
        &self.blocks[id as usize % HISTORY]
    }

    pub(super) fn position(&self, now: Instant) -> Option<Position> {
        self.position_at(self.at(now))
    }

    fn position_at(&self, now: u64) -> Option<Position> {
        let latest = self.latest.load(SeqCst);
        if latest == 0 {
            return Some(Position {
                frame: self.initial,
                advancing: false,
                observation_age_ns: 0,
            });
        }
        let mut before = None;
        for id in (latest.saturating_sub(HISTORY as u64 - 1).max(1)..=latest).rev() {
            let Some(block) = self.slot(id).read(id) else {
                continue;
            };
            if now >= block.start {
                let duration = block.end.saturating_sub(block.start).max(1);
                let progress =
                    now.saturating_sub(block.start).min(duration) as f64 / duration as f64;
                return Some(Position {
                    frame: block.from + (block.to - block.from) * progress,
                    advancing: now < block.end,
                    observation_age_ns: now.saturating_sub(block.observed),
                });
            }
            before = Some(Position {
                frame: block.from,
                advancing: true,
                observation_age_ns: now.saturating_sub(block.observed),
            });
        }
        before
    }
}

#[derive(Default)]
pub(super) struct Writer {
    observed: u64,
    schedule: u64,
    elapsed: u64,
    block: Block,
    continuing: bool,
}

impl Writer {
    pub(super) fn begin(&mut self, clock: &Clock, now: Instant) {
        self.begin_at(clock.at(now));
    }

    fn begin_at(&mut self, observed: u64) {
        let next = self.schedule + self.elapsed;
        self.observed = observed;
        // Small callback jitter need not move the sample grid. Re-anchor to
        // the measured time after a missed period or device scheduling change.
        self.schedule = if self.elapsed > 0 && observed.abs_diff(next) <= JITTER_NS {
            next
        } else {
            observed.max(self.block.start)
        };
        self.elapsed = 0;
        self.continuing = false;
    }

    pub(super) fn rendered(&mut self, clock: &Clock, from: f64, to: f64, seconds: f64, rate: f64) {
        let start = self.schedule + self.elapsed;
        self.elapsed += (seconds * 1e9).round() as u64;
        if to <= from {
            self.continuing = false;
            return;
        }
        if !self.continuing {
            self.block = Block {
                id: self.block.id + 1,
                observed: self.observed,
                start,
                from,
                ..Block::default()
            };
            self.continuing = true;
        }
        self.block.to = to;
        // Use only the source portion actually rendered if this chunk
        // drained or stalled before its end. No extrapolation past `to`.
        let source_ns = ((to - from) / rate * 1e9).round() as u64;
        self.block.end = (start + source_ns).min(self.schedule + self.elapsed);
        clock.slot(self.block.id).write(self.block);
        clock.latest.store(self.block.id, SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_at_arbitrary_times_uses_only_rendered_samples() {
        let clock = Clock::new(10);
        let mut writer = Writer::default();
        writer.begin_at(10_000_000);
        writer.rendered(&clock, 10., 490., 0.01, 48_000.);
        writer.begin_at(19_500_000);
        writer.rendered(&clock, 490., 970., 0.01, 48_000.);
        assert_eq!(clock.position_at(15_000_000).unwrap().frame, 250.);
        assert_eq!(clock.position_at(19_750_000).unwrap().frame, 478.);
        assert_eq!(clock.position_at(25_000_000).unwrap().frame, 730.);
        let stalled = clock.position_at(116_000_000).unwrap();
        assert_eq!(stalled.frame, 970.);
        assert!(!stalled.advancing);
        writer.begin_at(117_000_000);
        writer.rendered(&clock, 970., 970., 0.004, 48_000.);
        writer.rendered(&clock, 970., 1162., 0.004, 48_000.);
        assert_eq!(clock.position_at(120_000_000).unwrap().frame, 970.);
        assert_eq!(clock.position_at(123_000_000).unwrap().frame, 1066.);
    }

    #[test]
    fn interrupted_publication_never_exposes_mixed_time_and_position() {
        let clock = Clock::new(0);
        let mut writer = Writer::default();
        writer.begin_at(20_000_000);
        writer.rendered(&clock, 0., 480., 0.01, 48_000.);
        assert_eq!(clock.position_at(25_000_000).unwrap().frame, 240.);
        let slot = clock.slot(1);
        slot.sequence.fetch_add(1, SeqCst);
        slot.words[5].store(960.0f64.to_bits(), SeqCst);
        assert!(clock.position_at(25_000_000).is_none());
        assert_eq!(Clock::new(36).position_at(900_000_000).unwrap().frame, 36.);
    }

    #[test]
    fn ordinary_callback_jitter_keeps_240hz_projection_monotonic_at_both_zoom_extremes() {
        let clock = Clock::new(44_100 * 180);
        let mut writer = Writer::default();
        let rate = 44_100.;
        let mut base = 44_100. * 180.;
        let mut time = 10_000_000;
        let mut callback = 0;
        let mut previous = base;
        for display in 0..480 {
            let now = 11_000_000 + display * 1_000_000_000 / 240;
            while time <= now {
                let jitter = [0, 220_000, -180_000, 90_000][callback % 4];
                writer.begin_at((time as i64 + jitter) as u64);
                for _ in 0..7 {
                    writer.rendered(&clock, base, base + 63., 63. / rate, rate);
                    base += 63.;
                }
                time += 10_000_000;
                callback += 1;
            }
            let position = clock.position_at(now).unwrap();
            assert!(position.frame >= previous && position.frame <= base);
            if display > 0 {
                let frames = position.frame - previous;
                assert!((frames - rate / 240.).abs() <= 1e-3, "{display}: {frames}");
                for span in [32., 8. * rate, 120. * rate] {
                    let pixels = frames * 1920. / span;
                    assert!((pixels - rate * 1920. / 240. / span).abs() < 0.061);
                }
            }
            previous = position.frame;
        }
    }
}

//! A bounded view of rendered source time, independent of transport polling.
use std::{
    sync::atomic::{AtomicU64, Ordering::SeqCst},
    time::Instant,
};

const HISTORY: usize = 8;
// A visual estimate can bridge a late publication for at most four milliseconds.
const MAX_PREDICTION_NS: u64 = 4_000_000;
const DISCONTINUITY_NS: u64 = 25_000_000;
const PHASE_FILTER_NS: f64 = 120_000_000.;
const PHASE_RECOVERY_NS: f64 = 250_000_000.;
const MAX_RATE_CORRECTION: f64 = 0.005;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Block {
    id: u64,
    observed: u64,
    start: u64,
    end: u64,
    from: f64,
    to: f64,
    predict_ns: u64,
    phase_ns: i64,
}

#[derive(Debug, Default)]
struct Slot {
    sequence: AtomicU64,
    words: [AtomicU64; 8],
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
            block.predict_ns,
            block.phase_ns as u64,
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
        let [found, observed, start, end, from, to, predict_ns, phase_ns] =
            self.words.each_ref().map(|word| word.load(SeqCst));
        (found == id && begin == self.sequence.load(SeqCst)).then(|| Block {
            id,
            observed,
            start,
            end,
            from: f64::from_bits(from),
            to: f64::from_bits(to),
            predict_ns,
            phase_ns: phase_ns as i64,
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
    pub phase_ns: i64,
    pub prediction_ns: u64,
}

impl Position {
    pub(crate) fn held(frame: f64) -> Self {
        Self {
            frame,
            advancing: false,
            observation_age_ns: 0,
            phase_ns: 0,
            prediction_ns: 0,
        }
    }
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
            return Some(Position::held(self.initial));
        }
        let mut before = None;
        for id in (latest.saturating_sub(HISTORY as u64 - 1).max(1)..=latest).rev() {
            let Some(block) = self.slot(id).read(id) else {
                continue;
            };
            if now >= block.start {
                let duration = block.end.saturating_sub(block.start).max(1);
                let prediction_ns = if id == latest {
                    now.saturating_sub(block.end).min(block.predict_ns)
                } else {
                    0
                };
                let progress = (now.saturating_sub(block.start).min(duration) + prediction_ns)
                    as f64
                    / duration as f64;
                return Some(Position {
                    frame: block.from + (block.to - block.from) * progress,
                    advancing: now < block.end + if id == latest { block.predict_ns } else { 0 },
                    observation_age_ns: now.saturating_sub(block.observed),
                    phase_ns: block.phase_ns,
                    prediction_ns,
                });
            }
            before = Some(Position {
                frame: block.from,
                advancing: true,
                observation_age_ns: now.saturating_sub(block.observed),
                phase_ns: block.phase_ns,
                prediction_ns: 0,
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
    in_block: bool,
    uninterrupted: bool,
    filtered_phase: f64,
    rate_correction: f64,
    phase_ns: i64,
}

impl Writer {
    pub(super) fn begin(&mut self, clock: &Clock, now: Instant) {
        self.begin_at(clock.at(now));
    }

    fn begin_at(&mut self, observed: u64) {
        let next = self.schedule + self.elapsed;
        self.observed = observed;
        self.phase_ns = observed as i64 - next as i64;
        if self.uninterrupted && self.phase_ns.unsigned_abs() < DISCONTINUITY_NS {
            // Keep adjacent blocks continuous. Change the slope only by a
            // small amount, based on sustained phase error rather than the
            // arrival time of any one callback.
            let weight = (self.elapsed as f64 / PHASE_FILTER_NS).min(1.);
            self.filtered_phase += (self.phase_ns as f64 - self.filtered_phase) * weight;
            self.rate_correction = (self.filtered_phase / PHASE_RECOVERY_NS)
                .clamp(-MAX_RATE_CORRECTION, MAX_RATE_CORRECTION);
            self.schedule = next;
        } else {
            self.schedule = observed.max(self.block.end);
            self.filtered_phase = 0.;
            self.rate_correction = 0.;
            self.phase_ns = 0;
        }
        self.elapsed = 0;
        self.in_block = false;
    }

    pub(super) fn rendered(
        &mut self,
        clock: &Clock,
        from: f64,
        to: f64,
        seconds: f64,
        rate: f64,
        predict_until: f64,
    ) {
        let wall_scale = (1. + self.rate_correction) * 1e9;
        let start = self.schedule + self.elapsed;
        self.elapsed += (seconds * wall_scale).round() as u64;
        if to <= from {
            self.uninterrupted = false;
            self.in_block = false;
            if self.block.predict_ns > 0 {
                self.block.predict_ns = 0;
                clock.slot(self.block.id).write(self.block);
            }
            return;
        }
        if !self.in_block {
            self.block = Block {
                id: self.block.id + 1,
                observed: self.observed,
                start,
                from,
                phase_ns: self.phase_ns,
                ..Block::default()
            };
            self.in_block = true;
        }
        self.block.to = to;
        let source_ns = ((to - from) / rate * wall_scale).round() as u64;
        self.block.end = (start + source_ns).min(self.schedule + self.elapsed);
        // Allow one source frame of resampler rounding when classifying a
        // short render. A partial or empty block seals its predecessor.
        let full = to - from + 1. >= seconds * rate;
        self.uninterrupted = full;
        self.block.predict_ns = if full {
            let block_duration = self.block.end.saturating_sub(self.block.start).max(1);
            // Use the actual slope of the published block so the prediction
            // cannot cross the decoded, playable source bound.
            let available =
                (predict_until - to).max(0.) * block_duration as f64 / (to - self.block.from);
            (available as u64).min(MAX_PREDICTION_NS)
        } else {
            0
        };
        if !full {
            self.in_block = false;
        }
        clock.slot(self.block.id).write(self.block);
        clock.latest.store(self.block.id, SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_interpolates_history_and_holds_without_prediction() {
        let clock = Clock::new(10);
        let mut writer = Writer::default();
        writer.begin_at(10_000_000);
        writer.rendered(&clock, 10., 490., 0.01, 48_000., 970.);
        writer.begin_at(20_000_000);
        writer.rendered(&clock, 490., 970., 0.01, 48_000., 970.);
        assert_eq!(clock.position_at(15_000_000).unwrap().frame, 250.);
        assert_eq!(clock.position_at(19_750_000).unwrap().frame, 478.);
        assert_eq!(clock.position_at(25_000_000).unwrap().frame, 730.);
        let stalled = clock.position_at(116_000_000).unwrap();
        assert_eq!(stalled.frame, 970.);
        assert!(!stalled.advancing);
        writer.begin_at(117_000_000);
        writer.rendered(&clock, 970., 970., 0.004, 48_000., 1162.);
        writer.rendered(&clock, 970., 1162., 0.004, 48_000., 1162.);
        assert_eq!(clock.position_at(120_000_000).unwrap().frame, 970.);
        assert_eq!(clock.position_at(123_000_000).unwrap().frame, 1066.);
    }

    #[test]
    fn interrupted_publication_never_exposes_mixed_time_and_position() {
        let clock = Clock::new(0);
        let mut writer = Writer::default();
        writer.begin_at(20_000_000);
        writer.rendered(&clock, 0., 480., 0.01, 48_000., 970.);
        assert_eq!(clock.position_at(25_000_000).unwrap().frame, 240.);
        let slot = clock.slot(1);
        slot.sequence.fetch_add(1, SeqCst);
        slot.words[5].store(960.0f64.to_bits(), SeqCst);
        assert!(clock.position_at(25_000_000).is_none());
        assert_eq!(Clock::new(36).position_at(900_000_000).unwrap().frame, 36.);
    }

    #[test]
    fn delayed_callbacks_are_published_only_when_they_arrive() {
        for refresh in [60, 120, 144, 240] {
            for callback_ms in [3, 5, 10, 20] {
                let rate = 48_000.;
                let initial = 48_000 * 1_800;
                let clock = Clock::new(initial);
                let mut writer = Writer::default();
                let mut committed = initial as f64;
                let period = callback_ms * 1_000_000;
                let jitter = [0, 220_000, -180_000, 90_000, 2_400_000, -140_000];
                let mut callback = 0;
                let mut previous = None;
                let mut max_error: f64 = 0.;
                for display in 0..(refresh * 3) {
                    let now = 41_000_000 + display * 1_000_000_000 / refresh;
                    loop {
                        let arrival = (10_000_000 + callback as u64 * period)
                            .checked_add_signed(jitter[callback % jitter.len()])
                            .unwrap();
                        if arrival > now {
                            break;
                        }
                        writer.begin_at(arrival);
                        let seconds = period as f64 / 1e9;
                        let to = committed + seconds * rate;
                        writer.rendered(&clock, committed, to, seconds, rate, f64::INFINITY);
                        committed = to;
                        callback += 1;
                    }
                    let position = clock.position_at(now).unwrap();
                    assert!(
                        position.frame <= committed + rate * MAX_PREDICTION_NS as f64 / 1e9 + 1e-6
                    );
                    let ideal = initial as f64 + (now - 10_000_000) as f64 * rate / 1e9;
                    max_error = max_error.max((position.frame - ideal).abs());
                    if let Some(last) = previous {
                        let step: f64 = position.frame - last;
                        let nominal = rate / refresh as f64;
                        assert!(
                            (step / nominal - 1.).abs() < 0.006,
                            "{refresh} Hz / {callback_ms} ms at frame {display}: step {step}"
                        );
                    }
                    previous = Some(position.frame);
                }
                assert!(
                    max_error < rate * 0.002,
                    "{refresh} Hz / {callback_ms} ms: {max_error} samples"
                );
            }
        }
    }

    #[test]
    fn prediction_is_limited_by_decoded_source_and_ends_on_starvation() {
        let clock = Clock::new(0);
        let mut writer = Writer::default();
        writer.begin_at(10_000_000);
        writer.rendered(&clock, 0., 480., 0.01, 48_000., 576.);
        let limit = clock.position_at(24_000_000).unwrap();
        assert_eq!(limit.frame, 576.);
        assert!(!limit.advancing);
        writer.begin_at(21_000_000);
        writer.rendered(&clock, 480., 480., 0.01, 48_000., 576.);
        assert_eq!(clock.position_at(24_000_000).unwrap().frame, 480.);
        assert!(!clock.position_at(24_000_000).unwrap().advancing);
        writer.begin_at(70_000_000);
        writer.rendered(&clock, 480., 600., 0.01, 48_000., 600.);
        assert_eq!(clock.position_at(90_000_000).unwrap().frame, 600.);
        assert!(!clock.position_at(90_000_000).unwrap().advancing);
    }
}

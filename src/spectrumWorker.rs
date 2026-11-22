//! A single worker serves all visible playing panels. Control messages are
//! issued by the UI. Sample and presentation queues have fixed capacities.
use crate::{
    capture::{BLOCK, Block, Capture, Demand},
    diagnostics,
    spectrum::*,
};
use async_channel::{Receiver, Sender};
use kira::Frame;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, atomic::Ordering::Relaxed, mpsc},
    time::{Duration, Instant},
};

pub struct Reading {
    pub epoch: u64,
    pub rate: u32,
    pub len: usize,
    pub count: usize,
    pub levels: [f32; POINTS],
    pub peaks: [f32; POINTS],
    pub stereo: Stereo,
    pub published: Instant,
}
type Update = Option<Arc<Reading>>;
enum Control {
    Add(usize, Settings, Sender<Update>),
    Remove(usize),
}

pub struct Hub {
    pub capture: Arc<Capture>,
    tx: RefCell<Option<mpsc::Sender<Control>>>,
    next: std::cell::Cell<usize>,
}
pub struct Subscription {
    _demand: Demand,
    id: usize,
    tx: mpsc::Sender<Control>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.tx.send(Control::Remove(self.id));
    }
}
impl Hub {
    pub fn new(capture: Arc<Capture>) -> Self {
        Self {
            capture,
            tx: RefCell::default(),
            next: 0.into(),
        }
    }
    pub fn subscribe(
        &self,
        settings: Settings,
    ) -> anyhow::Result<(Subscription, Receiver<Update>)> {
        let mut worker = self.tx.borrow_mut();
        if worker.is_none() {
            let (tx, rx) = mpsc::channel();
            let capture = self.capture.clone();
            std::thread::Builder::new()
                .name("disqro-spectrum".into())
                .spawn(move || run(capture, rx))?;
            *worker = Some(tx);
        }
        let tx = worker.as_ref().unwrap();
        let id = self.next.get();
        self.next.set(id.wrapping_add(1));
        let demand = self.capture.subscribe();
        let (send, recv) = async_channel::bounded(1);
        if tx.send(Control::Add(id, settings, send)).is_err() {
            *worker = None;
            anyhow::bail!("Spectrum worker stopped");
        }
        Ok((
            Subscription {
                _demand: demand,
                id,
                tx: tx.clone(),
            },
            recv,
        ))
    }
}

struct History {
    frames: Vec<Frame>,
    valid: usize,
    end: u64,
    epoch: u64,
    rate: u32,
    continuity: u64,
    audible_end: u64,
}
impl History {
    fn new() -> Self {
        Self {
            frames: vec![Frame::ZERO; MAX_LEN * 2],
            valid: 0,
            end: 0,
            epoch: 0,
            rate: 0,
            continuity: 0,
            audible_end: 0,
        }
    }
    fn push(&mut self, b: Block) {
        if b.epoch != self.epoch || b.start != self.end || b.rate != self.rate {
            self.valid = 0;
            self.continuity += 1;
            self.audible_end = b.start;
        }
        self.rate = b.rate;
        self.epoch = b.epoch;
        self.end = b.start + BLOCK as u64;
        if let Some(i) = b.frames.iter().rposition(|f| f.left != 0. || f.right != 0.) {
            self.audible_end = b.start + i as u64 + 1;
        }
        // BLOCK divides MAX_LEN; the mirror gives every trailing FFT one contiguous slice.
        let i = b.start as usize % MAX_LEN;
        self.frames[i..i + BLOCK].copy_from_slice(&b.frames);
        self.frames[MAX_LEN + i..MAX_LEN + i + BLOCK].copy_from_slice(&b.frames);
        self.valid = (self.valid + BLOCK).min(MAX_LEN);
    }
    fn tail(&self, n: usize) -> &[Frame] {
        let i = MAX_LEN + self.end as usize % MAX_LEN;
        &self.frames[i - n..i]
    }
}

struct Client {
    settings: Settings,
    tx: Sender<Update>,
    filter: Ballistics,
    projection: Option<Projection>,
    continuity: u64,
    last_end: u64,
    settled: bool,
}
impl Client {
    fn new(settings: Settings, tx: Sender<Update>) -> Self {
        Self {
            settings,
            tx,
            filter: Ballistics::default(),
            projection: None,
            continuity: 0,
            last_end: 0,
            settled: false,
        }
    }
    fn clear(&mut self) {
        if self.projection.take().is_some() {
            let _ = self.tx.force_send(None);
        }
        self.filter = Ballistics::default();
        self.last_end = 0;
        self.settled = false;
    }
    fn update(&mut self, h: &History, transforms: &mut HashMap<usize, (u64, Transform)>) {
        let n = self.settings.resolution.len(h.rate);
        if self.continuity != h.continuity {
            self.clear();
            self.continuity = h.continuity;
        }
        if h.rate == 0 || h.valid < n || self.last_end == h.end {
            return;
        }
        let silent = h.end - h.audible_end >= n as u64;
        let dt = if self.last_end == 0 {
            1. / self.settings.speed.hz() as f32
        } else {
            (h.end - self.last_end) as f32 / h.rate as f32
        };
        self.last_end = h.end;
        if silent && self.settled {
            return;
        }
        let projection = self
            .projection
            .get_or_insert_with(|| Projection::new(h.rate, n, self.settings));
        let mut targets = [FLOOR; POINTS];
        let (stereo, count) = if silent {
            (Stereo::default(), projection.len())
        } else {
            let (end, transform) = transforms
                .entry(n)
                .or_insert_with(|| (u64::MAX, Transform::new(n)));
            if *end != h.end {
                let _time = diagnostics::Timer::measure(|d| &d.fft_us);
                transform.measure(h.tail(n));
                *end = h.end;
                if let Some(d) = diagnostics::get() {
                    d.fft_pairs.fetch_add(1, Relaxed);
                }
            }
            (
                transform.stereo,
                projection.targets(transform, &mut targets),
            )
        };
        self.filter.step(&targets[..count], dt, self.settings);
        self.settled = silent && self.filter.peaks[..count].iter().all(|p| *p <= FLOOR);
        let reading = Reading {
            epoch: h.epoch,
            rate: h.rate,
            len: n,
            count,
            stereo,
            levels: self.filter.levels,
            peaks: self.filter.peaks,
            published: Instant::now(),
        };
        let _ = self.tx.force_send(Some(Arc::new(reading)));
        if let Some(d) = diagnostics::get() {
            d.spectrum_updates.fetch_add(1, Relaxed);
        }
    }
}

fn run(capture: Arc<Capture>, rx: mpsc::Receiver<Control>) {
    let mut clients = HashMap::<usize, Client>::new();
    let mut transforms = HashMap::new();
    let mut h = History::new();
    let mut tick = 0u64;
    let mut due = Instant::now();
    let mut last_input = due;
    loop {
        let now = Instant::now();
        let message = if clients.is_empty() {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => break,
            }
        } else {
            // A zero timeout still handles pending control messages under load.
            match rx.recv_timeout(due.saturating_duration_since(now)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => None,
            }
        };
        if let Some(c) = message {
            match c {
                Control::Add(id, s, tx) => {
                    clients.insert(id, Client::new(s, tx));
                    due = Instant::now();
                    tick = 0;
                }
                Control::Remove(id) => {
                    clients.remove(&id);
                }
            }
            continue;
        }
        let now = Instant::now();
        let epoch = capture.epoch();
        let before = h.end;
        let continuity = h.continuity;
        let head = capture.drain(|b| {
            if b.epoch == epoch {
                h.push(b)
            }
        });
        // A rebuilt device can restart its frame sequence at zero. Invalidate
        // timestamps while keeping the immutable plans and their scratch space.
        if continuity != h.continuity {
            for (end, _) in transforms.values_mut() {
                *end = u64::MAX;
            }
        }
        if h.end != before {
            last_input = now;
        }
        let hz = clients
            .values()
            .map(|c| c.settings.speed.hz())
            .max()
            .unwrap_or(15);
        let current = h.epoch == epoch
            && head.saturating_sub(h.end) < (h.rate / 20) as u64
            && now - last_input < Duration::from_millis(150);
        for client in clients.values_mut() {
            if !current {
                if client.projection.is_some() {
                    client.clear();
                }
            } else if tick.is_multiple_of(60 / u64::from(client.settings.speed.hz())) {
                client.update(&h, &mut transforms);
            }
        }
        tick = tick.wrapping_add(60 / u64::from(hz));
        let period = Duration::from_secs_f64(1. / f64::from(hz));
        due += period;
        let finished = Instant::now();
        if due <= finished {
            due = finished + period;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed(h: &mut History, start: u64, epoch: u64, count: usize, rate: u32, value: Frame) {
        for offset in (0..count).step_by(BLOCK) {
            h.push(Block {
                epoch,
                start: start + offset as u64,
                rate,
                frames: [value; BLOCK],
            });
        }
    }

    #[test]
    fn slow_panel_keeps_only_latest_and_discontinuities_require_a_full_new_window() {
        let mut history = History::new();
        let (tx, rx) = async_channel::bounded(1);
        let mut left = Client::new(
            Settings {
                mode: Mode::Left,
                ..Default::default()
            },
            tx,
        );
        let (tx, side_rx) = async_channel::bounded(1);
        let mut side = Client::new(
            Settings {
                mode: Mode::Side,
                style: Style::Bands,
                ..Default::default()
            },
            tx,
        );
        let mut plans = HashMap::new();
        let n = Resolution::Standard.len(48000);
        feed(&mut history, 0, 1, n, 48000, Frame::new(0.5, -0.5));
        for client in [&mut left, &mut side] {
            client.update(&history, &mut plans);
        }
        assert_eq!(plans.len(), 1);
        assert_eq!(side_rx.try_recv().unwrap().unwrap().count, BANDS.len());
        let start = history.end;
        feed(&mut history, start, 1, n, 48000, Frame::new(0.25, 0.25));
        left.update(&history, &mut plans);
        let latest = rx.try_recv().unwrap().unwrap();
        assert_eq!(latest.stereo.correlation, Some(1.));
        assert_eq!(rx.len(), 0);
        let start = history.end + BLOCK as u64; // one full producer block was lost
        feed(&mut history, start, 1, BLOCK, 48000, Frame::new(1., 0.));
        left.update(&history, &mut plans);
        assert!(rx.try_recv().unwrap().is_none());
        let start = history.end;
        feed(&mut history, start, 2, n, 96000, Frame::ZERO);
        left.update(&history, &mut plans);
        assert!(rx.is_empty());
        let start = history.end;
        feed(&mut history, start, 2, n, 96000, Frame::ZERO);
        left.update(&history, &mut plans);
        let silent = rx.try_recv().unwrap().unwrap();
        assert_eq!((silent.epoch, silent.rate, silent.len), (2, 96000, n * 2));
        assert_eq!(silent.levels, [FLOOR; POINTS]);
        let start = history.end;
        feed(&mut history, start, 2, BLOCK, 96000, Frame::ZERO);
        left.update(&history, &mut plans);
        assert!(rx.is_empty(), "settled silence should not wake the panel");
    }
}

//! Pointer-driven audition on the existing Kira mixer. A decoder worker
//! exchanges preallocated PCM pages with the sound in both directions.
use super::WaveSeekAnchor;
use crate::{timeline::Source, waveform_source::Reader};
use anyhow::Result;
use kira::{AudioManager, Frame, backend::Backend, info::Info, sound::Sound, sound::SoundData};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    convert::Infallible,
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering::*},
        mpsc,
    },
    time::{Duration, Instant},
};

const PAGE: usize = 16_384;
const CACHE: usize = 6;
const IN_FLIGHT: usize = 3;
const POLL: Duration = Duration::from_millis(4);
const MOTION_TIMEOUT_NS: u64 = 65_000_000;
const MAX_RATE: f64 = 8.;
const MAX_DISTANCE_SECONDS: f64 = 0.018;
const FADE_SECONDS: f64 = 0.003;

#[derive(Clone, Copy, Debug)]
pub(super) struct Motion {
    serial: u64,
    id: u64,
    pub frame: f64,
    previous: f64,
    velocity: f64,
    at: u64,
}

#[derive(Debug)]
pub(crate) struct Control {
    epoch: Instant,
    next: AtomicU64,
    sequence: AtomicU64,
    pointer: [AtomicU64; 5],
}

impl Default for Control {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            next: AtomicU64::new(1),
            sequence: AtomicU64::new(0),
            pointer: Default::default(),
        }
    }
}

impl Control {
    fn nanos(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.epoch).as_nanos() as u64
    }

    fn write(&self, id: u64, frame: f64, previous: f64, velocity: f64, at: Instant) {
        // One foreground writer. A render block can use its previous sample
        // if it interrupts this publication.
        self.sequence.fetch_add(1, SeqCst);
        for (slot, value) in self.pointer.iter().zip([
            id,
            frame.to_bits(),
            previous.to_bits(),
            velocity.to_bits(),
            self.nanos(at),
        ]) {
            slot.store(value, SeqCst);
        }
        self.sequence.fetch_add(1, SeqCst);
    }

    fn read(&self) -> Option<Motion> {
        let serial = self.sequence.load(SeqCst);
        if serial == 0 || !serial.is_multiple_of(2) {
            return None;
        }
        let [id, frame, previous, velocity, at] =
            self.pointer.each_ref().map(|word| word.load(SeqCst));
        (serial == self.sequence.load(SeqCst)).then_some(Motion {
            serial,
            id,
            frame: f64::from_bits(frame),
            previous: f64::from_bits(previous),
            velocity: f64::from_bits(velocity),
            at,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum End {
    Release = 1,
    Cancel = 2,
}

#[derive(Debug)]
pub(crate) struct Gesture {
    pub id: u64,
    pub anchor: WaveSeekAnchor,
    pub origin: f64,
    pub was_playing: bool,
    pub bounds: Range<usize>,
    /// The authored program the gesture navigates in Full CD, or `None` for
    /// the queue-scoped Track perspective.
    pub program: Option<crate::library::TrackList>,
    control: Arc<Control>,
    end: AtomicU8,
    closed: AtomicU8,
}

impl Gesture {
    pub(crate) fn new(
        control: Arc<Control>,
        anchor: WaveSeekAnchor,
        origin: f64,
        was_playing: bool,
        bounds: Range<usize>,
    ) -> Self {
        Self {
            id: control.next.fetch_add(1, Relaxed),
            anchor,
            origin,
            was_playing,
            bounds,
            program: None,
            control,
            end: AtomicU8::new(0),
            closed: AtomicU8::new(0),
        }
    }

    /// Navigates the program's chapters regardless of the queue sequence.
    pub(crate) fn within_program(mut self, program: crate::library::TrackList) -> Self {
        self.program = Some(program);
        self
    }

    pub(crate) fn move_to(&self, frame: f64, previous: f64, velocity: f64, at: Instant) {
        if !self.closed() && self.ending().is_none() {
            let clamp =
                |frame: f64| frame.clamp(self.bounds.start as f64, (self.bounds.end - 1) as f64);
            self.control
                .write(self.id, clamp(frame), clamp(previous), velocity, at);
        }
    }

    pub(crate) fn frame(&self) -> Option<f64> {
        self.control
            .read()
            .filter(|m| m.id == self.id)
            .map(|m| m.frame)
    }

    pub(crate) fn finish(&self, end: End) {
        let _ = self.end.compare_exchange(0, end as u8, Release, Relaxed);
    }

    pub(crate) fn ending(&self) -> Option<End> {
        match self.end.load(Acquire) {
            1 => Some(End::Release),
            2 => Some(End::Cancel),
            _ => None,
        }
    }

    pub(crate) fn closed(&self) -> bool {
        self.closed.load(Acquire) != 0
    }

    pub(super) fn close(&self) {
        self.closed.store(1, Release);
    }
}

#[derive(Default)]
struct Config {
    id: AtomicU64,
    source: AtomicU64,
    start: AtomicUsize,
    end: AtomicUsize,
    rate: AtomicU32,
    volume: AtomicU32,
    failed: AtomicU64,
}

#[derive(Clone)]
struct Plan {
    id: u64,
    generation: u64,
    source: Arc<Source>,
}

struct Page {
    generation: u64,
    number: usize,
    len: usize,
    frames: Box<[Frame]>,
}

impl Page {
    fn new() -> Self {
        Self {
            generation: 0,
            number: 0,
            len: 0,
            frames: vec![Frame::ZERO; PAGE].into_boxed_slice(),
        }
    }
}

pub(super) struct Output {
    config: Arc<Config>,
    plan: Arc<Mutex<Option<Plan>>>,
    wake: mpsc::SyncSender<()>,
    source: Option<Arc<Source>>,
    generation: u64,
}

impl Output {
    pub fn install<B: Backend>(
        device: &mut AudioManager<B>,
        control: Arc<Control>,
    ) -> Result<Self> {
        let config = Arc::new(Config::default());
        let plan = Arc::new(Mutex::new(None));
        let (wake, listen) = mpsc::sync_channel(1);
        let (mut recycle, free) = RingBuffer::new(CACHE + IN_FLIGHT);
        let (write, ready) = RingBuffer::new(IN_FLIGHT);
        for _ in 0..CACHE + IN_FLIGHT {
            assert!(recycle.push(Page::new()).is_ok());
        }
        let sound = Audition {
            control: control.clone(),
            config: config.clone(),
            free: recycle,
            ready,
            pages: Default::default(),
            cursor: Cursor::default(),
            observed_motion: 0,
        };
        let worker_config = config.clone();
        let worker_plan = plan.clone();
        std::thread::Builder::new()
            .name("disqro-scrub-pcm".into())
            .spawn(move || {
                Decoder {
                    control,
                    config: worker_config,
                    plan: worker_plan,
                    free,
                    ready: write,
                    held: None,
                    published: Default::default(),
                    reader: None,
                }
                .run(listen);
            })?;
        device.play(sound)?;
        Ok(Self {
            config,
            plan,
            wake,
            source: None,
            generation: 0,
        })
    }

    pub fn start(&mut self, gesture: &Gesture, volume: f32) {
        self.stop();
        let source = &gesture.anchor.source;
        if self.source.as_ref() != Some(source) {
            self.source = Some(source.clone());
            self.generation += 1;
        }
        *self.plan.lock().unwrap() = Some(Plan {
            id: gesture.id,
            generation: self.generation,
            source: source.clone(),
        });
        self.set_volume(volume);
        self.config.source.store(self.generation, SeqCst);
        self.config.start.store(gesture.bounds.start, SeqCst);
        self.config.end.store(gesture.bounds.end, SeqCst);
        self.config.rate.store(source.rate, SeqCst);
        self.config.id.store(gesture.id, SeqCst);
        let _ = self.wake.try_send(());
    }

    pub fn stop(&self) {
        self.config.id.store(0, SeqCst);
        self.plan.lock().unwrap().take();
        let _ = self.wake.try_send(());
    }

    pub fn set_volume(&self, volume: f32) {
        self.config.volume.store(volume.to_bits(), Relaxed);
    }

    pub fn failed(&self, id: u64) -> bool {
        self.config.failed.load(Acquire) == id
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Decoder {
    control: Arc<Control>,
    config: Arc<Config>,
    plan: Arc<Mutex<Option<Plan>>>,
    free: Consumer<Page>,
    ready: Producer<Page>,
    held: Option<Page>,
    published: [Option<(u64, usize)>; CACHE],
    reader: Option<(u64, Reader)>,
}

impl Decoder {
    fn run(&mut self, wake: mpsc::Receiver<()>) {
        loop {
            let plan = self.plan.lock().unwrap().clone();
            if let Some(p) = plan {
                if self.load_one(&p) {
                    continue;
                }
                if wake.recv_timeout(POLL) == Err(mpsc::RecvTimeoutError::Disconnected) {
                    break;
                }
            } else if wake.recv().is_err() {
                break;
            }
        }
    }

    fn load_one(&mut self, plan: &Plan) -> bool {
        let current = || self.config.id.load(Acquire) == plan.id;
        if self.config.failed.load(Acquire) == plan.id || !current() {
            return false;
        }
        let Some(motion) = self.control.read().filter(|m| m.id == plan.id) else {
            return false;
        };
        let center = motion.frame as usize / PAGE;
        let direction = if motion.velocity < 0. { -1 } else { 1 };
        let needed = [0, -direction, direction, -2 * direction, 2 * direction];
        let Some(number) = needed
            .into_iter()
            .filter_map(|d| center.checked_add_signed(d))
            .find(|&n| {
                n * PAGE < plan.source.frames
                    && self.published[n % CACHE] != Some((plan.generation, n))
            })
        else {
            return false;
        };
        if self.ready.is_full() {
            return false;
        }
        let Some(mut page) = self.held.take().or_else(|| self.free.pop().ok()) else {
            return false;
        };
        let start = number * PAGE;
        let len = PAGE.min(plan.source.frames - start);
        let decoded = (|| {
            if self
                .reader
                .as_ref()
                .is_none_or(|(generation, _)| *generation != plan.generation)
            {
                self.reader = Some((plan.generation, Reader::open(&plan.source)?));
                if let Some(trace) = crate::diagnostics::get() {
                    trace.scrub_decoder_opens.fetch_add(1, Relaxed);
                }
            }
            self.reader
                .as_mut()
                .unwrap()
                .1
                .read_into(start, &mut page.frames[..len], &current)
        })();
        if decoded.is_err() || !current() {
            if current() {
                self.config.failed.store(plan.id, Release);
            }
            self.held = Some(page);
            return false;
        }
        page.generation = plan.generation;
        page.number = number;
        page.len = len;
        match self.ready.push(page) {
            Ok(()) => {
                self.published[number % CACHE] = Some((plan.generation, number));
                true
            }
            Err(rtrb::PushError::Full(page)) => {
                self.held = Some(page);
                false
            }
        }
    }
}

#[derive(Default)]
struct Cursor {
    motion: Option<Motion>,
    position: f64,
    step: f64,
    level: f32,
    sample: Frame,
    last: Frame,
    blend_from: Frame,
    blend: f32,
}

impl Cursor {
    fn aim(&mut self, motion: Motion, rate: f64) {
        if self.motion.is_some_and(|old| old.serial == motion.serial) {
            return;
        }
        let speed = motion.velocity.clamp(-MAX_RATE * rate, MAX_RATE * rate);
        let lag = motion.frame - self.position;
        let max_distance = (speed.abs() * MAX_DISTANCE_SECONDS).min(PAGE as f64);
        let reversal = speed * self.step < 0.;
        if self.motion.is_none_or(|old| old.id != motion.id)
            || reversal
            || speed * lag <= 0.
            || lag.abs() > max_distance
        {
            let travel = (motion.frame - motion.previous).abs().min(max_distance);
            self.position = motion.frame - travel.copysign(speed);
            self.blend_from = self.last;
            self.blend = 1.;
        }
        self.step = speed;
        self.motion = Some(motion);
    }

    fn render(
        &mut self,
        out: &mut [Frame],
        dt: f64,
        read: impl Fn(f64) -> Option<Frame>,
        active: bool,
        volume: f32,
    ) {
        if !active && self.level == 0. && self.blend == 0. {
            out.fill(Frame::ZERO);
            return;
        }
        let increment = (dt / FADE_SECONDS).min(1.) as f32;
        let target = self.motion.map_or(self.position, |motion| motion.frame);
        for frame in out {
            let distance = target - self.position;
            let audible = if active && self.step * distance > 0. {
                read(self.position)
            } else {
                None
            };
            if let Some(sample) = audible {
                self.sample = sample;
                self.position += (self.step * dt)
                    .abs()
                    .min(distance.abs())
                    .copysign(distance);
                self.level = (self.level + increment).min(1.);
            } else {
                self.level = (self.level - increment).max(0.);
            }
            let signal = self.sample * (self.level * volume);
            self.blend = (self.blend - increment).max(0.);
            self.last = signal * (1. - self.blend) + self.blend_from * self.blend;
            *frame = self.last;
        }
    }
}

struct Audition {
    control: Arc<Control>,
    config: Arc<Config>,
    free: Producer<Page>,
    ready: Consumer<Page>,
    pages: [Option<Page>; CACHE],
    cursor: Cursor,
    observed_motion: u64,
}

impl SoundData for Audition {
    type Error = Infallible;
    type Handle = ();

    fn into_sound(self) -> Result<(Box<dyn Sound>, ()), Infallible> {
        Ok((Box::new(self), ()))
    }
}

impl Sound for Audition {
    fn process(&mut self, out: &mut [Frame], dt: f64, _: &Info) {
        for _ in 0..IN_FLIGHT {
            let Ok(page) = self.ready.pop() else { break };
            let slot = page.number % CACHE;
            if let Some(previous) = self.pages[slot].replace(page) {
                // All pages are preallocated. The page just popped from the
                // ready queue leaves space for this one in the recycle ring.
                assert!(self.free.push(previous).is_ok());
            }
        }
        let id = self.config.id.load(SeqCst);
        let generation = self.config.source.load(SeqCst);
        let bounds = self.config.start.load(SeqCst)..self.config.end.load(SeqCst);
        let rate = f64::from(self.config.rate.load(SeqCst));
        let volume = f32::from_bits(self.config.volume.load(Relaxed));
        if id != 0
            && self.config.id.load(SeqCst) == id
            && let Some(motion) = self.control.read().filter(|motion| motion.id == id)
        {
            self.cursor.aim(motion, rate);
        }
        let now = self.control.nanos(Instant::now());
        let active = id != 0
            && self.config.id.load(SeqCst) == id
            && self.config.failed.load(Acquire) != id
            && self
                .cursor
                .motion
                .is_some_and(|m| m.id == id && now.saturating_sub(m.at) < MOTION_TIMEOUT_NS);
        let pages = &self.pages;
        let read = |position: f64| sample_at(pages, generation, &bounds, position);
        let first = self.cursor.position;
        self.cursor.render(out, dt, read, active, volume);
        if self.cursor.position != first
            && let Some(motion) = self.cursor.motion
            && self.observed_motion != motion.serial
        {
            self.observed_motion = motion.serial;
            if let Some(trace) = crate::diagnostics::get() {
                let elapsed = self.control.nanos(Instant::now()).saturating_sub(motion.at);
                trace.scrub_rendered.fetch_add(1, Relaxed);
                trace.scrub_render_us.fetch_max(elapsed / 1000, Relaxed);
            }
        }
    }

    fn finished(&self) -> bool {
        false
    }
}

fn sample_at(
    pages: &[Option<Page>; CACHE],
    generation: u64,
    bounds: &Range<usize>,
    position: f64,
) -> Option<Frame> {
    let first = position.floor() as usize;
    if !bounds.contains(&first) {
        return None;
    }
    let sample = |frame: usize| {
        let number = frame / PAGE;
        let page = pages[number % CACHE].as_ref()?;
        let index = frame % PAGE;
        (page.generation == generation && page.number == number && index < page.len)
            .then(|| page.frames[index])
    };
    let a = sample(first)?;
    let blend = (position - first as f64) as f32;
    if blend == 0. || first + 1 == bounds.end {
        return Some(a);
    }
    let b = sample(first + 1)?;
    Some(a * (1. - blend) + b * blend)
}

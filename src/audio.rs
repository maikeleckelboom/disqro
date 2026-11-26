use crate::{
    library::{DiscContext, Track},
    listening::{self, End},
    queue::{Queue, Repeat, Selection},
};
use anyhow::{Context, Result};
use kira::{
    AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Tween,
    sound::{
        PlaybackState,
        streaming::{Decoder as StreamDecoder, StreamingSoundData, StreamingSoundHandle},
    },
    track::MainTrackBuilder,
};

use std::{
    io,
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[path = "audioClock.rs"]
mod clock;

pub use crate::recording::Recording;
pub(crate) use clock::Position as RendererPosition;

#[path = "audioScrub.rs"]
pub(crate) mod scrub;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub track: Option<Arc<Track>>,
    /// The queue entry that is playing, which tells two copies of a track apart.
    pub entry: Option<u64>,
    pub source_revision: Option<crate::cache::Stamp>,
    pub source_object: Option<crate::store::NativeFile>,
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub previous: bool,
    pub next: bool,
    pub error: Option<String>,
    pub skipped: Option<(u64, String)>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub queue_revision: u64,
    occurrence_revision: u64,
    seek_revision: u64,
    pub(crate) presentation: Option<Presentation>,
    pub(crate) scrub: Option<ScrubPosition>,
    pub(crate) scrub_error: Option<(u64, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrubPosition {
    pub id: u64,
    pub frame: f64,
    pub was_playing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlaybackIdentity {
    entry: u64,
    occurrence: u64,
    seek: u64,
    voice: Option<u64>,
    queue: u64,
}

impl PlaybackIdentity {
    /// A queue occurrence preserves position across pause or output recovery.
    /// Adjacent CUE occurrences can also continue on the same physical voice.
    pub(crate) fn continues(self, previous: Self) -> bool {
        self.seek == previous.seek
            && (self.occurrence == previous.occurrence
                || (self.voice.is_some() && self.voice == previous.voice))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Presentation {
    id: u64,
    origin: usize,
    limit: usize,
    clock: Arc<clock::Clock>,
}

impl PartialEq for Presentation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.limit == other.limit
    }
}

impl Presentation {
    pub(crate) fn position(&self, at: Instant) -> Option<RendererPosition> {
        let mut position = self.clock.position(at)?;
        position.frame = (position.frame + self.origin as f64).min(self.limit as f64);
        position.advancing &= position.frame < self.limit as f64;
        Some(position)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WaveSeekAnchor {
    pub identity: PlaybackIdentity,
    pub source: Arc<crate::timeline::Source>,
    pub playable: Range<usize>,
    pub sheet: Option<(std::path::PathBuf, crate::cache::Revision)>,
}

/// An explicit seek within the active recording's authored program. The
/// program, not the queue, defines where a listener may go; the transport
/// reconciles the queue only when the destination is committed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProgramSeek {
    /// The queue entry that was active when the seek was requested.
    pub origin: u64,
    /// Every chapter of the program in authored order.
    pub program: crate::library::TrackList,
    /// The destination's index in `program`.
    pub chapter: usize,
    pub source_revision: crate::cache::Stamp,
    pub sheet_revision: Option<crate::cache::Stamp>,
    /// Destination position from the chapter's start, and its length.
    pub seconds: f64,
    pub duration: f64,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TransportState {
    Playing,
    Paused,
    Scrubbing,
    Finished,
    Stopped,
}
impl Status {
    pub(crate) fn playback_identity(&self) -> Option<PlaybackIdentity> {
        Some(PlaybackIdentity {
            entry: self.entry?,
            occurrence: self.occurrence_revision,
            seek: self.seek_revision,
            voice: self.presentation.as_ref().map(|clock| clock.id),
            queue: self.queue_revision,
        })
    }

    pub fn playback_state(&self) -> TransportState {
        if self.scrub.is_some() {
            TransportState::Scrubbing
        } else if self.playing {
            TransportState::Playing
        } else if self.error.is_some() {
            TransportState::Stopped
        } else if self.duration > 0. && self.position >= self.duration {
            TransportState::Finished
        } else if self.source_revision.is_some() || self.position > 0. {
            TransportState::Paused
        } else {
            TransportState::Stopped
        }
    }
}

/// Every control path — the window, the tray, media keys — sends these to
/// the one transport, so they can never disagree about what is playing.
pub enum Command {
    /// Replace the queue with these tracks and play from the index.
    Play(Selection, usize),
    PlayDisc {
        tracks: crate::library::TrackList,
        from_chapter: Option<usize>,
    },
    PlayList(Selection, bool),
    Toggle,
    SetPlaying(bool),
    Stop,
    /// Seek to a fraction of the current track.
    Seek(f64),
    /// Seek to a time in seconds within the current track.
    SeekTo(f64),
    SeekWave {
        anchor: WaveSeekAnchor,
        frame: usize,
    },
    BeginWaveScrub(Arc<scrub::Gesture>),
    SeekProgram(Box<ProgramSeek>),
    Step(i8),
    Volume(f32),
    Jump(u64),
    PlayNext(Selection),
    Enqueue(Selection),
    Remove(Vec<u64>),
    MoveUpcoming(u64, Option<u64>),
    ScheduleNext(u64),
    ContinueDisc(u64, crate::library::TrackList, DiscContext),
    ClearUpcoming,
    ClearHistory,
    Shuffle(bool),
    Repeat(Repeat),
    /// Bring back a saved queue, paused at the saved position.
    Restore(Box<Queue>, f64),
    Snapshot(mpsc::SyncSender<Option<crate::profile::SavedQueue>>),
    FinishListening(mpsc::SyncSender<()>),
    EraseListening(Option<i64>, async_channel::Sender<Result<()>>),
}

impl Command {
    fn supersedes_play(&self) -> bool {
        matches!(
            self,
            Self::Play(..)
                | Self::PlayDisc { .. }
                | Self::PlayList(..)
                | Self::Toggle
                | Self::SetPlaying(_)
                | Self::Jump(_)
                | Self::Stop
                | Self::FinishListening(_)
                | Self::Step(_)
                | Self::Restore(..)
        )
    }
}

struct Shared {
    status: Status,
    queue: Arc<Queue>,
}

/// Only adjacent seeks (or adjacent volume changes) may replace each other.
/// A transport or queue action seals the preceding slot, preserving FIFO order.
type Slot = Arc<Mutex<Option<Pending>>>;

struct Pending {
    command: Command,
    entry: Option<u64>,
}
impl Pending {
    fn replaces(&self, old: &Self) -> bool {
        let replace = matches!(
            (&self.command, &old.command),
            (Command::Volume(_), Command::Volume(_))
                | (
                    Command::Seek(_) | Command::SeekTo(_),
                    Command::Seek(_) | Command::SeekTo(_)
                )
        ) || matches!((&self.command, &old.command), (
            Command::SeekWave { anchor, .. }, Command::SeekWave { anchor: previous, .. }
        ) if anchor == previous)
            || matches!((&self.command, &old.command), (
            Command::SeekProgram(seek), Command::SeekProgram(previous)
        ) if seek.origin == previous.origin && seek.program == previous.program);
        replace && self.entry == old.entry
    }
    fn applies_to(&self, entry: Option<u64>) -> bool {
        !matches!(
            self.command,
            Command::Seek(_)
                | Command::SeekTo(_)
                | Command::SeekProgram(_)
                | Command::SeekWave { .. }
        ) || self.entry == entry
    }
}

#[derive(Clone)]
pub struct Commands {
    pub play_request: crate::store::Ticket,
    pub(crate) scrub_control: Arc<scrub::Control>,
    tx: mpsc::SyncSender<Slot>,
    tail: Arc<Mutex<std::sync::Weak<Mutex<Option<Pending>>>>>,
    shared: Arc<Mutex<Shared>>,
}
impl Commands {
    pub fn finish_listening(&self) -> Result<()> {
        let (send, receive) = mpsc::sync_channel(1);
        self.send(Command::FinishListening(send))?;
        receive
            .recv_timeout(Duration::from_secs(5))
            .context("Playback did not finish saving")?;
        Ok(())
    }

    fn channel(shared: Arc<Mutex<Shared>>, capacity: usize) -> (Self, mpsc::Receiver<Slot>) {
        let (tx, rx) = mpsc::sync_channel(capacity);
        (
            Self {
                tx,
                shared,
                play_request: Default::default(),
                scrub_control: Arc::default(),
                tail: Default::default(),
            },
            rx,
        )
    }
    // Called on the shutdown worker. FIFO puts the receipt after all commands
    // accepted before Quit, even if the UI has not received their notifications.
    pub fn snapshot(&self) -> Result<Option<crate::profile::SavedQueue>> {
        let (tx, rx) = mpsc::sync_channel(1);
        let slot = Arc::new(Mutex::new(Some(Pending {
            command: Command::Snapshot(tx),
            entry: None,
        })));
        {
            let mut tail = self.tail.lock().unwrap();
            self.tx
                .send(slot)
                .map_err(|_| anyhow::anyhow!("The playback queue could not be saved."))?;
            *tail = Default::default();
        }
        Ok(rx.recv()?)
    }
    pub fn send(&self, command: Command) -> Result<()> {
        let entry = self.shared.lock().unwrap().status.entry;
        self.send_for(command, entry)
    }
    fn send_for(&self, command: Command, entry: Option<u64>) -> Result<()> {
        self.enqueue(command, entry, None).map(|_| ())
    }
    pub fn play_if_current(&self, command: Command, generation: u64) -> Result<bool> {
        self.enqueue(command, None, Some(generation))
    }
    fn enqueue(
        &self,
        command: Command,
        entry: Option<u64>,
        generation: Option<u64>,
    ) -> Result<bool> {
        let mut tail = self.tail.lock().unwrap();
        if generation.is_some_and(|g| !self.play_request.is_current(g)) {
            return Ok(false);
        }
        let supersedes = command.supersedes_play();
        let pending = Pending { command, entry };
        if let Some(slot) = tail.upgrade() {
            let mut last = slot.lock().unwrap();
            if last.as_ref().is_some_and(|old| pending.replaces(old)) {
                *last = Some(pending);
                return Ok(true);
            }
        }
        let slot = Arc::new(Mutex::new(Some(pending)));
        self.tx.try_send(slot.clone()).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("Playback controls are busy. Try the action again.")
            }
            mpsc::TrySendError::Disconnected(_) => {
                anyhow::anyhow!("Playback controls are unavailable.")
            }
        })?;
        *tail = Arc::downgrade(&slot);
        if supersedes {
            self.play_request.next();
        }
        Ok(true)
    }
}
pub struct Transport {
    pub signal: crate::signal::SignalMonitor,
    tx: Commands,
    latest: Arc<Mutex<Shared>>,
    quiet: Arc<AtomicBool>,
    pub changed: async_channel::Receiver<()>,
}
impl Transport {
    pub fn new(volume: f32, listening: Option<listening::Writer>) -> Self {
        let latest = Arc::new(Mutex::new(Shared {
            status: Status::default(),
            queue: Arc::default(),
        }));
        let (tx, rx) = Commands::channel(latest.clone(), 64);
        let scrub_control = tx.scrub_control.clone();
        let shared = latest.clone();
        let signal = crate::signal::SignalMonitor::default();
        let monitor = signal.clone();
        let quiet = Arc::new(AtomicBool::new(false));
        let is_quiet = quiet.clone();
        let (wake, changed) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("disqro-transport".into())
            .spawn(move || {
                let mut engine = Engine {
                    volume,
                    signal: monitor,
                    listening,
                    scrub_control,
                    ..Default::default()
                };
                let mut published = 0;
                let interval = Duration::from_millis(40);
                let mut next_tick = std::time::Instant::now() + interval;
                loop {
                    let scrubbing = engine.scrubbing.is_some();
                    let interval = if scrubbing {
                        Duration::from_millis(8)
                    } else {
                        interval
                    };
                    next_tick = next_tick.min(Instant::now() + interval);
                    let ticking = engine.status.playing || engine.pause_checkpoint || scrubbing;
                    let command = if ticking
                        && !engine.retry_next
                        && std::time::Instant::now() >= next_tick
                    {
                        None
                    } else if ticking {
                        match rx.recv_timeout(
                            next_tick.saturating_duration_since(std::time::Instant::now()),
                        ) {
                            Ok(c) => Some(c),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(_) => break,
                        }
                    } else {
                        match rx.recv() {
                            Ok(c) => Some(c),
                            Err(_) => break,
                        }
                    };
                    let mut scrub_admission = false;
                    let result = match command {
                        Some(slot) => {
                            let pending =
                                slot.lock().unwrap().take().expect("one transport consumer");
                            scrub_admission =
                                matches!(&pending.command, Command::BeginWaveScrub(_));
                            if pending.applies_to(engine.status.entry) {
                                engine.command(pending.command)
                            } else {
                                Ok(())
                            }
                        }
                        None => {
                            next_tick = std::time::Instant::now() + interval;
                            engine.tick()
                        }
                    };
                    if let Err(error) = result {
                        engine.retry_next = false;
                        engine.end_voice(End::SourceFailed);
                        engine.status.error = Some(format!("{error:#}"));
                    }
                    engine.refresh_flags();
                    let mut s = shared.lock().unwrap();
                    if s.status != engine.status {
                        // While the window is hidden, only changes other than
                        // the advancing position wake the interface.
                        let significant = {
                            let mut a = s.status.clone();
                            a.position = engine.status.position;
                            a != engine.status
                        };
                        s.status.clone_from(&engine.status);
                        if published != engine.queue.revision {
                            published = engine.queue.revision;
                            s.queue = Arc::new(engine.queue.clone());
                        }
                        if significant || !is_quiet.load(Ordering::Relaxed) {
                            let _ = wake.try_send(());
                        }
                    } else if scrub_admission {
                        let _ = wake.try_send(());
                    }
                }
                engine.end_voice(End::Shutdown);
            })
            .expect("transport thread");
        Self {
            tx,
            signal,
            latest,
            quiet,
            changed,
        }
    }
    pub fn send(&self, command: Command, entry: Option<u64>) -> Result<()> {
        self.tx.send_for(command, entry)
    }
    pub fn sender(&self) -> Commands {
        self.tx.clone()
    }
    pub fn state(&self) -> (Status, Arc<Queue>) {
        let shared = self.latest.lock().unwrap();
        (shared.status.clone(), shared.queue.clone())
    }
    /// Stops position-only updates from waking the interface.
    pub fn set_quiet(&self, quiet: bool) {
        self.quiet.store(quiet, Ordering::Relaxed);
    }
}

/// A CUE chapter decoded by the running voice, and where it starts in it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Chapter {
    id: u64,
    offset: f64,
}

/// Only errors carrying this context are eligible for automatic skipping.
#[derive(Debug)]
struct RecordingFailure(String);

impl std::fmt::Display for RecordingFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cannot play {}", self.0)
    }
}

#[derive(Default)]
struct Engine {
    signal: crate::signal::SignalMonitor,
    device: Option<AudioManager<DefaultBackend>>,
    voice: Option<OutputVoice>,
    scrub_control: Arc<scrub::Control>,
    scrub_output: Option<scrub::Output>,
    scrubbing: Option<ActiveScrub>,
    listening: Option<listening::Writer>,
    credit: Option<VoiceCredit>,
    occurrence: Option<(u64, listening::Occurrence)>,
    suppressed_entry: Option<u64>,
    pause_checkpoint: bool,
    queue: Queue,
    /// Consecutive chapters of one disc image share a decoder.
    group: Vec<Chapter>,
    chapter: usize,
    rate: u32,
    /// Where the group ends in the decoder, earlier than `length` when the
    /// queue changed after the group was opened.
    limit: f64,
    length: f64,
    /// Last authored tick opened by this decoder, or EOF.
    decoder_end: Option<u64>,
    // Only populated while tracing. Compare consecutive ticks on the same
    // advancing voice, including contiguous CUE chapter boundaries.
    position_probe: Option<(std::time::Instant, f64)>,
    discarded_errors: u64,
    consecutive_failures: usize,
    retry_next: bool,
    volume: f32,
    status: Status,
    voice_id: u64,
}

struct ActiveScrub {
    gesture: Arc<scrub::Gesture>,
    updated: Instant,
}

const RENDERING: u8 = 1;
const CLOSED: u8 = 2;

const PAUSE: u8 = 1;
const RESUME: u8 = 2;
const PROGRESS_BLOCKS: usize = 8192;

#[derive(Default)]
struct OutputState {
    gate: AtomicU8,
    command: AtomicU8,
    volume: AtomicU32,
    state: AtomicU8,
    cursor: AtomicUsize,
    // The actual contiguous decoded extent, including the voice's seek origin.
    decoded: AtomicUsize,
    decode_failed: AtomicBool,
    error: Mutex<Option<io::Error>>,
}

impl OutputState {
    fn publish_cursor(
        &self,
        frame: usize,
        progress: Option<(&mut rtrb::Producer<RenderProgress>, RenderProgress)>,
        #[cfg(test)] before_cursor: impl FnOnce(),
    ) {
        if let Some((queue, block)) = progress {
            let _ = queue.push(block);
        }
        // A reader may see the ring entry early. The cursor commits the
        // boundary for both the playhead and listening accounting.
        #[cfg(test)]
        before_cursor();
        self.cursor.store(frame, Ordering::Release);
    }
}

struct OutputVoice {
    state: Arc<OutputState>,
    rate: f64,
    clock: Arc<clock::Clock>,
}

impl OutputVoice {
    fn position(&self) -> f64 {
        self.state.cursor.load(Ordering::Acquire) as f64 / self.rate
    }
    fn state(&self) -> PlaybackState {
        match self.state.state.load(Ordering::Acquire) {
            n if n == PlaybackState::Stopped as u8 => PlaybackState::Stopped,
            n if n == PlaybackState::Paused as u8 => PlaybackState::Paused,
            n if n == PlaybackState::Pausing as u8 => PlaybackState::Pausing,
            n if n == PlaybackState::Resuming as u8 => PlaybackState::Resuming,
            n if n == PlaybackState::WaitingToResume as u8 => PlaybackState::WaitingToResume,
            _ => PlaybackState::Playing,
        }
    }
    fn pop_error(&self) -> Option<io::Error> {
        self.state.error.lock().unwrap().take()
    }
}

impl Drop for OutputVoice {
    fn drop(&mut self) {
        self.state.gate.fetch_or(CLOSED, Ordering::AcqRel);
    }
}

struct ObservedDecoder {
    recording: Recording,
    state: Arc<OutputState>,
    cursor: usize,
    failed: bool,
}

impl StreamDecoder for ObservedDecoder {
    type Error = io::Error;

    fn sample_rate(&self) -> u32 {
        self.recording.sample_rate()
    }
    fn num_frames(&self) -> usize {
        self.recording.num_frames()
    }
    fn seek(&mut self, start: usize) -> io::Result<usize> {
        self.cursor = self.recording.seek(start)?;
        self.state.decoded.store(self.cursor, Ordering::Release);
        Ok(self.cursor)
    }
    fn decode(&mut self) -> io::Result<Vec<kira::Frame>> {
        if self.failed {
            // Kira's scheduler can retry until its renderer observes the
            // failure. The failed voice cannot recover or busy-loop on I/O.
            std::thread::sleep(Duration::from_millis(1));
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        match self.recording.decode() {
            Ok(frames) => {
                self.cursor += frames.len();
                self.state.decoded.store(self.cursor, Ordering::Release);
                Ok(frames)
            }
            Err(error) => {
                self.failed = true;
                self.state.decode_failed.store(true, Ordering::Release);
                *self.state.error.lock().unwrap() =
                    Some(io::Error::new(error.kind(), error.to_string()));
                Err(error)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct RenderProgress {
    from: i64,
    to: i64,
    at: i64,
}

/// A voice has one immutable source timeline. Only this sound reads the
/// Kira handle on the renderer thread. The transport uses atomics in both
/// directions and a preallocated SPSC queue for timestamped progress.
struct ObservedSound {
    inner: Box<dyn kira::sound::Sound>,
    handle: StreamingSoundHandle<io::Error>,
    state: Arc<OutputState>,
    rate: f64,
    frames: usize,
    volume: u32,
    cursor: i64,
    clock: Arc<clock::Clock>,
    timing: clock::Writer,
    rendered: f64,
    progress: Option<rtrb::Producer<RenderProgress>>,
}

impl kira::sound::Sound for ObservedSound {
    fn on_start_processing(&mut self) {
        self.timing.begin(&self.clock, Instant::now());
        match self.state.command.swap(0, Ordering::AcqRel) {
            PAUSE => self.handle.pause(Tween::default()),
            RESUME => self.handle.resume(Tween::default()),
            _ => {}
        }
        let volume = self.state.volume.load(Ordering::Relaxed);
        if volume != self.volume {
            self.volume = volume;
            self.handle
                .set_volume(db(f32::from_bits(volume)), Tween::default());
        }
        self.inner.on_start_processing();
    }

    fn process(&mut self, out: &mut [kira::Frame], dt: f64, info: &kira::info::Info) {
        if self
            .state
            .gate
            .compare_exchange(0, RENDERING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            out.fill(kira::Frame::ZERO);
            return;
        }
        let at = self.progress.as_ref().map(|_| listening::timestamp());
        let was_finished = self.inner.finished();
        self.inner.process(out, dt, info);
        self.inner.on_start_processing();
        let frames = self.state.decoded.load(Ordering::Acquire);
        let state = self.handle.state();
        let rendered = (self.handle.position() * self.rate).min(frames as f64);
        // Kira's position can stay at the previous ring cursor after it drains
        // the last block. The immutable voice has no Kira stop or seek command.
        // Only a renderer transition to Stopped with every frame decoded
        // proves that the ring drained naturally.
        let frame = if !was_finished
            && state == PlaybackState::Stopped
            && frames == self.frames
            && !self.state.decode_failed.load(Ordering::Acquire)
        {
            frames
        } else {
            ((rendered + 1e-6).floor() as usize).min(frames)
        };
        let position = position_us(frame as f64 / self.rate);
        if position > self.cursor {
            self.state.publish_cursor(
                frame,
                self.progress.as_mut().zip(at).map(|(queue, at)| {
                    (
                        queue,
                        RenderProgress {
                            from: self.cursor,
                            to: position,
                            at,
                        },
                    )
                }),
                #[cfg(test)]
                || {},
            );
            self.cursor = position;
        }
        let to = rendered.max(frame as f64).max(self.rendered);
        self.timing.rendered(
            &self.clock,
            self.rendered,
            to,
            out.len() as f64 * dt,
            self.rate,
        );
        self.rendered = to;
        self.state.state.store(state as u8, Ordering::Release);
        self.state.gate.fetch_and(!RENDERING, Ordering::Release);
    }

    fn finished(&self) -> bool {
        self.state.gate.load(Ordering::Relaxed) & CLOSED != 0 || self.inner.finished()
    }
}

struct ObservedData {
    recording: Recording,
    start: usize,
    volume: f32,
    fade_in: Option<Tween>,
    progress: Option<rtrb::Producer<RenderProgress>>,
}

impl kira::sound::SoundData for ObservedData {
    type Error = io::Error;
    type Handle = OutputVoice;

    fn into_sound(self) -> io::Result<(Box<dyn kira::sound::Sound>, Self::Handle)> {
        let rate = f64::from(self.recording.sample_rate());
        let frames = self.recording.num_frames();
        let cursor = position_us(self.start as f64 / rate);
        let clock = Arc::new(clock::Clock::new(self.start));
        let state = Arc::new(OutputState {
            cursor: AtomicUsize::new(self.start),
            volume: AtomicU32::new(self.volume.to_bits()),
            ..Default::default()
        });
        let data = StreamingSoundData::from_decoder(ObservedDecoder {
            recording: self.recording,
            state: state.clone(),
            cursor: self.start,
            failed: false,
        })
        .volume(db(self.volume))
        .fade_in_tween(self.fade_in)
        .start_position(kira::sound::PlaybackPosition::Samples(self.start));
        let (inner, handle) = data.into_sound()?;
        Ok((
            Box::new(ObservedSound {
                inner,
                handle,
                state: state.clone(),
                rate,
                frames,
                volume: self.volume.to_bits(),
                cursor,
                clock: clock.clone(),
                timing: clock::Writer::default(),
                rendered: self.start as f64,
                progress: self.progress,
            }),
            OutputVoice { state, rate, clock },
        ))
    }
}

struct CreditSegment {
    id: u64,
    start: i64,
    recording: listening::Recording,
}

struct VoiceCredit {
    progress: rtrb::Consumer<RenderProgress>,
    cursor: i64,
    activity_at: i64,
    segments: Vec<CreditSegment>,
}

fn position_us(seconds: f64) -> i64 {
    (seconds * 1_000_000.).round() as i64
}
static SILENT_QUALIFICATION: std::sync::OnceLock<()> = std::sync::OnceLock::new();

pub fn silent_qualification() -> bool {
    SILENT_QUALIFICATION.get().is_some()
}

pub fn qualify_silently() {
    SILENT_QUALIFICATION.get_or_init(|| ());
}

fn output_track(signal: crate::signal::SignalMonitor, silent: bool) -> MainTrackBuilder {
    let track = MainTrackBuilder::new().with_effect(signal);
    if silent {
        track.volume(Decibels::SILENCE)
    } else {
        track
    }
}

fn db(volume: f32) -> Decibels {
    if volume <= 0. {
        Decibels::SILENCE
    } else {
        Decibels(20. * volume.log10())
    }
}
/// Whether `b` continues `a` in the same recording without a gap.
fn contiguous(a: &Track, b: &Track) -> bool {
    a.source == b.source
        && a.disc
            .as_ref()
            .zip(b.disc.as_ref())
            .is_some_and(|(a, b)| a.sheet == b.sheet && a.end == Some(b.start))
}

pub(crate) fn scrub_run(
    queue: &Queue,
    source: &crate::timeline::Source,
) -> Option<(Range<usize>, Range<usize>)> {
    let current = queue.current?;
    let entries = &queue.entries;
    let mut allowed = source.playable(&entries.get(current)?.track)?;
    let mut first = current;
    let mut last = current;
    if queue.repeat != Repeat::One {
        while let Some(previous) = first.checked_sub(1)
            && contiguous(&entries[previous].track, &entries[first].track)
            && let Some(range) = source.playable(&entries[previous].track)
        {
            first = previous;
            allowed.start = range.start;
        }
        while let Some(next) = entries.get(last + 1)
            && contiguous(&entries[last].track, &next.track)
            && let Some(range) = source.playable(&next.track)
        {
            last += 1;
            allowed.end = range.end;
        }
    }
    Some((first..last + 1, allowed))
}

/// The authored chapters around the one spanning `playable` that continue
/// each other in `source`, independent of the queue: their indices in
/// `program` and the source frames they cover.
pub(crate) fn program_run(
    program: &[Arc<Track>],
    source: &crate::timeline::Source,
    playable: &Range<usize>,
) -> Option<(Range<usize>, Range<usize>)> {
    let ranges: Vec<_> = program.iter().map(|track| source.playable(track)).collect();
    let at = ranges
        .iter()
        .position(|range| range.as_ref() == Some(playable))?;
    let joined = |a: usize, b: usize| {
        ranges[a]
            .as_ref()
            .zip(ranges[b].as_ref())
            .is_some_and(|(a, b)| a.end == b.start)
    };
    let mut first = at;
    while first > 0 && joined(first - 1, first) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < ranges.len() && joined(last, last + 1) {
        last += 1;
    }
    let start = ranges[first].as_ref()?.start;
    let end = ranges[last].as_ref()?.end;
    Some((first..last + 1, start..end))
}

fn disc_start(t: &Track) -> u64 {
    t.disc.as_ref().map_or(0, |d| d.start)
}
impl Engine {
    fn stop_voice(&mut self) {
        self.end_voice(End::Stopped);
    }
    fn end_voice(&mut self, reason: End) {
        self.close_scrub();
        self.detach_voice();
        self.finish_occurrence(reason, listening::timestamp());
        self.suppressed_entry = None;
    }
    fn detach_voice(&mut self) {
        if let Some(voice) = &self.voice {
            let state = &voice.state;
            state.gate.fetch_or(CLOSED, Ordering::AcqRel);
            let deadline = std::time::Instant::now() + Duration::from_millis(40);
            while state.gate.load(Ordering::Acquire) & RENDERING != 0 {
                if std::time::Instant::now() >= deadline {
                    if let Some(writer) = &self.listening {
                        writer.notice(
                            "A stalled audio callback could not finish reporting its final block.",
                        );
                    }
                    break;
                }
                std::thread::yield_now();
            }
            self.credit_output();
        }
        self.credit = None;
        self.pause_checkpoint = false;
        self.position_probe = None;
        self.voice = None;
        self.status.presentation = None;
        self.status.source_revision = None;
        self.status.source_object = None;
        self.group.clear();
        self.status.playing = false;
    }
    fn finish_occurrence(&mut self, reason: End, at: i64) {
        if let Some((_, occurrence)) = self.occurrence.take()
            && let Some(writer) = &self.listening
        {
            let _ = writer.save(occurrence.finish(at, reason));
        }
    }
    fn flush_occurrence(&mut self) {
        if let Some((id, occurrence)) = &mut self.occurrence
            && let Some(snapshot) = occurrence.checkpoint()
            && let Some(writer) = &self.listening
            && !writer.save(snapshot)
        {
            self.suppressed_entry = Some(*id);
            self.occurrence = None;
        }
    }
    fn credit_output(&mut self) -> f64 {
        let Some(voice) = &self.voice else { return 0. };
        // Use the same completed-block boundary for both the credit and the
        // queue playhead. A block published during a queue edit follows the
        // program effective after that boundary.
        let position = voice.position();
        let cursor = position_us(position);
        let target = cursor.min(position_us(self.limit));
        while let Some(block) = self.credit.as_mut().and_then(|c| {
            let block = *c.progress.peek().ok()?;
            if c.cursor >= target || block.to > cursor {
                return None;
            }
            // A queue edit can restore a previously shortened CUE program.
            // Keep the timestamp until the entire block is eligible.
            if block.to <= target {
                c.progress.pop().unwrap();
            }
            Some(block)
        }) {
            // Overflow can leave a gap before the next recorded block. Place
            // only the missing duration against the nearest available edge.
            let before = block.from.min(target);
            if self.credit.as_ref().is_some_and(|c| c.cursor < before) {
                self.credit_progress(before, block.at - (block.from - before));
            }
            let position = block.to.min(target);
            self.credit_progress(position, block.at + position - block.from);
        }
        // A trailing overflow has no later timing evidence. Extend the last
        // accounted edge by at most the uncredited, committed source advance.
        if let Some(credit) = &self.credit
            && target > credit.cursor
        {
            self.credit_progress(target, credit.activity_at + (target - credit.cursor));
        }
        position
    }
    fn credit_progress(&mut self, position: i64, observed_at: i64) {
        let Some(mut credit) = self.credit.take_if(|credit| position > credit.cursor) else {
            return;
        };
        let end = position_us(self.limit);
        let mut from = (observed_at - (position - credit.cursor)).max(credit.activity_at);
        while credit.cursor < position {
            let ix = credit
                .segments
                .partition_point(|s| s.start <= credit.cursor)
                - 1;
            let segment = &credit.segments[ix];
            let boundary = credit.segments.get(ix + 1).map_or(end, |s| s.start);
            let amount = position.min(boundary) - credit.cursor;
            if self
                .occurrence
                .as_ref()
                .is_some_and(|(id, _)| *id != segment.id)
            {
                self.finish_occurrence(End::Completed, from);
            }
            if self.suppressed_entry != Some(segment.id)
                && let Some(writer) = &self.listening
            {
                if writer.suspended() {
                    self.suppressed_entry = Some(segment.id);
                } else {
                    self.suppressed_entry = None;
                    let (_, occurrence) = self.occurrence.get_or_insert_with(|| {
                        (
                            segment.id,
                            listening::Occurrence::new(segment.recording.clone(), from),
                        )
                    });
                    let duration = (boundary - segment.start) as f64 / 1_000_000.;
                    if let Some(checkpoint) = occurrence.credit(Some(duration), from, amount)
                        && !writer.save(checkpoint)
                    {
                        self.suppressed_entry = Some(segment.id);
                        self.occurrence = None;
                    }
                }
            }
            credit.cursor += amount;
            from += amount;
        }
        credit.activity_at = from;
        self.credit = Some(credit);
    }
    /// Shows an entry as current without opening the output.
    fn select(&mut self, index: Option<usize>, position: f64) {
        self.queue.set_current(index);
        let entry = self.queue.current_entry();
        self.status.track = entry.map(|e| e.track.clone());
        if self.status.entry != entry.map(|e| e.id) {
            self.status.occurrence_revision += 1;
        }
        self.status.entry = entry.map(|e| e.id);
        let duration = entry.and_then(|e| e.track.duration);
        self.status.duration = duration.unwrap_or(0.);
        self.status.position =
            duration.map_or(position.max(0.), |end| position.clamp(0., end.max(0.)));
        if entry.is_none() {
            self.status.position = 0.;
        }
        self.status.error = None;
    }
    fn play(&mut self, index: usize, position: f64) -> Result<()> {
        self.end_voice(End::Replaced);
        if self
            .queue
            .entries
            .get(index)
            .is_some_and(|entry| Some(entry.id) == self.status.entry)
        {
            self.status.occurrence_revision += 1;
        }
        self.start_voice(index, position, None)
    }
    fn start_voice(&mut self, index: usize, position: f64, fade_in: Option<Tween>) -> Result<()> {
        let entry = self
            .queue
            .entries
            .get(index)
            .context("Track is unavailable")?
            .clone();
        self.select(Some(index), 0.);
        let mut last = index;
        let mut group = vec![Chapter {
            id: entry.id,
            offset: 0.,
        }];
        if self.queue.repeat != Repeat::One {
            while let Some(next) = self.queue.entries.get(last + 1) {
                if !contiguous(&self.queue.entries[last].track, &next.track) {
                    break;
                }
                group.push(Chapter {
                    id: next.id,
                    offset: 0.,
                });
                last += 1;
            }
        }
        let mut end = self.queue.entries[last]
            .track
            .disc
            .as_ref()
            .and_then(|d| d.end);
        let open = |end| Recording::open(&entry.track, end);
        let grouped = open(end).and_then(|recording| {
            let rate = recording.sample_rate();
            let offset =
                crate::recording::disc_frame(disc_start(&self.queue.entries[last].track), rate)
                    .saturating_sub(crate::recording::disc_frame(disc_start(&entry.track), rate));
            anyhow::ensure!(
                offset < recording.num_frames(),
                "A CUE track begins beyond the recording"
            );
            Ok(recording)
        });
        // A malformed later chapter must not make an earlier playable one fail.
        let mut recording = match grouped {
            Err(_) if last != index => {
                group.truncate(1);
                end = entry.track.disc.as_ref().and_then(|d| d.end);
                open(end)
            }
            result => result,
        }
        .with_context(|| RecordingFailure(entry.track.title.clone()))?;
        if let Some(d) = crate::diagnostics::get() {
            d.opens.fetch_add(1, Ordering::Relaxed);
            recording.playback_probe = Some(std::time::Instant::now());
        }
        self.status.source_revision = recording.stamp.clone();
        self.status.source_object = recording.object;
        self.rate = recording.sample_rate();
        let first = crate::recording::disc_frame(disc_start(&entry.track), self.rate);
        for (chapter, queued) in group.iter_mut().zip(&self.queue.entries[index..]) {
            let frame = crate::recording::disc_frame(disc_start(&queued.track), self.rate);
            chapter.offset = (frame - first) as f64 / f64::from(self.rate);
        }
        self.decoder_end = end;
        self.length = recording.num_frames() as f64 / f64::from(recording.sample_rate());
        let frame_count = recording.num_frames();
        let start = ((position * f64::from(self.rate)).round() as usize).min(frame_count - 1);
        self.ensure_output()?;
        self.signal.reset();
        let device = self.device.as_mut().unwrap();
        let progress = if self.listening.is_some() {
            let activity_at = listening::timestamp();
            let (publish, progress) = rtrb::RingBuffer::new(PROGRESS_BLOCKS);
            let source_object = recording.object;
            let sheet_file = entry
                .track
                .disc
                .as_ref()
                .and_then(|disc| std::fs::File::open(&disc.sheet).ok());
            let sheet = sheet_file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .and_then(|m| crate::cache::Stamp::from_metadata(&m));
            let sheet_object = sheet_file.as_ref().and_then(crate::store::NativeFile::of);
            self.credit = Some(VoiceCredit {
                progress,
                cursor: position_us(start as f64 / f64::from(self.rate)),
                activity_at,
                segments: group
                    .iter()
                    .zip(&self.queue.entries[index..])
                    .map(|(chapter, entry)| CreditSegment {
                        id: chapter.id,
                        start: position_us(chapter.offset),
                        recording: listening::Recording {
                            track: entry.track.clone(),
                            source: self.status.source_revision.clone(),
                            sheet: sheet.clone(),
                            source_object,
                            sheet_object,
                        },
                    })
                    .collect(),
            });
            Some(publish)
        } else {
            None
        };
        let played = device.play(ObservedData {
            recording,
            start,
            volume: self.volume,
            fade_in,
            progress,
        });
        let voice = played.map_err(|e| match e {
            kira::PlaySoundError::IntoSoundError(e) => {
                anyhow::Error::new(e).context(RecordingFailure(entry.track.title.clone()))
            }
            other => anyhow::Error::new(other).context("Cannot start the output voice"),
        })?;
        self.voice_id += 1;
        self.status.presentation = Some(Presentation {
            id: self.voice_id,
            origin: first,
            limit: first + frame_count,
            clock: voice.clock.clone(),
        });
        self.voice = Some(voice);
        if let Some(d) = crate::diagnostics::get() {
            d.voices.fetch_add(1, Ordering::Relaxed);
        }
        self.group = group;
        self.chapter = 0;
        self.limit = self.length;
        self.status.playing = true;
        self.update_position(start as f64 / f64::from(self.rate));
        Ok(())
    }
    fn update_position(&mut self, p: f64) {
        if self.group.is_empty() {
            return;
        }
        let k = self.group.iter().rposition(|c| c.offset <= p).unwrap_or(0);
        if k != self.chapter || self.status.entry != Some(self.group[k].id) {
            self.chapter = k;
            let index = self.queue.index_of(self.group[k].id);
            self.queue.set_current(index);
            let entry = self.queue.current_entry();
            self.status.track = entry.map(|e| e.track.clone());
            self.status.occurrence_revision += 1;
            self.status.entry = entry.map(|e| e.id);
        }
        let start = self.group[k].offset;
        let end = self.group.get(k + 1).map_or(self.limit, |c| c.offset);
        self.status.duration = end - start;
        self.status.position = (p - start).clamp(0., self.status.duration);
    }
    /// Remap the queued chapters inside the already open decoder range. An edit
    /// can shorten its run or restore a previously removed continuation.
    fn revalidate(&mut self) {
        let Some(current) = self.group.get(self.chapter).copied() else {
            return;
        };
        let Some(ix) = self
            .queue
            .current
            .filter(|&i| self.queue.entries[i].id == current.id)
        else {
            return;
        };
        let active = &self.queue.entries[ix].track;
        let Some(segment) = &active.disc else { return };
        let offset = |tick: u64| {
            let frame = |tick| crate::recording::disc_frame(tick, self.rate);
            current.offset
                + frame(tick).saturating_sub(frame(segment.start)) as f64 / f64::from(self.rate)
        };
        self.group.truncate(self.chapter + 1);
        let mut last = active;
        if self.queue.repeat != Repeat::One {
            for next in &self.queue.entries[ix + 1..] {
                if !contiguous(last, &next.track) {
                    break;
                }
                let next_end = next.track.disc.as_ref().and_then(|d| d.end);
                if self
                    .decoder_end
                    .is_some_and(|end| next_end.is_none_or(|next| next > end))
                {
                    break;
                }
                let start = offset(disc_start(&next.track));
                self.group.push(Chapter {
                    id: next.id,
                    offset: start,
                });
                last = &next.track;
            }
        }
        self.limit = last
            .disc
            .as_ref()
            .and_then(|d| d.end)
            .map_or(self.length, |end| offset(end).min(self.length));
        if let Some(clock) = &mut self.status.presentation {
            clock.limit = clock.origin + (self.limit * f64::from(self.rate)).round() as usize;
        }
        if let Some(credit) = &mut self.credit {
            // The queued prefix has already been attributed. A queue edit
            // can change the identity of a later entry at the same offset.
            credit.segments.truncate(self.chapter + 1);
            let recording = &credit.segments[self.chapter].recording;
            let source = recording.source.clone();
            let sheet = recording.sheet.clone();
            let source_object = recording.source_object;
            let sheet_object = recording.sheet_object;
            credit.segments.extend(
                self.group
                    .iter()
                    .skip(self.chapter + 1)
                    .zip(&self.queue.entries[ix + 1..])
                    .map(|(chapter, entry)| CreditSegment {
                        id: chapter.id,
                        start: position_us(chapter.offset),
                        recording: listening::Recording {
                            track: entry.track.clone(),
                            source: source.clone(),
                            sheet: sheet.clone(),
                            source_object,
                            sheet_object,
                        },
                    }),
            );
        }
    }
    fn refresh_flags(&mut self) {
        let s = &mut self.status;
        s.shuffle = self.queue.shuffle;
        s.repeat = self.queue.repeat;
        s.queue_revision = self.queue.revision;
        s.next = self.queue.current.is_some() && self.queue.next_index(false).is_some();
        s.previous = self.queue.current.is_some()
            && (self.queue.previous_index().is_some() || s.position > 0.);
    }
    /// Try at most one occurrence per tick so controls can interrupt recovery.
    /// Preserve the count across voices that open but fail during decoding.
    fn recording_failed(&mut self, error: anyhow::Error) -> Result<()> {
        self.end_voice(End::SourceFailed);
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.queue.entries.len().min(32)
            || self.queue.next_index(false).is_none()
        {
            return Err(error);
        }
        self.status.skipped = self
            .status
            .entry
            .map(|id| (id, format!("Skipped recording · {error:#}")));
        self.retry_next = true;
        self.status.playing = true;
        Ok(())
    }

    fn play_eligible(&mut self, index: usize) -> Result<()> {
        match self.play(index, 0.) {
            Err(e) if e.is::<RecordingFailure>() => self.recording_failed(e),
            result => result,
        }
    }
    fn advance(&mut self, ended: bool) -> Result<()> {
        match self.queue.next_index(ended) {
            Some(i) => self.play_eligible(i),
            None => {
                self.stop_voice();
                self.status.position = self.status.duration;
                Ok(())
            }
        }
    }
    fn resume(&mut self) -> Result<()> {
        if self.voice.is_some() {
            if let Some(v) = &mut self.voice
                && !self.status.playing
            {
                v.state.command.store(RESUME, Ordering::Release);
                self.status.playing = true;
            }
            return Ok(());
        }
        let Some(index) = self.queue.current.or_else(|| self.queue.next_index(false)) else {
            return Ok(());
        };
        let position = if self.queue.current == Some(index) {
            if self.status.duration > 0. && self.status.position >= self.status.duration {
                // Replaying the selected entry from its start is a new
                // occurrence, even though the queue entry itself is stable.
                return self.play(index, 0.);
            }
            self.status.position
        } else {
            0.
        };
        self.start_voice(index, position, None)
    }
    fn pause(&mut self) {
        if let Some(v) = &mut self.voice
            && self.status.playing
        {
            v.state.command.store(PAUSE, Ordering::Release);
            self.position_probe = None;
            self.pause_checkpoint = true;
        }
        self.status.playing = false;
        self.credit_output();
        self.flush_occurrence();
    }
    fn command(&mut self, c: Command) -> Result<()> {
        if let Some(d) = crate::diagnostics::get() {
            d.commands.fetch_add(1, Ordering::Relaxed);
        }
        let c = match c {
            Command::BeginWaveScrub(gesture) => return self.begin_wave_scrub(gesture),
            other => other,
        };
        if !matches!(
            &c,
            Command::Volume(_) | Command::Snapshot(_) | Command::EraseListening(..)
        ) && self
            .close_scrub()
            .is_some_and(|gesture| gesture.was_playing)
            && self.voice.is_some()
        {
            self.resume()?;
        }
        if self.retry_next {
            match c {
                Command::Toggle => {
                    self.status.playing = !self.status.playing;
                    return Ok(());
                }
                Command::SetPlaying(on) => {
                    self.status.playing = on;
                    return Ok(());
                }
                Command::Seek(_)
                | Command::SeekTo(_)
                | Command::SeekProgram(_)
                | Command::SeekWave { .. } => return Ok(()),
                _ => {}
            }
        }
        // Account for the old CUE program before changing the queue.
        if self.scrubbing.is_none()
            && self.voice.is_some()
            && !matches!(
                &c,
                Command::Seek(_)
                    | Command::SeekTo(_)
                    | Command::SeekProgram(_)
                    | Command::SeekWave { .. }
            )
        {
            let position = self.credit_output();
            self.update_position(position.min(self.limit));
        }
        if c.supersedes_play() {
            if self.retry_next {
                self.status.playing = false;
            }
            self.retry_next = false;
            self.consecutive_failures = 0;
        }
        match c {
            Command::FinishListening(reply) => {
                self.end_voice(End::Shutdown);
                let _ = reply.send(());
            }
            Command::EraseListening(id, reply) => {
                self.credit_output();
                self.flush_occurrence();
                if let Some(writer) = &self.listening {
                    writer.erase(id, reply);
                } else {
                    let _ =
                        reply.try_send(Err(anyhow::anyhow!("Listening history is unavailable.")));
                }
            }
            Command::PlayDisc {
                tracks,
                from_chapter,
            } => {
                if from_chapter.is_none() {
                    self.queue.shuffle = false;
                    if self.queue.repeat == Repeat::One {
                        self.queue.set_repeat(Repeat::Off);
                    }
                }
                let i = from_chapter.unwrap_or(0);
                self.queue.replace(tracks.into(), i);
                for entry in Arc::make_mut(&mut self.queue.entries) {
                    entry.disc_context = DiscContext::Selected;
                }
                self.play(i, 0.)?;
            }
            Command::PlayList(selection, shuffle) => {
                self.queue.shuffle = shuffle;
                // Shuffle the entire list before selecting the first occurrence.
                let start = if shuffle { selection.tracks.len() } else { 0 };
                self.queue.replace(selection, start);
                self.play(0, 0.)?;
            }
            Command::Play(selection, i) => {
                self.queue.replace(selection, i);
                self.play(i, 0.)?;
            }
            Command::Volume(v) => {
                self.volume = v.clamp(0., 1.);
                if let Some(output) = &self.scrub_output {
                    output.set_volume(self.volume);
                }
                if let Some(h) = &mut self.voice {
                    h.state
                        .volume
                        .store(self.volume.to_bits(), Ordering::Release);
                }
            }
            Command::Toggle if self.status.playing => self.pause(),
            Command::Toggle | Command::SetPlaying(true) => self.resume()?,
            Command::SetPlaying(false) => self.pause(),
            Command::Stop => {
                self.stop_voice();
                self.status.occurrence_revision += 1;
                self.status.position = 0.;
            }
            Command::Step(delta) => {
                let Some(current) = self.queue.current else {
                    return Ok(());
                };
                let target = if delta < 0 {
                    if self.status.position > 3. {
                        Some(current)
                    } else {
                        self.queue.previous_index().or(Some(current))
                    }
                } else {
                    self.queue.next_index(false)
                };
                if let Some(i) = target {
                    if delta > 0 {
                        self.play_eligible(i)?;
                    } else {
                        self.play(i, 0.)?;
                    }
                }
            }
            Command::Jump(id) => {
                if let Some(i) = self.queue.index_of(id) {
                    self.play(i, 0.)?;
                }
            }
            Command::Seek(fraction) => {
                let position = fraction.clamp(0., 1.) * self.status.duration;
                self.seek_to(position)?;
            }
            Command::SeekTo(position) => self.seek_to(position)?,
            Command::SeekWave { anchor, frame } => {
                if self.accepts_wave_seek(&anchor, frame) {
                    // The backend may already have reached the next chapter
                    // while the transport or the file checks were pending.
                    let at_end = if self.voice.is_some() {
                        let position = self.credit_output();
                        self.update_position(position.min(self.limit));
                        self.status.playing && position >= self.limit
                    } else {
                        false
                    };
                    if !at_end
                        && self.status.playback_identity() == Some(anchor.identity)
                        && self.queue.revision == anchor.identity.queue
                    {
                        let rate = f64::from(anchor.source.rate);
                        if !self.status.playing {
                            self.status.duration = anchor.playable.len() as f64 / rate;
                        }
                        self.seek_position((frame - anchor.playable.start) as f64 / rate)?;
                    }
                }
            }
            Command::SeekProgram(seek) => self.seek_program(&seek)?,
            Command::PlayNext(tracks) => {
                self.queue.play_next(&tracks);
                self.revalidate();
            }
            Command::Enqueue(tracks) => {
                self.queue.add(&tracks);
                self.revalidate();
            }
            Command::MoveUpcoming(id, before) => {
                self.queue.move_upcoming(id, before);
                self.revalidate();
            }
            Command::ScheduleNext(id) => {
                self.queue.schedule_next(id);
                self.revalidate();
            }
            Command::Remove(ids) => {
                // The queue keeps the active occurrence, so its voice,
                // position and listening occurrence continue unchanged.
                if self.queue.remove(&ids) {
                    self.revalidate();
                }
            }
            Command::ContinueDisc(entry, program, context) => {
                if self.queue.continue_disc(entry, &program, context) {
                    self.revalidate();
                }
            }
            Command::ClearUpcoming => {
                if self.retry_next {
                    self.retry_next = false;
                    self.stop_voice();
                }
                self.queue.clear_upcoming();
                self.revalidate();
            }
            Command::ClearHistory => {
                self.queue.clear_history();
                self.revalidate();
            }
            Command::Shuffle(on) => {
                self.queue.set_shuffle(on);
                self.revalidate();
            }
            Command::Repeat(r) => {
                self.queue.set_repeat(r);
                self.revalidate();
            }
            Command::Snapshot(reply) => {
                let snapshot =
                    (!self.queue.entries.is_empty()).then(|| crate::profile::SavedQueue {
                        queue: self.queue.clone(),
                        position: self.status.position,
                    });
                let _ = reply.send(snapshot);
            }
            Command::Restore(queue, position) => {
                self.stop_voice();
                let revision = self.queue.revision + 1;
                self.queue = *queue;
                self.queue.revision = revision;
                self.select(self.queue.current, position);
            }
            Command::BeginWaveScrub(_) => unreachable!("scrub admission was handled above"),
        }
        Ok(())
    }
    /// Validates the destination, then reconciles the queue and commits the
    /// seek as one transport operation. A stale or changed destination is
    /// rejected before the queue changes.
    fn seek_program(&mut self, seek: &ProgramSeek) -> Result<()> {
        let Some(target) = seek.program.get(seek.chapter) else {
            return Ok(());
        };
        let sheet_unchanged = match (&target.disc, &seek.sheet_revision) {
            (Some(disc), Some(stamp)) => {
                crate::cache::Stamp::read(&disc.sheet).as_ref() == Some(stamp)
            }
            (None, None) => true,
            _ => false,
        };
        if self.queue.current_entry().map(|e| e.id) != Some(seek.origin)
            || crate::cache::Stamp::read(&target.source).as_ref() != Some(&seek.source_revision)
            || !sheet_unchanged
            || !(0. ..seek.duration).contains(&seek.seconds)
        {
            return Ok(());
        }
        let Some(i) = self.queue.program_destination(&seek.program, seek.chapter) else {
            return Ok(());
        };
        if self.status.playing {
            if self.queue.current == Some(i) {
                self.seek_to(seek.seconds)?;
            } else {
                self.play(i, seek.seconds)?;
            }
        } else {
            if self.queue.current == Some(i) {
                self.detach_voice();
                self.flush_occurrence();
            } else {
                self.end_voice(End::Replaced);
            }
            self.select(Some(i), 0.);
            self.status.seek_revision += 1;
            self.status.duration = seek.duration;
            self.status.position = seek.seconds;
        }
        Ok(())
    }

    fn accepts_wave_seek(&self, anchor: &WaveSeekAnchor, frame: usize) -> bool {
        let Some(entry) = self.queue.current_entry() else {
            return false;
        };
        self.status.playback_identity() == Some(anchor.identity)
            && self.queue.revision == anchor.identity.queue
            && anchor.playable.contains(&frame)
            && anchor.source.playable(&entry.track).as_ref() == Some(&anchor.playable)
            && self
                .status
                .source_revision
                .as_ref()
                .is_none_or(|stamp| stamp == &anchor.source.stamp)
            && self
                .status
                .source_object
                .is_none_or(|object| Some(object) == anchor.source.object)
            && match (&entry.track.disc, &anchor.sheet) {
                (None, None) => true,
                (Some(disc), Some((path, revision))) => {
                    disc.sheet == *path
                        && crate::cache::Revision::read(path).as_ref() == Some(revision)
                }
                _ => false,
            }
            && anchor.source.available
            && anchor.source.unchanged()
    }

    fn ensure_output(&mut self) -> Result<()> {
        if self.device.is_none() {
            self.device = Some(
                AudioManager::<DefaultBackend>::new(AudioManagerSettings {
                    main_track_builder: output_track(
                        self.signal.clone(),
                        cfg!(test) || silent_qualification(),
                    ),
                    ..Default::default()
                })
                .context("Windows audio output is unavailable")?,
            );
        }
        Ok(())
    }

    fn begin_wave_scrub(&mut self, gesture: Arc<scrub::Gesture>) -> Result<()> {
        if self.retry_next
            || self.scrubbing.is_some()
            || self.status.error.is_some()
            || self.status.playing != gesture.was_playing
            || gesture.ending() == Some(scrub::End::Cancel)
            || !self.accepts_wave_seek(&gesture.anchor, gesture.origin.floor() as usize)
            || self.scrub_bounds(&gesture).as_ref() != Some(&gesture.bounds)
        {
            gesture.close();
            return Ok(());
        }
        if self.voice.is_some() {
            let position = self.credit_output();
            self.update_position(position.min(self.limit));
            if self.status.playback_identity() != Some(gesture.anchor.identity)
                || (self.status.playing && position >= self.limit)
            {
                gesture.close();
                return Ok(());
            }
        }
        if gesture.ending().is_none() {
            let ready = self.ensure_output().and_then(|()| {
                if self.scrub_output.is_none() {
                    self.scrub_output = Some(scrub::Output::install(
                        self.device.as_mut().unwrap(),
                        self.scrub_control.clone(),
                    )?);
                }
                Ok(())
            });
            if let Err(error) = ready {
                self.status.scrub_error = Some((
                    gesture.id,
                    format!("Could not start waveform scrubbing ({error:#})."),
                ));
                gesture.close();
                return Ok(());
            }
        }
        if gesture.ending() == Some(scrub::End::Cancel) {
            gesture.close();
            return Ok(());
        }
        self.pause();
        if let Some(output) = &mut self.scrub_output {
            output.start(&gesture, self.volume);
        }
        self.status.scrub = Some(ScrubPosition {
            id: gesture.id,
            frame: gesture.frame().unwrap_or(gesture.origin),
            was_playing: gesture.was_playing,
        });
        crate::diagnostics::stage(
            crate::diagnostics::begin_stage(),
            "wave_scrub_begin",
            gesture.origin.to_bits(),
            gesture.id,
        );
        self.scrubbing = Some(ActiveScrub {
            gesture,
            updated: Instant::now(),
        });
        self.update_scrub()
    }

    /// The frames a gesture may reach: the program's chapters in its source
    /// for Full CD, the contiguous queue sequence for Track.
    fn scrub_bounds(&self, gesture: &scrub::Gesture) -> Option<Range<usize>> {
        let source = &gesture.anchor.source;
        match &gesture.program {
            Some(program) => program_run(program, source, &gesture.anchor.playable),
            None => scrub_run(&self.queue, source),
        }
        .map(|(_, bounds)| bounds)
    }

    fn close_scrub(&mut self) -> Option<Arc<scrub::Gesture>> {
        let active = self.scrubbing.take()?;
        if let Some(output) = &self.scrub_output {
            output.stop();
        }
        self.status.scrub = None;
        active.gesture.close();
        Some(active.gesture)
    }

    fn update_scrub(&mut self) -> Result<()> {
        let Some(active) = &mut self.scrubbing else {
            return Ok(());
        };
        let gesture = &active.gesture;
        let ending = gesture.ending();
        let invalid = self.status.playback_identity() != Some(gesture.anchor.identity)
            || self.queue.revision != gesture.anchor.identity.queue;
        let failed = self
            .scrub_output
            .as_ref()
            .is_some_and(|output| output.failed(gesture.id));
        if invalid || failed || ending.is_some() {
            let frame = gesture.frame().unwrap_or(gesture.origin);
            let gesture = self.close_scrub().unwrap();
            if failed {
                self.status.scrub_error = Some((
                    gesture.id,
                    "Could not read audio for waveform scrubbing.".into(),
                ));
            }
            if invalid {
                if gesture.was_playing && self.voice.is_some() {
                    self.resume()?;
                }
                return Ok(());
            }
            if ending == Some(scrub::End::Release) && !failed {
                return self.finish_wave_scrub(&gesture, frame);
            }
            if gesture.was_playing {
                return self.finish_wave_scrub(&gesture, gesture.origin);
            }
            // A paused transport never moved. In particular, a naturally
            // finished entry must stay at EOF rather than its last sample.
            Self::trace_scrub_end(&gesture, gesture.origin.round() as usize);
            return Ok(());
        }
        // The detail decks sample pointer motion without polling. The compact
        // strip consumes a coalesced transport snapshot.
        if active.updated.elapsed() >= Duration::from_millis(32)
            && let Some(frame) = gesture.frame()
        {
            active.updated = Instant::now();
            self.status.scrub = Some(ScrubPosition {
                id: gesture.id,
                frame,
                was_playing: gesture.was_playing,
            });
        }
        Ok(())
    }

    fn finish_wave_scrub(&mut self, gesture: &scrub::Gesture, frame: f64) -> Result<()> {
        let source = &gesture.anchor.source;
        if !self.accepts_wave_seek(&gesture.anchor, gesture.origin.floor() as usize) {
            self.end_voice(End::SourceFailed);
            return Ok(());
        }
        if self.scrub_bounds(gesture).as_ref() != Some(&gesture.bounds) {
            return Ok(());
        }
        let frame = frame.round() as usize;
        let destination = if let Some(program) = &gesture.program {
            // Full CD: the authored chapter, reconciled into the queue now.
            let (mut chapters, _) =
                program_run(program, source, &gesture.anchor.playable).unwrap_or((0..0, 0..0));
            chapters
                .find_map(|chapter| {
                    let segment = source.playable(&program[chapter])?;
                    segment.contains(&frame).then_some((chapter, segment))
                })
                .and_then(|(chapter, segment)| {
                    Some((self.queue.program_destination(program, chapter)?, segment))
                })
        } else {
            scrub_run(&self.queue, source).and_then(|(mut indices, _)| {
                indices.find_map(|index| {
                    let segment = source.playable(&self.queue.entries[index].track)?;
                    segment.contains(&frame).then_some((index, segment))
                })
            })
        };
        let Some((index, segment)) = destination else {
            return Ok(());
        };
        self.detach_voice();
        self.flush_occurrence();
        let destination = self.queue.entries[index].id;
        if self.queue.current != Some(index)
            || self
                .occurrence
                .as_ref()
                .is_some_and(|(id, _)| *id != destination)
        {
            self.finish_occurrence(End::Replaced, listening::timestamp());
            self.suppressed_entry = None;
        }
        self.status.seek_revision += 1;
        let rate = f64::from(source.rate);
        let position = (frame - segment.start) as f64 / rate;
        if gesture.was_playing {
            self.start_voice(
                index,
                position,
                Some(Tween {
                    duration: Duration::from_millis(3),
                    ..Default::default()
                }),
            )?;
        } else {
            self.select(Some(index), 0.);
            self.status.duration = segment.len() as f64 / rate;
            self.status.position = position;
            self.status.source_revision = Some(source.stamp.clone());
            self.status.source_object = source.object;
        }
        Self::trace_scrub_end(gesture, frame);
        Ok(())
    }

    fn trace_scrub_end(gesture: &scrub::Gesture, frame: usize) {
        crate::diagnostics::stage(
            crate::diagnostics::begin_stage(),
            "wave_scrub_end",
            frame as u64,
            gesture.id,
        );
    }

    fn seek_to(&mut self, position: f64) -> Result<()> {
        if self.status.duration <= 0. {
            return Ok(());
        }
        let position = position.clamp(0., self.status.duration);
        let position = if self.rate > 0 {
            position.min((self.status.duration - 1. / f64::from(self.rate)).max(0.))
        } else {
            position
        };
        self.seek_position(position)
    }

    fn seek_position(&mut self, position: f64) -> Result<()> {
        self.status.seek_revision += 1;
        if self.status.playing {
            if let Some(i) = self.queue.current {
                self.detach_voice();
                self.flush_occurrence();
                if self
                    .occurrence
                    .as_ref()
                    .is_some_and(|(id, _)| Some(*id) != self.queue.current_entry().map(|e| e.id))
                {
                    self.finish_occurrence(End::Replaced, listening::timestamp());
                }
                self.start_voice(i, position, None)?;
            }
        } else {
            let selected = self.queue.current;
            self.detach_voice();
            self.flush_occurrence();
            self.queue.set_current(selected);
            self.status.position = position;
        }
        Ok(())
    }
    fn output_error(&mut self, error: kira::backend::cpal::cpal::Error) -> Result<()> {
        use kira::backend::cpal::cpal::ErrorKind;
        if let Some(d) = crate::diagnostics::get() {
            d.device_errors.fetch_add(1, Ordering::Relaxed);
        }
        if matches!(
            error.kind(),
            ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
        ) {
            return Ok(());
        }
        self.retry_next = false;
        let position = self.credit_output();
        self.update_position(position.min(self.limit));
        self.end_voice(End::OutputFailed);
        self.scrub_output = None;
        self.device = None;
        self.discarded_errors = 0;
        Err(error).context("Windows audio output failed")
    }
    fn tick(&mut self) -> Result<()> {
        while let Some(error) = self
            .device
            .as_mut()
            .and_then(|d| d.backend_mut().pop_error())
        {
            self.output_error(error)?;
        }
        self.update_scrub()?;
        if self.scrubbing.is_some() {
            // The normal voice can still commit the tail of its pause ramp,
            // even across a CUE boundary. Credit those blocks without moving
            // the selected occurrence before the scrub is committed.
            if self.pause_checkpoint {
                self.credit_output();
                if let Some(voice) = &self.voice
                    && !voice.state().is_advancing()
                {
                    self.pause_checkpoint = false;
                    if let Some(error) = voice.pop_error() {
                        self.end_voice(End::SourceFailed);
                        self.status.error = Some(format!("Cannot play the recording: {error}"));
                    } else {
                        self.flush_occurrence();
                    }
                }
            }
            return Ok(());
        }
        if !self.status.playing {
            if self.pause_checkpoint {
                if let Some(voice) = &mut self.voice
                    && voice.state() == PlaybackState::Stopped
                {
                    if let Some(error) = voice.pop_error() {
                        self.end_voice(End::SourceFailed);
                        self.status.error = Some(format!("Cannot play the recording: {error}"));
                    } else {
                        let position = self.credit_output();
                        self.update_position(position.min(self.limit));
                        self.end_voice(End::Completed);
                    }
                    return Ok(());
                }
                let position = self.credit_output();
                self.update_position(position.min(self.limit));
                if self
                    .voice
                    .as_ref()
                    .is_none_or(|v| !v.state().is_advancing())
                {
                    self.pause_checkpoint = false;
                    self.credit_output();
                    self.flush_occurrence();
                }
            }
            return Ok(());
        }
        if std::mem::take(&mut self.retry_next) {
            return self.advance(false);
        }
        let Some(v) = &mut self.voice else {
            return Ok(());
        };
        if let Some(d) = crate::diagnostics::get() {
            if let Some(device) = &mut self.device {
                let backend = device.backend_mut();
                while let Some(ratio) = backend.pop_cpu_usage() {
                    d.callbacks.fetch_add(1, Ordering::Relaxed);
                    d.cpu_permille
                        .fetch_max((ratio * 1000.) as u64, Ordering::Relaxed);
                }
                if let Some(total) = backend.num_stream_errors_discarded() {
                    d.discarded_errors.fetch_add(
                        total.saturating_sub(self.discarded_errors),
                        Ordering::Relaxed,
                    );
                    self.discarded_errors = total;
                }
            }
            if self.status.playing && v.state() == PlaybackState::Playing {
                let now = std::time::Instant::now();
                let pos = v.position();
                if let Some((last, prev)) = self.position_probe.replace((now, pos)) {
                    let error = ((pos - prev) - (now - last).as_secs_f64()).abs();
                    d.position_error_us
                        .fetch_max((error * 1e6) as u64, Ordering::Relaxed);
                }
            }
            if self.status.playing
                && self
                    .group
                    .get(self.chapter)
                    .is_some_and(|c| (v.position() - c.offset - self.status.position).abs() < 1e-9)
            {
                d.stalls.fetch_add(1, Ordering::Relaxed);
            }
        }
        let stopped = v.state() == PlaybackState::Stopped;
        if let Some(error) = v.pop_error() {
            if let Some(d) = crate::diagnostics::get() {
                d.decode_errors.fetch_add(1, Ordering::Relaxed);
            }
            let position = v.position();
            self.update_position(position.min(self.limit));
            let title = self
                .status
                .track
                .as_ref()
                .map(|t| t.title.clone())
                .unwrap_or_default();
            return self
                .recording_failed(anyhow::Error::new(error).context(RecordingFailure(title)));
        }
        let position = self.credit_output();
        let ended = stopped || position >= self.limit;
        self.update_position(position.min(self.limit));
        if ended {
            self.consecutive_failures = 0;
            self.end_voice(End::Completed);
            self.advance(true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "audioTests.rs"]
pub(crate) mod tests;

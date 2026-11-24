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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub use crate::recording::Recording;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub track: Option<Arc<Track>>,
    /// The queue entry that is playing, which tells two copies of a track apart.
    pub entry: Option<u64>,
    pub source_revision: Option<crate::cache::Stamp>,
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
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TransportState {
    Playing,
    Paused,
    Finished,
    Stopped,
}
impl Status {
    pub fn playback_state(&self) -> TransportState {
        if self.playing {
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
    SeekOnDisc {
        target: u64,
        source_revision: crate::cache::Stamp,
        sheet_revision: Option<crate::cache::Stamp>,
        revision: u64,
        seconds: f64,
        duration: f64,
    },
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
        matches!(
            (&self.command, &old.command),
            (Command::Volume(_), Command::Volume(_))
                | (
                    Command::Seek(_) | Command::SeekTo(_),
                    Command::Seek(_) | Command::SeekTo(_)
                )
        ) && self.entry == old.entry
    }
    fn applies_to(&self, entry: Option<u64>) -> bool {
        !matches!(
            self.command,
            Command::Seek(_) | Command::SeekTo(_) | Command::SeekOnDisc { .. }
        ) || self.entry == entry
    }
}

#[derive(Clone)]
pub struct Commands {
    pub play_request: crate::store::Ticket,
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
                    ..Default::default()
                };
                let mut published = 0;
                let interval = Duration::from_millis(40);
                let mut next_tick = std::time::Instant::now() + interval;
                loop {
                    let ticking = engine.status.playing || engine.pause_checkpoint;
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
                    let result = match command {
                        Some(slot) => {
                            let pending =
                                slot.lock().unwrap().take().expect("one transport consumer");
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
                        engine.end_voice(End::SourceFailed, false);
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
                    }
                }
                engine.end_voice(End::Shutdown, false);
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
    voice: Option<StreamingSoundHandle<io::Error>>,
    listening: Option<listening::Writer>,
    credit: Option<VoiceCredit>,
    occurrence: Option<(u64, listening::Occurrence)>,
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
}

const RENDERING: u8 = 1;
const CLOSED: u8 = 2;

/// The renderer publishes its consumed source cursor at the end of each
/// render block. A transport seek constructs a fresh voice and cursor.
/// A closed voice cannot start another block, even before Kira unloads it.
struct ObservedSound {
    inner: Box<dyn kira::sound::Sound>,
    gate: Arc<std::sync::atomic::AtomicU8>,
}

impl kira::sound::Sound for ObservedSound {
    fn on_start_processing(&mut self) {
        self.inner.on_start_processing();
    }

    fn process(&mut self, out: &mut [kira::Frame], dt: f64, info: &kira::info::Info) {
        if self
            .gate
            .compare_exchange(0, RENDERING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            out.fill(kira::Frame::ZERO);
            return;
        }
        self.inner.process(out, dt, info);
        // Kira updates its handle position from its consumed ring cursor
        // here. The extra publication also allows us to drain a stopped
        // voice without attributing its last block to a replacement.
        self.inner.on_start_processing();
        self.gate.fetch_and(!RENDERING, Ordering::Release);
    }

    fn finished(&self) -> bool {
        self.gate.load(Ordering::Relaxed) & CLOSED != 0 || self.inner.finished()
    }
}

struct ObservedData {
    data: StreamingSoundData<io::Error>,
    gate: Arc<std::sync::atomic::AtomicU8>,
}

impl kira::sound::SoundData for ObservedData {
    type Error = io::Error;
    type Handle = StreamingSoundHandle<io::Error>;

    fn into_sound(self) -> io::Result<(Box<dyn kira::sound::Sound>, Self::Handle)> {
        let (inner, handle) = self.data.into_sound()?;
        Ok((
            Box::new(ObservedSound {
                inner,
                gate: self.gate,
            }),
            handle,
        ))
    }
}

struct CreditSegment {
    id: u64,
    start: i64,
    recording: listening::Recording,
}

struct VoiceCredit {
    gate: Arc<std::sync::atomic::AtomicU8>,
    cursor: i64,
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
fn disc_start(t: &Track) -> u64 {
    t.disc.as_ref().map_or(0, |d| d.start)
}
impl Engine {
    fn stop_voice(&mut self) {
        self.end_voice(End::Stopped, false);
    }
    fn end_voice(&mut self, reason: End, complete: bool) {
        self.detach_voice(complete);
        self.finish_occurrence(reason, listening::timestamp());
    }
    fn detach_voice(&mut self, complete: bool) {
        if let Some(credit) = &self.credit {
            credit.gate.fetch_or(CLOSED, Ordering::AcqRel);
            let deadline = std::time::Instant::now() + Duration::from_millis(4);
            while credit.gate.load(Ordering::Acquire) & RENDERING != 0
                && std::time::Instant::now() < deadline
            {
                std::thread::yield_now();
            }
            self.credit_output(complete);
            self.flush_occurrence();
        }
        self.credit = None;
        self.pause_checkpoint = false;
        self.position_probe = None;
        if let Some(mut v) = self.voice.take() {
            v.stop(Tween {
                duration: Duration::ZERO,
                ..Default::default()
            });
        }
        self.status.source_revision = None;
        self.group.clear();
        self.status.playing = false;
    }
    fn finish_occurrence(&mut self, reason: End, at: i64) {
        if let Some((_, occurrence)) = self.occurrence.take()
            && let Some(writer) = &self.listening
        {
            writer.save(occurrence.finish(at, reason));
        }
    }
    fn flush_occurrence(&mut self) {
        if let Some((_, occurrence)) = &mut self.occurrence
            && let Some(snapshot) = occurrence.checkpoint()
            && let Some(writer) = &self.listening
        {
            writer.save(snapshot);
        }
    }
    fn credit_output(&mut self, completed: bool) {
        let Some(v) = &self.voice else {
            return;
        };
        let end = position_us(self.limit);
        let position = if completed {
            end
        } else {
            position_us(v.position()).min(end)
        };
        self.credit_progress(position, listening::timestamp());
    }
    fn credit_progress(&mut self, position: i64, observed_at: i64) {
        let Some(mut credit) = self.credit.take() else {
            return;
        };
        let end = position_us(self.limit);
        let mut from = observed_at - (position - credit.cursor).max(0);
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
            let (_, occurrence) = self.occurrence.get_or_insert_with(|| {
                (
                    segment.id,
                    listening::Occurrence::new(segment.recording.clone(), from),
                )
            });
            let duration = (boundary - segment.start) as f64 / 1_000_000.;
            if let Some(checkpoint) = occurrence.credit(Some(duration), from, amount)
                && let Some(writer) = &self.listening
            {
                writer.save(checkpoint);
            }
            credit.cursor += amount;
            from += amount;
        }
        self.credit = Some(credit);
    }
    /// Shows an entry as current without opening the output.
    fn select(&mut self, index: Option<usize>, position: f64) {
        self.queue.set_current(index);
        let entry = self.queue.current_entry();
        self.status.track = entry.map(|e| e.track.clone());
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
        self.end_voice(End::Replaced, false);
        self.start_voice(index, position)
    }
    fn start_voice(&mut self, index: usize, position: f64) -> Result<()> {
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
        self.rate = recording.sample_rate();
        let first = crate::recording::disc_frame(disc_start(&entry.track), self.rate);
        for (chapter, queued) in group.iter_mut().zip(&self.queue.entries[index..]) {
            let frame = crate::recording::disc_frame(disc_start(&queued.track), self.rate);
            chapter.offset = (frame - first) as f64 / f64::from(self.rate);
        }
        self.decoder_end = end;
        self.length = recording.num_frames() as f64 / f64::from(recording.sample_rate());
        let start =
            ((position * f64::from(self.rate)).round() as usize).min(recording.num_frames() - 1);
        let data = StreamingSoundData::from_decoder(recording)
            .volume(db(self.volume))
            .start_position(kira::sound::PlaybackPosition::Samples(start));
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
        self.signal.reset();
        let device = self.device.as_mut().unwrap();
        let played = if self.listening.is_some() {
            let gate = Arc::new(std::sync::atomic::AtomicU8::new(0));
            let sheet = entry
                .track
                .disc
                .as_ref()
                .and_then(|d| crate::cache::Stamp::read(&d.sheet));
            self.credit = Some(VoiceCredit {
                gate: gate.clone(),
                cursor: position_us(start as f64 / f64::from(self.rate)),
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
                        },
                    })
                    .collect(),
            });
            device.play(ObservedData { data, gate })
        } else {
            device.play(data)
        };
        self.voice = Some(played.map_err(|e| match e {
            kira::PlaySoundError::IntoSoundError(e) => {
                anyhow::Error::new(e).context(RecordingFailure(entry.track.title.clone()))
            }
            other => anyhow::Error::new(other).context("Cannot start the output voice"),
        })?);
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
        if let Some(credit) = &mut self.credit {
            // The queued prefix has already been attributed. A queue edit
            // can change the identity of a later entry at the same offset.
            credit.segments.truncate(self.chapter + 1);
            let recording = &credit.segments[self.chapter].recording;
            let source = recording.source.clone();
            let sheet = recording.sheet.clone();
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
        self.end_voice(End::SourceFailed, false);
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
                v.resume(Tween::default());
                self.status.playing = true;
            }
            return Ok(());
        }
        let Some(index) = self.queue.current.or_else(|| self.queue.next_index(false)) else {
            return Ok(());
        };
        let position = if self.queue.current == Some(index)
            && (self.status.duration == 0. || self.status.position < self.status.duration)
        {
            self.status.position
        } else {
            0.
        };
        self.start_voice(index, position)
    }
    fn pause(&mut self) {
        if let Some(v) = &mut self.voice
            && self.status.playing
        {
            v.pause(Tween::default());
            self.position_probe = None;
            self.pause_checkpoint = self.credit.is_some();
        }
        self.status.playing = false;
        self.credit_output(false);
        self.flush_occurrence();
    }
    fn command(&mut self, c: Command) -> Result<()> {
        if let Some(d) = crate::diagnostics::get() {
            d.commands.fetch_add(1, Ordering::Relaxed);
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
                Command::Seek(_) | Command::SeekTo(_) | Command::SeekOnDisc { .. } => return Ok(()),
                _ => {}
            }
        }
        // Account for the old CUE program before changing the queue.
        if let Some(voice) = &self.voice
            && self.credit.is_some()
            && self.status.playing
            && !matches!(
                &c,
                Command::Seek(_) | Command::SeekTo(_) | Command::SeekOnDisc { .. }
            )
        {
            let position = voice.position().min(self.limit);
            self.credit_progress(position_us(position), listening::timestamp());
            self.update_position(position);
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
                self.end_voice(End::Shutdown, false);
                let _ = reply.send(());
            }
            Command::EraseListening(id, reply) => {
                self.credit_output(false);
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
                if let Some(h) = &mut self.voice {
                    h.set_volume(db(self.volume), Tween::default());
                }
            }
            Command::Toggle if self.status.playing => self.pause(),
            Command::Toggle | Command::SetPlaying(true) => self.resume()?,
            Command::SetPlaying(false) => self.pause(),
            Command::Stop => {
                self.stop_voice();
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
            Command::SeekOnDisc {
                target,
                source_revision,
                sheet_revision,
                revision,
                seconds,
                duration,
            } => {
                if self.queue.revision == revision
                    && let Some(i) = self.queue.index_of(target)
                    && crate::cache::Stamp::read(&self.queue.entries[i].track.source)
                        == Some(source_revision)
                    && sheet_revision.is_none_or(|s| {
                        self.queue.entries[i]
                            .track
                            .disc
                            .as_ref()
                            .is_some_and(|d| crate::cache::Stamp::read(&d.sheet) == Some(s))
                    })
                {
                    if self.status.playing {
                        if self.queue.current == Some(i) {
                            self.seek_to(seconds)?;
                        } else {
                            self.play(i, seconds)?;
                        }
                    } else {
                        if self.queue.current == Some(i) {
                            self.detach_voice(false);
                        } else {
                            self.end_voice(End::Replaced, false);
                        }
                        self.select(Some(i), 0.);
                        self.status.duration = duration;
                        self.status.position = seconds;
                    }
                }
            }
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
                if self.queue.remove(&ids) {
                    let resume = self.status.playing;
                    self.retry_next = false;
                    self.stop_voice();
                    match self.queue.current {
                        Some(i) if resume => self.play_eligible(i)?,
                        current => self.select(current, 0.),
                    }
                } else {
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
        }
        Ok(())
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
        if self.status.playing {
            if let Some(i) = self.queue.current {
                self.detach_voice(false);
                if self
                    .occurrence
                    .as_ref()
                    .is_some_and(|(id, _)| Some(*id) != self.queue.current_entry().map(|e| e.id))
                {
                    self.finish_occurrence(End::Replaced, listening::timestamp());
                }
                self.start_voice(i, position)?;
            }
        } else {
            let selected = self.queue.current;
            self.detach_voice(false);
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
        self.end_voice(End::OutputFailed, false);
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
        if !self.status.playing {
            if self.pause_checkpoint {
                if let Some(voice) = &mut self.voice
                    && voice.state() == PlaybackState::Stopped
                {
                    if let Some(error) = voice.pop_error() {
                        self.end_voice(End::SourceFailed, false);
                        self.status.error = Some(format!("Cannot play the recording: {error}"));
                    } else {
                        self.update_position(self.limit);
                        self.end_voice(End::Completed, true);
                    }
                    return Ok(());
                }
                self.credit_output(false);
                if self
                    .voice
                    .as_ref()
                    .is_none_or(|v| !v.state().is_advancing())
                {
                    self.pause_checkpoint = false;
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
        let position = if stopped { self.limit } else { v.position() };
        let ended = stopped || position >= self.limit;
        self.credit_output(stopped);
        self.update_position(position.min(self.limit));
        if ended {
            self.consecutive_failures = 0;
            self.end_voice(End::Completed, stopped);
            self.advance(true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "audioTests.rs"]
pub(crate) mod tests;

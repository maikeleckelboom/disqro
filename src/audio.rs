use crate::{
    library::Track,
    queue::{Queue, Repeat},
};
use anyhow::{Context, Result, ensure};
use kira::{
    AudioManager, Decibels, DefaultBackend, Tween,
    sound::{
        PlaybackState,
        streaming::{Decoder as StreamDecoder, StreamingSoundData, StreamingSoundHandle},
    },
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
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub previous: bool,
    pub next: bool,
    pub error: Option<String>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub queue_revision: u64,
}

/// Every control path — the window, the tray, media keys — sends these to
/// the one transport, so they can never disagree about what is playing.
pub enum Command {
    /// Replace the queue with these tracks and play from the index.
    Play(crate::library::TrackList, usize),
    Toggle,
    SetPlaying(bool),
    Stop,
    /// Seek to a fraction of the current track.
    Seek(f64),
    /// Seek to a time in seconds within the current track.
    SeekTo(f64),
    Step(i8),
    Volume(f32),
    Jump(u64),
    PlayNext(crate::library::TrackList),
    Enqueue(crate::library::TrackList),
    Remove(Vec<u64>),
    Move(u64, usize),
    ClearUpcoming,
    ClearHistory,
    Shuffle(bool),
    Repeat(Repeat),
    /// Bring back a saved queue, paused at the saved position.
    Restore(Box<Queue>, f64),
    Snapshot(mpsc::SyncSender<Option<crate::profile::SavedQueue>>),
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
        !matches!(self.command, Command::Seek(_) | Command::SeekTo(_)) || self.entry == entry
    }
}

#[derive(Clone)]
pub struct Commands {
    tx: mpsc::SyncSender<Slot>,
    tail: Arc<Mutex<std::sync::Weak<Mutex<Option<Pending>>>>>,
    shared: Arc<Mutex<Shared>>,
}
impl Commands {
    fn channel(shared: Arc<Mutex<Shared>>, capacity: usize) -> (Self, mpsc::Receiver<Slot>) {
        let (tx, rx) = mpsc::sync_channel(capacity);
        (
            Self {
                tx,
                shared,
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
        let pending = Pending { command, entry };
        let mut tail = self.tail.lock().unwrap();
        if let Some(slot) = tail.upgrade() {
            let mut last = slot.lock().unwrap();
            if last.as_ref().is_some_and(|old| pending.replaces(old)) {
                *last = Some(pending);
                return Ok(());
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
        Ok(())
    }
}
pub struct Transport {
    tx: Commands,
    latest: Arc<Mutex<Shared>>,
    quiet: Arc<AtomicBool>,
    pub changed: async_channel::Receiver<()>,
}
impl Transport {
    pub fn new(volume: f32) -> Self {
        let latest = Arc::new(Mutex::new(Shared {
            status: Status::default(),
            queue: Arc::default(),
        }));
        let (tx, rx) = Commands::channel(latest.clone(), 64);
        let shared = latest.clone();
        let quiet = Arc::new(AtomicBool::new(false));
        let is_quiet = quiet.clone();
        let (wake, changed) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("disqro-transport".into())
            .spawn(move || {
                let mut engine = Engine {
                    volume,
                    ..Default::default()
                };
                let mut published = 0;
                let interval = Duration::from_millis(40);
                let mut next_tick = std::time::Instant::now() + interval;
                loop {
                    let command = if engine.status.playing && std::time::Instant::now() >= next_tick
                    {
                        None
                    } else if engine.status.playing {
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
                        engine.stop_voice();
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
            })
            .expect("transport thread");
        Self {
            tx,
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

#[derive(Default)]
struct Engine {
    device: Option<AudioManager<DefaultBackend>>,
    voice: Option<StreamingSoundHandle<io::Error>>,
    queue: Queue,
    /// Consecutive chapters of one disc image share a decoder.
    group: Vec<Chapter>,
    chapter: usize,
    /// Where the group ends in the decoder, earlier than `length` when the
    /// queue changed after the group was opened.
    limit: f64,
    length: f64,
    volume: f32,
    status: Status,
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
        if let Some(mut v) = self.voice.take() {
            v.stop(Tween {
                duration: Duration::ZERO,
                ..Default::default()
            });
        }
        self.group.clear();
        self.status.playing = false;
    }
    /// Shows an entry as current without opening the output.
    fn select(&mut self, index: Option<usize>, position: f64) {
        self.queue.set_current(index);
        let entry = self.queue.current_entry();
        self.status.track = entry.map(|e| e.track.clone());
        self.status.entry = entry.map(|e| e.id);
        self.status.duration = entry.and_then(|e| e.track.duration).unwrap_or(0.);
        self.status.position = position.clamp(0., self.status.duration.max(0.));
        self.status.error = None;
    }
    fn play(&mut self, index: usize, position: f64) -> Result<()> {
        self.stop_voice();
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
                    offset: (disc_start(&next.track) - disc_start(&entry.track)) as f64 / 75.,
                });
                last += 1;
            }
        }
        let end = self.queue.entries[last]
            .track
            .disc
            .as_ref()
            .and_then(|d| d.end);
        let recording = Recording::open(&entry.track, end)
            .with_context(|| format!("Cannot play {}", entry.track.title))?;
        self.length = recording.num_frames() as f64 / f64::from(recording.sample_rate());
        ensure!(
            group.last().unwrap().offset < self.length,
            "A CUE track begins beyond the recording"
        );
        let data = StreamingSoundData::from_decoder(recording)
            .volume(db(self.volume))
            .start_position(position.min((self.length - 0.001).max(0.)));
        if self.device.is_none() {
            self.device = Some(
                AudioManager::<DefaultBackend>::new(Default::default())
                    .context("Windows audio output is unavailable")?,
            );
        }
        self.voice = Some(self.device.as_mut().unwrap().play(data)?);
        self.group = group;
        self.chapter = 0;
        self.limit = self.length;
        self.status.playing = true;
        self.update_position(position);
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
    /// After the queue changed, keeps only the chapters that still follow the
    /// current one in the queue. The voice then stops where the group now ends.
    fn revalidate(&mut self) {
        let Some(current) = self.group.get(self.chapter).copied() else {
            return;
        };
        let Some(mut ix) = self.queue.index_of(current.id) else {
            return;
        };
        let mut valid = self.chapter + 1;
        if self.queue.repeat != Repeat::One {
            while let Some(c) = self.group.get(valid) {
                if self.queue.entries.get(ix + 1).map(|e| e.id) != Some(c.id) {
                    break;
                }
                ix += 1;
                valid += 1;
            }
        }
        if let Some(cut) = self.group.get(valid) {
            self.limit = cut.offset;
            self.group.truncate(valid);
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
    fn advance(&mut self, ended: bool) -> Result<()> {
        match self.queue.next_index(ended) {
            Some(i) => self.play(i, 0.),
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
        let Some(index) = self
            .queue
            .current
            .or((!self.queue.entries.is_empty()).then_some(0))
        else {
            return Ok(());
        };
        let position =
            if self.queue.current == Some(index) && self.status.position < self.status.duration {
                self.status.position
            } else {
                0.
            };
        self.play(index, position)
    }
    fn pause(&mut self) {
        if let Some(v) = &mut self.voice
            && self.status.playing
        {
            v.pause(Tween::default());
            self.status.playing = false;
        }
    }
    fn command(&mut self, c: Command) -> Result<()> {
        match c {
            Command::Play(tracks, i) => {
                self.queue.replace(&tracks, i);
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
                    self.play(i, 0.)?;
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
            Command::PlayNext(tracks) => {
                self.queue.play_next(&tracks);
                self.revalidate();
            }
            Command::Enqueue(tracks) => {
                self.queue.add(&tracks);
                self.revalidate();
            }
            Command::Move(id, to) => {
                self.queue.move_entry(id, to);
                self.revalidate();
            }
            Command::Remove(ids) => {
                if self.queue.remove(&ids) {
                    let resume = self.status.playing;
                    self.stop_voice();
                    match self.queue.current {
                        Some(i) if resume => self.play(i, 0.)?,
                        current => self.select(current, 0.),
                    }
                } else {
                    self.revalidate();
                }
            }
            Command::ClearUpcoming => {
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
                self.queue = *queue;
                self.queue.revision += 1;
                self.select(self.queue.current, position);
            }
        }
        Ok(())
    }
    fn seek_to(&mut self, position: f64) -> Result<()> {
        if self.status.duration <= 0. {
            return Ok(());
        }
        let position = position.clamp(0., (self.status.duration - 0.001).max(0.));
        if self.status.playing {
            if let Some(i) = self.queue.current {
                self.play(i, position)?;
            }
        } else {
            let selected = self.queue.current;
            self.stop_voice();
            self.queue.set_current(selected);
            self.status.position = position;
        }
        Ok(())
    }
    fn tick(&mut self) -> Result<()> {
        let Some(v) = &mut self.voice else {
            return Ok(());
        };
        if let Some(error) = v.pop_error() {
            return Err(error.into());
        }
        let stopped = v.state() == PlaybackState::Stopped;
        let position = if stopped { self.limit } else { v.position() };
        let ended = stopped || position >= self.limit;
        self.update_position(position.min(self.limit));
        if ended {
            self.stop_voice();
            self.advance(true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "audioTests.rs"]
pub(crate) mod tests;

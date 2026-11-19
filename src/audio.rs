use crate::library::Track;
use anyhow::{Context, Result, ensure};
use kira::{
    AudioManager, DefaultBackend, Tween,
    sound::{
        PlaybackState,
        streaming::{Decoder as StreamDecoder, StreamingSoundData, StreamingSoundHandle},
    },
};

use std::{
    io,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

pub use crate::recording::Recording;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub track: Option<Arc<Track>>,
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub previous: bool,
    pub next: bool,
    pub error: Option<String>,
}
pub enum Command {
    Play(Arc<[Track]>, usize),
    Toggle,
    Stop,
    Seek(f64),
    Step(i8),
    Volume(f32),
}
pub struct Transport {
    tx: mpsc::Sender<Command>,
    latest: Arc<Mutex<Status>>,
    pub changed: async_channel::Receiver<()>,
}
impl Transport {
    pub fn new(volume: f32) -> Self {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(Status::default()));
        let shared = latest.clone();
        let (wake, changed) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let mut engine = Engine {
                volume,
                ..Default::default()
            };
            loop {
                let command = if engine.status.playing {
                    match rx.recv_timeout(Duration::from_millis(40)) {
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
                    Some(c) => engine.command(c),
                    None => engine.tick(),
                };
                if let Err(error) = result {
                    engine.stop_voice();
                    engine.status.error = Some(format!("{error:#}"));
                }
                let mut previous = shared.lock().unwrap();
                if *previous != engine.status {
                    previous.clone_from(&engine.status);
                    let _ = wake.try_send(());
                }
            }
        });
        Self {
            tx,
            latest,
            changed,
        }
    }
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
    pub fn status(&self) -> Status {
        self.latest.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Engine {
    device: Option<AudioManager<DefaultBackend>>,
    voice: Option<StreamingSoundHandle<io::Error>>,
    queue: Arc<[Track]>,
    index: usize,
    group_start: usize,
    group_end: usize,
    length: f64,
    volume: f32,
    status: Status,
}
fn db(volume: f32) -> f32 {
    if volume <= 0. {
        -60.
    } else {
        (20. * volume.log10()).max(-60.)
    }
}
impl Engine {
    fn stop_voice(&mut self) {
        if let Some(mut v) = self.voice.take() {
            v.stop(Tween {
                duration: Duration::ZERO,
                ..Default::default()
            });
        }
        self.status.playing = false;
    }
    fn play(&mut self, index: usize, position: f64) -> Result<()> {
        self.stop_voice();
        let track = self.queue.get(index).context("Track is unavailable")?;
        self.index = index;
        self.group_start = index;
        self.group_end = index;
        self.status = Status {
            track: Some(Arc::new(track.clone())),
            previous: index > 0,
            next: index + 1 < self.queue.len(),
            ..Default::default()
        };
        // One uninterrupted decoder carries consecutive chapters of a disc image.
        while let Some((a, b)) = self
            .queue
            .get(self.group_end)
            .zip(self.queue.get(self.group_end + 1))
        {
            if a.source != b.source
                || !a
                    .disc
                    .as_ref()
                    .zip(b.disc.as_ref())
                    .is_some_and(|(a, b)| a.sheet == b.sheet && a.end == Some(b.start))
            {
                break;
            }
            self.group_end += 1;
        }
        let recording = Recording::open(
            track,
            self.queue[self.group_end].disc.as_ref().and_then(|d| d.end),
        )
        .with_context(|| format!("Cannot play {}", track.title))?;
        self.length = recording.num_frames() as f64 / f64::from(recording.sample_rate());
        ensure!(
            self.offset(self.group_end) < self.length,
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
        self.status.playing = true;
        self.update_position(position);
        Ok(())
    }
    fn offset(&self, index: usize) -> f64 {
        let start = |i: usize| self.queue[i].disc.as_ref().map_or(0, |d| d.start);
        (start(index) - start(self.group_start)) as f64 / 75.
    }
    fn update_position(&mut self, p: f64) {
        let index = (self.group_start..=self.group_end)
            .rev()
            .find(|&i| self.offset(i) <= p)
            .unwrap_or(self.group_start);
        if self.index != index {
            self.index = index;
            self.status.track = Some(Arc::new(self.queue[index].clone()));
        }
        let start = self.offset(index);
        self.status.duration = if index < self.group_end {
            self.offset(index + 1)
        } else {
            self.length
        } - start;
        self.status.position = (p - start).clamp(0., self.status.duration);
        self.status.previous = index > 0 || self.status.position > 0.;
        self.status.next = index + 1 < self.queue.len();
    }
    fn command(&mut self, c: Command) -> Result<()> {
        match c {
            Command::Play(q, i) => {
                self.queue = q;
                self.play(i, 0.)?;
            }
            Command::Volume(v) => {
                self.volume = v.clamp(0., 1.);
                if let Some(h) = &mut self.voice {
                    h.set_volume(db(self.volume), Tween::default());
                }
            }
            Command::Toggle if self.voice.is_none() && !self.queue.is_empty() => {
                let position = if self.status.position < self.status.duration {
                    self.status.position
                } else {
                    0.
                };
                self.play(self.index, position)?
            }
            Command::Toggle => {
                if let Some(v) = &mut self.voice {
                    if self.status.playing {
                        v.pause(Tween::default());
                    } else {
                        v.resume(Tween::default());
                    }
                    self.status.playing = !self.status.playing;
                }
            }
            Command::Stop => {
                self.stop_voice();
                self.status.position = 0.;
            }
            Command::Step(delta) if !self.queue.is_empty() => {
                let i = if delta < 0 && self.status.position > 3. {
                    self.index
                } else {
                    self.index
                        .saturating_add_signed(delta as isize)
                        .min(self.queue.len() - 1)
                };
                self.play(i, 0.)?;
            }
            Command::Seek(fraction) if self.status.duration > 0. => {
                let position = fraction.clamp(0., 1.) * (self.status.duration - 0.001).max(0.);
                if self.status.playing {
                    self.play(self.index, position)?;
                } else {
                    self.stop_voice();
                    self.status.position = position;
                    self.status.previous = self.index > 0 || position > 0.;
                }
            }
            _ => {}
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
        let ended = v.state() == PlaybackState::Stopped;
        let position = if ended { self.length } else { v.position() };
        self.update_position(position);
        if ended {
            self.stop_voice();
            if self.group_end + 1 < self.queue.len() {
                self.play(self.group_end + 1, 0.)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "audioTests.rs"]
mod tests;

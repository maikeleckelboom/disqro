//! Transport-owned listening sessions, with persistence on a separate worker.
use crate::{cache::Stamp, library::Track, store};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub const MINUTE_US: i64 = 60_000_000;
const CHECKPOINT: Duration = Duration::from_secs(4);

pub fn timestamp() -> i64 {
    // Once anchored, the transport timeline cannot jump with wall-clock edits.
    static ANCHOR: std::sync::OnceLock<(Instant, i64)> = std::sync::OnceLock::new();
    let (clock, utc) =
        ANCHOR.get_or_init(|| (Instant::now(), chrono::Utc::now().timestamp_micros()));
    utc + clock.elapsed().as_micros() as i64
}

pub fn qualification(duration: Option<f64>) -> i64 {
    duration
        .filter(|seconds| seconds.is_finite() && *seconds > 0.)
        .map_or(30_000_000, |seconds| {
            (seconds * 500_000.).ceil().clamp(1., 30_000_000.) as i64
        })
}

#[derive(Clone, Copy)]
pub enum End {
    Completed,
    Replaced,
    Stopped,
    SourceFailed,
    OutputFailed,
    Shutdown,
}
impl End {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Replaced => "replaced",
            Self::Stopped => "stopped",
            Self::SourceFailed => "source-failed",
            Self::OutputFailed => "output-failed",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone)]
pub struct Recording {
    pub track: Arc<Track>,
    pub source: Option<Stamp>,
    pub sheet: Option<Stamp>,
}

#[derive(Clone)]
pub struct Checkpoint {
    pub token: String,
    pub recording: Recording,
    pub started_at: i64,
    pub activity_at: i64,
    pub qualified_at: Option<i64>,
    pub end: Option<(i64, End)>,
    pub credited_us: i64,
    pub sequence: i64,
    pub minutes: BTreeMap<i64, i64>,
}

pub struct Occurrence {
    state: Checkpoint,
    last_checkpoint: i64,
}
impl Occurrence {
    pub fn new(recording: Recording, started_at: i64) -> Self {
        Self {
            state: Checkpoint {
                token: format!("{:032x}", fastrand::u128(..)),
                recording,
                started_at,
                activity_at: started_at,
                qualified_at: None,
                end: None,
                credited_us: 0,
                sequence: 0,
                minutes: BTreeMap::new(),
            },
            last_checkpoint: started_at,
        }
    }

    pub fn credit(&mut self, duration: Option<f64>, from: i64, amount: i64) -> Option<Checkpoint> {
        debug_assert!(amount > 0);
        let from = from.max(self.state.activity_at);
        let end = from + amount;
        let old = self.state.credited_us;
        self.state.credited_us += amount;
        self.state.activity_at = end;
        let threshold = qualification(duration);
        let qualifies = self.state.qualified_at.is_none() && self.state.credited_us >= threshold;
        if qualifies {
            self.state.qualified_at = Some(from + (threshold - old).max(0));
        }
        let mut cursor = from;
        while cursor < end {
            let minute = cursor.div_euclid(MINUTE_US) * MINUTE_US;
            let next = end.min(minute + MINUTE_US);
            *self.state.minutes.entry(minute).or_default() += next - cursor;
            cursor = next;
        }
        if qualifies
            || self.state.sequence == 0
            || end - self.last_checkpoint >= CHECKPOINT.as_micros() as i64
        {
            self.checkpoint()
        } else {
            None
        }
    }

    pub fn checkpoint(&mut self) -> Option<Checkpoint> {
        if self.state.minutes.is_empty() && self.state.end.is_none() {
            return None;
        }
        self.state.sequence += 1;
        self.last_checkpoint = self.state.activity_at;
        let minutes = std::mem::take(&mut self.state.minutes);
        let mut snapshot = self.state.clone();
        snapshot.minutes = minutes;
        Some(snapshot)
    }

    pub fn finish(mut self, at: i64, reason: End) -> Checkpoint {
        self.state.end = Some((at.max(self.state.activity_at), reason));
        self.checkpoint().expect("a finished occurrence")
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Totals {
    pub plays: i64,
    pub last_played: Option<i64>,
    pub credited_us: i64,
}

pub fn duration(us: i64) -> String {
    let seconds = us.max(0) / 1_000_000;
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, 0) if us > 0 => "<1 s".into(),
        (0, 0, s) => format!("{s} s"),
        (0, m, s) => format!("{m}:{s:02}"),
        (h, m, s) => format!("{h}:{m:02}:{s:02}"),
    }
}

pub fn local_date(at: i64, format: &str) -> String {
    chrono::DateTime::from_timestamp_micros(at)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format(format)
                .to_string()
        })
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Period {
    Week,
    #[default]
    Month,
    All,
}
impl Period {
    pub const OPTIONS: [Self; 3] = [Self::Week, Self::Month, Self::All];
    pub fn label(self) -> &'static str {
        match self {
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
            Self::All => "All time",
        }
    }
    pub fn start(self, today: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
        match self {
            Self::Week => today.checked_sub_days(chrono::Days::new(6)),
            Self::Month => today.checked_sub_days(chrono::Days::new(29)),
            Self::All => None,
        }
    }
}

/// Calendar boundaries are resolved individually (the day need not be 24 h).
pub fn midnight(zone: &impl chrono::TimeZone, day: chrono::NaiveDate) -> i64 {
    let start = day.and_hms_opt(0, 0, 0).unwrap();
    // A local day may begin during a clock jump, or be skipped completely.
    (0..=1440)
        .find_map(|minute| {
            zone.from_local_datetime(&(start + chrono::TimeDelta::minutes(minute)))
                .earliest()
        })
        .expect("a calendar boundary within one day")
        .timestamp_micros()
}

enum Job {
    Save(Box<Checkpoint>),
    Erase(Option<i64>, async_channel::Sender<Result<()>>),
    Barrier(mpsc::SyncSender<Result<()>>),
}

#[derive(Clone)]
pub struct Writer(mpsc::Sender<Job>);

#[derive(Clone, Copy)]
pub struct Change {
    pub track: Option<store::TrackId>,
    pub order: bool,
    pub removed: bool,
}
pub struct Updates {
    pub changed: async_channel::Receiver<()>,
    pub pending: Arc<std::sync::Mutex<Vec<Change>>>,
    pub error: Arc<std::sync::Mutex<Option<String>>>,
}

impl Writer {
    pub fn spawn(path: PathBuf) -> (Self, Updates) {
        let (send, receive) = mpsc::channel();
        let (notify, changed) = async_channel::bounded(1);
        let pending = Arc::new(std::sync::Mutex::new(Vec::<Change>::new()));
        let error = Arc::new(std::sync::Mutex::new(None));
        let changes = pending.clone();
        let failure = error.clone();
        std::thread::Builder::new()
            .name("disqro-listening".into())
            .spawn(move || {
                let opening = || -> Result<_> {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let owner = std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(false)
                        .write(true)
                        .open(path.with_extension("listening.lock"))?;
                    owner
                        .try_lock()
                        .context("Listening history is open in another instance")?;
                    let mut db = store::Store::open(&path)?;
                    db.recover_listening()?;
                    Ok((owner, db))
                };
                let mut database = opening().map_err(|e| format!("{e:#}"));
                if let Err(message) = &database {
                    *failure.lock().unwrap() = Some(message.clone());
                    let _ = notify.try_send(());
                }
                let publish = |change: Change| {
                    let mut changes = changes.lock().unwrap();
                    if let Some(existing) = changes.iter_mut().find(|c| c.track == change.track) {
                        existing.order |= change.order;
                        existing.removed |= change.removed;
                    } else {
                        changes.push(change);
                    }
                    let _ = notify.try_send(());
                };
                let mut backlog = VecDeque::new();
                let mut failed = None::<String>;
                loop {
                    let job = match if backlog.is_empty() {
                        receive
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    } else {
                        receive.recv_timeout(Duration::from_secs(1))
                    } {
                        Ok(job) => Some(job),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let control = match job {
                        Some(Job::Save(snapshot)) => {
                            backlog.push_back(snapshot);
                            None
                        }
                        other => other,
                    };
                    if database.is_err() {
                        database = opening().map_err(|e| format!("{e:#}"));
                    }
                    let result = (|| -> Result<()> {
                        let (_, db) = database.as_mut().map_err(|e| anyhow::anyhow!("{e}"))?;
                        while let Some(snapshot) = backlog.front() {
                            if let Some(change) = db.checkpoint_listening(snapshot)? {
                                publish(change);
                            }
                            backlog.pop_front();
                        }
                        Ok(())
                    })();
                    if let Err(e) = &result {
                        let message = format!("{e:#}");
                        if failed.as_ref() != Some(&message) {
                            *failure.lock().unwrap() = Some(message.clone());
                            let _ = notify.try_send(());
                        }
                        failed = Some(message);
                    } else {
                        failed = None;
                    }
                    match control {
                        Some(Job::Erase(id, reply)) => {
                            let result = result.and_then(|()| {
                                let (_, db) = database.as_mut().expect("verified writer");
                                publish(db.erase_listening(id)?);
                                Ok(())
                            });
                            let _ = reply.try_send(result);
                        }
                        Some(Job::Barrier(reply)) => {
                            let _ = reply.send(result);
                        }
                        _ => {}
                    }
                }
            })
            .expect("start listening history writer");
        (
            Self(send),
            Updates {
                changed,
                pending,
                error,
            },
        )
    }
    pub fn save(&self, snapshot: Checkpoint) {
        let _ = self.0.send(Job::Save(Box::new(snapshot)));
    }
    pub fn erase(&self, id: Option<i64>, reply: async_channel::Sender<Result<()>>) {
        if self.0.send(Job::Erase(id, reply.clone())).is_err() {
            let _ = reply.try_send(Err(anyhow::anyhow!("Listening history is unavailable.")));
        }
    }
    pub fn barrier(&self) -> Result<()> {
        let (send, receive) = mpsc::sync_channel(1);
        self.0
            .send(Job::Barrier(send))
            .context("Listening history is unavailable")?;
        receive
            .recv_timeout(Duration::from_secs(15))
            .context("Listening history did not finish saving")?
    }
}

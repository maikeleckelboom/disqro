//! One cancellable worker retains at most one open analysis decoder. The UI
//! receives immutable, resolution-limited projections, never full PCM pages.
use crate::{
    cache::Cache,
    library::{DiscContext, Track},
    overview::Bin,
    timeline::{Program, Segment},
    waveform::{self, Measure, PAGE, Reader},
    waveform_detail::{Store, View},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
        mpsc,
    },
    time::{Duration, Instant},
};
const WIDTH: usize = 1536;
#[derive(Clone, Default)]
pub struct Shape {
    pub track: Arc<[Bin]>,
    pub disc: Arc<[Bin]>,
}
pub enum Update {
    Program(Arc<Program>),
    Wave(usize, Shape),
    Failed(String),
    Changed,
    Detail,
}
struct Request {
    id: u64,
    track: Arc<Track>,
    context: DiscContext,
    elapsed: f64,
}
pub struct Analysis {
    next: Arc<Mutex<Option<Request>>>,
    wake: mpsc::SyncSender<()>,
    revision: Arc<AtomicU64>,
    focus: Arc<AtomicUsize>,
    detail: Arc<Mutex<Store>>,
    next_view: AtomicU64,
}
impl Analysis {
    pub fn new() -> (Self, async_channel::Receiver<(u64, Update)>) {
        let (wake, rx) = mpsc::sync_channel(1);
        let (tx, updates) = async_channel::bounded(4);
        let next = Arc::new(Mutex::new(None::<Request>));
        let revision = Arc::new(AtomicU64::new(0));
        let focus = Arc::new(AtomicUsize::new(0));
        let detail = Arc::new(Mutex::new(Store::default()));
        let (pending, version, active) = (next.clone(), revision.clone(), focus.clone());
        let tiles = detail.clone();
        std::thread::Builder::new()
            .name("disqro-waveform".into())
            .spawn(move || {
                let mut cache = None;
                let mut reader = None;
                let mut watching: Option<(u64, Arc<Program>)> = None;
                loop {
                    match rx.recv_timeout(Duration::from_secs(2)) {
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if let Some((id, p)) = &watching
                                && !p.unchanged()
                            {
                                let _ = tx.send_blocking((*id, Update::Changed));
                                watching = None;
                                reader = None;
                            }
                            continue;
                        }
                        Ok(()) => {}
                    }
                    let job = pending.lock().unwrap().take();
                    let Some(job) = job else {
                        if watching
                            .as_ref()
                            .is_some_and(|(id, _)| version.load(Relaxed) != *id)
                        {
                            watching = None;
                            reader = None;
                        }
                        if let Some((id, _)) = &watching {
                            let current = || version.load(Relaxed) == *id;
                            let mut session = DecoderSession {
                                cache: cache.as_ref(),
                                reader: &mut reader,
                            };
                            while load_detail(*id, &tiles, &mut session, &current, &tx) {}
                        }
                        continue;
                    };
                    watching = None;
                    reader = None;
                    let current = || version.load(Relaxed) == job.id;
                    if cache.is_none() {
                        cache = crate::profile::location()
                            .and_then(|p| Cache::open(&p.with_file_name("Cache")).ok());
                    }
                    let send = |u| current() && tx.send_blocking((job.id, u)).is_ok();
                    match Program::resolve(&job.track, job.context, cache.as_ref()) {
                        Ok(program) => {
                            let p = Arc::new(program);
                            let start = p.index(&job.track).unwrap_or(0);
                            active.store(start, Relaxed);
                            if send(Update::Program(p.clone())) {
                                analyse(
                                    &p,
                                    (start, job.elapsed),
                                    &active,
                                    DecoderSession {
                                        cache: cache.as_ref(),
                                        reader: &mut reader,
                                    },
                                    current,
                                    &send,
                                    &|session| {
                                        load_detail(job.id, &tiles, session, &current, &tx);
                                    },
                                );
                                watching = Some((job.id, p));
                            }
                        }
                        Err(e) => {
                            send(Update::Failed(e.to_string()));
                        }
                    }
                }
            })
            .expect("waveform worker");
        (
            Self {
                next,
                wake,
                revision,
                focus,
                detail,
                next_view: AtomicU64::new(0),
            },
            updates,
        )
    }
    pub fn request(&self, track: Arc<Track>, context: DiscContext, elapsed: f64) -> u64 {
        let id = self.revision.fetch_add(1, Relaxed) + 1;
        self.detail.lock().unwrap().reset(id);
        *self.next.lock().unwrap() = Some(Request {
            id,
            track,
            context,
            elapsed,
        });
        let _ = self.wake.try_send(());
        id
    }
    pub fn focus(&self, index: usize) {
        self.focus.store(index, Relaxed);
    }
    pub fn cancel(&self) {
        let id = self.revision.fetch_add(1, Relaxed) + 1;
        self.detail.lock().unwrap().reset(id);
        *self.next.lock().unwrap() = None;
        let _ = self.wake.try_send(());
    }

    pub fn view(&self) -> View {
        View::new(
            self.next_view.fetch_add(1, Relaxed),
            self.detail.clone(),
            self.wake.clone(),
        )
    }
}

struct DecoderSession<'a> {
    cache: Option<&'a Cache>,
    reader: &'a mut Option<Reader>,
}

fn load_detail(
    revision: u64,
    detail: &Mutex<Store>,
    session: &mut DecoderSession<'_>,
    current: &impl Fn() -> bool,
    updates: &async_channel::Sender<(u64, Update)>,
) -> bool {
    if !current() {
        return false;
    }
    let Some(key) = detail.lock().unwrap().next(revision) else {
        return false;
    };
    let tile = key.load(session.cache, session.reader, current).ok();
    if detail.lock().unwrap().publish(revision, key, tile) {
        let _ = updates.try_send((revision, Update::Detail));
    }
    true
}
impl Drop for Analysis {
    fn drop(&mut self) {
        self.cancel();
    }
}
struct Column {
    m: Measure,
    frames: usize,
}
struct Projection {
    stride: usize,
    columns: Vec<Column>,
}
impl Projection {
    fn new(cells: usize, width: usize) -> Self {
        let stride = cells.div_ceil(width.max(1));
        Self {
            stride,
            columns: (0..cells.div_ceil(stride))
                .map(|_| Column {
                    m: Measure::default(),
                    frames: 0,
                })
                .collect(),
        }
    }
    fn add(&mut self, cell: usize, m: &Measure, frames: usize) {
        let c = &mut self.columns[cell / self.stride];
        c.m.merge(m);
        c.frames += frames;
    }
    fn bins(&self, seg: &Segment, first: usize) -> Arc<[Bin]> {
        let frame =
            |i| waveform::edge(first + i * self.stride, seg.source.rate).min(seg.frames.end);
        self.columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let complete = c.frames > 0 && c.frames == frame(i + 1) - frame(i);
                let [(low, low_at), (high, high_at)] = c.m.extrema();
                Bin {
                    end_frame: frame(i + 1) - seg.frames.start,
                    low,
                    high,
                    low_at: low_at.saturating_sub(seg.frames.start),
                    high_at: high_at.saturating_sub(seg.frames.start),
                    mean_square: if complete {
                        (c.m.energy / c.frames as f64) as f32
                    } else {
                        f32::NAN
                    },
                    band_rms: if complete {
                        c.m.bands
                            .map(|power| (power / c.frames as f64).sqrt() as f32)
                    } else {
                        [0.; 3]
                    },
                }
            })
            .collect()
    }
}
fn analyse(
    p: &Program,
    (start, elapsed): (usize, f64),
    focus: &AtomicUsize,
    mut session: DecoderSession<'_>,
    current: impl Fn() -> bool,
    send: &impl Fn(Update) -> bool,
    before_page: &impl Fn(&mut DecoderSession<'_>),
) {
    let mut done = vec![false; p.segments.len()];
    while current() {
        let active = focus.load(Relaxed).min(done.len() - 1);
        let Some(i) = (active..done.len()).chain(0..active).find(|&i| !done[i]) else {
            break;
        };
        let seg = &p.segments[i];
        done[i] = true;
        if !seg.source.available {
            continue;
        }
        let rate = seg.source.rate;
        let first = waveform::ceil_cell(seg.frames.start, rate);
        let end = waveform::ceil_cell(seg.frames.end, rate);
        let mut track = Projection::new(end - first, WIDTH);
        let width = ((u128::from(seg.time.end - seg.time.start) * WIDTH as u128)
            .div_ceil(u128::from(p.duration))) as usize;
        let mut disc = Projection::new(end - first, width);
        let min = first / PAGE;
        let max = (end - 1) / PAGE;
        let hot = if i == start {
            (waveform::ceil_cell(seg.elapsed_frame(elapsed), rate) / PAGE).clamp(min, max)
        } else {
            min
        };
        let mut published = None;
        for page in (hot..=max).chain(min..hot) {
            if !current() {
                return;
            }
            before_page(&mut session);
            let data =
                match waveform::load(&seg.source, page, session.cache, session.reader, &current) {
                    Ok(data) => data,
                    Err(e) => {
                        if !current() {
                            return;
                        }
                        if !p.unchanged() {
                            send(Update::Changed);
                            return;
                        }
                        send(Update::Failed(format!("Waveform unavailable · {e}")));
                        break;
                    }
                };
            for (j, m) in data.iter().enumerate() {
                let cell = page * PAGE + j;
                if (first..end).contains(&cell) {
                    let n = waveform::edge(cell + 1, rate).min(seg.frames.end)
                        - waveform::edge(cell, rate);
                    track.add(cell - first, m, n);
                    disc.add(cell - first, m, n);
                }
            }
            let last = if hot == min { max } else { hot - 1 };
            if page == last
                || published.is_none_or(|t: Instant| t.elapsed() >= Duration::from_millis(70))
            {
                let track = track.bins(seg, first);
                if page != last
                    && published.is_none()
                    && track.iter().all(|bin| !bin.mean_square.is_finite())
                {
                    continue;
                }
                if !send(Update::Wave(
                    i,
                    Shape {
                        track,
                        disc: disc.bins(seg, first),
                    },
                )) {
                    return;
                }
                published = Some(Instant::now());
            }
        }
    }
}

#[cfg(test)]
#[path = "waveformBaselineTests.rs"]
mod baseline_tests;

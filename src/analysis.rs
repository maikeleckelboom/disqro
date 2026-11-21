//! One cancellable worker retains at most one open analysis decoder. The UI
//! receives immutable, resolution-limited projections, never full PCM pages.
use crate::{
    cache::Cache,
    library::{DiscContext, Track},
    overview::Bin,
    timeline::{Program, Segment},
    waveform::{self, Measure, PAGE},
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
}
impl Analysis {
    pub fn new() -> (Self, async_channel::Receiver<(u64, Update)>) {
        let (wake, rx) = mpsc::sync_channel(1);
        let (tx, updates) = async_channel::bounded(4);
        let next = Arc::new(Mutex::new(None::<Request>));
        let revision = Arc::new(AtomicU64::new(0));
        let focus = Arc::new(AtomicUsize::new(0));
        let (pending, version, active) = (next.clone(), revision.clone(), focus.clone());
        std::thread::Builder::new()
            .name("disqro-waveform".into())
            .spawn(move || {
                let mut cache = None;
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
                            }
                            continue;
                        }
                        Ok(()) => {}
                    }
                    watching = None;
                    let Some(job) = pending.lock().unwrap().take() else {
                        continue;
                    };
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
                                    start,
                                    job.elapsed,
                                    &active,
                                    cache.as_ref(),
                                    current,
                                    &send,
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
            },
            updates,
        )
    }
    pub fn request(&self, track: Arc<Track>, context: DiscContext, elapsed: f64) -> u64 {
        let id = self.revision.fetch_add(1, Relaxed) + 1;
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
        self.revision.fetch_add(1, Relaxed);
        *self.next.lock().unwrap() = None;
        let _ = self.wake.try_send(());
    }
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
                Bin {
                    end_frame: frame(i + 1) - seg.frames.start,
                    low: c.m.lo[0].min(c.m.lo[1]),
                    high: c.m.hi[0].max(c.m.hi[1]),
                    mean_square: if complete {
                        (c.m.energy / c.frames as f64) as f32
                    } else {
                        f32::NAN
                    },
                }
            })
            .collect()
    }
}
fn analyse(
    p: &Program,
    start: usize,
    elapsed: f64,
    focus: &AtomicUsize,
    cache: Option<&Cache>,
    current: impl Fn() -> bool,
    send: &impl Fn(Update) -> bool,
) {
    let mut done = vec![false; p.segments.len()];
    let mut reader = None;
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
            let data = match waveform::load(&seg.source, page, cache, &mut reader, &current) {
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
                if !send(Update::Wave(
                    i,
                    Shape {
                        track: track.bins(seg, first),
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

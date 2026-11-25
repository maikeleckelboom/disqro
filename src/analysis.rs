//! One cancellable worker retains at most one open analysis decoder. The UI
//! receives immutable, resolution-limited projections, never full PCM pages.
use crate::{
    cache::Cache,
    library::{DiscContext, Track},
    timeline::Program,
    waveform::{self, Evidence, Reader},
    waveform_detail::{Store, View},
    waveform_port_analysis::{EVIDENCE_FRAMES, OVERVIEW_COLUMNS, Plan},
    waveform_render::Page,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
        mpsc,
    },
    time::{Duration, Instant},
};
#[derive(Clone, Default)]
pub struct Shape {
    pub track: Arc<Page>,
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
                                    start,
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
    pub fn request(&self, track: Arc<Track>, context: DiscContext) -> u64 {
        let id = self.revision.fetch_add(1, Relaxed) + 1;
        self.detail.lock().unwrap().reset(id);
        *self.next.lock().unwrap() = Some(Request { id, track, context });
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
fn analyse(
    program: &Program,
    start: usize,
    focus: &AtomicUsize,
    mut session: DecoderSession<'_>,
    current: impl Fn() -> bool,
    send: &impl Fn(Update) -> bool,
    before_chunk: &impl Fn(&mut DecoderSession<'_>),
) {
    let mut held = vec![None::<Arc<Page>>; program.segments.len()];
    for preview in [true, false] {
        let mut done = vec![false; held.len()];
        let mut active = if preview {
            start
        } else {
            focus.load(Relaxed).min(held.len().saturating_sub(1))
        };
        while current() {
            let Some(i) = (active..done.len()).chain(0..active).find(|&i| !done[i]) else {
                break;
            };
            active = focus.load(Relaxed).min(done.len() - 1);
            done[i] = true;
            let segment = &program.segments[i];
            if !segment.source.available || held[i].as_ref().is_some_and(|page| !page.preview) {
                continue;
            }
            let plan = Plan::overview(segment.source.rate, segment.frames.len(), OVERVIEW_COLUMNS);
            let exact = Evidence {
                playable: segment.frames.clone(),
                plan,
            };
            let publish = |page| send(Update::Wave(i, Shape { track: page }));
            before_chunk(&mut session);
            if preview && let Some(page) = waveform::cached(&segment.source, &exact, session.cache)
            {
                let page = Arc::new(page);
                held[i] = Some(page.clone());
                if !publish(page) {
                    return;
                }
                continue;
            }
            let request = if preview {
                Evidence {
                    plan: plan.preview(),
                    ..exact
                }
            } else {
                exact
            };
            let result = measure_overview(
                &segment.source,
                &request,
                held[i].clone(),
                &mut session,
                &current,
                before_chunk,
                &publish,
            );
            match result {
                Ok(Some(page)) => held[i] = Some(page),
                Ok(None) => return,
                Err(error) => {
                    if !current() {
                        return;
                    }
                    if !program.unchanged() {
                        send(Update::Changed);
                        return;
                    }
                    send(Update::Failed(format!("Waveform unavailable · {error}")));
                }
            }
        }
    }
}

fn measure_overview(
    source: &crate::timeline::Source,
    request: &Evidence,
    geometry: Option<Arc<Page>>,
    session: &mut DecoderSession<'_>,
    current: &impl Fn() -> bool,
    before_chunk: &impl Fn(&mut DecoderSession<'_>),
    publish: &impl Fn(Arc<Page>) -> bool,
) -> anyhow::Result<Option<Arc<Page>>> {
    let mut analyzer = request.begin(source.rate)?;
    if let Some(geometry) = geometry {
        analyzer.reuse_geometry(geometry)?;
    }
    let range = analyzer.decode_range();
    let origin = request.playable.start;
    let mut published = None;
    let mut first = range.start;
    while first < range.end {
        if !current() {
            return Ok(None);
        }
        before_chunk(session);
        let count = if request.plan.preview && published.is_none() {
            (request.plan.frames_per_column * 4).clamp(4096, EVIDENCE_FRAMES)
        } else {
            EVIDENCE_FRAMES
        };
        let end = (first + count).min(range.end);
        waveform::feed(
            source,
            origin + first..origin + end,
            &mut analyzer,
            session.reader,
            current,
        )?;
        first = end;
        if request.plan.preview
            && end < range.end
            && published.is_none_or(|t: Instant| t.elapsed() >= Duration::from_millis(70))
            && let Some(page) = analyzer.overview_prefix()
        {
            if !publish(Arc::new(page)) {
                return Ok(None);
            }
            published = Some(Instant::now());
        }
    }
    let page = Arc::new(analyzer.finish()?);
    if !request.plan.preview {
        waveform::store(source, request, session.cache, &page);
    }
    Ok(publish(page.clone()).then_some(page))
}

#[cfg(test)]
#[path = "waveformBaselineTests.rs"]
mod baseline_tests;

//! A scan owns its directory stack and publishes bounded batches. Replacing its
//! receiver cancels the walk, including a producer waiting for a slow UI.
use crate::{
    cache::{Cache, Stamp},
    library::{self, Track},
};
use async_channel::{Receiver, Sender};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "discoveryTests.rs"]
mod tests;

pub struct Root {
    pub dirs: Vec<PathBuf>,
    pub siblings: Option<Vec<PathBuf>>,
}
#[derive(Default)]
pub struct Batch {
    pub root: Option<Root>,
    pub rows: Vec<(usize, Option<Arc<Track>>)>,
    pub covers: Vec<(PathBuf, crate::artwork::CoverFile)>,
    pub equivalent_sheets: Vec<(PathBuf, PathBuf)>,
    pub notice: Option<String>,
    /// Folders that could not be listed completely. Their contents are
    /// unknown rather than absent, so the indexer never marks them missing.
    pub incomplete: Vec<PathBuf>,
    /// Sheets that exist but could not be interpreted on this visit.
    pub failed_sheets: Vec<PathBuf>,
}
struct Request {
    path: PathBuf,
    recursive: bool,
    output: Sender<Batch>,
}
pub struct Scanner(Sender<Request>);

impl Scanner {
    pub fn new() -> Self {
        let (tx, rx) = async_channel::bounded::<Request>(1);
        // A blocked network filesystem call cannot be interrupted on Windows.
        // A second worker lets a newer local request proceed in that case.
        for i in 0..2 {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("disqro-discovery-{i}"))
                .spawn(move || {
                    while let Ok(job) = rx.recv_blocking() {
                        let cache = crate::profile::location()
                            .and_then(|p| Cache::open(&p.with_file_name("Cache")).ok());
                        scan(&job.path, job.recursive, cache.as_ref(), &job.output);
                    }
                })
                .expect("start discovery worker");
        }
        Self(tx)
    }
    pub fn open(&self, path: PathBuf, recursive: bool) -> Receiver<Batch> {
        let (output, rx) = async_channel::bounded(3);
        let _ = self.0.force_send(Request {
            path,
            recursive,
            output,
        });
        rx
    }
}

#[derive(Hash, Eq, PartialEq)]
struct Segment {
    source: Arc<Path>,
    number: u8,
    start: u64,
    end: Option<u64>,
    title: String,
    album: String,
    artist: String,
}
/// Deduplicate whole programs. A different source, sequence or authored
/// program remains a separate edition, even if some chapters are identical.
#[derive(Default)]
pub(crate) struct Sources {
    programs: HashMap<Vec<Segment>, PathBuf>,
    covered: HashSet<Arc<Path>>,
    files: HashMap<Arc<Path>, usize>,
    enrich: Vec<(usize, Arc<Track>)>,
    next: usize,
}
impl Sources {
    fn add(&mut self, track: Track, batch: &mut Batch) {
        let track = Arc::new(track);
        if track.disc.as_ref().is_none_or(|d| d.end.is_none()) {
            self.enrich.push((self.next, track.clone()));
        }
        batch.rows.push((self.next, Some(track)));
        self.next += 1;
    }
    fn folder(&mut self, tracks: Vec<Track>, batch: &mut Batch) {
        let mut paths = HashMap::<PathBuf, Arc<Path>>::new();
        let mut key = |p: &Path| {
            paths
                .entry(p.to_owned())
                .or_insert_with(|| p.canonicalize().unwrap_or_else(|_| p.to_owned()).into())
                .clone()
        };
        let mut iter = tracks.into_iter().peekable();
        while let Some(first) = iter.next() {
            let Some(disc) = &first.disc else {
                let path = key(&first.source);
                if !self.covered.contains(&path) && !self.files.contains_key(&path) {
                    self.files.insert(path, self.next);
                    self.add(first, batch);
                }
                continue;
            };
            let sheet = disc.sheet.clone();
            let mut program = vec![first];
            while iter
                .peek()
                .is_some_and(|t| t.disc.as_ref().is_some_and(|d| d.sheet == sheet))
            {
                program.push(iter.next().unwrap());
            }
            let segments = program
                .iter()
                .map(|track| {
                    let d = track.disc.as_ref().unwrap();
                    Segment {
                        source: key(&track.source),
                        number: d.number,
                        start: d.start,
                        end: d.end,
                        title: track.title.clone(),
                        album: track.album.clone(),
                        artist: track.artist.clone(),
                    }
                })
                .collect();
            match self.programs.entry(segments) {
                std::collections::hash_map::Entry::Occupied(existing) => {
                    batch
                        .equivalent_sheets
                        .push((sheet, existing.get().clone()));
                    continue;
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(sheet);
                }
            }
            for track in program {
                let path = key(&track.source);
                if self.covered.insert(path.clone())
                    && let Some(old) = self.files.remove(&path)
                {
                    batch.rows.push((old, None));
                }
                self.add(track, batch);
            }
        }
    }
}

/// Walks a folder, and its subfolders when `recursive`, publishing
/// deduplicated tracks in bounded batches. Returns the state that header
/// enrichment needs, or `None` once `publish` refuses a batch or `alive`
/// turns false. The listening browser and the library indexer share it, so
/// both agree on what counts as a track.
pub(crate) fn walk(
    root: &Path,
    recursive: bool,
    cache: Option<&Cache>,
    alive: &dyn Fn() -> bool,
    publish: &mut dyn FnMut(Batch) -> bool,
) -> Option<Sources> {
    let mut todo = vec![root.to_owned()];
    let mut sources = Sources::default();
    let mut batch = Batch::default();
    let mut first = true;
    let mut published = Instant::now();
    while let Some(path) = todo.pop() {
        if !alive() {
            return None;
        }
        match library::browse_while(&path, cache, alive) {
            Ok(folder) => {
                if path == root {
                    batch.root = Some(Root {
                        siblings: path.parent().map(library::subfolders),
                        dirs: folder.dirs.clone(),
                    });
                }
                if recursive {
                    todo.extend(folder.dirs.into_iter().rev());
                }
                if let Some(cover) = folder.cover.and_then(crate::artwork::CoverFile::new) {
                    batch.covers.push((path.clone(), cover));
                }
                if batch.notice.is_none() {
                    batch.notice = folder.notices.into_iter().next();
                }
                if !folder.complete {
                    batch.incomplete.push(path.clone());
                }
                batch.failed_sheets.extend(folder.failed_sheets);
                sources.folder(folder.tracks, &mut batch);
            }
            Err(error) => {
                batch.notice.get_or_insert_with(|| format!("{error:#}"));
                batch.incomplete.push(path.clone());
            }
        }
        if batch.root.is_some()
            || (first && !batch.rows.is_empty())
            || batch.rows.len() >= 2048
            || published.elapsed() >= Duration::from_millis(100)
        {
            first &= batch.rows.is_empty();
            if !publish(std::mem::take(&mut batch)) {
                return None;
            }
            published = Instant::now();
        }
    }
    if !publish(batch) {
        return None;
    }
    Some(sources)
}

fn scan(root: &Path, recursive: bool, cache: Option<&Cache>, out: &Sender<Batch>) {
    let alive = || !out.is_closed();
    let mut publish = |batch| out.send_blocking(batch).is_ok();
    let Some(sources) = walk(root, recursive, cache, &alive, &mut publish) else {
        return;
    };
    let mut batch = Batch::default();
    let mut published = Instant::now();
    // Source headers are read after all cheap discovery. Sibling chapters
    // whose duration is known from INDEX 01 never need a probe.
    let remaining_files: HashSet<_> = sources.files.into_values().collect();
    for (index, track) in sources.enrich {
        if out.is_closed() {
            return;
        }
        if track.disc.is_none() && !remaining_files.contains(&index) {
            continue;
        }
        let stamp = Stamp::read(&track.source);
        let cached = stamp.as_ref().and_then(|s| cache?.tags(&track.source, s));
        let tags = match cached {
            Some(t) => t,
            None => {
                let mut source = Track::file(track.source.clone());
                if !source.read_tags() {
                    continue;
                }
                if let (Some(c), Some(stamp)) = (cache, stamp.as_ref())
                    && Stamp::read(&source.source).as_ref() == Some(stamp)
                {
                    // A changed source will miss on the next visit.
                    let _ = c.store_tags(&source, stamp);
                }
                source
            }
        };
        let enriched = if let Some(d) = &track.disc {
            let mut t = (*track).clone();
            t.duration = tags
                .duration
                .map(|seconds| (seconds - d.start as f64 / 75.).max(0.));
            t
        } else {
            tags
        };
        batch.rows.push((index, Some(Arc::new(enriched))));
        if batch.rows.len() >= 2048 || published.elapsed() >= Duration::from_millis(100) {
            if out.send_blocking(std::mem::take(&mut batch)).is_err() {
                return;
            }
            published = Instant::now();
        }
    }
    if !batch.rows.is_empty() {
        let _ = out.send_blocking(batch);
    }
}

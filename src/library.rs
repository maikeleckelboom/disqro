use anyhow::{Context, Result};
use lofty::{config::ParseOptions, prelude::*, probe::Probe};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub type TrackList = std::sync::Arc<[std::sync::Arc<Track>]>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Identity in the currently open library. Saved queues can also be
    /// restored alongside a rebuilt database, so persist the locator only.
    #[serde(skip)]
    pub library_id: Option<crate::store::TrackId>,
    pub source: PathBuf,
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub duration: Option<f64>,
    /// The tagged track number of a file. A CUE chapter uses its sheet number.
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub disc: Option<DiscTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiscTrack {
    pub sheet: PathBuf,
    pub number: u8,
    pub start: u64,
    pub end: Option<u64>,
}

/// Only a deliberately selected CD can supply the context missing from a recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscContext {
    #[default]
    Inferred,
    Selected,
}
impl DiscContext {
    pub fn allows(self, track: &Track) -> bool {
        self == Self::Selected || track.has_disc_context()
    }
}

impl Track {
    pub fn file(path: PathBuf) -> Self {
        Self {
            library_id: None,
            title: name(&path),
            source: path,
            artist: String::new(),
            album: String::new(),
            duration: None,
            number: None,
            disc: None,
            year: None,
            disc_number: None,
        }
    }
    /// The number shown in a track list: the CUE track, or the tagged number.
    pub fn display_number(&self) -> Option<u32> {
        match &self.disc {
            Some(d) => (d.number > 0).then_some(u32::from(d.number)),
            None => self.number,
        }
    }
    pub fn has_disc_context(&self) -> bool {
        self.disc.is_some()
            || !self.album.trim().is_empty()
            || self.disc_number.is_some_and(|n| n > 0)
            || folder_disc_number(&self.source).is_some()
    }
    pub fn format(&self) -> String {
        self.source
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_uppercase()
    }
    pub fn same_item(&self, other: &Self) -> bool {
        match (self.library_id, other.library_id) {
            (Some(a), Some(b)) => a == b,
            _ => self.source == other.source && self.disc == other.disc,
        }
    }
    pub fn read_tags(&mut self) -> bool {
        let Ok(probe) = Probe::open(&self.source) else {
            return false;
        };
        let Ok(file) = probe
            .options(ParseOptions::new().read_cover_art(false))
            .read()
        else {
            return false;
        };
        self.duration = Some(file.properties().duration().as_secs_f64());
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            if let Some(title) = tag.title() {
                self.title = title.into_owned();
            }
            self.artist = tag.artist().unwrap_or_default().into_owned();
            self.album = tag.album().unwrap_or_default().into_owned();
            self.number = tag.track();
            self.disc_number = tag.disk();
            self.year = tag.date().map(|d| u32::from(d.year));
        }
        true
    }
}

/// A numbered CD directory, such as `CD1` or `Disc 2 - Live`, supplies
/// context when ordinary files have no tags.
pub fn folder_disc_number(file: &Path) -> Option<u32> {
    crate::release::disc_folder(file.parent()?.file_name()?.to_str()?).map(|(n, _)| n)
}

pub fn name(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
pub fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        ["flac", "wav", "mp3", "aif", "aiff", "m4a", "opus", "ogg"]
            .iter()
            .any(|ext| s.eq_ignore_ascii_case(ext))
    })
}
pub struct Folder {
    pub dirs: Vec<PathBuf>,
    pub tracks: Vec<Track>,
    pub cover: Option<PathBuf>,
    pub notices: Vec<String>,
    /// False when an entry of the folder could not be read.
    pub complete: bool,
    /// Sheets that exist but could not be interpreted.
    pub failed_sheets: Vec<PathBuf>,
}
impl Default for Folder {
    fn default() -> Self {
        Self {
            dirs: vec![],
            tracks: vec![],
            cover: None,
            notices: vec![],
            complete: true,
            failed_sheets: vec![],
        }
    }
}
#[cfg(test)]
pub fn browse(path: &Path) -> Result<Folder> {
    browse_while(path, None, || true)
}
pub fn browse_while(
    path: &Path,
    cache: Option<&crate::cache::Cache>,
    current: impl Fn() -> bool,
) -> Result<Folder> {
    let mut folder = Folder::default();
    let mut files = Vec::new();
    let mut discs: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("Cannot open {}", path.display()))? {
        anyhow::ensure!(current(), "Folder changed");
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                folder.notices.push(e.to_string());
                folder.complete = false;
                continue;
            }
        };
        let p = entry.path();
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(e) => {
                folder.notices.push(e.to_string());
                folder.complete = false;
                continue;
            }
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if !hidden(&entry) {
                folder.dirs.push(p);
            }
        } else if !kind.is_file() {
            continue;
        } else if is_audio(&p) {
            files.push(p);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("cue")) {
            discs.push(p);
        } else if let Some(rank) = cover_rank(&p)
            && folder
                .cover
                .as_ref()
                .and_then(|c| cover_rank(c))
                .is_none_or(|r| rank < r)
        {
            folder.cover = Some(p);
        }
    }
    sort_by_name(&mut folder.dirs, folder_name);
    discs.sort();
    browse_files(folder, files, &discs, &discs, cache, current)
}

/// Builds the recordings of a folder from some of its files. Only `sheets`
/// contribute CUE programs, while every sheet in `claims` removes the audio
/// files it plays from the loose recordings.
pub fn browse_files(
    mut folder: Folder,
    mut files: Vec<PathBuf>,
    sheets: &[PathBuf],
    claims: &[PathBuf],
    cache: Option<&crate::cache::Cache>,
    current: impl Fn() -> bool,
) -> Result<Folder> {
    let mut covered = HashSet::new();
    let mut tracks: Vec<Track> = Vec::new();
    let mut absent = Vec::new();
    for sheet in claims {
        anyhow::ensure!(current(), "Folder changed");
        let shown = sheets.contains(sheet);
        let sheet = sheet.clone();
        match cache.map_or_else(|| crate::cue::read(&sheet), |c| c.program(&sheet)) {
            Ok(program) => {
                let sources = program
                    .iter()
                    .map(|t| t.source.as_path())
                    .collect::<HashSet<_>>();
                let present = sources
                    .iter()
                    .filter_map(|&path| match path.canonicalize() {
                        Ok(canonical) => Some((path.to_owned(), canonical)),
                        Err(e) => {
                            if shown && e.kind() != std::io::ErrorKind::NotFound {
                                folder.failed_sheets.push(sheet.clone());
                            }
                            None
                        }
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                covered.extend(present.values().cloned());
                if !shown {
                    continue;
                }
                if present.is_empty() {
                    absent.push((sheet, program));
                    continue;
                }
                if present.len() < sources.len() {
                    folder.notices.push(format!(
                        "{} · some source recordings are unavailable",
                        folder_name(&sheet)
                    ));
                }
                tracks.extend(
                    program
                        .into_iter()
                        .filter(|t| present.contains_key(&t.source)),
                );
            }
            Err(error) if shown => {
                folder
                    .notices
                    .push(format!("{} · {error}", folder_name(&sheet)));
                folder.failed_sheets.push(sheet);
            }
            Err(_) => {}
        }
    }
    for (sheet, program) in absent {
        // EAC collections often retain a second sheet for an absent WAV rip.
        // Suppress it only when its complete track program matches a present edition.
        let duplicate = tracks.windows(program.len()).any(|candidate| {
            candidate.iter().zip(&program).all(|(a, b)| {
                a.title == b.title
                    && a.album == b.album
                    && a.artist == b.artist
                    && a.source.file_stem() == b.source.file_stem()
                    && a.disc.as_ref().zip(b.disc.as_ref()).is_some_and(|(a, b)| {
                        (a.number, a.start, a.end) == (b.number, b.start, b.end)
                    })
            })
        });
        if !duplicate {
            folder.notices.push(format!(
                "{} · source recording unavailable",
                folder_name(&sheet)
            ));
        }
    }
    sort_by_name(&mut files, name);
    tracks.extend(
        files
            .into_iter()
            .filter(|p| {
                covered.is_empty() || !p.canonicalize().is_ok_and(|key| covered.contains(&key))
            })
            .map(Track::file),
    );
    folder.tracks = tracks;
    Ok(folder)
}

/// The folder's cover image, by the same preference as a folder listing.
pub fn find_cover(dir: &Path) -> Option<PathBuf> {
    COVERS.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

const COVERS: [&str; 6] = [
    "cover.jpg",
    "folder.jpg",
    "front.jpg",
    "cover.png",
    "folder.png",
    "front.png",
];

fn cover_rank(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    COVERS
        .iter()
        .position(|candidate| name.eq_ignore_ascii_case(candidate))
}

/// Folders that browsing and recursive discovery pass over: hidden and system
/// folders such as `$RECYCLE.BIN`, links, and dot-folders such as `.git` or
/// `.cache`, which tools create without the hidden attribute on Windows. Any
/// of them still opens when the listener navigates to it directly.
fn hidden(entry: &fs::DirEntry) -> bool {
    if entry.file_name().to_string_lossy().starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        const REPARSE_POINT: u32 = 0x400;
        entry
            .metadata()
            .is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM | REPARSE_POINT) != 0)
    }
    #[cfg(not(windows))]
    false
}
/// The visible subfolders of a folder, sorted by name.
pub fn subfolders(path: &Path) -> Vec<PathBuf> {
    try_subfolders(path).unwrap_or_default()
}

pub(crate) fn try_subfolders(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dir = fs::read_dir(path)?;
    let mut dirs: Vec<PathBuf> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !hidden(e))
        .map(|e| e.path())
        .collect();
    sort_by_name(&mut dirs, folder_name);
    Ok(dirs)
}

/// A folder entry's name in the order Explorer shows it, so `CD 2`
/// precedes `CD 10`. Names that compare equal fall back to their exact
/// spelling, so every listing has one total order to resume from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameKey {
    name: String,
    #[cfg(windows)]
    wide: Vec<u16>,
}

impl NameKey {
    pub fn new(name: String) -> Self {
        Self {
            #[cfg(windows)]
            wide: name.encode_utf16().chain([0]).collect(),
            name,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl Ord for NameKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        #[cfg(windows)]
        let order = {
            use windows::{Win32::UI::Shell::StrCmpLogicalW, core::PCWSTR};
            // Both keys are NUL-terminated and outlive the call.
            unsafe { StrCmpLogicalW(PCWSTR(self.wide.as_ptr()), PCWSTR(other.wide.as_ptr())) }
                .cmp(&0)
        };
        #[cfg(not(windows))]
        let order = self.name.to_lowercase().cmp(&other.name.to_lowercase());
        order.then_with(|| self.name.cmp(&other.name))
    }
}

impl PartialOrd for NameKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The first `limit` entries after a resume point, kept while a directory
/// streams past. Memory stays proportional to `limit`, whatever the size of
/// the directory.
struct Window {
    after: Option<NameKey>,
    limit: usize,
    kept: std::collections::BinaryHeap<(NameKey, PathBuf)>,
    /// Entries after the resume point, including those not kept.
    seen: usize,
}

impl Window {
    fn new(after: Option<&str>, limit: usize) -> Self {
        Self {
            after: after.map(|name| NameKey::new(name.to_owned())),
            limit,
            kept: Default::default(),
            seen: 0,
        }
    }

    fn offer(&mut self, key: NameKey, path: PathBuf) {
        if self.after.as_ref().is_some_and(|after| key <= *after) {
            return;
        }
        self.seen += 1;
        if self.kept.len() < self.limit {
            self.kept.push((key, path));
        } else if self.kept.peek().is_some_and(|(largest, _)| key < *largest) {
            self.kept.pop();
            self.kept.push((key, path));
        }
    }

    /// The kept entries in order, and whether more follow them.
    fn finish(self) -> (Vec<(NameKey, PathBuf)>, bool) {
        let more = self.seen > self.kept.len();
        (self.kept.into_sorted_vec(), more)
    }
}

/// Where a paged folder listing resumes: after a subfolder, or after a
/// recording file once every subfolder has been listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListingCursor {
    Dirs(String),
    Files(String),
}

/// The files of one page of a folder listing.
pub enum ListedFiles {
    /// Every recording and sheet of the folder. Small folders keep their
    /// albums and programs together.
    All {
        audio: Vec<PathBuf>,
        sheets: Vec<PathBuf>,
    },
    /// The next files by name in a folder with too many to group at once,
    /// with every sheet whose claims keep its audio files out of the list.
    Page {
        audio: Vec<PathBuf>,
        sheets: Vec<PathBuf>,
        claims: Vec<PathBuf>,
        /// A resume point when more files follow.
        next: Option<String>,
    },
}

/// One bounded page of a folder: its next subfolders and, once they are
/// exhausted, its recordings.
pub struct FolderListing {
    pub dirs: Vec<PathBuf>,
    /// A resume point when more subfolders follow.
    pub next_dir: Option<String>,
    /// Present once every subfolder is listed.
    pub files: Option<ListedFiles>,
    pub cover: Option<PathBuf>,
    pub notices: Vec<String>,
    pub complete: bool,
}

/// Recordings and sheets a folder groups into albums and programs at once.
pub const GROUPED_FILES: usize = 4096;
/// Sheets whose claims a paged folder resolves.
const CLAIMED_SHEETS: usize = 256;

/// Lists one page of `path` in a single pass over its entries. Only names
/// are held, at most `limit` of each kind beyond the resume point, so a
/// directory of any size takes bounded memory. Links, junctions, hidden and
/// system entries are skipped as in `browse_while`, so no listing can cycle.
pub fn list_page(
    path: &Path,
    after: Option<&ListingCursor>,
    limit: usize,
    files_wanted: bool,
    current: impl Fn() -> bool,
) -> Result<FolderListing> {
    let (dirs_after, files_after) = match after {
        None => (None, None),
        Some(ListingCursor::Dirs(name)) => (Some(name.as_str()), None),
        Some(ListingCursor::Files(name)) => (None, Some(name.as_str())),
    };
    let listing_dirs = !matches!(after, Some(ListingCursor::Files(_)));
    let mut dirs = Window::new(dirs_after, if listing_dirs { limit } else { 0 });
    let mut files = Window::new(files_after, limit);
    let mut all_files: Option<Vec<PathBuf>> = Some(Vec::new());
    let mut sheets = Vec::new();
    let mut too_many_sheets = false;
    let mut cover: Option<PathBuf> = None;
    let mut notices = Vec::new();
    let mut complete = true;
    let mut visited = 0usize;
    for entry in fs::read_dir(path).with_context(|| format!("Cannot open {}", path.display()))? {
        visited += 1;
        if visited.is_multiple_of(256) {
            anyhow::ensure!(current(), crate::store::Cancelled);
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                notices.push(e.to_string());
                complete = false;
                continue;
            }
        };
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(e) => {
                notices.push(e.to_string());
                complete = false;
                continue;
            }
        };
        if kind.is_symlink() {
            continue;
        }
        let p = entry.path();
        if kind.is_dir() {
            if listing_dirs && !hidden(&entry) {
                dirs.offer(NameKey::new(folder_name(&p)), p);
            }
            continue;
        }
        if !kind.is_file() || !files_wanted {
            continue;
        }
        let sheet = p.extension().is_some_and(|s| s.eq_ignore_ascii_case("cue"));
        if sheet {
            if sheets.len() < CLAIMED_SHEETS {
                sheets.push(p.clone());
            } else {
                too_many_sheets = true;
            }
        }
        if sheet || is_audio(&p) {
            files.offer(NameKey::new(name(&p)), p.clone());
            if let Some(list) = &mut all_files {
                if list.len() < GROUPED_FILES {
                    list.push(p);
                } else {
                    all_files = None;
                }
            }
        } else if let Some(rank) = cover_rank(&p)
            && cover
                .as_ref()
                .and_then(|c| cover_rank(c))
                .is_none_or(|r| rank < r)
        {
            cover = Some(p);
        }
    }
    anyhow::ensure!(current(), crate::store::Cancelled);
    let (dirs, more_dirs) = dirs.finish();
    let next_dir = more_dirs.then(|| dirs.last().map(|(key, _)| key.as_str().to_owned()));
    let files = (!more_dirs && files_wanted).then(|| match all_files {
        Some(all) if files_after.is_none() => {
            let (sheets, audio) = all
                .into_iter()
                .partition(|p| p.extension().is_some_and(|s| s.eq_ignore_ascii_case("cue")));
            ListedFiles::All { audio, sheets }
        }
        _ => {
            if too_many_sheets {
                notices.push(
                    "Some recordings played by CUE sheets may also appear on their own.".into(),
                );
            }
            let (page, more) = files.finish();
            let next = more
                .then(|| page.last().map(|(key, _)| key.as_str().to_owned()))
                .flatten();
            let (page_sheets, audio) = page.into_iter().map(|(_, p)| p).partition(|p: &PathBuf| {
                p.extension().is_some_and(|s| s.eq_ignore_ascii_case("cue"))
            });
            ListedFiles::Page {
                audio,
                sheets: page_sheets,
                claims: sheets,
                next,
            }
        }
    });
    Ok(FolderListing {
        dirs: dirs.into_iter().map(|(_, p)| p).collect(),
        next_dir: next_dir.flatten(),
        files,
        cover,
        notices,
        complete,
    })
}

/// Orders paths by name as Explorer does, so `CD 2` precedes `CD 10`.
pub fn sort_by_name(paths: &mut [PathBuf], name: fn(&Path) -> String) {
    #[cfg(windows)]
    {
        let mut keyed: Vec<(Vec<u16>, PathBuf)> = paths
            .iter_mut()
            .map(|p| {
                let wide = name(p).encode_utf16().chain([0]).collect();
                (wide, std::mem::take(p))
            })
            .collect();
        keyed.sort_by(|(a, _), (b, _)| {
            use windows::{Win32::UI::Shell::StrCmpLogicalW, core::PCWSTR};
            // Both keys are NUL-terminated and outlive the call.
            let order = unsafe { StrCmpLogicalW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr())) };
            order.cmp(&0)
        });
        for (slot, (_, path)) in paths.iter_mut().zip(keyed) {
            *slot = path;
        }
    }
    #[cfg(not(windows))]
    paths.sort_by_cached_key(|p| name(p).to_lowercase());
}

/// Whether `path` is `root` or lies beneath it, ignoring case and trailing
/// separators as Windows does.
pub fn within(path: &Path, root: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let (path, root) = (norm(path), norm(root));
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

/// A short name for a folder; a drive root keeps its letter.
pub fn display_name(path: &Path) -> String {
    match path.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => path
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_owned(),
    }
}

pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

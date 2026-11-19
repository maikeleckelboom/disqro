use anyhow::{Context, Result};
use lofty::{config::ParseOptions, prelude::*, probe::Probe};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub source: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<f64>,
    pub disc: Option<DiscTrack>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscTrack {
    pub sheet: PathBuf,
    pub number: u8,
    pub start: u64,
    pub end: Option<u64>,
}

impl Track {
    pub fn file(path: PathBuf) -> Self {
        Self {
            title: name(&path),
            source: path,
            artist: String::new(),
            album: String::new(),
            duration: None,
            disc: None,
        }
    }
    pub fn format(&self) -> String {
        self.source
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_uppercase()
    }
    pub fn same_item(&self, other: &Self) -> bool {
        self.source == other.source && self.disc == other.disc
    }
    pub fn read_tags(&mut self) {
        let Ok(probe) = Probe::open(&self.source) else {
            return;
        };
        let Ok(file) = probe
            .options(ParseOptions::new().read_cover_art(false))
            .read()
        else {
            return;
        };
        self.duration = Some(file.properties().duration().as_secs_f64());
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            if let Some(title) = tag.title() {
                self.title = title.into_owned();
            }
            self.artist = tag.artist().unwrap_or_default().into_owned();
            self.album = tag.album().unwrap_or_default().into_owned();
        }
    }
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
#[derive(Default)]
pub struct Folder {
    pub dirs: Vec<PathBuf>,
    pub tracks: Arc<[Track]>,
    pub notices: Vec<String>,
}
pub fn browse(path: &Path) -> Result<Folder> {
    let mut folder = Folder::default();
    let mut files = Vec::new();
    let mut discs = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("Cannot open {}", path.display()))? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                folder.notices.push(e.to_string());
                continue;
            }
        };
        let p = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            folder.dirs.push(p);
        } else if is_audio(&p) {
            files.push(p);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("cue")) {
            discs.push(p);
        }
    }
    folder
        .dirs
        .sort_by_cached_key(|p| folder_name(p).to_lowercase());
    discs.sort();
    let mut covered = HashSet::new();
    let mut tracks = Vec::new();
    let mut absent = Vec::new();
    for sheet in discs {
        match crate::cue::read(&sheet) {
            Ok(program) => {
                let sources = program
                    .iter()
                    .map(|t| t.source.as_path())
                    .collect::<HashSet<_>>();
                let present = sources
                    .iter()
                    .filter_map(|path| path.canonicalize().ok())
                    .collect::<Vec<_>>();
                if present.is_empty() {
                    absent.push((sheet, program));
                    continue;
                }
                covered.extend(present);
                tracks.extend(program);
            }
            Err(error) => folder
                .notices
                .push(format!("{} · {error}", folder_name(&sheet))),
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
    files.sort_by_cached_key(|p| name(p).to_lowercase());
    tracks.extend(
        files
            .into_iter()
            .filter(|p| {
                covered.is_empty() || !p.canonicalize().is_ok_and(|key| covered.contains(&key))
            })
            .map(Track::file),
    );
    folder.tracks = tracks.into();
    Ok(folder)
}

pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

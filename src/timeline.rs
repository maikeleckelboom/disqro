//! Authored listening time. Frame zero is the decoder's trimmed source origin.
use crate::{
    cache::{Cache, Revision, Stamp},
    library::{DiscContext, Track, TrackList},
    recording::{Recording, disc_frame},
};
use anyhow::{Context, Result, ensure};
use kira::sound::streaming::Decoder;
use rusqlite::params;
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const SECOND: u64 = 1_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProgramKind {
    Album,
    Cd,
    Disc,
    Recording,
    Other,
}
impl ProgramKind {
    fn of(track: &Track, has_sibling_disc: bool) -> Self {
        let anchor = track.disc.as_ref().map_or(&track.source, |d| &d.sheet);
        let folder = anchor
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str());
        if folder.is_some_and(|name| {
            crate::release::disc_folder(name).is_some() && crate::release::names_cd(name)
        }) || (track.disc.is_some()
            && anchor
                .file_stem()
                .and_then(|n| n.to_str())
                .is_some_and(crate::release::names_cd))
        {
            Self::Cd
        } else if track.disc.is_some() {
            Self::Recording
        } else if folder.is_some_and(|name| crate::release::disc_folder(name).is_some())
            || has_sibling_disc
            || track.disc_number.is_some_and(|n| n > 1)
        {
            Self::Disc
        } else if !track.album.trim().is_empty() {
            Self::Album
        } else {
            Self::Other
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Album => "Album",
            Self::Cd => "Full CD",
            Self::Disc => "Disc",
            Self::Recording => "Full Recording",
            Self::Other => "Full Program",
        }
    }
}

/// The larger waveform perspective, including the disabled state while a
/// known program is being resolved or could not be built.
#[derive(Clone, Copy, Debug)]
pub struct Overview {
    pub label: &'static str,
    pub enabled: bool,
}
impl Overview {
    pub fn for_track(track: &Track, program: Option<&Program>) -> Option<Self> {
        let kind = program.map_or_else(|| ProgramKind::of(track, false), |p| p.kind);
        let authored = track.disc.is_some();
        let enabled = program.is_some_and(|p| p.full_cd && (authored || p.segments.len() > 1));
        let single = program.is_some_and(|p| p.full_cd && p.segments.len() == 1);
        (enabled || authored || (!single && kind != ProgramKind::Other)).then_some(Self {
            label: kind.label(),
            enabled,
        })
    }

    pub fn unavailable_tip(self, reason: Option<&str>, checking: bool) -> String {
        let label = self.label;
        match reason {
            Some(reason) => format!("{label} waveform unavailable · {reason}"),
            None if checking => format!("Checking {label} waveform availability"),
            None => format!("{label} waveform unavailable"),
        }
    }
}
pub fn frame_time(frame: usize, rate: u32) -> u64 {
    ((frame as u128 * u128::from(SECOND) + u128::from(rate / 2)) / u128::from(rate)) as u64
}
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Source {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub object: Option<crate::store::NativeFile>,
    pub rate: u32,
    pub frames: usize,
    pub available: bool,
}

impl Source {
    pub fn playable(&self, track: &Track) -> Option<Range<usize>> {
        let disc = track.disc.as_ref();
        let first = disc.map_or(0, |d| disc_frame(d.start, self.rate));
        let end = disc
            .and_then(|d| d.end)
            .map_or(self.frames, |e| disc_frame(e, self.rate));
        (self.path == track.source && first < end && end <= self.frames).then_some(first..end)
    }

    pub fn unchanged(&self) -> bool {
        if !self.available {
            return Stamp::read(&self.path).is_none();
        }
        let Ok(file) = std::fs::File::open(&self.path) else {
            return false;
        };
        file.metadata()
            .ok()
            .and_then(|m| Stamp::from_metadata(&m))
            .as_ref()
            == Some(&self.stamp)
            && self
                .object
                .is_none_or(|known| crate::store::NativeFile::of(&file) == Some(known))
    }
}
#[derive(Debug)]
pub struct Segment {
    pub track: Arc<Track>,
    pub source: Arc<Source>,
    pub frames: Range<usize>,
    pub time: Range<u64>,
}
impl Segment {
    pub fn at(&self, frame: usize) -> u64 {
        self.time.start
            + (frame_time(
                frame.clamp(self.frames.start, self.frames.end),
                self.source.rate,
            ) - frame_time(self.frames.start, self.source.rate))
    }
    /// Program time at a fractional source `frame`, for positions between
    /// frames such as an interpolated playhead. Agrees with [`Self::at`] at
    /// whole frames to within a nanosecond.
    pub fn at_fractional(&self, frame: f64) -> f64 {
        let (start, end) = (self.frames.start as f64, self.frames.end as f64);
        self.time.start as f64
            + (frame.clamp(start, end) - start) * SECOND as f64 / f64::from(self.source.rate)
    }
    pub fn elapsed_frame(&self, elapsed: f64) -> usize {
        self.frames.start
            + ((elapsed.max(0.) * f64::from(self.source.rate)).round() as usize)
                .min(self.frames.len())
    }
    fn frame_at(&self, time: u64) -> usize {
        let ns = time - self.time.start + frame_time(self.frames.start, self.source.rate);
        // Invert round-to-nearest frame starts: choose the latest start <= ns.
        (((u128::from(ns) + 1) * u128::from(self.source.rate)
            - 1
            - u128::from(self.source.rate / 2))
            / u128::from(SECOND)) as usize
    }
}
#[derive(Debug)]
pub struct Program {
    pub segments: Vec<Segment>,
    pub duration: u64,
    pub full_cd: bool,
    pub limitation: Option<String>,
    pub sheet: Option<(PathBuf, Revision)>,
    kind: ProgramKind,
    watch: Vec<(PathBuf, Option<Stamp>)>,
}
impl Program {
    pub fn inspection_chapter(&self, source: &Arc<Source>, frame: usize) -> Option<(u8, f64, f64)> {
        let segment = self
            .segments
            .iter()
            .find(|s| Arc::ptr_eq(&s.source, source) && s.frames.contains(&frame))?;
        let chapter = segment.track.disc.as_ref()?.number;
        let elapsed = (frame - segment.frames.start) as f64 / f64::from(source.rate);
        let time = segment.time.start as f64 / SECOND as f64 + elapsed;
        Some((chapter, elapsed, time))
    }

    pub fn index(&self, t: &Track) -> Option<usize> {
        self.segments
            .iter()
            .position(|s| s.track.source == t.source && s.track.disc == t.disc)
    }
    pub fn locate(&self, time: u64) -> Option<(usize, usize)> {
        let i = self.segments.partition_point(|s| s.time.end <= time);
        let seg = self.segments.get(i)?;
        Some((i, seg.frame_at(time)))
    }
    pub fn resolve(
        track: &Arc<Track>,
        context: DiscContext,
        cache: Option<&Cache>,
    ) -> Result<Self> {
        let mut watch = Vec::new();
        let mut kind = ProgramKind::of(track, false);
        let program = (|| -> Result<Self> {
            ensure!(context.allows(track), "No program context");
            let (tracks, sheet): (TrackList, _) = if let Some(d) = &track.disc {
                let revision = Revision::read(&d.sheet);
                watch.push((d.sheet.clone(), revision.as_ref().map(|r| r.stamp.clone())));
                let revision = revision.context("CUE sheet unavailable")?;
                let list = crate::cue::read(&d.sheet)?
                    .into_iter()
                    .map(Arc::new)
                    .collect();
                ensure!(
                    Revision::read(&d.sheet).as_ref() == Some(&revision),
                    "CUE sheet changed"
                );
                (list, Some((d.sheet.clone(), revision)))
            } else {
                let local = crate::discovery::local_program(
                    track.source.parent().context("Missing folder")?,
                    track,
                    context,
                    cache,
                )?;
                if let Some(actual) = local.tracks.iter().find(|t| t.same_item(track)) {
                    kind = ProgramKind::of(actual, local.has_sibling_disc);
                }
                (local.tracks, None)
            };
            for t in tracks.iter() {
                if watch.last().is_none_or(|(path, _)| path != &t.source) {
                    watch.push((t.source.clone(), Stamp::read(&t.source)));
                }
            }
            ensure!(
                !tracks.is_empty()
                    && (track.disc.is_some() || track.has_disc_context() || tracks.len() > 1),
                "No larger sequence was found"
            );
            ensure!(tracks.len() <= 256, "Program exceeds the overview limit");
            let p = Self::build(&tracks, sheet, cache, true, kind)?;
            ensure!(
                p.index(track).is_some(),
                "Playing track differs from the current program"
            );
            Ok(p)
        })();
        let failure = match program {
            Ok(p) => return Ok(p),
            Err(e) => e,
        };
        let mut p = Self::build(std::slice::from_ref(track), None, cache, false, kind)?;
        p.watch = watch;
        if context.allows(track) {
            p.limitation = Some(failure.to_string());
        }
        Ok(p)
    }
    fn build(
        tracks: &[Arc<Track>],
        sheet: Option<(PathBuf, Revision)>,
        cache: Option<&Cache>,
        full_cd: bool,
        kind: ProgramKind,
    ) -> Result<Self> {
        let mut sources = HashMap::<PathBuf, Arc<Source>>::new();
        let mut segments = Vec::new();
        let mut duration = 0u64;
        for t in tracks {
            let source = match sources.get(&t.source) {
                Some(s) => s.clone(),
                None => {
                    let s = Arc::new(describe(&t.source, sheet.as_ref(), cache)?);
                    sources.insert(t.source.clone(), s.clone());
                    s
                }
            };
            let frames = source.playable(t).context("Invalid chapter boundary")?;
            let start = duration;
            duration = duration
                .checked_add(
                    frame_time(frames.end, source.rate) - frame_time(frames.start, source.rate),
                )
                .context("Program duration overflow")?;
            segments.push(Segment {
                track: t.clone(),
                source,
                frames,
                time: start..duration,
            });
        }
        ensure!(duration > 0, "Empty program");
        Ok(Self {
            segments,
            duration,
            full_cd,
            limitation: None,
            sheet,
            kind,
            watch: Vec::new(),
        })
    }
    pub fn unchanged(&self) -> bool {
        if self.watch.iter().any(|(p, s)| Stamp::read(p) != *s) {
            return false;
        }
        if let Some((p, s)) = &self.sheet
            && Revision::read(p).as_ref() != Some(s)
        {
            return false;
        }
        let mut last = None;
        self.segments.iter().all(|seg| {
            if last == Some(&seg.source.path) {
                return true;
            }
            last = Some(&seg.source.path);
            seg.source.unchanged()
        })
    }
}
fn describe(
    path: &Path,
    sheet: Option<&(PathBuf, Revision)>,
    cache: Option<&Cache>,
) -> Result<Source> {
    let key = path.as_os_str().as_encoded_bytes();
    let source = if let Some(stamp) = Stamp::read(path) {
        let recording = Recording::open(&Track::file(path.into()), None)?;
        let rate = recording.sample_rate();
        let frames = recording.num_frames();
        ensure!(
            recording.stamp.as_ref() == Some(&stamp) && Stamp::read(path).as_ref() == Some(&stamp),
            "Recording changed"
        );
        // Keep a verified length tied to the authored sheet revision for offline files.
        if let (Some(c), Some((p, s))) = (cache, sheet) {
            let _ = c.0.execute(
                "INSERT OR REPLACE INTO disc_sources_v1 VALUES(?,?,?,?,?,?,?,?)",
                params![
                    p.as_os_str().as_encoded_bytes(),
                    s.stamp.bytes,
                    s.stamp.modified,
                    key,
                    stamp.bytes,
                    stamp.modified,
                    rate,
                    frames as i64
                ],
            );
        }
        Source {
            path: path.into(),
            stamp,
            object: recording.object,
            rate,
            frames,
            available: true,
        }
    } else {
        let known=cache.zip(sheet).and_then(|(c,(p,s))|c.0.query_row(
            "SELECT bytes,modified,rate,frames FROM disc_sources_v1 WHERE sheet=? AND sheet_bytes=? AND sheet_modified=? AND path=?",
            params![p.as_os_str().as_encoded_bytes(),s.stamp.bytes,s.stamp.modified,key],|r|Ok(Source{path:path.into(),stamp:Stamp{bytes:r.get(0)?,modified:r.get(1)?},object:None,rate:r.get(2)?,frames:r.get::<_,i64>(3)? as usize,available:false})).ok());
        known.with_context(|| {
            format!(
                "Unknown duration for {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        })?
    };
    ensure!(
        (8000..=384000).contains(&source.rate)
            && source.frames > 0
            && source.frames as u128 * u128::from(SECOND) / u128::from(source.rate)
                < u128::from(u64::MAX),
        "Unsupported source duration"
    );
    Ok(source)
}

#[cfg(test)]
mod tests;

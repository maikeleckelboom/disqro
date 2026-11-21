//! Authored listening time. Frame zero is the decoder's trimmed source origin.
use crate::{
    cache::{Cache, Stamp},
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
/// Whether `track` comes from a CD: a CUE chapter, known before its program
/// resolves, or one of several tracks that resolved as one CD. A recording
/// that stands alone, including a one-track album, has no Full CD.
pub fn from_cd(track: &Track, program: Option<&Program>) -> bool {
    track.disc.is_some() || program.is_some_and(|p| p.full_cd && p.segments.len() > 1)
}
pub fn frame_time(frame: usize, rate: u32) -> u64 {
    ((frame as u128 * u128::from(SECOND) + u128::from(rate / 2)) / u128::from(rate)) as u64
}
#[derive(Debug)]
pub struct Source {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub rate: u32,
    pub frames: usize,
    pub available: bool,
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
    pub sheet: Option<(PathBuf, Stamp)>,
    watch: Vec<(PathBuf, Option<Stamp>)>,
}
impl Program {
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
        let disc = (|| -> Result<Self> {
            ensure!(context.allows(track), "No CD context");
            let (tracks, sheet): (TrackList, _) = if let Some(d) = &track.disc {
                let stamp = Stamp::read(&d.sheet);
                watch.push((d.sheet.clone(), stamp.clone()));
                let stamp = stamp.context("CUE sheet unavailable")?;
                let list = crate::cue::read(&d.sheet)?
                    .into_iter()
                    .map(Arc::new)
                    .collect();
                ensure!(
                    Stamp::read(&d.sheet).as_ref() == Some(&stamp),
                    "CUE sheet changed"
                );
                (list, Some((d.sheet.clone(), stamp)))
            } else {
                (
                    crate::discovery::local_program(
                        track.source.parent().context("Missing folder")?,
                        track,
                        context,
                        cache,
                    )?,
                    None,
                )
            };
            for t in tracks.iter() {
                if watch.last().is_none_or(|(path, _)| path != &t.source) {
                    watch.push((t.source.clone(), Stamp::read(&t.source)));
                }
            }
            ensure!(
                track.disc.is_some() || track.has_disc_context() || tracks.len() > 1,
                "No authored CD"
            );
            ensure!(tracks.len() <= 256, "CD exceeds the overview limit");
            let p = Self::build(&tracks, sheet, cache, true)?;
            ensure!(
                p.index(track).is_some(),
                "Playing chapter differs from the current CD"
            );
            Ok(p)
        })();
        let failure = match disc {
            Ok(p) => return Ok(p),
            Err(e) => e,
        };
        let mut p = Self::build(std::slice::from_ref(track), None, cache, false)?;
        p.watch = watch;
        if context.allows(track) {
            p.limitation = Some(format!("Full CD unavailable: {failure}"));
        }
        Ok(p)
    }
    fn build(
        tracks: &[Arc<Track>],
        sheet: Option<(PathBuf, Stamp)>,
        cache: Option<&Cache>,
        full_cd: bool,
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
            let d = t.disc.as_ref();
            let first = d.map_or(0, |d| disc_frame(d.start, source.rate));
            let end = d
                .and_then(|d| d.end)
                .map_or(source.frames, |e| disc_frame(e, source.rate));
            ensure!(
                first < end && end <= source.frames,
                "Invalid chapter boundary"
            );
            let start = duration;
            duration = duration
                .checked_add(frame_time(end, source.rate) - frame_time(first, source.rate))
                .context("CD duration overflow")?;
            segments.push(Segment {
                track: t.clone(),
                source,
                frames: first..end,
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
            watch: Vec::new(),
        })
    }
    pub fn unchanged(&self) -> bool {
        if self.watch.iter().any(|(p, s)| Stamp::read(p) != *s) {
            return false;
        }
        if let Some((p, s)) = &self.sheet
            && Stamp::read(p).as_ref() != Some(s)
        {
            return false;
        }
        let mut last = None;
        self.segments.iter().all(|seg| {
            if last == Some(&seg.source.path) {
                return true;
            }
            last = Some(&seg.source.path);
            Stamp::read(&seg.source.path).as_ref()
                == seg.source.available.then_some(&seg.source.stamp)
        })
    }
}
fn describe(
    path: &Path,
    sheet: Option<&(PathBuf, Stamp)>,
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
                    s.bytes,
                    s.modified,
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
            rate,
            frames,
            available: true,
        }
    } else {
        let known=cache.zip(sheet).and_then(|(c,(p,s))|c.0.query_row(
            "SELECT bytes,modified,rate,frames FROM disc_sources_v1 WHERE sheet=? AND sheet_bytes=? AND sheet_modified=? AND path=?",
            params![p.as_os_str().as_encoded_bytes(),s.bytes,s.modified,key],|r|Ok(Source{path:path.into(),stamp:Stamp{bytes:r.get(0)?,modified:r.get(1)?},rate:r.get(2)?,frames:r.get::<_,i64>(3)? as usize,available:false})).ok());
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
#[path = "timeline_tests.rs"]
mod tests;

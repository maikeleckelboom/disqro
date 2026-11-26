//! Disqro's side of Waveform Native: validated, bounded source reads through
//! the recording decoder, and the SQLite store for encoded evidence.
//!
//! Waveform Native owns the measurements, their encoding and projection.
//! Disqro owns decoding, file identity and revision checks, and storage.
use crate::{
    cache::{Cache, Stamp},
    library::Track,
    recording::Recording,
    timeline::Source,
    waveform_port_analysis::{LEAF, Plan},
    waveform_render::Page,
};
use anyhow::{Result, ensure};
use kira::{Frame, sound::streaming::Decoder};
use rusqlite::params;
use std::{ops::Range, path::PathBuf, sync::atomic::Ordering::Relaxed};
use waveform::{Analyzer, EvidenceRequest, FrameRange, SampleRate, evidence_codec};

pub struct Reader {
    path: PathBuf,
    stamp: Stamp,
    object: Option<crate::store::NativeFile>,
    decoder: Recording,
    cursor: usize,
    buffer: Vec<Frame>,
    used: usize,
}
impl Reader {
    pub(crate) fn open(s: &Source) -> Result<Self> {
        let decoder = Recording::open(&Track::file(s.path.clone()), None)?;
        ensure!(
            decoder.stamp.as_ref() == Some(&s.stamp)
                && decoder.object == s.object
                && decoder.sample_rate() == s.rate
                && decoder.num_frames() == s.frames,
            "Recording changed"
        );
        Ok(Self {
            path: s.path.clone(),
            stamp: s.stamp.clone(),
            object: s.object,
            decoder,
            cursor: 0,
            buffer: vec![],
            used: 0,
        })
    }
    fn position(&mut self, frame: usize) -> Result<()> {
        if self.cursor != frame {
            self.decoder.seek(frame)?;
            self.cursor = frame;
            self.buffer.clear();
            self.used = 0;
        }
        Ok(())
    }

    pub(crate) fn read_into(
        &mut self,
        start: usize,
        output: &mut [Frame],
        current: &impl Fn() -> bool,
    ) -> Result<()> {
        self.position(start)?;
        self.visit(start + output.len(), current, |first, chunk| {
            let index = first - start;
            output[index..index + chunk.len()].copy_from_slice(chunk);
        })
    }

    fn visit(
        &mut self,
        end: usize,
        current: &impl Fn() -> bool,
        mut visit: impl FnMut(usize, &[Frame]),
    ) -> Result<()> {
        while self.cursor < end {
            if self.used == self.buffer.len() {
                ensure!(current(), "Analysis canceled");
                self.buffer = self.decoder.decode()?;
                self.used = 0;
                ensure!(!self.buffer.is_empty(), "Unexpected source end");
            }
            let count = (end - self.cursor).min(self.buffer.len() - self.used);
            visit(self.cursor, &self.buffer[self.used..self.used + count]);
            self.cursor += count;
            self.used += count;
        }
        Ok(())
    }
}

/// Reads a bounded sample interval using the same validated source and decoder
/// as the measurement pages. The caller owns the interval size and residency.
pub fn samples(
    s: &Source,
    range: Range<usize>,
    reader: &mut Option<Reader>,
    current: &impl Fn() -> bool,
) -> Result<Vec<Frame>> {
    ensure!(current(), "Analysis canceled");
    ensure!(
        range.start < range.end && range.end <= s.frames && source_unchanged(s),
        "Sample range unavailable"
    );
    let reader = reader_for(s, reader)?;
    reader.position(range.start)?;
    let mut frames = Vec::with_capacity(range.len());
    reader.visit(range.end, current, |_, chunk| {
        frames.extend_from_slice(chunk)
    })?;
    ensure!(source_unchanged(s), "Recording changed during analysis");
    Ok(frames)
}

fn source_unchanged(s: &Source) -> bool {
    s.available && s.unchanged()
}

fn reader_for<'a>(s: &Source, reader: &'a mut Option<Reader>) -> Result<&'a mut Reader> {
    if reader
        .as_ref()
        .is_none_or(|r| r.path != s.path || r.stamp != s.stamp || r.object != s.object)
    {
        *reader = Some(Reader::open(s)?);
    }
    Ok(reader.as_mut().unwrap())
}

/// One measurement plan over a playable interval of a physical source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Evidence {
    pub playable: Range<usize>,
    pub plan: Plan,
}

impl Evidence {
    /// The engine request for this plan.
    pub fn request(&self) -> Result<EvidenceRequest> {
        Ok(EvidenceRequest::new(
            FrameRange::new(self.playable.start as u64, self.playable.end as u64)?,
            self.plan,
        )?)
    }

    pub fn begin(&self, rate: u32) -> Result<Analyzer> {
        Ok(Analyzer::new(SampleRate::new(rate)?, &self.request()?)?)
    }

    // Persist overview and reusable base tiers. Close-up refinements stay in memory.
    fn persistent(&self) -> bool {
        !self.plan.preview
            && (self.plan.overview || self.plan.leaf_frames.is_some_and(|w| w >= LEAF))
    }
}

pub fn cached(source: &Source, request: &Evidence, cache: Option<&Cache>) -> Option<Page> {
    if !request.persistent() || !source_unchanged(source) {
        return None;
    }
    let engine = request.request().ok()?;
    let db = &cache?.0;
    let mut query = db.prepare_cached(
        "SELECT data FROM wave_evidence_v4 WHERE path=? AND requested=? AND size=? AND modified=? AND volume IS ? AND object IS ?",
    ).ok()?;
    let page = query
        .query_row(
            params![
                source.path.as_os_str().as_encoded_bytes(),
                engine.canonical_bytes(),
                source.stamp.bytes,
                source.stamp.modified,
                source.object.map(|p| p.0),
                source.object.map(|p| p.1)
            ],
            |row| {
                let bytes = row.get_ref(0)?.as_blob()?;
                Ok(evidence_codec::decode(bytes, &engine).ok())
            },
        )
        .ok()
        .flatten()?;
    if let Some(d) = crate::diagnostics::get() {
        d.page_hits.fetch_add(1, Relaxed);
    }
    Some(page)
}

pub fn store(source: &Source, request: &Evidence, cache: Option<&Cache>, page: &Page) {
    if let Some(d) = crate::diagnostics::get() {
        d.page_misses.fetch_add(1, Relaxed);
    }
    let Some(cache) = cache.filter(|_| request.persistent()) else {
        return;
    };
    let Ok(engine) = request.request() else {
        return;
    };
    let Ok(bytes) = evidence_codec::encode(page, &engine) else {
        return;
    };
    if let Ok(tx) = cache.0.unchecked_transaction() {
        let written = tx.prepare_cached(
            "INSERT OR REPLACE INTO wave_evidence_v4(path,requested,size,modified,volume,object,data) VALUES(?,?,?,?,?,?,?)",
        ).and_then(|mut query| query.execute(params![
            source.path.as_os_str().as_encoded_bytes(), engine.canonical_bytes(), source.stamp.bytes,
            source.stamp.modified, source.object.map(|p| p.0), source.object.map(|p| p.1), bytes
        ]));
        if written.is_ok() {
            if tx.last_insert_rowid() % 32 == 0 {
                let _ = tx.execute(
                    "DELETE FROM wave_evidence_v4 WHERE rowid <= (SELECT rowid FROM wave_evidence_v4 ORDER BY rowid DESC LIMIT 1 OFFSET 512)",
                    [],
                );
            }
            let _ = tx.commit();
        }
    }
}

pub fn feed(
    source: &Source,
    range: Range<usize>,
    analyzer: &mut Analyzer,
    reader: &mut Option<Reader>,
    current: &impl Fn() -> bool,
) -> Result<()> {
    ensure!(current(), "Analysis canceled");
    ensure!(
        range.end <= source.frames && source_unchanged(source),
        "Recording changed"
    );
    let reader = reader_for(source, reader)?;
    reader.position(range.start)?;
    let mut result = Ok(());
    let mut samples = Vec::new();
    reader.visit(range.end, current, |_, frames| {
        if result.is_ok() {
            samples.clear();
            samples.extend(frames.iter().map(|f| [f.left, f.right]));
            result = analyzer.push(&samples);
        }
    })?;
    result?;
    ensure!(
        source_unchanged(source),
        "Recording changed during analysis"
    );
    Ok(())
}

pub fn load(
    source: &Source,
    request: &Evidence,
    cache: Option<&Cache>,
    reader: &mut Option<Reader>,
    current: &impl Fn() -> bool,
) -> Result<Page> {
    ensure!(current(), "Analysis canceled");
    if let Some(page) = cached(source, request, cache) {
        return Ok(page);
    }
    let mut analyzer = request.begin(source.rate)?;
    let range = analyzer.decode_range();
    feed(
        source,
        request.playable.start + range.start..request.playable.start + range.end,
        &mut analyzer,
        reader,
        current,
    )?;
    let page = analyzer.finish()?;
    store(source, request, cache, &page);
    Ok(page)
}

//! Physical source pages. Four cells per CUE tick keep every supported INDEX
//! on an exact cell edge at every sample rate. The last cell ends at decoded EOF.
use crate::{
    cache::{Cache, Stamp},
    library::Track,
    recording::Recording,
    timeline::Source,
};
use anyhow::{Context, Result, ensure};
use kira::{Frame, sound::streaming::Decoder};
use rusqlite::params;
use std::{ops::Range, path::PathBuf, sync::atomic::Ordering};

#[path = "waveformBands.rs"]
mod bands;

pub const HZ: usize = 300;
pub const PAGE: usize = 1200;
const RECORD: usize = 56;
pub fn edge(cell: usize, rate: u32) -> usize {
    ((cell as u128 * u128::from(rate) + (HZ / 2) as u128) / HZ as u128) as usize
}
/// First grid boundary at or beyond a frame (also the number of cells to EOF).
pub fn ceil_cell(frame: usize, rate: u32) -> usize {
    ((frame as u128 * HZ as u128).saturating_sub((HZ / 2) as u128)).div_ceil(u128::from(rate))
        as usize
}
#[derive(Clone, Copy)]
pub struct Measure {
    pub lo: [f32; 2],
    pub hi: [f32; 2],
    pub lo_at: [usize; 2],
    pub hi_at: [usize; 2],
    pub energy: f64,
    pub bands: [f64; 3],
}
impl Default for Measure {
    fn default() -> Self {
        Self {
            lo: [f32::INFINITY; 2],
            hi: [f32::NEG_INFINITY; 2],
            lo_at: [usize::MAX; 2],
            hi_at: [usize::MAX; 2],
            energy: 0.,
            bands: [0.; 3],
        }
    }
}
impl Measure {
    pub fn add(&mut self, frame: usize, f: Frame, bands: [f64; 3]) {
        for (channel, sample) in [f.left, f.right].into_iter().enumerate() {
            if sample < self.lo[channel] {
                (self.lo[channel], self.lo_at[channel]) = (sample, frame);
            }
            if sample > self.hi[channel] {
                (self.hi[channel], self.hi_at[channel]) = (sample, frame);
            }
        }
        self.energy += (f64::from(f.left).powi(2) + f64::from(f.right).powi(2)) * 0.5;
        for (sum, energy) in self.bands.iter_mut().zip(bands) {
            *sum += energy;
        }
    }
    pub fn merge(&mut self, m: &Self) {
        for c in 0..2 {
            if m.lo[c] < self.lo[c] || (m.lo[c] == self.lo[c] && m.lo_at[c] < self.lo_at[c]) {
                (self.lo[c], self.lo_at[c]) = (m.lo[c], m.lo_at[c]);
            }
            if m.hi[c] > self.hi[c] || (m.hi[c] == self.hi[c] && m.hi_at[c] < self.hi_at[c]) {
                (self.hi[c], self.hi_at[c]) = (m.hi[c], m.hi_at[c]);
            }
        }
        self.energy += m.energy;
        for (sum, energy) in self.bands.iter_mut().zip(m.bands) {
            *sum += energy;
        }
    }
    pub fn extrema(&self) -> [(f32, usize); 2] {
        let low = if self.lo[0] == self.lo[1] {
            usize::from(self.lo_at[1] < self.lo_at[0])
        } else {
            usize::from(self.lo[1] < self.lo[0])
        };
        let high = if self.hi[0] == self.hi[1] {
            usize::from(self.hi_at[1] < self.hi_at[0])
        } else {
            usize::from(self.hi[1] > self.hi[0])
        };
        [
            (self.lo[low], self.lo_at[low]),
            (self.hi[high], self.hi_at[high]),
        ]
    }

    fn bytes(&self, start: usize, v: &mut Vec<u8>) {
        for f in self.lo.iter().chain(&self.hi) {
            v.extend(f.to_le_bytes());
        }
        v.extend(self.energy.to_le_bytes());
        for power in self.bands {
            v.extend(power.to_le_bytes());
        }
        for frame in self.lo_at.into_iter().chain(self.hi_at) {
            v.extend(
                u16::try_from(frame.saturating_sub(start))
                    .unwrap_or(u16::MAX)
                    .to_le_bytes(),
            );
        }
    }
    fn read(bytes: &[u8; RECORD], start: usize, end: usize) -> Option<Self> {
        let f = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let power = |i: usize| f64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let frame =
            |i: usize| start + usize::from(u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()));
        let m = Self {
            lo: [f(0), f(4)],
            hi: [f(8), f(12)],
            lo_at: [frame(48), frame(50)],
            hi_at: [frame(52), frame(54)],
            energy: power(16),
            bands: [power(24), power(32), power(40)],
        };
        (m.energy.is_finite()
            && m.energy >= 0.
            && m.bands.iter().all(|b| b.is_finite() && *b >= 0.)
            && (0..2).all(|c| {
                m.lo[c].is_finite()
                    && m.hi[c].is_finite()
                    && m.lo[c] <= m.hi[c]
                    && m.lo_at[c] < end
                    && m.hi_at[c] < end
            }))
        .then_some(m)
    }
}
pub struct Reader {
    path: PathBuf,
    stamp: Stamp,
    object: Option<crate::store::NativeFile>,
    decoder: Recording,
    cursor: usize,
    buffer: Vec<Frame>,
    used: usize,
    history: Vec<Frame>,
}
impl Reader {
    fn open(s: &Source) -> Result<Self> {
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
            history: vec![],
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

    fn visit(
        &mut self,
        end: usize,
        current: &impl Fn() -> bool,
        mut visit: impl FnMut(usize, &[Frame]),
    ) -> Result<()> {
        while self.cursor < end {
            if self.used == self.buffer.len() {
                ensure!(current(), "Analysis cancelled");
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

    fn page(
        &mut self,
        s: &Source,
        first: usize,
        count: usize,
        current: &impl Fn() -> bool,
    ) -> Result<Vec<Measure>> {
        let start = edge(first, s.rate);
        let mut bands = bands::Bands::new(s.rate);
        let preroll = s.rate as usize / 10;
        let preroll_start = start.saturating_sub(preroll);
        let mut history = std::mem::take(&mut self.history);
        // Use the same preceding samples whether the page follows a cold seek
        // or is served from a running decoder. Filter state is local to this page.
        if self.cursor == start && history.len() == start - preroll_start {
            for &sample in &history {
                bands.power(sample);
            }
        } else {
            self.position(preroll_start)?;
            self.visit(start, current, |_, frames| {
                for &sample in frames {
                    bands.power(sample);
                }
            })?;
        }
        let page_end = edge(first + count, s.rate).min(s.frames);
        let retain_from = page_end.saturating_sub(preroll);
        history.clear();
        let mut page = vec![Measure::default(); count];
        for (i, m) in page.iter_mut().enumerate() {
            let end = edge(first + i + 1, s.rate).min(s.frames);
            self.visit(end, current, |cursor, frames| {
                for (offset, &sample) in frames.iter().enumerate() {
                    m.add(cursor + offset, sample, bands.power(sample));
                }
                if let Some(tail) = frames.get(retain_from.saturating_sub(cursor)..) {
                    history.extend_from_slice(tail);
                }
            })?;
        }
        self.history = history;
        Ok(page)
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
    ensure!(current(), "Analysis cancelled");
    ensure!(
        range.start < range.end && range.end <= s.frames && source_unchanged(s),
        "Sample range unavailable"
    );
    let reader = reader_for(s, reader)?;
    reader.position(range.start)?;
    reader.history.clear();
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

pub fn load(
    s: &Source,
    number: usize,
    cache: Option<&Cache>,
    reader: &mut Option<Reader>,
    current: &impl Fn() -> bool,
) -> Result<Vec<Measure>> {
    ensure!(source_unchanged(s), "Recording changed or unavailable");
    let first = number * PAGE;
    let count = ceil_cell(s.frames, s.rate)
        .checked_sub(first)
        .context("Page outside source")?
        .min(PAGE);
    ensure!(count > 0, "Empty page");
    let key = s.path.as_os_str().as_encoded_bytes();
    let volume = s.object.map(|file| file.0);
    let object = s.object.map(|file| file.1);
    let cached = cache.and_then(|c| {
        let mut statement = c.0.prepare_cached(
            "SELECT data FROM wave_pages_v2 WHERE path=? AND page=? AND bytes=? AND modified=? AND volume IS ? AND object IS ? AND length(data)=?",
        ).ok()?;
        statement.query_row(
            params![key, number as i64, s.stamp.bytes, s.stamp.modified, volume, object, (count * RECORD) as i64],
            |row| {
                let bytes = row.get_ref(0)?.as_blob()?;
                let mut start = edge(first, s.rate);
                Ok(bytes.as_chunks::<RECORD>().0.iter().enumerate().map(|(i, bytes)| {
                    let end = edge(first + i + 1, s.rate).min(s.frames);
                    let result = Measure::read(bytes, start, end);
                    start = end;
                    result
                }).collect::<Option<Vec<_>>>())
            }
        ).ok().flatten()
    });
    if let Some(page) = cached {
        if let Some(d) = crate::diagnostics::get() {
            d.page_hits.fetch_add(1, Ordering::Relaxed);
        }
        return Ok(page);
    }
    ensure!(current(), "Analysis cancelled");
    let page = reader_for(s, reader)?.page(s, first, count, current)?;
    ensure!(source_unchanged(s), "Recording changed during analysis");
    if let Some(d) = crate::diagnostics::get() {
        d.page_misses.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(c) = cache {
        let mut bytes = Vec::with_capacity(count * RECORD);
        for (i, measure) in page.iter().enumerate() {
            measure.bytes(edge(first + i, s.rate), &mut bytes);
        }
        if let Ok(tx) = c.0.unchecked_transaction() {
            let stored = tx.prepare_cached(
                "INSERT OR REPLACE INTO wave_pages_v2(path,bytes,modified,volume,object,page,data) VALUES(?,?,?,?,?,?,?)"
            ).and_then(|mut statement| statement.execute(
                params![key, s.stamp.bytes, s.stamp.modified, volume, object, number as i64, bytes]
            ));
            // Prune in batches. Each full page is 67,200 bytes.
            if stored.is_ok() {
                if tx.last_insert_rowid() % 64 == 0 {
                    let _ = tx.execute(
                        "DELETE FROM wave_pages_v2 WHERE rowid <= (SELECT rowid FROM wave_pages_v2 ORDER BY rowid DESC LIMIT 1 OFFSET 4096)",
                        [],
                    );
                }
                let _ = tx.commit();
            }
        }
    }
    Ok(page)
}

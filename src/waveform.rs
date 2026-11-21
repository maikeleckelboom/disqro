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
use std::{path::PathBuf, sync::atomic::Ordering};

pub const HZ: usize = 300;
pub const PAGE: usize = 1200;
const RECORD: usize = 24;
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
    pub energy: f64,
}
impl Default for Measure {
    fn default() -> Self {
        Self {
            lo: [f32::INFINITY; 2],
            hi: [f32::NEG_INFINITY; 2],
            energy: 0.,
        }
    }
}
impl Measure {
    fn add(&mut self, f: &Frame) {
        self.lo[0] = self.lo[0].min(f.left);
        self.lo[1] = self.lo[1].min(f.right);
        self.hi[0] = self.hi[0].max(f.left);
        self.hi[1] = self.hi[1].max(f.right);
        self.energy += (f64::from(f.left).powi(2) + f64::from(f.right).powi(2)) * 0.5;
    }
    pub fn merge(&mut self, m: &Self) {
        for c in 0..2 {
            self.lo[c] = self.lo[c].min(m.lo[c]);
            self.hi[c] = self.hi[c].max(m.hi[c]);
        }
        self.energy += m.energy;
    }
    fn bytes(&self, v: &mut Vec<u8>) {
        for f in self.lo.iter().chain(&self.hi) {
            v.extend(f.to_le_bytes());
        }
        v.extend(self.energy.to_le_bytes());
    }
    fn read(bytes: &[u8; RECORD]) -> Option<Self> {
        let f = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let m = Self {
            lo: [f(0), f(4)],
            hi: [f(8), f(12)],
            energy: f64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        };
        (m.energy.is_finite()
            && m.energy >= 0.
            && (0..2).all(|c| m.lo[c].is_finite() && m.hi[c].is_finite() && m.lo[c] <= m.hi[c]))
        .then_some(m)
    }
}
pub struct Reader {
    path: PathBuf,
    stamp: Stamp,
    decoder: Recording,
    cursor: usize,
    buffer: Vec<Frame>,
    used: usize,
}
impl Reader {
    fn open(s: &Source) -> Result<Self> {
        let decoder = Recording::open(&Track::file(s.path.clone()), None)?;
        ensure!(
            decoder.stamp.as_ref() == Some(&s.stamp)
                && decoder.sample_rate() == s.rate
                && decoder.num_frames() == s.frames,
            "Recording changed"
        );
        Ok(Self {
            path: s.path.clone(),
            stamp: s.stamp.clone(),
            decoder,
            cursor: 0,
            buffer: vec![],
            used: 0,
        })
    }
    fn page(
        &mut self,
        s: &Source,
        first: usize,
        count: usize,
        current: &impl Fn() -> bool,
    ) -> Result<Vec<Measure>> {
        let start = edge(first, s.rate);
        if self.cursor != start {
            self.decoder.seek(start)?;
            self.cursor = start;
            self.buffer.clear();
            self.used = 0;
        }
        let mut page = vec![Measure::default(); count];
        for (i, m) in page.iter_mut().enumerate() {
            let end = edge(first + i + 1, s.rate).min(s.frames);
            while self.cursor < end {
                if self.used == self.buffer.len() {
                    ensure!(current(), "Analysis cancelled");
                    self.buffer = self.decoder.decode()?;
                    self.used = 0;
                    ensure!(!self.buffer.is_empty(), "Unexpected source end");
                }
                let n = (end - self.cursor).min(self.buffer.len() - self.used);
                for f in &self.buffer[self.used..self.used + n] {
                    m.add(f);
                }
                self.cursor += n;
                self.used += n;
            }
        }
        Ok(page)
    }
}
pub fn load(
    s: &Source,
    number: usize,
    cache: Option<&Cache>,
    reader: &mut Option<Reader>,
    current: &impl Fn() -> bool,
) -> Result<Vec<Measure>> {
    ensure!(
        s.available && Stamp::read(&s.path).as_ref() == Some(&s.stamp),
        "Recording changed or unavailable"
    );
    let first = number * PAGE;
    let count = ceil_cell(s.frames, s.rate)
        .checked_sub(first)
        .context("Page outside source")?
        .min(PAGE);
    ensure!(count > 0, "Empty page");
    let key = s.path.as_os_str().as_encoded_bytes();
    let cached = cache.and_then(|c| {
        c.0.query_row(
            "SELECT data FROM wave_pages_v1 WHERE path=? AND page=? AND bytes=? AND modified=? AND length(data)=?",
            params![key,number as i64,s.stamp.bytes,s.stamp.modified,(count*RECORD) as i64],
            |r| r.get::<_,Vec<u8>>(0)
        ).ok()
    });
    if let Some(bytes) = cached
        && let Some(page) = bytes
            .as_chunks::<RECORD>()
            .0
            .iter()
            .map(Measure::read)
            .collect::<Option<Vec<_>>>()
    {
        if let Some(d) = crate::diagnostics::get() {
            d.page_hits.fetch_add(1, Ordering::Relaxed);
        }
        return Ok(page);
    }
    ensure!(current(), "Analysis cancelled");
    if reader
        .as_ref()
        .is_none_or(|r| r.path != s.path || r.stamp != s.stamp)
    {
        *reader = Some(Reader::open(s)?);
    }
    let page = reader.as_mut().unwrap().page(s, first, count, current)?;
    ensure!(
        Stamp::read(&s.path).as_ref() == Some(&s.stamp),
        "Recording changed during analysis"
    );
    if let Some(d) = crate::diagnostics::get() {
        d.page_misses.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(c) = cache {
        let mut bytes = Vec::with_capacity(count * RECORD);
        for m in &page {
            m.bytes(&mut bytes);
        }
        if let Ok(tx) = c.0.unchecked_transaction() {
            let stored = tx.execute(
                "INSERT OR REPLACE INTO wave_pages_v1(path,bytes,modified,page,data) VALUES(?,?,?,?,?)",
                params![key, s.stamp.bytes, s.stamp.modified, number as i64, bytes],
            );
            // Prune in batches. Each full page is 28,800 bytes.
            if stored.is_ok() {
                if tx.last_insert_rowid() % 64 == 0 {
                    let _ = tx.execute(
                        "DELETE FROM wave_pages_v1 WHERE rowid <= (SELECT rowid FROM wave_pages_v1 ORDER BY rowid DESC LIMIT 1 OFFSET 4096)",
                        [],
                    );
                }
                let _ = tx.commit();
            }
        }
    }
    Ok(page)
}

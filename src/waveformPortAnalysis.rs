//! Source facts for the Dekzer presentation. Producers may request just
//! geometry leaves and octave cells, or complete layered/overview facts.
use crate::waveform_render::{FACTS, Octave, Page, SourceDetail};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[path = "waveformFilters.rs"]
mod filters;
#[path = "waveformOctave.rs"]
mod octave;
use filters::FallbackSpectrum;
use octave::{OCTAVE_BANDS, OctaveAnalyzer, OctaveDescriptor, SOURCE_HOP};

pub const LEAF: usize = 32;
pub const MAX_LEAF: usize = 65_536;
pub const OVERVIEW_COLUMNS: usize = 4096;
pub const EVIDENCE_FRAMES: usize = 262_144;
pub const DETAIL_SECONDS: f64 = 120.0;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Spectrum {
    #[serde(rename = "stroke-hann512")]
    Stroke,
    #[serde(rename = "blackman512")]
    Blackman,
}

impl Spectrum {
    pub fn available(rate: u32, stroke: bool) -> Option<Self> {
        OctaveAnalyzer::supports_rate(rate, stroke).then_some(if stroke {
            Self::Stroke
        } else {
            Self::Blackman
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub start_frame: usize,
    pub end_frame: usize,
    pub frames_per_column: usize,
    pub leaf_frames: Option<usize>,
    pub octave: Option<Spectrum>,
    pub overview: bool,
    #[serde(default)]
    pub fallback_energy: bool,
    #[serde(default)]
    pub preview: bool,
}

impl Plan {
    pub fn overview(rate: u32, frames: usize, columns: usize) -> Self {
        Self {
            start_frame: 0,
            end_frame: frames,
            frames_per_column: frames.div_ceil(columns.max(1)).max(1),
            leaf_frames: None,
            octave: Spectrum::available(rate, false),
            overview: true,
            fallback_energy: !OctaveAnalyzer::supports_rate(rate, false),
            preview: false,
        }
    }

    pub fn preview(self) -> Self {
        Self {
            octave: None,
            fallback_energy: false,
            preview: true,
            ..self
        }
    }

    pub fn measure_columns(self) -> bool {
        self.leaf_frames.is_none() || self.fallback_energy
    }

    pub fn pool_octaves(self) -> bool {
        self.overview || (self.octave == Some(Spectrum::Blackman) && self.frames_per_column > 256)
    }
}

struct Column {
    frames: usize,
    bounds: [f32; 2],
    witnesses: [usize; 2],
    edges: [[f32; 2]; 2],
    power: f64,
    band_power: [f64; 3],
}

impl Default for Column {
    fn default() -> Self {
        Self {
            frames: 0,
            bounds: [f32::INFINITY, f32::NEG_INFINITY],
            witnesses: [0; 2],
            edges: [[0.0; 2]; 2],
            power: 0.0,
            band_power: [0.0; 3],
        }
    }
}

impl Column {
    fn observe_geometry(&mut self, f: [f32; 2], bounds: [f32; 2]) {
        if bounds[0] < self.bounds[0] {
            self.bounds[0] = bounds[0];
            self.witnesses[0] = self.frames;
        }
        if bounds[1] > self.bounds[1] {
            self.bounds[1] = bounds[1];
            self.witnesses[1] = self.frames;
        }
        if self.frames == 0 {
            self.edges[0] = bounds;
        }
        self.edges[1] = bounds;
        let [left, right] = f.map(f64::from);
        self.power += (left * left + right * right) * 0.5;
    }

    fn observe_fallback(&mut self, powers: [f64; 3]) {
        for (total, power) in self.band_power.iter_mut().zip(powers) {
            *total += power;
        }
    }

    fn flush(&mut self, page: &mut Page, geometry: Option<&Page>) {
        if self.frames == 0 {
            return;
        }
        let rms = |energy: f64| (energy / self.frames as f64).sqrt() as f32;
        let index = page.extrema.len() / 2;
        if let Some(geometry) = geometry {
            page.facts
                .extend_from_slice(&geometry.facts[index * FACTS..][..FACTS]);
            page.extrema
                .extend_from_slice(&geometry.extrema[index * 2..][..2]);
        } else {
            page.facts.extend([
                self.bounds[0],
                self.bounds[1],
                rms(self.power),
                self.edges[0][0],
                self.edges[0][1],
                self.edges[1][0],
                self.edges[1][1],
            ]);
            page.extrema.extend(self.witnesses);
        }
        if let Some(energy) = &mut page.fallback_energy {
            energy.push(self.band_power.map(rms));
        }
        *self = Self::default();
    }
}

struct OctaveWork {
    analyzer: OctaveAnalyzer,
    // Whole-source views pool linear power before colour encoding.
    sums: Vec<[f64; OCTAVE_BANDS]>,
    powers: Vec<f64>,
    width: usize,
    start: usize,
}

impl OctaveWork {
    fn push(&mut self, frames: &[[f32; 2]]) -> Result<()> {
        let Self {
            analyzer,
            sums,
            powers,
            width,
            start,
        } = self;
        analyzer.push(frames, |cell| {
            collect_octave(cell, sums, powers, *width, *start)
        })?;
        Ok(())
    }

    fn finish(&mut self, reached_eof: bool) -> Result<()> {
        let Self {
            analyzer,
            sums,
            powers,
            width,
            start,
        } = self;
        ensure!(
            analyzer.is_complete() || reached_eof,
            "Missing octave lookahead"
        );
        analyzer.finish(|cell| collect_octave(cell, sums, powers, *width, *start))?;
        Ok(())
    }
}

fn collect_octave(
    cell: OctaveDescriptor,
    sums: &mut [[f64; OCTAVE_BANDS]],
    powers: &mut Vec<f64>,
    width: usize,
    first: usize,
) {
    if sums.is_empty() {
        powers.extend(cell.energy);
    } else {
        let mut start = (cell.start_frame as usize).saturating_sub(first);
        let end = (cell.end_frame as usize).saturating_sub(first);
        while start < end {
            let i = start / width;
            let until = end.min((i + 1) * width);
            for (sum, power) in sums[i].iter_mut().zip(cell.energy) {
                *sum += power * (until - start) as f64;
            }
            start = until;
        }
    }
}

pub struct Analyzer {
    plan: Plan,
    decode: Range<usize>,
    source_frames: usize,
    cursor: usize,
    page: Page,
    fallback: Option<FallbackSpectrum>,
    geometry: Option<std::sync::Arc<Page>>,
    column: Column,
    octave: Option<OctaveWork>,
}

impl Analyzer {
    pub fn new(rate: u32, origin: usize, source_frames: usize, plan: Plan) -> Result<Self> {
        ensure!(
            plan.start_frame < plan.end_frame
                && plan.end_frame <= source_frames
                && plan.frames_per_column > 0,
            "Invalid source evidence range"
        );
        ensure!(
            !plan.overview || plan.start_frame == 0,
            "Overview must start at segment zero"
        );
        ensure!(
            plan.leaf_frames
                .is_none_or(|w| w.is_power_of_two() && w <= MAX_LEAF),
            "Invalid source leaf width"
        );
        let start = plan.start_frame;
        let frame_count = plan.end_frame - start;
        let width = plan.frames_per_column;
        let columns = frame_count.div_ceil(width);
        let measure = plan.measure_columns();
        let decode_start = if plan.fallback_energy || plan.octave == Some(Spectrum::Blackman) {
            start.saturating_sub(rate as usize / 5)
        } else if plan.octave.is_some() {
            (start / SOURCE_HOP as usize * SOURCE_HOP as usize)
                .saturating_sub(octave::CELL_PREROLL as usize)
        } else {
            start
        };
        let decode_end = if plan.octave.is_some() {
            (plan.end_frame + 2048).min(source_frames)
        } else {
            plan.end_frame
        };
        let octave = plan
            .octave
            .map(|variant| -> Result<_> {
                Ok(OctaveWork {
                    analyzer: OctaveAnalyzer::new(
                        rate,
                        decode_start as u64,
                        start as u64,
                        plan.end_frame as u64,
                        variant == Spectrum::Stroke,
                    )?,
                    sums: if plan.pool_octaves() {
                        vec![[0.0; OCTAVE_BANDS]; columns]
                    } else {
                        vec![]
                    },
                    powers: Vec::new(),
                    width,
                    start,
                })
            })
            .transpose()?;
        let detail = plan.leaf_frames.map(|leaf_frames| SourceDetail {
            first_cell_frame: (origin + start) / leaf_frames * leaf_frames,
            start_frame: origin + start,
            frame_count,
            leaf_frames,
            bounds: Vec::with_capacity((frame_count.div_ceil(leaf_frames) + 1) * 2),
            offsets: Vec::with_capacity(frame_count.div_ceil(leaf_frames) + 1),
        });
        Ok(Self {
            decode: decode_start..decode_end,
            source_frames,
            plan,
            cursor: decode_start,
            octave,
            fallback: plan.fallback_energy.then(|| FallbackSpectrum::new(rate)),
            geometry: None,
            column: Column::default(),
            page: Page {
                start_frame: start,
                frames_per_column: width,
                frame_count,
                preview: plan.preview,
                fallback_energy: plan.fallback_energy.then(|| Vec::with_capacity(columns)),
                source_detail: detail,
                facts: if measure {
                    Vec::with_capacity(columns * FACTS)
                } else {
                    vec![]
                },
                extrema: if measure {
                    Vec::with_capacity(columns * 2)
                } else {
                    vec![]
                },
                ..Default::default()
            },
        })
    }

    pub fn decode_range(&self) -> Range<usize> {
        self.decode.clone()
    }

    pub fn reuse_geometry(&mut self, geometry: std::sync::Arc<Page>) -> Result<()> {
        ensure!(
            self.plan.overview && geometry.matches(&self.plan.preview(), 0),
            "overview geometry must cover the entire measurement"
        );
        self.geometry = Some(geometry);
        Ok(())
    }

    pub fn overview_prefix(&self) -> Option<Page> {
        let width = self.page.frames_per_column;
        let count = self.page.extrema.len() / 2;
        (count > 0 && self.plan.preview).then(|| Page {
            start_frame: 0,
            frames_per_column: width,
            frame_count: count * width,
            preview: self.plan.preview,
            facts: self.page.facts[..count * FACTS].to_vec(),
            extrema: self.page.extrema[..count * 2].to_vec(),
            ..Default::default()
        })
    }

    pub fn push(&mut self, frames: &[[f32; 2]]) -> Result<()> {
        ensure!(
            frames.len() <= self.decode.end - self.cursor,
            "Source evidence range exceeded"
        );
        if let Some(octave) = &mut self.octave {
            octave.push(frames)?;
        }
        let measure = self.plan.measure_columns();
        for &frame in frames {
            ensure!(
                frame[0].is_finite() && frame[1].is_finite(),
                "Nonfinite source sample"
            );
            let at = self.cursor;
            self.cursor += 1;
            if at >= self.plan.end_frame {
                continue;
            }
            let filtered = self.fallback.as_mut().map(|spectrum| spectrum.push(frame));
            if at < self.plan.start_frame {
                continue;
            }
            let bounds = [frame[0].min(frame[1]), frame[0].max(frame[1])];
            if let Some(ref mut leaves) = self.page.source_detail {
                let position = leaves.start_frame + at - self.plan.start_frame;
                push_leaf(leaves, bounds, position);
            }
            if measure {
                if self.geometry.is_none() {
                    self.column.observe_geometry(frame, bounds);
                }
                self.column.frames += 1;
            }
            if let Some(powers) = filtered {
                let limit = f64::from(f32::MAX);
                ensure!(
                    powers.iter().all(|p| *p <= limit * limit),
                    "Waveform spectral energy exceeds the finite summary range"
                );
                self.column.observe_fallback(powers);
            }
            if self.column.frames == self.plan.frames_per_column {
                self.column.flush(&mut self.page, self.geometry.as_deref());
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Page> {
        ensure!(
            self.cursor == self.decode.end,
            "Source ended before the evidence range"
        );
        self.column.flush(&mut self.page, self.geometry.as_deref());
        if let Some(mut octave) = self.octave {
            octave.finish(self.cursor == self.source_frames)?;
            if self.plan.pool_octaves() {
                let width = self.page.frames_per_column;
                self.page.spectrum = Some(
                    octave
                        .sums
                        .into_iter()
                        .enumerate()
                        .flat_map(|(i, row)| {
                            let frames = width.min(self.page.frame_count - i * width) as f64;
                            row.map(|power| power / frames)
                        })
                        .collect(),
                );
            } else {
                self.page.octave = Some(Octave {
                    first_cell_frame: self.page.start_frame / SOURCE_HOP as usize
                        * SOURCE_HOP as usize,
                    analysis: (self.plan.octave == Some(Spectrum::Stroke))
                        .then(|| "stroke-hann512".into()),
                    powers: octave.powers,
                });
            }
        }
        Ok(self.page)
    }
}

fn push_leaf(leaves: &mut SourceDetail, bounds: [f32; 2], position: usize) {
    let index = (position - leaves.first_cell_frame) / leaves.leaf_frames;
    let offset = (position % leaves.leaf_frames) as u32;
    let bits = leaves.offset_bits();
    let mask = (1 << bits) - 1;
    if index == leaves.offsets.len() {
        leaves.bounds.extend(bounds);
        leaves.offsets.push(offset | (offset << bits));
    } else {
        if bounds[0] < leaves.bounds[index * 2] {
            leaves.bounds[index * 2] = bounds[0];
            leaves.offsets[index] = (leaves.offsets[index] & !mask) | offset;
        }
        if bounds[1] > leaves.bounds[index * 2 + 1] {
            leaves.bounds[index * 2 + 1] = bounds[1];
            leaves.offsets[index] = (leaves.offsets[index] & mask) | (offset << bits);
        }
    }
}

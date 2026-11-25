//! Source evidence and display projection shared with the visual reference.
use serde::{Deserialize, Serialize};

#[cfg(windows)]
#[path = "waveformGpu.rs"]
pub(crate) mod gpu;
#[path = "waveformStroke.rs"]
mod stroke;

pub const FACTS: usize = 7;
pub const SUPPORTED_OCTAVE_RATES: [u32; 14] = [
    8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 176400,
    192000,
];
const OCTAVE_PALETTE: [[f64; 3]; 8] = [
    [149.47552508172873, 0.0, 0.0],
    [92.22871688887841, 0.0, 0.0],
    [67.9686850355117, 92.47310906347666, 0.0],
    [0.0, 160.54369951591167, 0.0],
    [0.0, 100.0, 48.0],
    [0.0, 0.0, 96.0],
    [0.0, 0.0, 144.0],
    [0.0, 0.0, 192.0],
];

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub start_frame: usize,
    pub frames_per_column: usize,
    pub frame_count: usize,
    #[serde(default)]
    pub preview: bool,
    pub facts: Vec<f32>,
    pub extrema: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_energy: Option<Vec<[f32; 3]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_detail: Option<SourceDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub octave: Option<Octave>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<Vec<f64>>,
}

impl Page {
    pub fn end(&self) -> usize {
        self.start_frame + self.frame_count
    }

    fn fact(&self, column: usize) -> &[f32; FACTS] {
        self.facts[column * FACTS..][..FACTS].try_into().unwrap()
    }

    fn leaf_width(&self) -> usize {
        self.source_detail
            .as_ref()
            .map_or(self.frames_per_column, |detail| detail.leaf_frames)
    }

    pub fn bytes(&self) -> usize {
        std::mem::size_of_val(self.facts.as_slice())
            + std::mem::size_of_val(self.extrema.as_slice())
            + self
                .fallback_energy
                .as_ref()
                .map_or(0, |v| std::mem::size_of_val(v.as_slice()))
            + self
                .spectrum
                .as_ref()
                .map_or(0, |v| std::mem::size_of_val(v.as_slice()))
            + self
                .octave
                .as_ref()
                .map_or(0, |o| std::mem::size_of_val(o.powers.as_slice()))
            + self.source_detail.as_ref().map_or(0, |d| {
                std::mem::size_of_val(d.bounds.as_slice())
                    + std::mem::size_of_val(d.offsets.as_slice())
            })
    }

    pub fn matches(&self, plan: &crate::waveform_port_analysis::Plan, origin: usize) -> bool {
        let frames = plan.end_frame - plan.start_frame;
        if self.start_frame != plan.start_frame
            || self.frame_count != frames
            || self.frames_per_column != plan.frames_per_column
            || self.preview != plan.preview
        {
            return false;
        }
        let width = self.frames_per_column;
        let columns = if plan.measure_columns() {
            frames.div_ceil(width)
        } else {
            0
        };
        if self.facts.len() != columns * FACTS
            || self.extrema.len() != columns * 2
            || self.fallback_energy.as_ref().map(Vec::len)
                != plan.fallback_energy.then_some(columns)
        {
            return false;
        }
        let leaves = match (plan.leaf_frames, &self.source_detail) {
            (None, None) => true,
            (Some(width), Some(detail)) => {
                let start = origin + self.start_frame;
                let base = start / width * width;
                let count = (start + frames - base).div_ceil(width);
                detail.start_frame == start
                    && detail.first_cell_frame == base
                    && detail.leaf_frames == width
                    && detail.frame_count == frames
                    && detail.offsets.len() == count
                    && detail.bounds.len() == count * 2
            }
            _ => false,
        };
        let base = self.start_frame / 256 * 256;
        let count = (plan.end_frame - base).div_ceil(256);
        let octave = if plan.octave.is_none() {
            self.spectrum.is_none() && self.octave.is_none()
        } else if plan.overview {
            self.spectrum
                .as_ref()
                .is_some_and(|s| s.len() == columns * 8)
                && self.octave.is_none()
        } else {
            self.octave
                .as_ref()
                .is_some_and(|o| o.first_cell_frame == base && o.powers.len() == count * 8)
                && self.spectrum.is_none()
        };
        leaves && octave
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDetail {
    pub first_cell_frame: usize,
    pub start_frame: usize,
    pub frame_count: usize,
    pub leaf_frames: usize,
    pub bounds: Vec<f32>,
    pub offsets: Vec<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Octave {
    pub first_cell_frame: usize,
    pub analysis: Option<String>,
    pub powers: Vec<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Style {
    pub gain: f64,
    pub intensity: f64,
    pub light: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            gain: 1.0,
            intensity: 1.0,
            light: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Detail,
    Overview,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub role: Role,
    pub style: Style,
    pub rate: u32,
    pub origin: usize,
    pub frame_count: usize,
    pub first: i64,
    pub pixels_per_second: f64,
    pub width: usize,
    pub height: usize,
}

impl View {
    fn y(&self, amplitude: f64) -> f64 {
        let height = self.height as f64;
        height / 2.0 - (amplitude * self.style.gain).clamp(-1.0, 1.0) * (height * 0.46)
    }

    #[cfg(any(test, feature = "qualification"))]
    fn core(&self, lo: f32, hi: f32) -> (f64, f64) {
        let (top, bottom) = (self.y(f64::from(hi)), self.y(f64::from(lo)));
        if bottom - top < 2.0 {
            let mid = (top + bottom) / 2.0;
            (mid - 1.0, mid + 1.0)
        } else {
            (top, bottom)
        }
    }

    fn background(&self) -> [f32; 3] {
        if self.style.light {
            [240., 243., 246.]
        } else {
            [0.; 3]
        }
    }

    pub fn required_range(&self) -> std::ops::Range<usize> {
        let partition = stroke::Partition::new(self.rate, self.pixels_per_second);
        let last_pixel = self.first + self.width as i64;
        let start = partition.start(partition.cell(self.first) - 1);
        let end = partition.start(partition.cell(last_pixel - 1) + 2);
        let (colour_from, colour_to) = match (
            partition.colour_frame(self.first),
            partition.colour_frame(last_pixel),
        ) {
            (Some(first), Some(last)) => (
                ((first / 256.0).floor() as i64 - 2) * 256,
                ((last / 256.0).floor() as i64 + 4) * 256,
            ),
            _ => {
                let rate = f64::from(self.rate);
                let step = octave_step(rate, self.pixels_per_second);
                let position = |x: i64| {
                    ((x as f64 * rate / self.pixels_per_second - rate * 0.001) / step).floor()
                };
                (
                    ((position(self.first) - 1.0) * step).floor() as i64,
                    ((position(last_pixel) + 2.0) * step).ceil() as i64,
                )
            }
        };
        let clip = |v: i64| (v.max(0) as usize).min(self.frame_count);
        clip(start.min(colour_from))..clip(end.max(colour_to))
    }
}

#[cfg(any(test, feature = "qualification"))]
#[derive(Serialize)]
pub enum Geometry {
    Stroke(stroke::Columns),
    Overview,
}

#[cfg(any(test, feature = "qualification"))]
pub fn render(pages: &[&Page], view: &View) -> (Vec<u8>, Geometry) {
    let [r, g, b] = view.background().map(|v| v as u8);
    let mut rgba = [r, g, b, 255].repeat(view.width * view.height);
    let geometry = if view.role == Role::Overview {
        if let Some(page) = pages.first() {
            overview(page, view, &mut rgba);
        }
        Geometry::Overview
    } else {
        let columns = stroke::columns(pages, view);
        stroke::raster(&columns.columns, view, &mut rgba);
        Geometry::Stroke(columns)
    };
    (rgba, geometry)
}

#[derive(Clone, Copy, Default)]
struct OverviewColumn {
    start: f64,
    end: f64,
    min: f64,
    max: f64,
    rms: f64,
    fallback: [f64; 3],
}

type Rect = [f64; 4];

const COVERAGE_SAMPLES: [[f64; 2]; 4] = [
    [0.375, 0.125],
    [0.875, 0.375],
    [0.125, 0.625],
    [0.625, 0.875],
];

fn snapped_path(intervals: &[Rect]) -> (bool, impl Iterator<Item = Rect> + '_) {
    // Skia Ganesh routes small paths through an independently resolved A8
    // mask. Large paths are composited on the multisampled destination.
    let body = || {
        intervals
            .iter()
            .copied()
            .filter(|r| r[0] < r[1] && r[2] < r[3])
    };
    let bounds = body().fold(
        [
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ],
        |a, r| {
            [
                a[0].min(r[0] as f32),
                a[1].max(r[1] as f32),
                a[2].min(r[2] as f32),
                a[3].max(r[3] as f32),
            ]
        },
    );
    let width = (bounds[1].ceil() - bounds[0].floor()).max(0.0);
    let height = (bounds[3].ceil() - bounds[2].floor()).max(0.0);
    let atlas = width.max(height) <= 1024.0 && width * height <= 128.0 * 128.0;
    let extent = (width.max(height) as usize).max(512).next_power_of_two() as f32;
    let snap = move |v: f64| {
        let v = v as f32;
        // Preserve mask-space f32 rounding before fixed pixel samples.
        let raster = if atlas {
            let ndc = v * (2.0 / extent) - 1.0;
            ndc * (extent / 2.0) + extent / 2.0
        } else {
            v
        };
        f64::from((raster * 16.0).round_ties_even()) / 16.0
    };
    (atlas, body().map(move |interval| interval.map(snap)))
}

#[cfg(any(test, feature = "qualification"))]
fn raster_body(mask: &mut [[u8; 3]], band: usize, view: &View, intervals: &[Rect]) -> bool {
    let (atlas, body) = snapped_path(intervals);
    for [x0, x1, y0, y1] in body {
        for x in (x0.floor().max(0.0) as usize)..(x1.ceil() as usize).min(view.width) {
            for y in (y0.floor().max(0.0) as usize)..(y1.ceil() as usize).min(view.height) {
                for (i, [dx, dy]) in COVERAGE_SAMPLES.into_iter().enumerate() {
                    if (x0..x1).contains(&(x as f64 + dx)) && (y0..y1).contains(&(y as f64 + dy)) {
                        mask[y * view.width + x][band] |= 1 << i;
                    }
                }
            }
        }
    }
    atlas
}

#[cfg(any(test, feature = "qualification"))]
fn blend(rgba: &mut [u8], mask: &[f32], opacity: f64, mut colour: impl FnMut(usize) -> [f64; 3]) {
    for (i, (rgba, &mask)) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(mask).enumerate() {
        let alpha = f64::from(mask.min(1.0)) * opacity;
        if alpha == 0.0 {
            continue;
        }
        for (c, v) in rgba.iter_mut().zip(colour(i)) {
            *c = (f64::from(*c) * (1.0 - alpha) + v * alpha).round() as u8;
        }
    }
}

fn overview_columns(page: &Page, view: &View) -> Vec<OverviewColumn> {
    if page.frame_count == 0 || page.facts.is_empty() {
        return vec![];
    }
    let source_columns = page.facts.len() / FACTS;
    let span = view.width as f64 * f64::from(view.rate) / view.pixels_per_second;
    let group = (span.round() as usize)
        .div_ceil(page.frames_per_column)
        .div_ceil((view.width / 3).max(1))
        .max(1);
    let x = |frame: usize| {
        frame as f64 * view.pixels_per_second / f64::from(view.rate) - view.first as f64
    };
    let mut columns = Vec::with_capacity(source_columns.div_ceil(group));
    for first in (0..source_columns).step_by(group) {
        let last = (first + group).min(source_columns);
        let frame = |i: usize| (i * page.frames_per_column).min(page.frame_count);
        let mut p = OverviewColumn {
            start: x(frame(first)),
            end: x(frame(last)),
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            ..Default::default()
        };
        for i in first..last {
            let n = (frame(i + 1) - frame(i)) as f64;
            let f = page.fact(i).map(f64::from);
            p.min = p.min.min(f[0]);
            p.max = p.max.max(f[1]);
            p.rms += f[2].powi(2) * n;
            if let Some(energy) = &page.fallback_energy {
                for (total, power) in p.fallback.iter_mut().zip(energy[i]) {
                    *total += f64::from(power).powi(2) * n;
                }
            }
        }
        let frames = (frame(last) - frame(first)) as f64;
        p.rms = f64::from((p.rms / frames).sqrt() as f32);
        p.fallback = p.fallback.map(|v| f64::from((v / frames).sqrt() as f32));
        columns.push(p);
    }
    columns
}

fn overview_colours(page: &Page, columns: &[OverviewColumn], view: &View) -> Vec<[f64; 3]> {
    if page.preview {
        return vec![[150.0, 168.0, 180.0]; view.width];
    }
    let mut octave = octave_nodes(&[page], view, true);
    (0..view.width)
        .map(|x| {
            octave(view.first + x as i64)
                .map(|rgb| rgb.map(f64::round_ties_even))
                .unwrap_or_else(|| {
                    let (a, b) = (x as f64, (x + 1) as f64);
                    let mut energy = [0.0; 3];
                    let first = columns.partition_point(|column| column.end <= a);
                    for p in columns[first..].iter().take_while(|p| p.start < b) {
                        let weight = b.min(p.end) - a.max(p.start);
                        for (sum, rms) in energy.iter_mut().zip(p.fallback) {
                            *sum += rms * rms * weight;
                        }
                    }
                    band_colour(energy.map(f64::sqrt), &view.style)
                })
        })
        .collect()
}

fn overview_paths(columns: &[OverviewColumn], view: &View) -> [Vec<Rect>; 2] {
    let mut peak = Vec::with_capacity(columns.len());
    let mut body = Vec::with_capacity(columns.len());
    for p in columns {
        let (lo, hi) = (p.min.min(0.0), p.max.max(0.0));
        let rms = p.rms * std::f64::consts::SQRT_2;
        peak.push([p.start, p.end, view.y(hi), view.y(lo)]);
        body.push([p.start, p.end, view.y(hi.min(rms)), view.y(lo.max(-rms))]);
    }
    [peak, body]
}

#[cfg(any(test, feature = "qualification"))]
fn overview(page: &Page, view: &View, pixels: &mut [u8]) {
    let columns = overview_columns(page, view);
    let colours = overview_colours(page, &columns, view);
    let [peak, body] = overview_paths(&columns, view);
    let mut masks = vec![[0; 3]; view.width * view.height];
    let atlas_peak = raster_body(&mut masks, 0, view, &peak);
    let atlas_body = raster_body(&mut masks, 1, view, &body);
    let coverage: Vec<_> = masks
        .iter()
        .map(|[peak, body, _]| {
            let count = |m: u8| m.count_ones() as f32 / 4.0;
            if atlas_peak || atlas_body {
                let body = count(*body);
                body + 0.65 * count(*peak) * (1.0 - body)
            } else {
                count(*body) + 0.65 * count(*peak & !*body)
            }
        })
        .collect();
    blend(pixels, &coverage, 1.0, |i| colours[i % view.width]);
}

fn octave_display(rgb: [f64; 3], style: &Style) -> [f64; 3] {
    let contrast = if style.light {
        let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        (105.0 / luminance.max(1.0)).min(0.72)
    } else {
        1.0
    };
    let neutral = if style.light { 90.0 } else { 150.0 };
    rgb.map(|v| (neutral + (v * contrast - neutral) * style.intensity).clamp(0.0, 255.0))
}

fn octave_step(rate: f64, pixels_per_second: f64) -> f64 {
    rate / (rate / 256.0).min(32.9399460609982 / 45.0 * pixels_per_second)
}

fn octave_nodes<'a>(
    pages: &[&'a Page],
    view: &'a View,
    overview: bool,
) -> impl FnMut(i64) -> Option<[f64; 3]> + 'a {
    let mut pages = pages.to_vec();
    pages.sort_by_key(|p| p.start_frame);
    let rate = f64::from(view.rate);
    let step = octave_step(rate, view.pixels_per_second);
    let lag = rate * 0.001;
    let last = ((view.frame_count as f64 / step).ceil() as usize).saturating_sub(1);
    let edge = move |n: usize| ((n as f64 * step).floor() as usize).min(view.frame_count);
    let mut pools = std::collections::HashMap::new();
    let mut pool = move |i: usize| {
        if let Some(&powers) = pools.get(&i) {
            return Some(powers);
        }
        let (mut at, end) = (edge(i), edge(i + 1));
        let length = (end - at) as f64;
        let mut result = [0.0; 8];
        if length == 0.0 {
            return None;
        }
        while at < end {
            let p = pages.iter().find(|p| {
                p.start_frame <= at
                    && p.end() > at
                    && (p.spectrum.is_some()
                        || p.octave.as_ref().is_some_and(|o| o.analysis.is_none()))
            })?;
            let (base, width, powers) = if let Some(spectrum) = &p.spectrum {
                (p.start_frame, p.frames_per_column, spectrum)
            } else {
                let octave = p.octave.as_ref()?;
                (octave.first_cell_frame, 256, &octave.powers)
            };
            let cell = (at - base) / width;
            let until = end.min(p.end()).min(base + (cell + 1) * width);
            let weight = (until - at) as f64 / length;
            for (sum, power) in result.iter_mut().zip(powers.get(cell * 8..cell * 8 + 8)?) {
                *sum += power * weight;
            }
            at = until;
        }
        pools.insert(i, result);
        Some(result)
    };
    move |x| {
        let time = ((x as f64 + 0.5) * rate) / view.pixels_per_second - lag;
        let mut left = last.min((time / step).floor().max(1.0) as usize - 1);
        if left < last && edge(left + 2) as f64 <= time {
            left += 1;
        }
        let right = last.min(left + 1);
        let a = pool(left)?;
        let b = pool(right)?;
        let (first, end) = (edge(left + 1), edge(right + 1));
        let mix = if end > first {
            ((time - first as f64) / (end - first) as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let k = 2.8606101989383417_f64.exp();
        let mut rgb = [0.0; 3];
        for (band, weights) in OCTAVE_PALETTE.into_iter().enumerate() {
            let power = a[band] + (b[band] - a[band]) * mix;
            let value = (k * power.max(0.0).sqrt()).ln_1p() / k.ln_1p();
            for (c, weight) in rgb.iter_mut().zip(weights) {
                *c += value * weight;
            }
        }
        let floor = if overview { 0.0 } else { 40.0 };
        rgb = rgb.map(|v| v.clamp(floor, 255.0));
        if rgb == [0.0; 3] {
            rgb = [40.0; 3];
        }
        Some(octave_display(rgb, &view.style))
    }
}

fn band_colour(energy: [f64; 3], style: &Style) -> [f64; 3] {
    let strength = energy[0].max(energy[1]).max(energy[2]).max(1e-20);
    let [r, g, b] = energy.map(|v| (v / strength).powf(0.75));
    let rgb = [
        30.0 + 225.0 * r,
        30.0 + 210.0 * g + 25.0 * b,
        36.0 + 219.0 * b,
    ];
    let neutral = if style.light { 90.0 } else { 150.0 };
    let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let contrast = if style.light {
        (105.0 / luminance).min(0.72)
    } else {
        1.0
    };
    let brightness = if style.light {
        1.0
    } else {
        (0.22 + 0.78 * (strength / 0.16).sqrt()).min(1.0)
    };
    rgb.map(|c| {
        ((neutral + (c * contrast - neutral) * style.intensity) * brightness)
            .clamp(0.0, 255.0)
            .round()
    })
}

fn fallback_colour<'a>(
    pages: &[&'a Page],
    view: &'a View,
) -> impl FnMut(i64) -> Option<[f64; 3]> + 'a {
    let mut octave = octave_nodes(pages, view, false);
    let pages = {
        let mut p = pages.to_vec();
        p.retain(|p| p.fallback_energy.is_some());
        p.sort_by_key(|p| (p.frames_per_column, p.start_frame));
        p
    };
    move |x| {
        if SUPPORTED_OCTAVE_RATES.contains(&view.rate) {
            return octave(x);
        }
        let fpp = f64::from(view.rate) / view.pixels_per_second;
        let start = x as f64 * fpp;
        let end = ((x + 1) as f64 * fpp).min(view.frame_count as f64);
        let mut at = start;
        let mut energy = [0.0; 3];
        while at < end {
            let p = pages
                .iter()
                .find(|p| p.start_frame as f64 <= at && p.end() as f64 > at)?;
            let i = (at as usize - p.start_frame) / p.frames_per_column;
            let until = end.min(p.end().min(p.start_frame + (i + 1) * p.frames_per_column) as f64);
            let fraction = (until - at) / (end - start);
            for (s, &rms) in energy.iter_mut().zip(p.fallback_energy.as_ref()?.get(i)?) {
                *s += f64::from(rms).powi(2) * fraction;
            }
            at = until;
        }
        Some(band_colour(energy.map(f64::sqrt), &view.style))
    }
}

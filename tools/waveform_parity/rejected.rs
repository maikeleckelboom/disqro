//! Source replay of PR #23. The runner inserts unmodified reviewed analysis,
//! projection and draw routines. The sink composites their GPUI quads to PNG.
#![allow(dead_code, unused_imports, non_snake_case)]
extern crate self as gpui_kit;
extern crate self as kira;

mod diagnostics {
    use std::sync::atomic::AtomicUsize;
    pub struct Diagnostics { pub paint_us: AtomicUsize, pub frames: AtomicUsize }
    pub fn get() -> Option<Diagnostics> { None }
    pub struct Timer;
    impl Timer { pub fn measure(_: impl Fn(&Diagnostics) -> &AtomicUsize) -> Self { Self } }
}

pub mod prelude {}

#[derive(Clone, Copy, Default)]
pub struct Rgba { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }
type Hsla = Rgba;
impl Rgba {
    fn opacity(mut self, opacity: f32) -> Self { self.a *= opacity; self }
}
fn rgb(rgb: u32) -> Rgba {
    let [_, r, g, b] = rgb.to_be_bytes();
    Rgba { r: f32::from(r) / 255.0, g: f32::from(g) / 255.0, b: f32::from(b) / 255.0, a: 1.0 }
}
mod theme {
    use super::Hsla;
    #[derive(Clone, Copy)]
    pub struct Wave {
        pub played_peak: Hsla, pub played_body: Hsla, pub ahead_peak: Hsla, pub ahead_body: Hsla,
    }
}
#[derive(Clone, Copy)]
struct Point { x: f32, y: f32 }
impl std::ops::Add for Point {
    type Output = Self;
    fn add(self, other: Self) -> Self { point(self.x + other.x, self.y + other.y) }
}
#[derive(Clone, Copy)]
struct Size { width: f32, height: f32 }
#[derive(Clone, Copy)]
struct Bounds { origin: Point, size: Size }
impl Bounds {
    fn new(origin: Point, size: Size) -> Self { Self { origin, size } }
}
fn px(x: f32) -> f32 { x }
fn point(x: f32, y: f32) -> Point { Point { x, y } }
fn size(width: f32, height: f32) -> Size { Size { width, height } }
fn fill(bounds: Bounds, color: Hsla) -> (Bounds, Hsla) { (bounds, color) }

struct Window { pixels: Vec<u8>, width: usize, height: usize }
impl Window {
    fn scale_factor(&self) -> f32 { 1.0 }
    fn paint_quad(&mut self, (bounds, color): (Bounds, Hsla)) {
        let x0 = bounds.origin.x;
        let y0 = bounds.origin.y;
        let x1 = x0 + bounds.size.width;
        let y1 = y0 + bounds.size.height;
        for y in (y0.floor() as usize)..(y1.ceil() as usize).min(self.height) {
            let y = y as f32;
            let vertical = (y1.min(y + 1.0) - y0.max(y)).max(0.0);
            for x in (x0.floor() as usize)..(x1.ceil() as usize).min(self.width) {
                let horizontal = (x1.min(x as f32 + 1.0) - x0.max(x as f32)).max(0.0);
                let alpha = color.a * vertical * horizontal;
                let at = (y as usize * self.width + x) * 4;
                for (channel, v) in self.pixels[at..at + 3].iter_mut().zip([color.r, color.g, color.b]) {
                    *channel = (f32::from(*channel) * (1.0 - alpha) + v * 255.0 * alpha).round() as u8;
                }
            }
        }
    }
}
trait IntoElement: Sized {
    fn size_full(self) -> Self { self }
    fn draw(self, window: &mut Window);
}
impl<F: FnOnce(Bounds, (), &mut Window, ())> IntoElement for F {
    fn draw(self, window: &mut Window) {
        self(Bounds::new(point(0.0, 0.0), size(window.width as f32, window.height as f32)),
            (), window, ());
    }
}
fn canvas<F: FnOnce(Bounds, (), &mut Window, ())>(_: impl FnOnce((), (), ()), paint: F) -> F { paint }

#[derive(Clone, Copy)]
pub struct Frame { pub left: f32, pub right: f32 }
mod waveform;
mod waveformBands;
mod overview;
use waveform::{Measure, ceil_cell, edge, PAGE};
use overview::{Bin, Style, Viewport};
use std::{
    env, fs::File, io::{BufReader, Read, Seek, SeekFrom}, ops::Range, path::{Path, PathBuf},
    sync::Arc,
};
use serde::Deserialize;

struct Source { rate: u32 }
struct Segment { source: Source, frames: Range<usize> }
include!("projection.rs");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    id: String, rate: u32, width: usize, height: usize, first: usize, pixels_per_second: f64,
    origin: usize, frame_count: usize, role: String, style: Mode,
}
#[derive(Deserialize)]
struct Mode { mode: String }
#[derive(Deserialize)]
struct Run { cases: Vec<Case> }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let run: Run = serde_json::from_reader(File::open(&args[0])?)?;
    let mut pcm = BufReader::new(File::open(&args[1])?);
    let out = PathBuf::from(&args[2]);
    let first = &run.cases[0];
    let end = first.origin + first.frame_count;
    let grid_first = ceil_cell(first.origin, first.rate);
    let cells = measure(&mut pcm, first.rate, grid_first, end)?;
    let segment = Segment { source: Source { rate: first.rate }, frames: first.origin..end };
    let mut projection = Projection::new(cells.len(), 1536);
    for (i, m) in cells.iter().enumerate() {
        let cell = grid_first + i;
        let count = edge(cell + 1, first.rate).min(end) - edge(cell, first.rate);
        projection.add(i, m, count);
    }
    let overview = projection.bins(&segment, grid_first);
    for case in run.cases {
        draw(&case, &out, &overview, &cells, grid_first)?;
    }
    Ok(())
}

fn measure(pcm: &mut BufReader<File>, rate: u32, first: usize, end: usize) -> std::io::Result<Vec<Measure>> {
    let last = ceil_cell(end, rate);
    let mut cells = Vec::with_capacity(last - first);
    let mut scratch = [0; 8];
    for page in first / PAGE..=(last - 1) / PAGE {
        let start = edge(page * PAGE, rate);
        let preroll = start.saturating_sub(rate as usize / 10);
        pcm.seek(SeekFrom::Start((preroll * 8) as u64))?;
        let mut bands = waveformBands::Bands::new(rate);
        let mut frames = preroll;
        let mut read = |frame: usize| {
            pcm.read_exact(&mut scratch)?;
            let f = |base| f32::from_le_bytes(scratch[base..base + 4].try_into().unwrap());
            let f = Frame { left: f(0), right: f(4) };
            Ok::<_, std::io::Error>((frame, f, bands.power(f)))
        };
        for frame in preroll..start { read(frame)?; }
        frames = frames.max(start);
        for cell in page * PAGE..((page + 1) * PAGE).min(last) {
            let mut m = Measure::default();
            let stop = edge(cell + 1, rate).min(end);
            for frame in frames..stop {
                let (at, f, power) = read(frame)?;
                m.add(at, f, power);
            }
            frames = stop;
            if cell >= first { cells.push(m); }
        }
    }
    Ok(cells)
}

fn draw(case: &Case, out: &Path, overview: &Arc<[Bin]>, cells: &[Measure], grid_first: usize)
    -> Result<(), Box<dyn std::error::Error>> {
    let end = case.origin + case.frame_count;
    let view = Viewport {
        first: case.first as f64 * f64::from(case.rate) / case.pixels_per_second,
        span: case.width as f64 * f64::from(case.rate) / case.pixels_per_second,
    };
    let detail = case.role == "detail" && view.span <= f64::from(case.rate) * 24.0;
    let (first_frame, bins) = if detail {
        let from = (ceil_cell(case.origin + view.first.floor() as usize + 1, case.rate) - 1).max(grid_first);
        let to = ceil_cell((case.origin + view.frame_at(1.).ceil() as usize).min(end), case.rate);
        let mut bins = Vec::with_capacity(to - from);
        for cell in from..to {
            let m = &cells[cell - grid_first];
            let start = edge(cell, case.rate);
            let stop = edge(cell + 1, case.rate).min(end);
            let n = (stop - start) as f64;
            let [(low, low_at), (high, high_at)] = m.extrema();
            bins.push(Bin { end_frame: stop - case.origin, low, high,
                low_at: low_at.saturating_sub(case.origin), high_at: high_at.saturating_sub(case.origin),
                mean_square: (m.energy / n) as f32, band_rms: m.bands.map(|b| (b / n).sqrt() as f32),
            });
        }
        (edge(from, case.rate).saturating_sub(case.origin), bins.into())
    } else { (0, overview.clone()) };
    let wave = theme::Wave { played_peak: rgb(0x91afcb).opacity(0.5), played_body: rgb(0xa9c3db),
        ahead_peak: rgb(0x34353b), ahead_body: rgb(0x4d4f57) };
    let style = if case.style.mode == "stroke" { Style::Spectral } else { Style::Bands };
    let mut window = Window { pixels: [0, 0, 0, 255].repeat(case.width * case.height),
        width: case.width, height: case.height };
    overview::paint_range(bins, first_frame, view, view.frame_at(0.5), style, wave).draw(&mut window);
    image::save_buffer(out.join(format!("{}.png", case.id)), &window.pixels,
        case.width as u32, case.height as u32, image::ColorType::Rgba8)?;
    Ok(())
}

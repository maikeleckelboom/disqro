//! Dekzer Stroke, from waveformStroke.ts, waveformSourceCells.ts and waveformDirect.ts.
//! The partition and coverage are display laws. They never feed a seek coordinate.
use super::{Page, View};
use serde::Serialize;

const HOP: f64 = 256.0;
const LOG_K: f64 = 2.8606101989383417;
const COEFFICIENTS: [[f64; 3]; 24] = [
    [30.995394, -0.90819, -0.263928],
    [19.333413, -3.313851, -1.72278],
    [8.395741, 18.934976, -2.852861],
    [-4.617554, 21.417598, -11.604659],
    [1.173827, 21.35068, 8.702214],
    [3.34275, 2.422165, 26.569969],
    [1.012681, -14.418055, 26.844175],
    [-7.485634, -3.082375, 75.728855],
    [90.542327, 1.774565, -0.279993],
    [56.303194, 0.511826, 1.085057],
    [26.758376, 57.509247, -5.187386],
    [-1.415483, 87.626811, 5.191878],
    [-2.230263, 51.469368, 43.142446],
    [1.362431, 2.420432, 30.67431],
    [-7.49192, 11.576582, 86.072166],
    [-2.14454, -2.154514, 81.434454],
    [33.423589, 2.212453, 1.15457],
    [20.565285, 4.681162, 5.084175],
    [14.580796, 27.770322, 2.834312],
    [4.626897, 35.411006, 10.081777],
    [6.248015, 19.199546, 1.061621],
    [-0.503712, 2.548138, 31.552237],
    [12.776072, -1.836481, 34.303679],
    [-5.345312, 8.354502, 53.149336],
];

const STATES_48K: [[f64; 3]; 4] = [
    [45.0, 1.36986, 0.63],
    [90.0, 1.36985, 0.6],
    [180.0, 1.354208, 0.644],
    [360.0, 1.34637, 0.62],
];
const STATES_44K: [[f64; 3]; 1] = [[180.0, 1.35811, 1.335]];

#[derive(Clone, Copy)]
struct Anchor {
    density: f64,
    cell: f64,
    phase: f64,
    block: f64,
    skip: f64,
    skip_phase: f64,
    pitch: f64,
    screen: f64,
}

impl Anchor {
    fn first_frame(self, cell: i64) -> f64 {
        let owner = |block: f64| {
            (((block * self.skip + self.skip_phase).floor() + self.phase) / self.cell).floor()
        };
        let cell = cell as f64;
        let mut block =
            (((cell * self.cell - self.phase).ceil() - self.skip_phase) / self.skip).ceil();
        while owner(block - 1.0) >= cell {
            block -= 1.0;
        }
        while owner(block) < cell {
            block += 1.0;
        }
        block * self.block
    }
}

/// Display cells are anchored at the playable segment's frame zero, before
/// translating to the recording's absolute sample witnesses.
pub(super) struct Partition {
    anchors: Option<(Anchor, Anchor)>,
    pps: f64,
    rate: f64,
    pitch: f64,
    screen: f64,
    mix: f64,
    block: f64,
}

impl Partition {
    pub(super) fn new(rate: u32, pps: f64) -> Self {
        let (states, block360, skip, skip_phase, screen_delta) = match rate {
            48_000 => (&STATES_48K[..], 133.0, 1.0, 0.0, 0.39),
            44_100 => (&STATES_44K[..], 122.0, 1.001436, 1.02, -0.19),
            _ => (&[][..], 0.0, 0.0, 0.0, 0.0),
        };
        let rate = f64::from(rate);
        if states.is_empty() {
            let pitch = if pps <= 90.0 {
                1.3665
            } else if pps <= 180.0 {
                lerp(1.3665, 1.351, (pps / 90.0).ln() / 2.0f64.ln())
            } else {
                lerp(1.351, 1.343, ((pps / 180.0).ln() / 2.0f64.ln()).min(1.0))
            };
            return Self {
                anchors: None,
                pps,
                rate,
                pitch,
                screen: 0.56,
                mix: 0.0,
                block: 0.0,
            };
        }
        let high = states.iter().position(|state| state[0] >= pps);
        let (lo, hi) = high.map_or((states.len() - 1, states.len() - 1), |i| {
            (i.saturating_sub(1), i)
        });
        let anchor = |[density, cell, phase]: [f64; 3]| {
            let block = block360 * (360.0 / density);
            Anchor {
                density,
                cell,
                phase,
                block,
                skip,
                skip_phase,
                pitch: (cell / skip) * block * density / rate,
                screen: (-(phase + skip_phase) / skip) * block * density / rate + screen_delta,
            }
        };
        let (a, b) = (anchor(states[lo]), anchor(states[hi]));
        let mix = if lo == hi {
            0.0
        } else {
            ((pps / a.density).ln() / (b.density / a.density).ln()).clamp(0.0, 1.0)
        };
        Self {
            anchors: Some((a, b)),
            pps,
            rate,
            mix,
            pitch: lerp(a.pitch, b.pitch, mix),
            screen: lerp(a.screen, b.screen, mix),
            block: block360 * 360.0 / pps,
        }
    }

    pub(super) fn cell(&self, x: i64) -> i64 {
        let x = x as f64;
        if self.anchors.is_none() {
            return ((x + 0.5 - self.screen) / self.pitch).floor() as i64;
        }
        let edge = |k: i64| (self.screen + k as f64 * self.pitch + 0.5).floor();
        let mut cell = ((x - self.screen) / self.pitch).floor() as i64;
        while edge(cell) > x {
            cell -= 1;
        }
        while edge(cell + 1) <= x {
            cell += 1;
        }
        cell
    }

    pub(super) fn start(&self, cell: i64) -> i64 {
        let Some((a, b)) = self.anchors else {
            return (cell as f64 * (self.pitch * self.rate / self.pps)).floor() as i64;
        };
        let first = a.first_frame(cell);
        let last = b.first_frame(cell);
        if self.pps == a.density {
            return first as i64;
        }
        if self.pps == b.density {
            return last as i64;
        }
        (lerp(first * a.density, last * b.density, self.mix) / self.pps).ceil() as i64
    }

    pub(super) fn colour_frame(&self, pixel: i64) -> Option<f64> {
        let (a, b) = self.anchors?;
        let cell = (pixel as f64 + 0.5 - self.screen) / self.pitch;
        let width = self.pitch * self.rate / (self.pps * self.block) * a.skip;
        let phase = lerp(a.phase, b.phase, self.mix);
        Some(self.block * ((cell * width - phase - a.skip_phase) / a.skip + 0.5) - 60.0)
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct Core {
    pub held: bool,
    pub bounds: Option<[f32; 2]>,
    pub witnesses: Option<[usize; 2]>,
}

#[derive(Clone, Copy)]
struct Leaf {
    start: usize,
    end: usize,
    bounds: [f32; 2],
    frames: [usize; 2],
    endpoints: Option<[[f32; 2]; 2]>,
}

/// The finest held tier owns each disjoint source interval. Coarse leaves can
/// bound a cropped interval, but only an in-range witness can shape its core.
pub(super) fn page_slices<'a>(pages: &[&'a Page]) -> Vec<(usize, usize, &'a Page)> {
    let mut pages = pages.to_vec();
    pages.sort_by_key(|p| {
        (
            p.leaf_width(),
            p.frames_per_column,
            p.start_frame,
            std::cmp::Reverse(p.frame_count),
        )
    });
    let mut edges: Vec<_> = pages
        .iter()
        .flat_map(|p| [p.start_frame, p.end()])
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let mut slices: Vec<(usize, usize, &Page)> = Vec::new();
    for edge in edges.windows(2) {
        let [start, end] = [edge[0], edge[1]];
        if let Some(&page) = pages
            .iter()
            .find(|p| p.start_frame <= start && p.end() >= end)
        {
            if let Some((_, previous_end, previous_page)) = slices.last_mut()
                && *previous_end == start
                && std::ptr::eq(*previous_page, page)
            {
                *previous_end = end;
            } else {
                slices.push((start, end, page));
            }
        }
    }
    slices
}

fn leaves(pages: &[&Page], origin: usize, lo: usize, hi: usize) -> Vec<Leaf> {
    let mut result = Vec::new();
    for (start, end, page) in page_slices(pages) {
        let start = origin + start;
        let end = origin + end;
        if start >= hi || end <= lo {
            continue;
        }
        let leaf_width = page.leaf_width();
        let detail = page.source_detail.as_ref();
        let base = detail.map_or(origin + page.start_frame, |detail| detail.first_cell_frame);
        let first = (start.max(lo) - base) / leaf_width;
        let last = (end.min(hi) - base).div_ceil(leaf_width);
        for index in first..last {
            let cell = base + index * leaf_width;
            let (from, to) = (cell.max(start), (cell + leaf_width).min(end));
            let (bounds, frames, endpoints) = if let Some(detail) = detail {
                let offsets = usize::from(detail.offsets[index]);
                (
                    [detail.bounds[index * 2], detail.bounds[index * 2 + 1]],
                    [cell + (offsets & 31), cell + ((offsets >> 5) & 31)],
                    None,
                )
            } else {
                let f = page.fact(index);
                (
                    [f[0], f[1]],
                    [
                        cell + page.extrema[index * 2],
                        cell + page.extrema[index * 2 + 1],
                    ],
                    Some([[f[3], f[4]], [f[5], f[6]]]),
                )
            };
            let endpoints = endpoints.map(|[first, last]| {
                [
                    if from == cell.max(origin + page.start_frame) {
                        first
                    } else {
                        [f32::NAN; 2]
                    },
                    if to == (cell + leaf_width).min(origin + page.end()) {
                        last
                    } else {
                        [f32::NAN; 2]
                    },
                ]
            });
            result.push(Leaf {
                start: from,
                end: to,
                bounds,
                frames,
                endpoints,
            });
        }
    }
    result
}

fn cores(pages: &[&Page], view: &View, starts: &[i64]) -> Vec<Core> {
    let clip = |local: i64| view.origin + (local.max(0) as usize).min(view.frame_count);
    let leaves = leaves(
        pages,
        view.origin,
        clip(starts[0]),
        clip(starts[starts.len() - 1]),
    );
    starts
        .windows(2)
        .map(|range| {
            let (start, end) = (clip(range[0]), clip(range[1]));
            let mut reached = start;
            let mut core = [f32::INFINITY, f32::NEG_INFINITY];
            let mut witnesses = [usize::MAX; 2];
            let mut silent = true;
            let mut include = |value: f32, frame: usize| {
                if value < core[0] || (value == core[0] && frame < witnesses[0]) {
                    core[0] = value;
                    witnesses[0] = frame;
                }
                if value > core[1] || (value == core[1] && frame < witnesses[1]) {
                    core[1] = value;
                    witnesses[1] = frame;
                }
            };
            if start == end {
                return Core {
                    held: true,
                    bounds: Some([0.0; 2]),
                    witnesses: None,
                };
            }
            for leaf in &leaves[leaves.partition_point(|leaf| leaf.end <= start)..] {
                if leaf.start >= end || leaf.start > reached {
                    break;
                }
                let a = leaf.start.max(start);
                let b = leaf.end.min(end);
                if leaf.bounds[0] == leaf.bounds[1] {
                    include(leaf.bounds[0], a);
                }
                for (extreme, witness) in leaf.bounds.into_iter().zip(leaf.frames) {
                    if (a..b).contains(&witness) {
                        include(extreme, witness);
                    }
                }
                if let Some([first, last]) = leaf.endpoints {
                    if a == leaf.start {
                        for value in first {
                            include(value, a);
                        }
                    }
                    if b == leaf.end {
                        for value in last {
                            include(value, b - 1);
                        }
                    }
                }
                silent &= leaf.bounds == [0.0; 2];
                reached = b;
            }
            let held = reached == end;
            let known = held && core[0].is_finite() && (silent || core != [0.0; 2]);
            Core {
                held,
                bounds: known.then_some(core),
                witnesses: known.then_some(witnesses),
            }
        })
        .collect()
}

struct ColourField<'a> {
    pages: Vec<&'a super::Octave>,
    computed: std::collections::HashMap<i64, [f32; 3]>,
    last: i64,
}

impl<'a> ColourField<'a> {
    fn new(pages: &[&'a Page], frame_count: usize) -> Self {
        let mut pages: Vec<_> = pages
            .iter()
            .filter_map(|p| {
                Some((
                    p.frames_per_column,
                    p.octave
                        .as_ref()
                        .filter(|o| o.analysis.as_deref() == Some("stroke-hann512"))?,
                ))
            })
            .collect();
        pages.sort_by_key(|(width, o)| (o.first_cell_frame, std::cmp::Reverse(*width)));
        Self {
            pages: pages.into_iter().map(|(_, o)| o).collect(),
            computed: Default::default(),
            last: frame_count.div_ceil(256).saturating_sub(1) as i64,
        }
    }

    fn read(&mut self, cell: i64) -> Option<[f32; 3]> {
        let cell = cell.max(0);
        if let Some(&rgb) = self.computed.get(&cell) {
            return Some(rgb);
        }
        let mut rgb = [0.0; 3];
        let k = LOG_K.exp();
        let normal = k.ln_1p();
        for (lag, coefficients) in COEFFICIENTS.as_chunks::<8>().0.iter().enumerate() {
            let index = (cell + lag as i64 - 1).clamp(0, self.last) as usize;
            let powers = self.pages.iter().rev().find_map(|page| {
                let offset = index.checked_sub(page.first_cell_frame / 256)? * 8;
                page.powers.get(offset..offset + 8)
            })?;
            for (&power, weights) in powers.iter().zip(coefficients) {
                let value = ((k * power.max(0.0).sqrt()).ln_1p() / normal) as f32;
                for (channel, weight) in rgb.iter_mut().zip(weights) {
                    *channel += f64::from(value) * weight;
                }
            }
        }
        let rgb = rgb.map(|v| v as f32);
        self.computed.insert(cell, rgb);
        Some(rgb)
    }

    fn colour(&mut self, frame: f64, view: &View) -> Option<[f64; 3]> {
        let position = frame / HOP;
        let left = position.floor();
        let a = self.read(left as i64)?;
        let b = self.read(left as i64 + 1)?;
        Some(super::octave_display(
            std::array::from_fn(|i| {
                lerp(f64::from(a[i]), f64::from(b[i]), position - left).clamp(40.0, 255.0)
            }),
            &view.style,
        ))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Columns {
    pub first_cell: i64,
    pub starts: Vec<i64>,
    pub cores: Vec<Core>,
    pub columns: Vec<[f32; 8]>,
}

pub(super) fn columns(pages: &[&Page], view: &View) -> Columns {
    let partition = Partition::new(view.rate, view.pixels_per_second);
    let first_cell = partition.cell(view.first) - 1;
    let end_cell = partition.cell(view.first + view.width as i64 - 1) + 2;
    let starts: Vec<_> = (first_cell..=end_cell)
        .map(|k| partition.start(k))
        .collect();
    let cores = cores(pages, view, &starts);
    let mut field = ColourField::new(pages, view.frame_count);
    let source_width = view.frame_count as f64 * view.pixels_per_second / f64::from(view.rate);
    let mut fallback = super::fallback_colour(pages, view);
    let columns = (0..view.width)
        .map(|x| {
            let pixel = view.first + x as i64;
            let cell = (partition.cell(pixel) - first_cell) as usize;
            let core = cores[cell];
            let px = pixel as f64;
            let coverage = ((px + 1.0).min(source_width) - px.max(0.0)).clamp(0.0, 1.0);
            let state = |code| [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, code];
            if coverage == 0.0 || !core.held {
                return state(-1.0);
            }
            let Some([lo, hi]) = core.bounds else {
                return state(-2.0);
            };
            if lo == 0.0 && hi == 0.0 {
                return state(0.0);
            }
            let before = cores[cell - 1].bounds.unwrap_or([lo, hi]);
            let after = cores[cell + 1].bounds.unwrap_or([lo, hi]);
            let (top, bottom) = view.core(lo, hi);
            let outer_top = top.min(view.y(f64::from(hi.max(before[1]).max(after[1]))));
            let outer_bottom = bottom.max(view.y(f64::from(lo.min(before[0]).min(after[0]))));
            let rgb = match partition.colour_frame(pixel) {
                Some(frame) => field.colour(frame, view),
                None => fallback(pixel),
            };
            let Some([red, green, blue]) = rgb else {
                return state(-1.0);
            };
            [
                top as f32,
                bottom as f32,
                outer_top as f32,
                outer_bottom as f32,
                red as f32,
                green as f32,
                blue as f32,
                coverage as f32,
            ]
        })
        .collect();
    Columns {
        first_cell,
        starts,
        cores,
        columns,
    }
}

/// Device-row integral of the exact direct fragment transfer. All inputs
/// have already passed through the reference column texture's f32 boundary.
pub(super) fn raster(columns: &[[f32; 8]], view: &View, pixels: &mut [u8]) {
    let background = view.background();
    let neutral = if view.style.light { 0.0 } else { 54.0 };
    let closeup = (8.0 * view.pixels_per_second / view.width as f64)
        .log2()
        .clamp(0.0, 1.0) as f32;
    let axis = view.height / 2;
    let axis_opacity = 0.35 * (1.0 - closeup);
    let ramp = |u: f32| u * u - u * u * u / 3.0;
    for (x, &[top, bottom, outer_top, outer_bottom, red, green, blue, q]) in
        columns.iter().enumerate()
    {
        if q < 0.0 {
            continue;
        }
        let rgb = [red, green, blue];
        if axis_opacity > 0.0 {
            let i = (axis * view.width + x) * 4;
            for (c, tone) in [150.0, 168.0, 180.0].into_iter().enumerate() {
                pixels[i + c] =
                    (background[c] * (1.0 - axis_opacity) + tone * axis_opacity).round() as u8;
            }
        }
        if q == 0.0 {
            continue;
        }
        for row in
            (outer_top.floor().max(0.0) as usize)..(outer_bottom.ceil() as usize).min(view.height)
        {
            let y = row as f32;
            let before = |a: f32, b: f32| {
                let from = y.max(a);
                let to = (y + 1.0).min(b);
                (from, to, b - a)
            };
            let (a, b, span) = before(outer_top, top);
            let rising = if b > a {
                (ramp((b - outer_top) / span) - ramp((a - outer_top) / span)) * span
            } else {
                0.0
            };
            let (a, b, span) = before(bottom, outer_bottom);
            let falling = if b > a {
                (ramp((outer_bottom - a) / span) - ramp((outer_bottom - b) / span)) * span
            } else {
                0.0
            };
            let solid = ((y + 1.0).min(bottom) - y.max(top)).max(0.0);
            let a = (rising + solid + falling).min(1.0);
            if a * q <= 0.0 {
                continue;
            }
            let i = (row * view.width + x) * 4;
            for c in 0..3 {
                let emitted = q * (a * rgb[c] + neutral * a * (1.0 - a));
                pixels[i + c] = (background[c] * (1.0 - a * q) + emitted).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform_render::{Mode, Role, SourceDetail, Style};

    #[test]
    fn complementary_leaves_distinguish_silence_missing_and_out_of_cell_witnesses() {
        let leaf = |start, width, bounds, offsets| Page {
            start_frame: start,
            frames_per_column: width,
            frame_count: width,
            source_detail: Some(SourceDetail {
                first_cell_frame: 96 + start,
                start_frame: 96 + start,
                frame_count: width,
                leaf_frames: width,
                bounds,
                offsets,
            }),
            ..Page::default()
        };
        let coarse = leaf(0, 8, vec![0.0, 1.0], vec![6 << 5]);
        let fine = leaf(0, 2, vec![0.0, 0.0], vec![0]);
        let view = View {
            role: Role::Detail,
            style: Style {
                mode: Mode::Stroke,
                gain: 1.0,
                intensity: 1.0,
                light: false,
            },
            rate: 48_000,
            origin: 96,
            frame_count: 10,
            first: 0,
            pixels_per_second: 360.0,
            width: 10,
            height: 10,
        };
        let solved = cores(&[&coarse, &fine], &view, &[0, 2, 4, 8, 10]);
        assert_eq!(solved[0].bounds, Some([0.0, 0.0]));
        assert!(solved[1].held && solved[1].bounds.is_none());
        assert_eq!(solved[2].bounds, Some([1.0, 1.0]));
        assert_eq!(solved[2].witnesses, Some([102, 102]));
        assert!(!solved[3].held && solved[3].bounds.is_none());
        let refined = leaf(2, 2, vec![-0.25, 0.0], vec![1 << 5]);
        let solved = cores(&[&coarse, &fine, &refined], &view, &[0, 2, 4, 8]);
        assert_eq!(solved[1].bounds, Some([-0.25, 0.0]));
        assert_eq!(solved[1].witnesses, Some([98, 99]));
    }
}

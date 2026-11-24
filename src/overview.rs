use gpui_kit::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::Ordering};

#[derive(Clone, Copy)]
pub struct Bin {
    pub end_frame: usize,
    pub low: f32,
    pub high: f32,
    pub low_at: usize,
    pub high_at: usize,
    pub mean_square: f32,
    pub band_rms: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[default]
    Spectral,
    Bands,
    Precision,
}

impl Style {
    pub const ALL: [Self; 3] = [Self::Spectral, Self::Bands, Self::Precision];

    pub fn label(self) -> &'static str {
        match self {
            Self::Spectral => "Spectral",
            Self::Bands => "Three-band",
            Self::Precision => "Precision",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub first: f64,
    pub span: f64,
}

impl Viewport {
    pub fn fit(frames: usize) -> Self {
        Self {
            first: 0.,
            span: frames.max(1) as f64,
        }
    }

    pub fn frame_at(self, fraction: f64) -> f64 {
        self.first + fraction * self.span
    }

    pub fn x(self, frame: f64) -> f64 {
        (frame - self.first) / self.span
    }

    pub fn column_edge(self, column: usize, width: usize) -> usize {
        self.frame_at(column as f64 / width as f64).ceil() as usize
    }

    pub fn zoom(self, span: f64, anchor: f64, frames: usize) -> Self {
        let limit = frames.max(1) as f64;
        let span = span.clamp(limit.min(32.), limit);
        let first = (self.frame_at(anchor) - span * anchor).clamp(0., limit - span);
        Self { first, span }
    }

    pub fn pan(self, delta_frames: f64, frames: usize) -> Self {
        Self {
            first: (self.first + delta_frames).clamp(0., (frames as f64 - self.span).max(0.)),
            span: self.span,
        }
    }
}

// Low, mid, high. The shared palette applies to the compact and detailed views.
const BAND_COLORS: [u32; 3] = [0xe5875e, 0xc78ccb, 0x6acbdc];

fn spectral_color(power: [f64; 3]) -> Option<Hsla> {
    let strength = power.map(|b| b.sqrt());
    let total = strength.iter().sum::<f64>();
    if !total.is_finite() || total == 0. {
        return None;
    }
    let mut color = Rgba {
        a: 1.,
        ..Rgba::default()
    };
    for (band, color_hex) in strength.into_iter().zip(BAND_COLORS) {
        let weight = (band / total) as f32;
        let tone = rgb(color_hex);
        color.r += tone.r * weight;
        color.g += tone.g * weight;
        color.b += tone.b * weight;
    }
    Some(color.into())
}
/// Draws the overview with the played part in the waveform's own colors.
pub fn paint(bins: Arc<[Bin]>, position: f32, wave: crate::theme::Wave) -> impl IntoElement {
    let total = bins.last().map_or(0, |b| b.end_frame);
    paint_range(
        bins,
        0,
        Viewport::fit(total),
        position as f64 * total as f64,
        Style::Spectral,
        wave,
    )
}

pub fn paint_range(
    bins: Arc<[Bin]>,
    first_frame: usize,
    viewport: Viewport,
    position: f64,
    style: Style,
    wave: crate::theme::Wave,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let _timer = crate::diagnostics::Timer::measure(|d| &d.paint_us);
            if let Some(d) = crate::diagnostics::get() {
                d.frames.fetch_add(1, Ordering::Relaxed);
            }
            let logical_width = f32::from(bounds.size.width);
            let first_visible =
                bins.partition_point(|bin| bin.end_frame <= viewport.first.floor() as usize);
            let end_visible =
                bins.partition_point(|bin| bin.end_frame < viewport.frame_at(1.).ceil() as usize);
            let bins_visible = end_visible.saturating_add(1).saturating_sub(first_visible);
            let width =
                ((logical_width * window.scale_factor()).floor() as usize).min(bins_visible);
            let height = f32::from(bounds.size.height);
            if width == 0 || bins.is_empty() {
                return;
            }
            let center = height / 2.;
            let step = logical_width / width as f32;
            let frame = |x: usize| viewport.column_edge(x, width);
            let mut index = first_visible;
            for x in 0..width {
                let first = frame(x);
                let end = frame(x + 1);
                if first == end || first < first_frame {
                    continue;
                }
                while bins.get(index).is_some_and(|b| b.end_frame <= first) {
                    index += 1;
                }
                let (mut lo, mut hi, mut energy) = (0f32, 0f32, 0f64);
                let mut bands = [0.; 3];
                let mut cursor = first;
                for b in &bins[index..] {
                    let next = b.end_frame.min(end);
                    let frames = (next - cursor) as f64;
                    energy += f64::from(b.mean_square) * frames;
                    for (sum, rms) in bands.iter_mut().zip(b.band_rms) {
                        *sum += f64::from(rms).powi(2) * frames;
                    }
                    if (first..end).contains(&b.low_at) {
                        lo = lo.min(b.low);
                    }
                    if (first..end).contains(&b.high_at) {
                        hi = hi.max(b.high);
                    }
                    cursor = next;
                    if cursor == end {
                        break;
                    }
                }
                if cursor != end || !energy.is_finite() {
                    continue;
                }
                let rms = (energy / (end - first) as f64).sqrt() as f32;
                let top = center * (1. - hi.clamp(0., 1.));
                let bottom = center + center * (-lo).clamp(0., 1.);
                let x0 = x as f32 * step;
                let is_played = (first as f64) < position;
                let column = |a: f32, b: f32| {
                    Bounds::new(
                        bounds.origin + point(px(x0), px(a)),
                        size(px(step), px(b - a)),
                    )
                };
                if style == Style::Bands {
                    for (band, (power, color)) in bands.into_iter().zip(BAND_COLORS).enumerate() {
                        let lane = center * (band as f32 * 2. + 1.) / 3.;
                        let radius =
                            ((power / (end - first) as f64).sqrt() as f32).min(1.) * center / 3.;
                        if radius > 0. && rms > 0. {
                            let hue: Hsla = rgb(color).into();
                            window.paint_quad(fill(column(lane - radius, lane + radius), hue));
                        }
                    }
                    continue;
                }
                let hue = (style == Style::Spectral && rms > 0.)
                    .then(|| spectral_color(bands))
                    .flatten();
                let (peak, body) = match hue {
                    Some(hue) if is_played => (hue.opacity(0.62), hue),
                    Some(hue) => (hue.opacity(0.43), hue.opacity(0.76)),
                    None if is_played => (wave.played_peak, wave.played_body),
                    None => (wave.ahead_peak, wave.ahead_body),
                };
                // Keep silence visible as a hairline so the track stays legible.
                let (top, bottom) = if rms == 0. {
                    (center - 0.5, center + 0.5)
                } else {
                    (top, bottom)
                };
                if top < bottom {
                    window.paint_quad(fill(column(top, bottom), peak));
                }
                let r = rms.clamp(0., 1.) * center;
                if style != Style::Precision && r >= 0.5 && top < bottom {
                    window.paint_quad(fill(
                        column((center - r).max(top), (center + r).min(bottom)),
                        body,
                    ));
                }
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::Viewport;

    #[test]
    fn zoom_and_pan_keep_source_coordinates_across_large_offsets() {
        let frames = 2_000_000_000;
        let whole = Viewport::fit(frames);
        let view = whole.zoom(880_123.72, 0.73, frames);
        for anchor in [0., 0.17, 0.5, 0.98, 1.] {
            let target = view.frame_at(anchor);
            let detail = view.zoom(73.45, anchor, frames);
            assert!((detail.frame_at(anchor) - target).abs() < 1e-6);
            assert!((detail.x(target) - anchor).abs() < 1e-8);
            let back = detail.zoom(view.span, anchor, frames);
            assert!((back.first - view.first).abs() < 1e-6);
        }
        assert_eq!(view.pan(-3e9, frames).first, 0.);
        assert_eq!(view.pan(3e9, frames).frame_at(1.), frames as f64);
        assert_eq!(view.zoom(frames as f64 * 2., 0.63, frames), whole);
    }
}

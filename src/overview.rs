use gpui_kit::{prelude::*, *};
use std::sync::{Arc, atomic::Ordering};
#[derive(Clone, Copy)]
pub struct Bin {
    pub end_frame: usize,
    pub low: f32,
    pub high: f32,
    pub mean_square: f32,
}
/// Draws the overview with the played part in the waveform's own colors.
pub fn paint(bins: Arc<[Bin]>, position: f32, wave: crate::theme::Wave) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let _timer = crate::diagnostics::Timer::measure(|d| &d.paint_us);
            if let Some(d) = crate::diagnostics::get() {
                d.frames.fetch_add(1, Ordering::Relaxed);
            }
            let width = f32::from(bounds.size.width).floor() as usize;
            let height = f32::from(bounds.size.height);
            let columns = width.min(bins.len());
            if columns == 0 {
                return;
            }
            let center = (height / 2.).floor();
            let played = position * columns as f32;
            let total = bins.last().unwrap().end_frame;
            let mut index = 0;
            for x in 0..columns {
                let first = (x as u128 * total as u128 / columns as u128) as usize;
                let end = ((x + 1) as u128 * total as u128 / columns as u128) as usize;
                while bins[index].end_frame <= first {
                    index += 1;
                }
                let (mut lo, mut hi, mut energy) = (0f32, 0f32, 0f64);
                let mut cursor = first;
                for b in &bins[index..] {
                    let next = b.end_frame.min(end);
                    energy += f64::from(b.mean_square) * (next - cursor) as f64;
                    lo = lo.min(b.low);
                    hi = hi.max(b.high);
                    cursor = next;
                    if cursor == end {
                        break;
                    }
                }
                if !energy.is_finite() {
                    continue;
                }
                let rms = (energy / (end - first) as f64).sqrt() as f32;
                let top = center * (1. - hi.clamp(0., 1.));
                let bottom = center + center * (-lo).clamp(0., 1.);
                let x0 = (x * width / columns) as f32;
                let x1 = ((x + 1) * width / columns) as f32;
                let is_played = (x as f32) < played;
                let (peak, body) = if is_played {
                    (wave.played_peak, wave.played_body)
                } else {
                    (wave.ahead_peak, wave.ahead_body)
                };
                // Keep silence visible as a hairline so the track stays legible.
                let (top, bottom) = if bottom - top < 1. {
                    (center - 0.5, center + 0.5)
                } else {
                    (top, bottom)
                };
                let column = |a: f32, b: f32| {
                    Bounds::new(
                        bounds.origin + point(px(x0), px(a)),
                        size(px(x1 - x0), px(b - a)),
                    )
                };
                window.paint_quad(fill(column(top, bottom), peak));
                let r = rms.clamp(0., 1.) * center;
                if r >= 0.5 {
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

use crate::waveform_detail::Tile;
use crate::waveform_render::{Page, Role, Style, View};
use gpui_kit::{prelude::*, *};
use std::{
    ops::Range,
    sync::{Arc, atomic::Ordering},
};

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

    pub fn centered(frame: f64, span: f64) -> Self {
        Self {
            first: frame - span * 0.5,
            span,
        }
    }

    pub fn zoom(self, span: f64, anchor: f64, limit: usize, frames: usize) -> Self {
        let limit = limit.max(1) as f64;
        let span = span.clamp(limit.min(32.), limit);
        let centre = self.frame_at(anchor) + span * (0.5 - anchor);
        Self::centered(centre.clamp(0.0, frames.saturating_sub(1) as f64), span)
    }
}

#[derive(Clone)]
pub struct Surface {
    pub role: Role,
    pub rate: u32,
    pub playable: Range<usize>,
    pub viewport: Viewport,
}

impl Surface {
    pub fn target(&self, bounds: Bounds<Pixels>, scale: f32) -> Option<(Bounds<Pixels>, View)> {
        let left = (f32::from(bounds.origin.x) * scale).round();
        let top = (f32::from(bounds.origin.y) * scale).round();
        let right = (f32::from(bounds.right()) * scale).round();
        let bottom = (f32::from(bounds.bottom()) * scale).round();
        let (width, height) = ((right - left) as usize, (bottom - top) as usize);
        if width == 0 || height == 0 || self.playable.is_empty() {
            return None;
        }
        Some((
            Bounds::new(
                point(px(left / scale), px(top / scale)),
                size(px(width as f32 / scale), px(height as f32 / scale)),
            ),
            self.view(width, height),
        ))
    }

    pub fn view(&self, width: usize, height: usize) -> View {
        let pixels_per_second = width as f64 * f64::from(self.rate) / self.viewport.span;
        View {
            role: self.role,
            style: Style::default(),
            rate: self.rate,
            origin: self.playable.start,
            frame_count: self.playable.len(),
            first: (self.viewport.first * pixels_per_second / f64::from(self.rate)).round() as i64,
            pixels_per_second,
            width,
            height,
        }
    }
}

#[cfg(windows)]
pub(crate) use crate::waveform_render::gpu::SurfaceTexture as Texture;

#[cfg(not(windows))]
#[derive(Default, Clone)]
pub struct Texture;

#[cfg(not(windows))]
impl Texture {
    pub fn clear(&self) {}
    pub fn unavailable(&self) -> bool {
        true
    }
}

#[derive(Default)]
pub struct Evidence {
    pub pages: Vec<Arc<Page>>,
    pub overview: Option<Arc<Page>>,
    pub overview_start: usize,
    pub samples: Vec<Arc<Tile>>,
}

pub fn paint(evidence: Evidence, surface: Surface, texture: Texture) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let _timer = crate::diagnostics::Timer::measure(|d| &d.paint_us);
            if let Some(d) = crate::diagnostics::get() {
                d.frames.fetch_add(1, Ordering::Relaxed);
            }
            let Some((target, view)) = surface.target(bounds, window.scale_factor()) else {
                return;
            };
            #[cfg(windows)]
            let _ = texture.paint(window, cx, target, view, &evidence, surface.viewport.first);
            #[cfg(not(windows))]
            let _ = (&texture, &evidence, target, view, cx);
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::{Surface, Viewport};
    use crate::waveform_render::Role;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn centred_follow_and_pointer_zoom_keep_exact_source_coordinates() {
        let frames = 2_000_000_000;
        let view = Viewport::centered(1_800_000_123.5, 880_123.72);
        for anchor in [0., 0.17, 0.5, 0.98, 1.] {
            let target = view.frame_at(anchor);
            let detail = view.zoom(73.45, anchor, frames, frames);
            assert!((detail.frame_at(anchor) - target).abs() < 1e-6);
            let back = detail.zoom(view.span, anchor, frames, frames);
            assert!((back.first - view.first).abs() < 1e-6);
        }
        let entry = Viewport::centered(0., 640.);
        assert_eq!(entry.first, -320.);
        assert_eq!(entry.zoom(32., 0.5, frames, frames).frame_at(0.5), 0.);
    }

    #[test]
    fn surface_targets_share_the_logical_transform_at_three_display_scales() {
        let surface = Surface {
            role: Role::Detail,
            rate: 192_000,
            playable: 0..24_001_123,
            viewport: Viewport::centered(12_000_553., 120. * 192_000.),
        };
        let logical = Bounds::new(point(px(25.3), px(39.7)), size(px(711.8), px(217.9)));
        for scale in [1., 1.5, 2.] {
            let (target, view) = surface.target(logical, scale).unwrap();
            assert_eq!(
                view.width,
                (f32::from(target.size.width) * scale).round() as usize
            );
            assert_eq!(
                view.height,
                (f32::from(target.size.height) * scale).round() as usize
            );
            assert!((f32::from(target.origin.x - logical.origin.x) * scale).abs() <= 0.5);
            let pixels = surface.viewport.first * view.pixels_per_second / f64::from(surface.rate);
            assert!((pixels - view.first as f64).abs() <= 0.5);
        }
    }
}

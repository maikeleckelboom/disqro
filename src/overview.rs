//! Waveform surfaces in Disqro's views, drawn by Waveform Native.
use gpui_kit::{prelude::*, *};
use std::{ops::Range, sync::atomic::Ordering};
use waveform::PixelViewport;
use waveform::{Role, Style, View};

pub use waveform::Viewport;
pub use waveform::gpui::{SurfaceEvidence as Evidence, WaveformSurface as Texture};

#[derive(Clone)]
pub struct Surface {
    pub role: Role,
    pub rate: u32,
    pub playable: Range<usize>,
    pub viewport: Viewport,
}

impl Surface {
    fn pixels(&self) -> PixelViewport {
        PixelViewport {
            role: self.role,
            style: Style::default(),
            rate: self.rate,
            origin: self.playable.start,
            frames: self.playable.len(),
            viewport: self.viewport,
        }
    }

    pub fn target(&self, bounds: Bounds<Pixels>, scale: f32) -> Option<(Bounds<Pixels>, View)> {
        if self.playable.is_empty() {
            return None;
        }
        waveform::gpui::target(&self.pixels(), bounds, scale)
    }

    pub fn target_on(
        &self,
        bounds: Bounds<Pixels>,
        scale: f32,
        axis: waveform::TimeAxis,
    ) -> Option<(Bounds<Pixels>, waveform::OrientedView)> {
        if self.playable.is_empty() {
            return None;
        }
        waveform::gpui::target_on(&self.pixels(), bounds, scale, axis)
    }

    #[cfg(test)]
    pub fn view(&self, width: usize, height: usize) -> View {
        self.pixels().project(width, height)
    }
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
            let _ = texture.paint(window, cx, target, view, &evidence, surface.viewport.first);
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::{Surface, Viewport};
    use gpui_kit::{Bounds, point, px, size};
    use waveform::Role;

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
            assert!((pixels - view.origin()).abs() < 1e-6);
        }
    }
}

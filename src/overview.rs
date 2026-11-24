use crate::waveform_render::{self, Mode, Page, Role, Style as DrawStyle, View};
use gpui_kit::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    ops::Range,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};

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
            Self::Spectral => "Stroke",
            Self::Bands => "Three-band",
            Self::Precision => "Precision",
        }
    }

    pub fn drawing(self, light: bool) -> DrawStyle {
        DrawStyle {
            mode: match self {
                Self::Spectral => Mode::Stroke,
                Self::Bands => Mode::ThreeBand,
                Self::Precision => Mode::Precision,
            },
            gain: 1.0,
            intensity: 1.0,
            light,
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

#[derive(Clone)]
pub struct Surface {
    pub role: Role,
    pub style: DrawStyle,
    pub rate: u32,
    pub playable: Range<usize>,
    pub viewport: Viewport,
}

impl Surface {
    pub fn view(&self, width: usize, height: usize) -> View {
        let pixels_per_second = width as f64 * f64::from(self.rate) / self.viewport.span;
        View {
            role: self.role,
            style: self.style,
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

#[derive(Default, Clone)]
pub struct Texture(Rc<RefCell<Option<Painted>>>);

struct Painted {
    view: View,
    pages: Vec<Arc<Page>>,
    image: Arc<RenderImage>,
    refine: Vec<Range<usize>>,
}

impl Texture {
    pub fn clear(&self, cx: &mut App) {
        if let Some(held) = self.0.borrow_mut().take() {
            cx.drop_image(held.image, None);
        }
    }
}

pub fn paint(
    pages: Vec<Arc<Page>>,
    surface: Surface,
    texture: Texture,
    refinement: impl Fn(&[Range<usize>], &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let _timer = crate::diagnostics::Timer::measure(|d| &d.paint_us);
            if let Some(d) = crate::diagnostics::get() {
                d.frames.fetch_add(1, Ordering::Relaxed);
            }
            let scale = window.scale_factor();
            let left = (f32::from(bounds.origin.x) * scale).round();
            let top = (f32::from(bounds.origin.y) * scale).round();
            let right = (f32::from(bounds.right()) * scale).round();
            let bottom = (f32::from(bounds.bottom()) * scale).round();
            let (width, height) = ((right - left) as usize, (bottom - top) as usize);
            if width == 0 || height == 0 || surface.playable.is_empty() {
                return;
            }
            let target = Bounds::new(
                point(px(left / scale), px(top / scale)),
                size(px(width as f32 / scale), px(height as f32 / scale)),
            );
            let view = surface.view(width, height);
            let mut held = texture.0.borrow_mut();
            if !held.as_ref().is_some_and(|old| {
                old.view == view
                    && old.pages.len() == pages.len()
                    && old.pages.iter().zip(&pages).all(|(a, b)| Arc::ptr_eq(a, b))
            }) {
                let source = pages.iter().map(Arc::as_ref).collect::<Vec<_>>();
                let (mut pixels, geometry) = waveform_render::render(&source, &view);
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let bitmap = image::RgbaImage::from_raw(width as u32, height as u32, pixels)
                    .expect("complete waveform texture");
                let image = Arc::new(RenderImage::new([image::Frame::new(bitmap)]));
                if let Some(old) = held.take() {
                    let _ = window.drop_image(old.image);
                }
                *held = Some(Painted {
                    view,
                    pages: pages.clone(),
                    image,
                    refine: geometry.refinement(&view),
                });
            }
            if let Some(held) = held.as_ref() {
                let _ = window.paint_image(
                    target,
                    bounds,
                    Default::default(),
                    held.image.clone(),
                    0,
                    false,
                );
                refinement(&held.refine, window, cx);
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

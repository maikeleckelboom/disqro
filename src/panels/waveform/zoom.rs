use super::*;
pub(super) use gpui_kit::component::input::InputState;
use gpui_kit::component::{
    input::{Input, InputEvent},
    tooltip::Tooltip,
};

/// Preserve an entered frame span exactly at its recording rate. Across
/// recordings, carry the duration just as the wheel control does.
#[derive(Clone, Copy, Debug)]
pub(super) enum Zoom {
    Seconds(f64),
    Frames { span: f64, rate: u32 },
}

impl Zoom {
    pub(super) fn frames_at(span: f64, rate: u32) -> Self {
        Self::Frames { span, rate }
    }

    pub(super) fn frames(self, rate: u32) -> f64 {
        match self {
            Self::Seconds(s) => s * f64::from(rate),
            Self::Frames {
                span,
                rate: previous,
            } if previous == rate => span,
            Self::Frames {
                span,
                rate: previous,
            } => span * (f64::from(rate) / f64::from(previous)),
        }
    }

    fn parse(text: &str, rate: u32, width: usize) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let unit = ["samples/px", "px/s", "samples", "ms", "s"]
            .into_iter()
            .find(|unit| text.ends_with(unit))?;
        let value: f64 = text.strip_suffix(unit)?.trim().parse().ok()?;
        if !value.is_finite() || value <= 0. || rate == 0 || width == 0 {
            return None;
        }
        let zoom = match unit {
            "s" => Self::Seconds(value),
            "ms" => Self::Seconds(value / 1000.),
            "samples" => Self::frames_at(value, rate),
            "samples/px" => Self::frames_at(value * width as f64, rate),
            "px/s" => Self::Seconds(width as f64 / value),
            _ => unreachable!(),
        };
        let frames = zoom.frames(rate);
        (frames.is_finite() && frames > 0.).then_some(zoom)
    }
}

impl Waveform {
    pub(super) fn cancel_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.zoom_editor.take().is_none() {
            return false;
        }
        self.zoom_focus.focus(window, cx);
        cx.notify();
        true
    }

    fn edit_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zoom_editor.is_some() {
            return;
        }
        let Some(track) = self.player.read(cx).wave_track() else {
            return;
        };
        let span = self.position.viewport.span;
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(format!("{span} samples")));
        cx.subscribe_in(&input, window, move |panel, input, event, window, cx| {
            if panel.zoom_editor.as_ref() != Some(input) {
                return;
            }
            match event {
                InputEvent::PressEnter { .. } => {
                    if let Some(current) = panel.player.read(cx).wave_track()
                        && current.source == track.source
                        && current.generation == track.generation
                        && let Some(layout) = panel
                            .layout
                            .get()
                            .filter(|l| l.scale == window.scale_factor())
                        && let Some(width) = waveform::PixelViewport::register(
                            waveform::viewport::LogicalRect {
                                x: layout.bounds.origin.x.into(),
                                y: layout.bounds.origin.y.into(),
                                width: layout.bounds.size.width.into(),
                                height: layout.bounds.size.height.into(),
                            },
                            layout.scale,
                        )
                        .map(|r| layout.axis.dimensions([r.width, r.height])[0])
                        && let Some(zoom) =
                            Zoom::parse(&input.read(cx).value(), current.source.rate, width)
                    {
                        let rate = current.source.rate;
                        let viewport = panel.position.viewport.zoom(
                            zoom.frames(rate),
                            0.5,
                            (DETAIL_SECONDS * f64::from(rate)) as usize,
                            current.source.frames,
                        );
                        panel.position.zoom = Zoom::frames_at(viewport.span, rate);
                        panel.displayed.borrow_mut().take();
                    }
                    panel.cancel_zoom(window, cx);
                }
                InputEvent::Blur => {
                    panel.zoom_editor = None;
                    cx.notify();
                }
                _ => {}
            }
        })
        .detach();
        self.zoom_editor = Some(input.clone());
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        cx.notify();
    }

    pub(super) fn zoom_control(&self, title: String, cx: &mut Context<Self>) -> AnyElement {
        if let Some(input) = &self.zoom_editor {
            return Input::new(input)
                .xsmall()
                .w(px(152.))
                .aria_label("Visible waveform span")
                .into_any_element();
        }
        let material = *materials(cx);
        div()
            .id("waveform-zoom")
            .track_focus(&self.zoom_focus)
            .role(gpui_kit::Role::Button)
            .aria_label(format!("Edit waveform zoom, {title}"))
            .text_xs()
            .text_color(material.text_muted)
            .cursor_text()
            .hover(|s| s.text_color(material.text))
            .focus_visible(|s| s.underline())
            .tooltip(|w, cx| {
                Tooltip::new("Edit visible span in s, ms, samples, px/s, or samples/px")
                    .build(w, cx)
            })
            .on_click(cx.listener(|panel, _, w, cx| panel.edit_zoom(w, cx)))
            .on_key_down(cx.listener(|panel, e: &KeyDownEvent, w, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    w.prevent_default();
                    panel.edit_zoom(w, cx);
                }
            }))
            .child(title)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::Zoom;
    #[test]
    fn exact_units_use_registered_physical_width_and_source_rate() {
        for rate in [44_100, 48_000, 192_000] {
            for width in [800, 1200, 1600] {
                let frames = |s| Zoom::parse(s, rate, width).unwrap().frames(rate);
                assert_eq!(frames("78.846 s"), 78.846 * f64::from(rate));
                assert_eq!(frames("145ms"), frames(".145 S"));
                assert_eq!(frames("1400 samples"), 1400.);
                assert_eq!(frames(".5 samples/px"), width as f64 * 0.5);
                assert_eq!(frames("180 px/s"), width as f64 / 180. * f64::from(rate));
                let span = frames("0.123456789123 samples/px");
                assert_eq!(frames(&format!("{span} samples")), span);
            }
        }
        for invalid in [
            "-1 s",
            "0 samples",
            "NaN ms",
            "inf px/s",
            "1e309 samples/px",
            "100",
            "44 kHz",
        ] {
            assert!(Zoom::parse(invalid, 44100, 800).is_none());
        }
    }
}

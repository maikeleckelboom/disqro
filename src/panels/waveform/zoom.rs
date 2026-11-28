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

    fn parse(text: &str, rate: u32) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let (number, unit) = text
            .strip_suffix("ms")
            .map(|v| (v, 0.001))
            .or_else(|| text.strip_suffix('s').map(|v| (v, 1.)))?;
        let seconds = number.trim().parse::<f64>().ok()? * unit;
        let frames = seconds * f64::from(rate);
        (frames.is_finite() && frames > 0.).then_some(Self::Seconds(seconds))
    }
}

pub(super) fn duration_label(seconds: f64) -> String {
    if seconds < 1. {
        format!("{:.3} ms", seconds * 1000.)
    } else {
        format!("{seconds:.3} s")
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
        let initial = duration_label(span / f64::from(track.source.rate));
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
        cx.subscribe_in(&input, window, move |panel, input, event, window, cx| {
            if panel.zoom_editor.as_ref() != Some(input) {
                return;
            }
            match event {
                InputEvent::PressEnter { .. } => {
                    if let Some(current) = panel.player.read(cx).wave_track()
                        && current.source == track.source
                        && current.generation == track.generation
                        && input.read(cx).value().as_ref() != initial
                        && let Some(zoom) =
                            Zoom::parse(&input.read(cx).value(), current.source.rate)
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
                Tooltip::new("Edit visible duration in seconds (s) or milliseconds (ms)")
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
    fn duration_zoom_preserves_deep_zoom_and_rejects_inspection_units() {
        for rate in [44_100, 48_000, 192_000] {
            let frames = |text| Zoom::parse(text, rate).unwrap().frames(rate);
            assert_eq!(frames("78.846 s"), 78.846 * f64::from(rate));
            assert_eq!(frames("125 MS"), frames(".125 s"));
            assert!(frames("0.1 ms") < 32.);
        }
        for text in [
            "-1 s",
            "0 ms",
            "NaN ms",
            "inf s",
            "1e309 s",
            "1e308 s",
            "32 samples",
            "180 px/s",
            "0.5 samples/px",
            "100",
            "44 kHz",
        ] {
            assert!(Zoom::parse(text, 48_000).is_none(), "{text}");
        }
    }
}

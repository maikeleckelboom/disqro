//! Transient measurements painted above the successfully drawn native surface.
use super::*;
use waveform::inspection::SampleHit;

/// Movement only schedules a scene update. The paint callback independently
/// resolves the vertex against the source and exact PCM of its own draw.
pub(super) fn listen(
    panel: &WeakEntity<Waveform>,
    texture: Texture,
    layout: Layout,
    hitbox: Hitbox,
    displayed: Option<SampleHit>,
    window: &mut Window,
) {
    let panel = panel.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || event.pressed_button.is_some() {
            return;
        }
        let hovered = if layout.scale == window.scale_factor() && hitbox.is_hovered(window) {
            texture.sample_hit(layout.physical(event.position))
        } else {
            None
        };
        if displayed != hovered {
            let _ = panel.update(cx, |_, cx| cx.notify());
        }
    });
}

pub(super) struct Reading {
    pub hit: SampleHit,
    pub chapter: Option<(u8, f64, f64)>,
    pub pointer: Point<Pixels>,
}

impl Reading {
    pub(super) fn paint(self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let SampleHit {
            frame,
            rate,
            stereo: [left, right],
        } = self.hit;
        crate::diagnostics::stage(
            crate::diagnostics::begin_stage(),
            "wave_sample_hit",
            frame,
            (u64::from(left.to_bits()) << 32) | u64::from(right.to_bits()),
        );
        let seconds = source_seconds(frame, rate);
        let mut labels = vec![
            format!("Frame {frame}  ·  {rate} Hz"),
            format!("Source  {seconds} s"),
            format!("Left  {}", amplitude(left)),
            format!("Right  {}", amplitude(right)),
        ];
        if let Some((number, track, program)) = self.chapter {
            labels.push(format!("Track {number}  {track:.12} s"));
            labels.push(format!("Program  {program:.12} s"));
        }
        let m = *materials(cx);
        let font = window.text_style().font();
        let lines: Vec<_> = labels
            .into_iter()
            .map(|s| {
                let run = TextRun {
                    len: s.len(),
                    font: font.clone(),
                    color: m.text,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(s.into(), px(12.), &[run], None)
            })
            .collect();
        let width = lines.iter().map(|l| l.width).max().unwrap_or_default() + px(16.);
        let height = px(16. * lines.len() as f32 + 10.);
        let place = |pointer: Pixels, start: Pixels, end: Pixels, length: Pixels| {
            let after = pointer + px(12.);
            if after + length <= end {
                after
            } else {
                (pointer - length - px(12.)).max(start)
            }
        };
        let origin = point(
            place(self.pointer.x, bounds.left(), bounds.right(), width),
            place(self.pointer.y, bounds.top(), bounds.bottom(), height),
        );
        window.paint_quad(fill(Bounds::new(origin, size(width, height)), m.overlay));
        for (i, line) in lines.iter().enumerate() {
            let at = origin + point(px(8.), px(5. + i as f32 * 16.));
            let _ = line.paint(at, px(16.), TextAlign::Left, None, window, cx);
        }
    }
}

/// Round only the fractional second. A large frame index must not consume
/// any of the twelve displayed fractional digits.
fn source_seconds(frame: u64, rate: u32) -> String {
    let rate = u64::from(rate);
    let fraction =
        (u128::from(frame % rate) * 1_000_000_000_000 + u128::from(rate / 2)) / u128::from(rate);
    format!("{}.{fraction:012}", frame / rate)
}

fn amplitude(value: f32) -> String {
    if value != 0. && !(0.00001..100_000_000.).contains(&value.abs()) {
        format!("{value:e}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{amplitude, source_seconds};

    #[test]
    fn measurements_keep_source_time_and_round_trip_decoded_channels() {
        assert_eq!(source_seconds(1, 44_100), "0.000022675737");
        assert_eq!(
            source_seconds(48_000 * 20_000_000 + 1, 48_000),
            "20000000.000020833333"
        );
        for value in [1.25, -1.75, -0., f32::MAX, f32::from_bits(1)] {
            assert_eq!(
                amplitude(value).parse::<f32>().unwrap().to_bits(),
                value.to_bits()
            );
        }
    }
}

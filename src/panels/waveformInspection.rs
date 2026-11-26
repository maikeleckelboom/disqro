//! Transient header measurements and a sample ruler for the displayed native surface.
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

fn line(window: &Window, text: String, pt: f32, color: Hsla, weight: FontWeight) -> ShapedLine {
    let mut font = window.text_style().font();
    font.weight = weight;
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.into(), px(pt), &[run], None)
}

const RULER_HEIGHT: f32 = 21.;

pub(super) fn paint_ruler(
    layout: Layout,
    view: &waveform::View,
    first: f64,
    evidence: &overview::Evidence,
    hit: Option<SampleHit>,
    window: &mut Window,
    cx: &mut App,
) {
    let m = *materials(cx);
    let b = layout.bounds;
    let top = b.bottom() - px(RULER_HEIGHT);
    let device_pixel = px(1. / layout.scale);
    let visible = (view.width as f64 / view.pixels_per_frame()).ceil() as usize;
    let widest_label = line(
        window,
        "8".repeat(visible.to_string().len()),
        10.,
        m.text_muted,
        FontWeight::NORMAL,
    );
    let label_spacing = f64::from(f32::from(widest_label.width + px(9.))) * f64::from(layout.scale);
    let mut ticks = waveform::inspection::ruler_ticks(view, first, label_spacing)
        .filter(|t| {
            evidence
                .samples
                .iter()
                .any(|block| block.sample(t.frame).is_some())
        })
        .peekable();
    if ticks.peek().is_none() {
        return;
    }
    window.paint_quad(fill(
        Bounds::new(point(b.left(), top), size(b.size.width, px(RULER_HEIGHT))),
        m.chrome.opacity(0.87),
    ));
    let mut label_end = b.left();
    while let Some(tick) = ticks.next() {
        let x = b.left() + px(tick.x as f32 / layout.scale);
        let inspected = hit.is_some_and(|h| h.frame == tick.frame);
        let ink = if inspected {
            m.accent
        } else if tick.major {
            m.text_muted
        } else {
            m.text_faint.opacity(0.72)
        };
        let h = if tick.major || inspected { 6. } else { 3. };
        // GPUI snaps quad edges. Split one physical pixel of coverage across
        // device columns so a fractional vertex and its tick share a center.
        for (column, coverage) in tick_coverage(tick.x) {
            if column < 0 || column >= view.width as i64 || coverage <= 0. {
                continue;
            }
            window.paint_quad(fill(
                Bounds::new(
                    point(b.left() + device_pixel * column as f32, top),
                    size(device_pixel, px(h)),
                ),
                ink.opacity(coverage),
            ));
        }
        if inspected {
            let dot = size(px(4.), px(4.));
            window.paint_quad(quad(
                Bounds::new(point(x - px(2.), top + px(4.)), dot),
                px(2.),
                m.accent,
                px(0.),
                m.accent,
                BorderStyle::Solid,
            ));
        }
        if tick.numbered || tick.number() == 1 || ticks.peek().is_none() {
            let label = line(
                window,
                tick.number().to_string(),
                10.,
                if inspected { m.accent } else { m.text_muted },
                FontWeight::NORMAL,
            );
            let Some(left) = centered_label(x, label.width, label_end, b.right()) else {
                continue;
            };
            label_end = left + label.width + px(4.);
            let _ = label.paint(
                point(left, top + px(6.)),
                px(13.),
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

fn tick_coverage(center: f64) -> [(i64, f32); 2] {
    let left = center - 0.5;
    let column = left.floor();
    let phase = (left - column) as f32;
    [(column as i64, 1. - phase), (column as i64 + 1, phase)]
}

fn centered_label(x: Pixels, width: Pixels, start: Pixels, end: Pixels) -> Option<Pixels> {
    let left = x - width * 0.5;
    (left >= start && left + width <= end).then_some(left)
}

pub(super) struct Reading {
    pub hit: SampleHit,
    pub chapter: Option<(u8, f64, f64)>,
    pub source_frames: usize,
}

/// Fixed field widths keep adjacent labels still as values change. Use the
/// second line of the existing header only when the primary group needs it.
/// Omit whole lower-priority fields at widths that cannot fit them.
fn field_positions(width: f32, fields: &[f32], primary: usize) -> Vec<(f32, usize)> {
    let gap = 10.;
    let required = fields[..primary].iter().sum::<f32>() + gap * primary.saturating_sub(1) as f32;
    let wrap = required > width;
    let mut x = 0.;
    let mut row = 0;
    let mut positions = Vec::new();
    for &field in fields {
        if x + field > width {
            if !wrap || row == 1 || field > width {
                break;
            }
            x = 0.;
            row = 1;
        }
        positions.push((x, row));
        x += field + gap;
    }
    positions
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
        let m = *materials(cx);
        let digits = self.source_frames.to_string().len();
        let time_digits = (self.source_frames as u64 / u64::from(rate))
            .to_string()
            .len()
            + 15;
        let cue = self.chapter.filter(|_| window.modifiers().shift);
        let mut fields = Vec::new();
        let primary = if let Some((number, track, program)) = cue {
            fields.push((
                format!("Track {number}"),
                format!("{track:.12} s"),
                time_digits,
            ));
            fields.push((
                "Program".into(),
                format!("{program:.12} s"),
                time_digits.max(20),
            ));
            fields.push(("Frame".into(), frame.to_string(), digits));
            2
        } else {
            fields.push(("Frame".into(), frame.to_string(), digits));
            fields.push(("L".into(), amplitude(left), 15));
            fields.push(("R".into(), amplitude(right), 15));
            fields.push((
                "Source".into(),
                format!("{} s", source_seconds(frame, rate)),
                time_digits,
            ));
            fields.push((
                "Rate".into(),
                format!("{rate} Hz"),
                rate.to_string().len() + 3,
            ));
            if self.chapter.is_some() {
                fields.push(("Shift for CUE times".into(), String::new(), 0));
            }
            3
        };
        let numeric = |text: String| {
            let run = TextRun {
                len: text.len(),
                font: font("Consolas"),
                color: m.text,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text.into(), px(12.), &[run], None)
        };
        let digit = numeric("0".into()).width;
        let mut widths = Vec::new();
        let mut lines = Vec::new();
        for (key, text, reserve) in fields {
            let label = line(window, key, 11., m.text_muted, FontWeight::NORMAL);
            let value = numeric(text);
            let width = label.width + px(4.) + value.width.max(digit * reserve as f32);
            widths.push(f32::from(width));
            lines.push((label, value));
        }
        let positions = field_positions(bounds.size.width.into(), &widths, primary);
        let wrapped = positions.last().is_some_and(|(_, row)| *row == 1);
        let line_height = px(14.);
        let top = bounds.center().y - line_height * if wrapped { 1. } else { 0.5 };
        for ((x, row), (label, value)) in positions.into_iter().zip(lines) {
            let at = point(bounds.left() + px(x), top + line_height * row as f32);
            let _ = label.paint(at, line_height, TextAlign::Left, None, window, cx);
            let _ = value.paint(
                at + point(label.width + px(4.), px(0.)),
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
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
    use super::{amplitude, centered_label, field_positions, px, source_seconds, tick_coverage};

    #[test]
    fn fractional_ticks_conserve_coverage_and_edge_labels_keep_their_centers() {
        for x in [0., 0.125, 1.5, 12.75, 900.625] {
            let pair = tick_coverage(x);
            assert_eq!(pair[0].1 + pair[1].1, 1.);
            assert_eq!(
                pair.iter()
                    .map(|(i, a)| (*i as f64 + 0.5) * f64::from(*a))
                    .sum::<f64>(),
                x
            );
        }
        assert_eq!(
            centered_label(px(20.5), px(11.), px(0.), px(100.)),
            Some(px(15.))
        );
        for x in [1., 98.] {
            assert!(centered_label(px(x), px(12.), px(0.), px(100.)).is_none());
        }
    }

    #[test]
    fn inline_fields_wrap_primary_measurements_and_drop_optional_whole_fields() {
        let fields = [60., 90., 90., 140., 85.];
        assert_eq!(field_positions(505., &fields, 3).len(), 5);
        assert_eq!(
            field_positions(280., &fields, 3),
            [(0., 0), (70., 0), (170., 0)]
        );
        assert_eq!(
            field_positions(180., &fields, 3),
            [(0., 0), (70., 0), (0., 1)]
        );
        assert_eq!(field_positions(40., &fields, 3), []);
    }

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

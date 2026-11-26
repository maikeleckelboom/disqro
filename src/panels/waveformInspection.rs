//! Transient header measurements and a sample ruler for the displayed native surface.
use super::*;
use gpui_kit::component::ActiveTheme;
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

/// Size the two channel fields from the visible PCM once per stationary view.
/// Weak identities invalidate these text metrics on replacement without pinning audio.
#[derive(Default)]
pub(super) struct ChannelWidths {
    range: Option<std::ops::RangeInclusive<u64>>,
    blocks: Vec<std::sync::Weak<waveform::SampleBlock>>,
    widths: [Pixels; 2],
    glyphs: [Pixels; 4],
}

impl ChannelWidths {
    pub(super) fn for_range(
        &mut self,
        range: std::ops::RangeInclusive<u64>,
        blocks: &[Arc<waveform::SampleBlock>],
        glyphs: [Pixels; 4],
    ) -> [Pixels; 2] {
        if self.glyphs != glyphs
            || self.range.as_ref() != Some(&range)
            || self.blocks.len() != blocks.len()
            || !self
                .blocks
                .iter()
                .zip(blocks)
                .all(|(old, new)| old.as_ptr() == Arc::as_ptr(new))
        {
            self.widths = [px(0.); 2];
            self.glyphs = glyphs;
            for block in blocks {
                let start = (*range.start()).max(block.first());
                let end = range.end().saturating_add(1).min(block.interval().end);
                if start < end {
                    for frame in &block.frames()
                        [(start - block.first()) as usize..(end - block.first()) as usize]
                    {
                        for (width, sample) in self.widths.iter_mut().zip(frame) {
                            let advance = amplitude(*sample)
                                .bytes()
                                .map(|byte| match byte {
                                    b'.' => glyphs[1],
                                    b'-' => glyphs[2],
                                    b'e' => glyphs[3],
                                    _ => glyphs[0],
                                })
                                .fold(px(0.), |sum, glyph| sum + glyph);
                            *width = (*width).max(advance);
                        }
                    }
                }
            }
            self.range = Some(range);
            self.blocks = blocks.iter().map(Arc::downgrade).collect();
        }
        self.widths
    }
}

pub(super) struct Reading {
    pub hit: SampleHit,
    pub chapter: Option<(u8, f64, f64)>,
    pub channel_widths: Rc<RefCell<ChannelWidths>>,
    pub samples: Vec<Arc<waveform::SampleBlock>>,
    pub first_frame: u64,
    pub last_frame: u64,
}

// Keep complete fields on a single baseline. At the minimum panel width
// keep the highest-priority fields and anchor the group to the trailing edge.
fn row_start(available: f32, widths: &[f32], gap: f32) -> (usize, f32) {
    let mut used = 0.;
    for (i, width) in widths.iter().enumerate() {
        let next = used + if i == 0 { 0. } else { gap } + width;
        if next > available {
            return (i, available - used);
        }
        used = next;
    }
    (widths.len(), available - used)
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
        let mut font = window.text_style().font();
        font.family = cx.theme().font_family.clone();
        let features = Arc::make_mut(&mut font.features.0);
        features.retain(|(tag, _)| tag != "tnum");
        features.push(("tnum".into(), 1));
        let shape = |text: String, color| {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text.into(), px(10.5), &[run], None)
        };
        let digit = shape("0".into(), m.text_muted).width;
        let mut fields = Vec::with_capacity(4);
        if window.modifiers().shift {
            fields.push((
                "Source".into(),
                format!("{} s", source_seconds(frame, rate)),
                px(0.),
            ));
            fields.push((String::new(), format!("{rate} Hz"), px(0.)));
            if let Some((number, track, program)) = self.chapter {
                fields.push((format!("Track {number}"), format!("{track:.12} s"), px(0.)));
                fields.push(("Program".into(), format!("{program:.12} s"), px(0.)));
            }
        } else {
            let glyphs = [
                digit,
                shape(".".into(), m.text_muted).width,
                shape("-".into(), m.text_muted).width,
                shape("e".into(), m.text_muted).width,
            ];
            let widths = self.channel_widths.borrow_mut().for_range(
                self.first_frame..=self.last_frame,
                &self.samples,
                glyphs,
            );
            fields.push((
                "Frame".into(),
                frame.to_string(),
                digit * (self.last_frame.checked_ilog10().unwrap_or(0) + 1) as f32,
            ));
            fields.push(("L".into(), amplitude(left), widths[0]));
            fields.push(("R".into(), amplitude(right), widths[1]));
        }
        let mut lines = Vec::new();
        let mut widths = Vec::new();
        for (key, text, reserve) in fields {
            let gap = if key.is_empty() { px(0.) } else { px(3.) };
            let label = shape(key, m.text_faint);
            let value = shape(text, m.text_muted);
            let width = label.width + gap + value.width.max(reserve);
            widths.push(f32::from(width));
            lines.push((label, gap, value));
        }
        let gap = 7.;
        let (count, start) = row_start(bounds.size.width.into(), &widths, gap);
        let mut x = bounds.left() + px(start);
        let height = px(14.);
        let y = bounds.center().y - height * 0.5;
        for ((label, key_gap, value), width) in lines.into_iter().zip(widths).take(count) {
            let _ = label.paint(point(x, y), height, TextAlign::Left, None, window, cx);
            let end = x + px(width);
            let _ = value.paint(
                point(x + label.width + key_gap, y),
                height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
            x = end + px(gap);
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
    use super::{amplitude, centered_label, px, row_start, source_seconds, tick_coverage};

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
    fn header_keeps_complete_fields_on_one_right_aligned_row() {
        let widths = [36., 45., 60.];
        assert_eq!(row_start(200., &widths, 7.), (3, 45.));
        assert_eq!(row_start(155., &widths, 7.), (3, 0.));
        assert_eq!(row_start(154., &widths, 7.), (2, 66.));
        assert_eq!(row_start(20., &widths, 7.), (0, 20.));
    }

    #[test]
    fn compact_channel_slots_follow_visible_frames_and_replaced_pcm() {
        use super::{Arc, ChannelWidths};
        use waveform::SampleBlock;
        let block = Arc::new(
            SampleBlock::new(50, vec![[0.12345678, 1.], [-1.25, 0.], [0., -2.5]]).unwrap(),
        );
        let mut widths = ChannelWidths::default();
        let glyphs = [px(5.), px(2.), px(3.), px(5.)];
        assert_eq!(
            widths.for_range(51..=52, &[block], glyphs),
            [px(20.), px(15.)]
        );
        let replacement = Arc::new(SampleBlock::new(51, vec![[0., 0.]]).unwrap());
        assert_eq!(
            widths.for_range(51..=52, &[replacement], glyphs),
            [px(5.), px(5.)]
        );
        assert_eq!(widths.for_range(51..=52, &[], glyphs), [px(0.), px(0.)]);
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

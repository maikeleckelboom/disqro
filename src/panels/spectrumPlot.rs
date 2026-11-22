use crate::spectrum::Style;
use crate::{
    diagnostics,
    spectrum::*,
    spectrum_worker::Reading,
    theme::{Materials, materials},
};
use gpui_kit::{
    component::{h_flex, v_flex},
    prelude::*,
    *,
};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, atomic::Ordering::Relaxed},
    time::Instant,
};

pub(super) struct Plot {
    pub(super) failed: bool,
    pub(super) reading: Option<Arc<Reading>>,
    pub(super) settings: Settings,
    labels: Option<Rc<Labels>>,
    painted: Rc<Cell<Option<Instant>>>,
}
struct Labels {
    freq: Vec<(f32, ShapedLine)>,
    db: Vec<(f32, ShapedLine)>,
    bands: Vec<ShapedLine>,
    color: Hsla,
}
impl Plot {
    pub fn new(settings: Settings) -> Self {
        Self {
            reading: None,
            failed: false,
            settings,
            labels: None,
            painted: Default::default(),
        }
    }
}
impl Labels {
    fn new(w: &Window, color: Hsla) -> Self {
        let font = w.text_style().font();
        let line = |s: &'static str| {
            w.text_system().shape_line(
                s.into(),
                px(10.),
                &[TextRun {
                    len: s.len(),
                    font: font.clone(),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
        };
        Self {
            color,
            freq: [
                (20., "20 Hz"),
                (50., "50"),
                (100., "100"),
                (200., "200"),
                (500., "500"),
                (1000., "1k"),
                (2000., "2k"),
                (5000., "5k"),
                (10000., "10k"),
                (20000., "20k"),
            ]
            .map(|(v, s)| (v, line(s)))
            .into(),
            db: [
                (0., "0"),
                (-12., "−12"),
                (-24., "−24"),
                (-36., "−36"),
                (-48., "−48"),
                (-60., "−60"),
                (-72., "−72"),
                (-84., "−84"),
            ]
            .map(|(v, s)| (v, line(s)))
            .into(),
            bands: BANDS.iter().map(|b| line(b.2)).collect(),
        }
    }
}
fn rule(w: &mut Window, at: Point<Pixels>, width: Pixels, height: Pixels, color: Hsla) {
    w.paint_quad(fill(Bounds::new(at, size(width, height)), color));
}
impl Render for Plot {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        if self.labels.as_ref().is_none_or(|l| l.color != m.text_muted) {
            self.labels = Some(Rc::new(Labels::new(w, m.text_muted)));
        }
        let labels = self.labels.clone().unwrap();
        let reading = self.reading.clone();
        let settings = self.settings;
        let painted = self.painted.clone();
        let format = reading
            .as_ref()
            .map(|r| format!("{} FFT   ·   {:.1} kHz", r.len, r.rate as f32 / 1000.))
            .unwrap_or_else(|| {
                if self.failed {
                    "Analysis unavailable"
                } else {
                    "Awaiting audio"
                }
                .into()
            });
        let stereo = reading.as_ref().map(|r| r.stereo).unwrap_or_default();
        let corr = stereo
            .correlation
            .map_or_else(|| "—".into(), |r| format!("{r:+.2}"));
        let mono = stereo
            .mono_retained()
            .map_or_else(|| "—".into(), |r| format!("{:.0}%", r * 100.));
        let phase = format!("Corr {corr}   ·   Mono {mono}");
        let phase_color = if reading.as_ref().is_some_and(|r| r.stereo.opposed()) {
            m.warning
        } else {
            m.text_muted
        };
        v_flex()
            .size_full()
            .child(
                canvas(
                    |_, _, _| (),
                    move |b, _, w, cx| {
                        let _timer = diagnostics::Timer::measure(|d| &d.spectrum_paint_us);
                        if let Some(d) = diagnostics::get() {
                            d.spectrum_paints.fetch_add(1, Relaxed);
                            if let Some(r) = &reading
                                && painted.replace(Some(r.published)) != Some(r.published)
                            {
                                d.publish_to_paint_us
                                    .fetch_max(r.published.elapsed().as_micros() as u64, Relaxed);
                            }
                        }
                        draw(b, &labels, reading.as_deref(), settings, m, w, cx);
                    },
                )
                .flex_1()
                .min_h_0()
                .w_full(),
            )
            .child(
                h_flex()
                    .px(px(10.))
                    .h(px(24.))
                    .gap(px(8.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(m.line)
                    .text_size(px(10.5))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .flex_1()
                            .text_color(m.text_muted)
                            .child(format),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(phase_color)
                            .child(phase),
                    ),
            )
    }
}
fn draw(
    b: Bounds<Pixels>,
    labels: &Labels,
    reading: Option<&Reading>,
    s: Settings,
    m: Materials,
    w: &mut Window,
    cx: &mut App,
) {
    let left = b.left() + px(32.);
    let right = b.right() - px(14.);
    let top = b.top() + px(5.);
    let bottom = b.bottom() - px(22.);
    if right <= left + px(8.) || bottom <= top + px(8.) {
        return;
    }
    let width = right - left;
    let height = bottom - top;
    let y = |db: f32| bottom - height * ((db - FLOOR) / (6. - FLOOR)).clamp(0., 1.);
    let high = s.range.high(reading.map_or(48000, |r| r.rate));
    let put = |line: &ShapedLine, x, cy, w: &mut Window, cx: &mut App| {
        let _ = line.paint(
            point(x, cy - px(6.5)),
            px(13.),
            TextAlign::Left,
            None,
            w,
            cx,
        );
    };
    let mut last = top - px(20.);
    for (db, label) in &labels.db {
        let at = y(*db);
        if at - last < px(16.) {
            continue;
        }
        rule(
            w,
            point(left, at),
            width,
            px(1.),
            if *db == 0. { m.control_line } else { m.line },
        );
        put(label, left - px(6.) - label.width, at, w, cx);
        last = at;
    }
    if s.style == Style::Trace {
        if high <= 20. {
            return;
        }
        let mut end = left - px(40.);
        for (hz, label) in &labels.freq {
            if *hz > high {
                break;
            }
            let x = left + width * ((hz / 20.).ln() / (high / 20.).ln());
            if label.width > width {
                continue;
            }
            let start = (x - label.width * 0.5).clamp(left - px(8.), right - label.width + px(8.));
            if start < end + px(5.) {
                continue;
            }
            rule(w, point(x, top), px(1.), height, m.line);
            put(label, start, bottom + px(10.), w, cx);
            end = start + label.width;
        }
        let Some(r) = reading else {
            return;
        };
        for (values, color, thickness, enabled) in [
            (
                &r.peaks,
                m.text_muted.opacity(0.65),
                1.,
                s.hold != Hold::Off,
            ),
            (&r.levels, m.accent, 1.7, true),
        ] {
            if !enabled || values[..r.count].iter().all(|v| *v <= FLOOR) {
                continue;
            }
            let mut path = PathBuilder::stroke(px(thickness));
            for (i, value) in values[..r.count].iter().enumerate() {
                let p = point(left + width * (i as f32 / (r.count - 1) as f32), y(*value));
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            if let Ok(path) = path.build() {
                w.paint_path(path, color);
            }
        }
    } else {
        let count = BANDS.iter().take_while(|band| band.0 < high).count();
        if count == 0 {
            return;
        }
        let step = width / count as f32;
        for (i, label) in labels.bands[..count].iter().enumerate() {
            let start = left + step * i as f32;
            let center = start + step * 0.5;
            if label.width + px(3.) < step {
                put(label, center - label.width * 0.5, bottom + px(10.), w, cx);
            }
            if let Some(r) = reading {
                let bar = step * 0.68;
                let x = center - bar * 0.5;
                let at = y(r.levels[i]);
                rule(w, point(x, at), bar, bottom - at, m.accent);
                if s.hold != Hold::Off && r.peaks[i] > FLOOR {
                    rule(w, point(x, y(r.peaks[i])), bar, px(1.5), m.text_muted);
                }
            }
        }
    }
}

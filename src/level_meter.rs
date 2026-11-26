//! Native level display. The player samples it on its existing visible transport
//! updates (~25 Hz); it owns no timer, audio buffers or playback commands.
use crate::{
    levels::{Levels, dbfs},
    signal::{LevelReader, SignalMonitor},
    theme::{Materials, materials},
};
use gpui_kit::{
    component::{h_flex, tooltip::Tooltip, v_flex},
    prelude::*,
    *,
};
use std::time::Instant;

const FLOOR: f32 = -60.;
const RELEASE_DB_PER_SECOND: f32 = 36.;

#[derive(Clone, Copy, PartialEq)]
struct Lane {
    rms: f32,
    peak: f32,
    held: f32,
    full_scale: bool,
}

impl Default for Lane {
    fn default() -> Self {
        Self {
            rms: FLOOR,
            peak: FLOOR,
            held: FLOOR,
            full_scale: false,
        }
    }
}

impl Lane {
    fn update(&mut self, levels: Levels, ch: usize, release: f32) {
        let fall = |last: f32, value: f32| dbfs(value).max(last - release).clamp(FLOOR, 0.);
        self.rms = fall(self.rms, levels.rms[ch]);
        self.peak = fall(self.peak, levels.peak[ch]);
        self.held = fall(self.held, levels.held[ch]);
        self.full_scale = levels.held[ch] >= 1.;
    }

    // About one display pixel on the compact meter. Retain full precision for
    // the next decay step and suppress repaints for sub-pixel movement.
    fn appearance(self) -> ([i32; 3], bool) {
        (
            [self.rms, self.peak, self.held].map(|v| (v * 2.).round() as i32),
            self.full_scale,
        )
    }
}

pub(crate) struct LevelMeter {
    monitor: SignalMonitor,
    reader: Option<LevelReader>,
    epoch: u64,
    lanes: [Lane; 2],
    last: Instant,
    axis: Axis,
    measured: Levels,
}

impl LevelMeter {
    pub fn new(monitor: SignalMonitor, axis: Axis) -> Self {
        Self {
            monitor,
            reader: None,
            epoch: 0,
            lanes: Default::default(),
            last: Instant::now(),
            axis,
            measured: Levels::default(),
        }
    }

    pub fn sample(&mut self, active: bool, cx: &mut Context<Self>) {
        let before = self.lanes.map(Lane::appearance);
        let now = Instant::now();
        if active {
            let reader = self.reader.get_or_insert_with(|| self.monitor.subscribe());
            let epoch = reader.epoch();
            if self.epoch != epoch {
                self.lanes = Default::default();
                self.measured = Levels::default();
                self.epoch = epoch;
            }
            if let Some(levels) = reader.latest() {
                let release = (now - self.last).as_secs_f32() * RELEASE_DB_PER_SECOND;
                self.measured = levels;
                for (ch, lane) in self.lanes.iter_mut().enumerate() {
                    lane.update(levels, ch, release);
                }
            }
        } else {
            self.reader = None;
            self.measured = Levels::default();
            self.lanes = Default::default();
        }
        self.last = now;
        if self.lanes.map(Lane::appearance) != before {
            cx.notify();
        }
    }
}

fn paint_lane(bounds: Bounds<Pixels>, axis: Axis, lane: Lane, m: Materials, w: &mut Window) {
    let length = match axis {
        Axis::Horizontal => bounds.size.width,
        Axis::Vertical => bounds.size.height,
    };
    let offset = |db: f32| length * ((db - FLOOR) / -FLOOR);
    let mut segment = |start: Pixels, end: Pixels, color| {
        let bounds = match axis {
            Axis::Horizontal => Bounds::new(
                point(bounds.left() + start, bounds.top()),
                size(end - start, bounds.size.height),
            ),
            Axis::Vertical => Bounds::new(
                point(bounds.left(), bounds.bottom() - end),
                size(bounds.size.width, end - start),
            ),
        };
        w.paint_quad(fill(bounds, color));
    };
    segment(px(0.), length, m.control);
    if lane.rms > FLOOR {
        segment(px(0.), offset(lane.rms), m.accent);
    }
    for db in [-45., -30., -15., -6.] {
        let at = offset(db);
        segment(at, (at + px(1.)).min(length), m.raised);
    }
    for (db, color) in [(lane.held, m.text_muted), (lane.peak, m.text)] {
        if db > FLOOR {
            let end = offset(db).max(px(1.)).min(length);
            segment((end - px(1.5)).max(px(0.)), end, color);
        }
    }
    if lane.full_scale {
        segment((length - px(3.)).max(px(0.)), length, m.danger);
    }
}

impl Render for LevelMeter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let axis = self.axis;
        let m = *materials(cx);
        let levels = self.measured;
        let lane = |ch: usize, name: &'static str, label: &'static str| {
            let state = self.lanes[ch];
            let bar = canvas(
                |_, _, _| (),
                move |b, _, w, _| paint_lane(b, axis, state, m, w),
            )
            .flex_1()
            .when(axis == Axis::Horizontal, |d| d.h(px(4.)))
            .when(axis == Axis::Vertical, |d| d.w(px(4.)));
            let text = div()
                .text_size(px(9.))
                .line_height(px(10.))
                .text_color(m.text_muted)
                .child(label);
            let row = match axis {
                Axis::Horizontal => h_flex()
                    .h(px(10.))
                    .items_center()
                    .gap(px(3.))
                    .child(text.w(px(8.)))
                    .child(bar),
                Axis::Vertical => v_flex()
                    .h_full()
                    .items_center()
                    .gap(px(3.))
                    .child(bar)
                    .child(text),
            };
            row.id(name)
                .role(Role::Meter)
                .aria_label(name)
                .aria_value(format!(
                    "{:.1} dBFS RMS, {:.1} dBFS sample peak",
                    dbfs(levels.rms[ch]),
                    dbfs(levels.peak[ch])
                ))
        };
        let meter = match axis {
            Axis::Horizontal => v_flex().w_full(),
            Axis::Vertical => h_flex().h_full(),
        };
        meter
            .id("stereo-levels")
            .child(lane(0, "Left level", "L"))
            .child(lane(1, "Right level", "R"))
            .when(axis == Axis::Horizontal, |meter| {
                meter.child(
                    h_flex()
                        .ml(px(11.))
                        .justify_between()
                        .text_size(px(8.5))
                        .line_height(px(10.))
                        .text_color(m.text_muted)
                        .child("−60")
                        .child("−30")
                        .child("0"),
                )
            })
            .tooltip(move |w, cx| {
                Tooltip::new(format!(
                "RMS / sample peak (dBFS) · 1 s peak hold\nL  {:.1} / {:.1}     R  {:.1} / {:.1}",
                dbfs(levels.rms[0]), dbfs(levels.peak[0]), dbfs(levels.rms[1]), dbfs(levels.peak[1])
            )).build(w, cx)
            })
    }
}

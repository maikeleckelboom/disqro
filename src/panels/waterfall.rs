//! Past ridges move together in an orthographic projection. Composite them
//! once per history layer and translate that texture between layer crossings.
//! The leading ridge always uses the most recent analysis window.
use super::*;

#[derive(PartialEq)]
struct Key {
    epoch: u64,
    continuity: u64,
    anchor: u64,
    start: u64,
    width: usize,
    height: usize,
    settings: spec::Settings,
}
#[derive(Default)]
pub(super) struct Backdrop {
    layer: Option<(Key, Arc<RenderImage>)>,
}
impl Backdrop {
    pub(super) fn take_image(&mut self) -> Option<Arc<RenderImage>> {
        self.layer.take().map(|(_, im)| im)
    }

    pub(super) fn paint(
        &mut self,
        a: Bounds<Pixels>,
        frame: &VisualFrame,
        r: &spec::History,
        s: spec::Settings,
        start: u64,
        w: &mut Window,
    ) {
        let count = (f32::from(a.size.height) / 8.).clamp(12., 40.) as u64;
        let duration = u64::from(r.rate) * u64::from(s.span.seconds());
        let step = (duration / (count * r.hop)).max(1) * r.hop;
        let now = r.end - r.hop;
        let origin = r
            .tiles
            .front()
            .expect("published history has columns")
            .center;
        // All tiles start on the same hop grid. Anchor the layer phase to
        // sample zero so evicting the oldest tile never shifts the ridges.
        let phase = origin % r.hop;
        let anchor = now - (now - phase) % step;
        let key = Key {
            epoch: frame.epoch,
            continuity: frame.continuity,
            anchor,
            start,
            width: (f32::from(a.size.width) * w.scale_factor())
                .round()
                .clamp(96., 1024.) as usize,
            height: (f32::from(a.size.height) * w.scale_factor())
                .round()
                .clamp(64., 640.) as usize,
            settings: s,
        };
        if self.layer.as_ref().is_none_or(|(k, _)| *k != key) {
            let image = backdrop(&key, r, duration, step);
            if let Some((_, old)) = self.layer.replace((key, image)) {
                let _ = w.drop_image(old);
            }
        }
        let shift = (now - anchor) as f32 / duration as f32;
        let image_bounds = Bounds::new(
            a.origin
                + point(
                    a.size.width * (0.15 * shift),
                    a.size.height * (-0.68 * shift),
                ),
            a.size,
        );
        let _ = w.paint_image(
            a,
            image_bounds,
            Default::default(),
            self.layer.as_ref().unwrap().1.clone(),
            0,
            false,
        );

        if now < start {
            return;
        }
        let Some(col) = r.column_at(now) else { return };
        let mut fill = PathBuilder::fill();
        let mut line = PathBuilder::stroke(px(1.5));
        fill.move_to(a.bottom_left());
        for (i, &v) in col.iter().enumerate() {
            let p = point(
                a.left() + a.size.width * (0.84 * i as f32 / (spec::ROWS - 1) as f32),
                a.bottom() - a.size.height * (0.30 * normalized(s, v)),
            );
            fill.line_to(p);
            if i == 0 {
                line.move_to(p);
            } else {
                line.line_to(p);
            }
        }
        fill.line_to(point(a.left() + a.size.width * 0.84, a.bottom()));
        fill.close();
        if let Ok(p) = fill.build() {
            w.paint_path(p, color(ramp(palette(s.palette), 0.18)));
        }
        if let Ok(p) = line.build() {
            w.paint_path(p, color(ramp(palette(s.palette), 1.)));
        }
    }
}
fn backdrop(k: &Key, r: &spec::History, duration: u64, step: u64) -> Arc<RenderImage> {
    let (w, h) = (k.width, k.height);
    let s = k.settings;
    let stops = palette(s.palette);
    let mut pixels = vec![0u8; w * h * 4];
    let width = (w as f32 * 0.84) as usize;
    for layer in (0..duration.div_ceil(step)).rev() {
        let Some(sample) = k.anchor.checked_sub(layer * step) else {
            continue;
        };
        if sample < k.start {
            continue;
        }
        let Some(col) = r.column_at(sample) else {
            continue;
        };
        let depth = (layer * step) as f32 / duration as f32;
        let left = (w as f32 * 0.15 * depth) as usize;
        let base = h - 1 - (h as f32 * 0.68 * depth) as usize;
        let body = bgra(ramp(stops, 0.08 + 0.10 * (1. - depth)));
        let amplitude = h as f32 * 0.30;
        for i in 0..width {
            let index = i as f32 * (spec::ROWS - 1) as f32 / (width - 1) as f32;
            let j = index as usize;
            let a = normalized(s, col[j]);
            let b = normalized(s, col[(j + 1).min(spec::ROWS - 1)]);
            let v = a + (b - a) * index.fract();
            let y = base as f32 - amplitude * v;
            let slope = (a - b) * amplitude * (spec::ROWS - 1) as f32 / width as f32;
            let thickness =
                (1. + slope * slope).sqrt().min(8.) * (h as f32 / 500.).clamp(0.85, 1.5);
            let ink = bgra(ramp(stops, (0.36 + 0.53 * v) * (1. - 0.27 * depth)));
            let x = left + i;
            let first = (y - thickness * 0.5).floor().max(0.) as usize;
            let last = (y + thickness * 0.5).ceil().max(0.) as usize;
            for row in first..=base {
                let p = &mut pixels[(row * w + x) * 4..][..4];
                if row > last {
                    p.copy_from_slice(&body);
                    continue;
                }
                let coverage = (0.5 + thickness * 0.5 - (row as f32 - y).abs()).clamp(0., 1.);
                if (row as f32) >= y {
                    p.copy_from_slice(&body);
                }
                for c in 0..4 {
                    p[c] = (f32::from(p[c]) * (1. - coverage) + f32::from(ink[c]) * coverage)
                        .round() as u8;
                }
            }
        }
    }
    bitmap(w, h, pixels)
}

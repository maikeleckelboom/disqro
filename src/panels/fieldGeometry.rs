//! Retained physical-pixel geometry for the stereo graticule.
use super::field;
use gpui_kit::*;

pub(super) fn area(b: Bounds<Pixels>, scale: f32) -> Option<(Bounds<Pixels>, usize)> {
    let length = f32::from((b.size.width - px(46.)).min(b.size.height - px(25.)));
    if length < 12. {
        return None;
    }
    // Odd physical diameter places zero at a pixel center, including mono.
    let device = ((length * scale) as usize).saturating_sub(1) | 1;
    let side = px(device as f32 / scale);
    let snap = |p: Pixels| px((f32::from(p) * scale).round() / scale);
    let c = b.center();
    Some((
        Bounds::new(
            point(snap(c.x - side * 0.5), snap(c.y - side * 0.5)),
            size(side, side),
        ),
        device.min(field::MAX_SIDE),
    ))
}

pub(super) struct Guide {
    area: Bounds<Pixels>,
    axes: field::Axes,
    scale: f32,
    lines: Vec<Bounds<Pixels>>,
    outline: Path<Pixels>,
}
impl Guide {
    pub(super) fn matches(&self, area: Bounds<Pixels>, axes: field::Axes, scale: f32) -> bool {
        (self.area, self.axes, self.scale) == (area, axes, scale)
    }
    pub(super) fn new(area: Bounds<Pixels>, axes: field::Axes, scale: f32) -> Option<Self> {
        let side = area.size.width;
        let c = area.center();
        let hair = px(1. / scale);
        let snap = |p: Pixels| px((f32::from(p) * scale).floor() / scale);
        let mut lines = Vec::with_capacity(14);
        let mut line = |x, y, width, height| {
            lines.push(Bounds::new(point(snap(x), snap(y)), size(width, height)))
        };
        line(c.x, area.top(), hair, side);
        line(area.left(), c.y, side, hair);
        let tick = px((4. * scale).round() / scale);
        for offset in [-0.375, -0.25, -0.125, 0.125, 0.25, 0.375] {
            line(c.x + side * offset, c.y - tick * 0.5, hair, tick);
            line(c.x - tick * 0.5, c.y + side * offset, tick, hair);
        }
        let mut path = PathBuilder::stroke(hair);
        let inset = point(hair * 0.5, hair * 0.5);
        let inner = Bounds::from_corners(area.origin + inset, area.bottom_right() - inset);
        let points = if axes == field::Axes::MidSide {
            [
                point(c.x, inner.top()),
                point(inner.right(), c.y),
                point(c.x, inner.bottom()),
                point(inner.left(), c.y),
            ]
        } else {
            [
                inner.origin,
                inner.top_right(),
                inner.bottom_right(),
                inner.bottom_left(),
            ]
        };
        path.move_to(points[0]);
        for &p in &points[1..] {
            path.line_to(p);
        }
        path.close();
        if axes == field::Axes::XY {
            path.move_to(inner.bottom_left());
            path.line_to(inner.top_right());
            path.move_to(inner.origin);
            path.line_to(inner.bottom_right());
        }
        Some(Self {
            area,
            axes,
            scale,
            lines,
            outline: path.build().ok()?,
        })
    }
    pub(super) fn paint(&self, w: &mut Window) {
        let ink: Hsla = rgb(0x35414b).into();
        for &bounds in &self.lines {
            w.paint_quad(fill(bounds, ink));
        }
        w.paint_path(self.outline.clone(), ink);
    }
}

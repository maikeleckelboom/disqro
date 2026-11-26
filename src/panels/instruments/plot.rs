use super::{Config, field, spec};
use crate::{
    diagnostics,
    spectrum::Stereo,
    spectrum_worker::{Payload, VisualFrame},
    theme::{Materials, materials},
};
use gpui_kit::{
    component::{h_flex, v_flex},
    prelude::*,
    *,
};
use image::RgbaImage;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{Arc, atomic::Ordering::Relaxed},
};

pub(super) struct FieldSize(pub usize);
impl EventEmitter<FieldSize> for Plot {}
pub(super) struct Plot {
    pub(super) field_side: Rc<Cell<usize>>,
    pub(super) config: Config,
    pub(super) frame: Option<Arc<VisualFrame>>,
    pub(super) failed: bool,
    pub(super) boundary: (u64, u64),
    cache: Rc<RefCell<Cache>>,
}
impl Plot {
    pub(super) fn new(config: Config, cx: &mut Context<Self>) -> Self {
        cx.on_release(|p, cx| p.cache.borrow_mut().clear_images(cx))
            .detach();
        Self {
            field_side: Rc::new(Cell::new(257)),
            config,
            frame: None,
            failed: false,
            boundary: (0, 0),
            cache: Default::default(),
        }
    }
    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.frame = None;
        self.failed = false;
        self.cache.borrow_mut().clear_images(cx);
        cx.notify();
    }
}
mod waterfall;
const PAPER: [u8; 3] = [16, 20, 27];
const EMBER: [[u8; 3]; 6] = [
    PAPER,
    [37, 45, 70],
    [81, 63, 99],
    [154, 79, 96],
    [224, 139, 81],
    [255, 229, 168],
];
const MARINE: [[u8; 3]; 6] = [
    PAPER,
    [29, 49, 68],
    [35, 81, 103],
    [42, 125, 138],
    [99, 182, 173],
    [222, 244, 201],
];
fn ramp(stops: &[[u8; 3]; 6], t: f32) -> [u8; 3] {
    let u = t.clamp(0., 1.) * 5.;
    let i = (u as usize).min(4);
    std::array::from_fn(|j| {
        (f32::from(stops[i][j])
            + (u - i as f32) * (f32::from(stops[i + 1][j]) - f32::from(stops[i][j])))
        .round() as u8
    })
}
fn color([r, g, b]: [u8; 3]) -> Hsla {
    rgb(u32::from_be_bytes([0, r, g, b])).into()
}
fn palette(p: spec::Palette) -> &'static [[u8; 3]; 6] {
    match p {
        spec::Palette::Ember => &EMBER,
        spec::Palette::Marine => &MARINE,
    }
}
fn normalized(s: spec::Settings, code: u8) -> f32 {
    ((spec::db(code) - s.floor.db()) / (s.ceiling.db() - s.floor.db())).clamp(0., 1.)
}
fn bitmap(w: usize, h: usize, pixels: Vec<u8>) -> Arc<RenderImage> {
    if let Some(d) = diagnostics::get() {
        d.texture_bytes.fetch_add(pixels.len() as u64, Relaxed);
    }
    Arc::new(RenderImage::new([image::Frame::new(
        RgbaImage::from_raw(w as u32, h as u32, pixels).expect("complete BGRA image"),
    )]))
}
fn bgra([r, g, b]: [u8; 3]) -> [u8; 4] {
    [b, g, r, 255]
}

struct FieldImage {
    frame: Arc<VisualFrame>,
    bands: Vec<Arc<RenderImage>>,
}

fn field_palette() -> &'static [[u8; 4]; 256] {
    static PALETTE: std::sync::LazyLock<[[u8; 4]; 256]> = std::sync::LazyLock::new(|| {
        let stops = [
            PAPER,
            [35, 75, 93],
            [53, 132, 153],
            [96, 183, 198],
            [166, 222, 227],
            [229, 245, 246],
        ];
        std::array::from_fn(|i| {
            // Let the guide show through the weakest samples. This linear
            // blend over PAPER exactly reconstructs the first palette interval.
            if i < 51 {
                let mut entry = bgra(stops[1]);
                entry[3] = (i * 5) as u8;
                entry
            } else {
                bgra(ramp(&stops, i as f32 / 255.))
            }
        })
    });
    &PALETTE
}

#[derive(Default)]
struct Cache {
    last_painted: Option<std::time::Instant>,
    tiles: HashMap<u64, (usize, Arc<RenderImage>)>,
    waterfall: waterfall::Backdrop,
    paint_key: Option<(u64, u64, spec::Palette, spec::Floor, spec::Ceiling, usize)>,
    field: Option<FieldImage>,
    guide: Option<field_geometry::Guide>,
    labels: HashMap<&'static str, ShapedLine>,
    luts: Option<[[u8; 4]; 256]>,
}
impl Cache {
    fn clear_images(&mut self, cx: &mut App) {
        let images = std::mem::take(&mut self.tiles);
        let field = self.field.take();
        let backdrop = self.waterfall.take_image();
        cx.defer(move |cx| {
            for (_, (_, im)) in images {
                cx.drop_image(im, None);
            }
            if let Some(field) = field {
                for im in field.bands {
                    cx.drop_image(im, None);
                }
            }
            if let Some(im) = backdrop {
                cx.drop_image(im, None);
            }
        });
        self.paint_key = None;
    }
    fn label(
        &mut self,
        text: &'static str,
        x: Pixels,
        y: Pixels,
        align: f32,
        w: &mut Window,
        cx: &mut App,
    ) {
        let line = self.labels.entry(text).or_insert_with(|| {
            w.text_system().shape_line(
                text.into(),
                px(10.),
                &[TextRun {
                    len: text.len(),
                    font: w.text_style().font(),
                    color: materials(cx).text_muted,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
        });
        let _ = line.paint(
            point(x - line.width * align, y - px(7.)),
            px(14.),
            TextAlign::Left,
            None,
            w,
            cx,
        );
    }
    fn heat(
        &mut self,
        area: Bounds<Pixels>,
        f: &VisualFrame,
        ring: &spec::History,
        s: spec::Settings,
        first: u64,
        w: &mut Window,
    ) {
        let duration = u64::from(ring.rate) * u64::from(s.span.seconds());
        let pixel_width = f32::from(area.size.width) * w.scale_factor();
        let columns_per_pixel =
            (duration as f32 / (ring.hop as f32 * pixel_width)).max(1.) as usize;
        let stride = (1usize << columns_per_pixel.ilog2()).min(spec::TILE);
        let key = (f.epoch, f.continuity, s.palette, s.floor, s.ceiling, stride);
        if self.paint_key != Some(key) {
            for (_, (_, im)) in self.tiles.drain() {
                let _ = w.drop_image(im);
            }
            self.luts = Some(std::array::from_fn(|i| {
                bgra(ramp(palette(s.palette), normalized(s, i as u8)))
            }));
            self.paint_key = Some(key);
        }
        let first = first.max(ring.end.saturating_sub(duration));
        let x = |sample: u64| {
            area.right()
                - area.size.width * ((ring.end.saturating_sub(sample)) as f32 / duration as f32)
        };
        self.tiles.retain(|start, (n, im)| {
            let keep = *start + *n as u64 * ring.hop > first;
            if !keep {
                let _ = w.drop_image(im.clone());
            }
            keep
        });
        let lut = self.luts.as_ref().unwrap();
        for t in &ring.tiles {
            let n = t.columns.len();
            let end = t.center + n as u64 * ring.hop;
            if end <= first {
                continue;
            }
            if self.tiles.get(&t.center).is_none_or(|(len, _)| *len != n) {
                // Whole tiles have exact equal-duration buckets. Keep the
                // incomplete live edge at its original hop spacing.
                let stride = if n == spec::TILE { stride } else { 1 };
                let width = n / stride;
                let mut pixels = vec![0; spec::ROWS * width * 4];
                for (x, group) in t.columns.chunks(stride).enumerate() {
                    for y in 0..spec::ROWS {
                        let peak = group
                            .iter()
                            .map(|col| col[spec::ROWS - 1 - y])
                            .max()
                            .unwrap();
                        let i = (y * width + x) * 4;
                        pixels[i..i + 4].copy_from_slice(&lut[peak as usize]);
                    }
                }
                if let Some((_, old)) = self
                    .tiles
                    .insert(t.center, (n, bitmap(width, spec::ROWS, pixels)))
                {
                    let _ = w.drop_image(old);
                }
            }
            let full =
                Bounds::from_corners(point(x(t.center), area.top()), point(x(end), area.bottom()));
            let clip = Bounds::from_corners(
                point(x(t.center.max(first)), area.top()),
                full.bottom_right(),
            );
            let _ = w.paint_image(
                clip,
                full,
                Default::default(),
                self.tiles[&t.center].1.clone(),
                0,
                false,
            );
        }
    }
    fn field_image(
        &mut self,
        area: Bounds<Pixels>,
        frame: &Arc<VisualFrame>,
        r: &field::Reading,
        w: &mut Window,
    ) {
        let side = r.side;
        let cached = self.field.get_or_insert_with(|| FieldImage {
            frame: frame.clone(),
            bands: Vec::new(),
        });
        if !Arc::ptr_eq(&cached.frame, frame) || cached.bands.len() != r.bands.len() {
            let Payload::Field(previous) = &cached.frame.data else {
                unreachable!()
            };
            let lut = field_palette();
            for (index, band) in r.bands.iter().enumerate() {
                if index < cached.bands.len() && previous.bands.get(index) == Some(band) {
                    continue;
                }
                let [x, y, right, bottom] = band.bounds;
                let pixels = band
                    .intensity
                    .iter()
                    .flat_map(|&v| lut[v as usize])
                    .collect();
                let image = bitmap(right - x, bottom - y, pixels);
                if let Some(old) = cached.bands.get_mut(index) {
                    let _ = w.drop_image(std::mem::replace(old, image));
                } else {
                    cached.bands.push(image);
                }
            }
            for image in cached.bands.drain(r.bands.len()..) {
                let _ = w.drop_image(image);
            }
            cached.frame = frame.clone();
        }
        for (band, im) in r.bands.iter().zip(&cached.bands) {
            let [x, y, right, bottom] = band.bounds;
            let p = |x: usize, y: usize| {
                point(
                    area.left() + area.size.width * (x as f32 / side as f32),
                    area.top() + area.size.height * (y as f32 / side as f32),
                )
            };
            let bounds = Bounds::from_corners(p(x, y), p(right, bottom));
            let _ = w.paint_image(bounds, bounds, Default::default(), im.clone(), 0, false);
        }
    }
}
fn rect(w: &mut Window, x: Pixels, y: Pixels, width: Pixels, height: Pixels, c: Hsla) {
    w.paint_quad(fill(Bounds::new(point(x, y), size(width, height)), c));
}
const FREQ: [(f32, &str); 10] = [
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
];
fn time_labels(span: spec::Span) -> [&'static str; 5] {
    match span {
        spec::Span::Short => ["−15 s", "−11.25", "−7.5", "−3.75", "0 s"],
        spec::Span::Normal => ["−30 s", "−22.5", "−15", "−7.5", "0 s"],
        spec::Span::Long => ["−60 s", "−45", "−30", "−15", "0 s"],
    }
}
fn graph(
    b: Bounds<Pixels>,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) -> Option<Bounds<Pixels>> {
    let a = Bounds::from_corners(
        point(b.left() + px(left), b.top() + px(top)),
        point(b.right() - px(right), b.bottom() - px(bottom)),
    );
    (a.size.width > px(12.) && a.size.height > px(12.)).then_some(a)
}
struct SpectralScene<'a> {
    frame: Option<&'a Arc<VisualFrame>>,
    settings: spec::Settings,
    boundary: (u64, u64),
}
impl SpectralScene<'_> {
    fn paint(&self, b: Bounds<Pixels>, cache: &mut Cache, w: &mut Window, cx: &mut App) {
        let s = self.settings;
        let rate = self.frame.map_or(48000, |f| f.rate);
        let high = s.analysis.range.high(rate);
        let data = self.frame.and_then(|f| {
            if let Payload::Spectral(ring) = &f.data {
                Some((f, ring))
            } else {
                None
            }
        });
        let start = if self.frame.is_some_and(|f| f.epoch == self.boundary.0) {
            self.boundary.1 + s.analysis.resolution.len(rate) as u64 / 2
        } else {
            0
        };
        if s.view == spec::View::Waterfall {
            let Some(a) = graph(b, 13., 12., 16., 20.) else {
                return;
            };
            if let Some((f, ring)) = data {
                cache.waterfall.paint(a, f, ring, s, start, w);
            }
            let width = a.size.width * 0.84;
            let mut previous = a.left() - px(60.);
            for (hz, label) in FREQ.into_iter().take_while(|(hz, _)| *hz <= high) {
                let x = a.left() + width * s.analysis.mapping.fraction(hz, high);
                if x >= previous + px(33.) {
                    cache.label(label, x, a.bottom() + px(10.), 0.3, w, cx);
                    previous = x;
                }
            }
            cache.label(
                time_labels(s.span)[0],
                a.right(),
                a.top() + px(6.),
                1.,
                w,
                cx,
            );
            return;
        }
        let Some(a) = graph(b, 36., 9., 12., 20.) else {
            return;
        };
        if let Some((f, ring)) = data {
            cache.heat(a, f, ring, s, start, w);
        }
        let grid = rgb(0xb4c2d0).opacity(0.12).into();
        let mut last = a.bottom() + px(25.);
        for (hz, label) in FREQ.into_iter().take_while(|(hz, _)| *hz <= high) {
            let y = a.bottom() - a.size.height * s.analysis.mapping.fraction(hz, high);
            if last - y < px(17.) {
                continue;
            }
            rect(w, a.left(), y, a.size.width, px(0.6), grid);
            cache.label(label, a.left() - px(5.), y, 1., w, cx);
            last = y;
        }
        for (i, label) in time_labels(s.span).into_iter().enumerate() {
            if a.size.width < px(225.) && i % 2 == 1 {
                continue;
            }
            let x = a.left() + a.size.width * (i as f32 / 4.);
            rect(w, x, a.top(), px(0.6), a.size.height, grid);
            cache.label(label, x, a.bottom() + px(10.), i as f32 / 4., w, cx);
        }
    }
}
mod field_geometry;

fn field_plot(
    b: Bounds<Pixels>,
    frame: Option<&Arc<VisualFrame>>,
    axes: field::Axes,
    cache: &mut Cache,
    w: &mut Window,
    cx: &mut App,
) {
    let scale = w.scale_factor();
    let Some((a, _)) = field_geometry::area(b, scale) else {
        return;
    };
    let c = a.center();
    if cache
        .guide
        .as_ref()
        .is_none_or(|g| !g.matches(a, axes, scale))
    {
        cache.guide = field_geometry::Guide::new(a, axes, scale);
    }
    if let Some(guide) = &cache.guide {
        guide.paint(w);
    }
    if let Some(f) = frame
        && let Payload::Field(r) = &f.data
    {
        cache.field_image(a, f, r, w);
    }
    let labels = if axes == field::Axes::MidSide {
        ["+M", "−M", "−S", "+S"]
    } else {
        ["+R", "−R", "−L", "+L"]
    };
    cache.label(labels[0], c.x, a.top() - px(6.), 0.5, w, cx);
    cache.label(labels[1], c.x, a.bottom() + px(6.), 0.5, w, cx);
    cache.label(labels[2], a.left() - px(4.), c.y, 1., w, cx);
    cache.label(labels[3], a.right() + px(4.), c.y, 0., w, cx);
}
fn ratio(a: f64, b: f64) -> String {
    match (a > 0., b > 0.) {
        (false, false) => "—".into(),
        (false, true) => "−∞ dB".into(),
        (true, false) => "+∞ dB".into(),
        (true, true) => format!("{:+.1} dB", 10. * (a / b).log10()),
    }
}
fn readouts(st: Stereo, m: Materials) -> impl IntoElement {
    let [l, r, mid, side] = st.energy;
    let balance = if l + r == 0. {
        "—".into()
    } else {
        let v = (r - l) / (l + r);
        if v.abs() < 0.005 {
            "Center".into()
        } else {
            format!("{} {:.0}%", if v < 0. { "L" } else { "R" }, v.abs() * 100.)
        }
    };
    let values = [
        (
            "Corr",
            st.correlation
                .map_or_else(|| "—".into(), |v| format!("{v:+.2}")),
        ),
        (
            "Mono",
            st.mono_retained()
                .map_or_else(|| "—".into(), |v| format!("{:.0}%", v * 100.)),
        ),
        ("Balance", balance),
        ("S/M", ratio(side, mid)),
    ];
    h_flex()
        .flex_wrap()
        .px(px(9.))
        .py(px(3.))
        .gap_x(px(12.))
        .children(values.into_iter().map(|(label, v)| {
            h_flex()
                .gap(px(4.))
                .child(div().text_color(m.text_muted).child(label))
                .child(
                    div()
                        .text_color(if label == "Corr" && st.opposed() {
                            m.warning
                        } else {
                            m.text
                        })
                        .child(v),
                )
        }))
}
impl Render for Plot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let frame = self.frame.clone();
        let cache = self.cache.clone();
        let config = self.config;
        let boundary = self.boundary;
        let info = match config {
            Config::Spectral(s) => format!(
                "{} FFT  ·  {} s",
                s.analysis
                    .resolution
                    .len(frame.as_ref().map_or(48000, |f| f.rate)),
                s.span.seconds()
            ),
            Config::Field(s) => format!(
                "{}  ·  {} half-life",
                s.analysis.gain.label(),
                s.analysis.persistence.label()
            ),
        };
        let status = if self.failed {
            "Analysis unavailable".to_owned()
        } else if frame.is_none() {
            "Awaiting audio".to_owned()
        } else {
            info
        };
        let stats = if let Config::Field(s) = config
            && s.statistics
        {
            Some(readouts(
                frame
                    .as_ref()
                    .and_then(|f| {
                        if let Payload::Field(r) = &f.data {
                            Some(r.stereo)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                m,
            ))
        } else {
            None
        };
        let plot_entity = cx.weak_entity();
        let field_side = self.field_side.clone();
        let plot = canvas(
            move |b, w, cx| {
                if matches!(config, Config::Field(_))
                    && let Some((_, side)) = field_geometry::area(b, w.scale_factor())
                    && field_side.replace(side) != side
                {
                    let _ = plot_entity.update(cx, |_, cx| cx.emit(FieldSize(side)));
                }
            },
            move |b, _, w, cx| {
                let _timer = diagnostics::Timer::measure(|d| &d.visual_paint_us);
                if let Some(d) = diagnostics::get() {
                    d.visual_paints.fetch_add(1, Relaxed);
                }
                w.paint_quad(fill(b, color(PAPER)));
                let mut cache = cache.borrow_mut();
                if let Some(d) = diagnostics::get()
                    && let Some(f) = &frame
                    && cache.last_painted.replace(f.published) != Some(f.published)
                {
                    d.visual_latency_us
                        .fetch_max(f.published.elapsed().as_micros() as u64, Relaxed);
                }
                match config {
                    Config::Spectral(settings) => SpectralScene {
                        frame: frame.as_ref(),
                        settings,
                        boundary,
                    }
                    .paint(b, &mut cache, w, cx),
                    Config::Field(s) => {
                        field_plot(b, frame.as_ref(), s.analysis.axes, &mut cache, w, cx)
                    }
                }
            },
        )
        .w_full()
        .flex_1()
        .min_h_0();
        let footer = h_flex()
            .h(px(22.))
            .px(px(9.))
            .gap(px(5.))
            .text_color(m.text_muted)
            .child(div().min_w_0().flex_1().truncate().child(status));
        let footer = match config {
            Config::Spectral(s) if s.view == spec::View::Waterfall => footer.child(format!(
                "Height  {:.0} to {:.0} dBFS",
                s.floor.db(),
                s.ceiling.db()
            )),
            Config::Spectral(s) => footer
                .child(format!("{:.0}", s.floor.db()))
                .child(h_flex().children((0..16).map(|i| {
                    div()
                        .w(px(3.))
                        .h(px(4.))
                        .bg(color(ramp(palette(s.palette), i as f32 / 15.)))
                })))
                .child(format!("{:.0} dBFS", s.ceiling.db())),
            Config::Field(_) => footer,
        };
        v_flex()
            .size_full()
            .text_size(px(10.5))
            .child(plot)
            .children(stats)
            .child(footer.flex_shrink_0())
    }
}

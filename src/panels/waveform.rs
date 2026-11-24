//! A dock viewport over the player's shared, source-owned waveform facts.
use super::{Home, Instances, Pane, PaneContent, PaneEvent, PanelType, Services};
use crate::{
    overview::{self, Style, Surface, Texture, Viewport},
    player::{Player, WaveTrack},
    theme::materials,
    waveform_detail::{Key, Tile, View},
    waveform_port_analysis::DETAIL_SECONDS,
    waveform_render::Role,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::*,
    *,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::Instant,
};

pub static TYPE: PanelType = PanelType {
    key: "disqro.waveform",
    title: "Waveform",
    icon: IconName::AudioLines,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |services, record, _, cx| {
        Rc::new(cx.new(|cx| Waveform::new(services, record.state(), cx)))
    },
};

struct StyleChanged;

struct Zoom {
    from: Viewport,
    end: Viewport,
    anchor: f64,
    started: Instant,
}

impl Zoom {
    fn at(&self, now: Instant, frames: usize) -> (Viewport, bool) {
        let t = (now.duration_since(self.started).as_secs_f64() / 0.105).min(1.);
        let progress = t * t * (3. - 2. * t);
        let span = self.from.span * (self.end.span / self.from.span).powf(progress);
        (self.from.zoom(span, self.anchor, frames), t == 1.)
    }
}

struct Waveform {
    player: Entity<Player>,
    detail: View,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    identity: Option<(u64, Arc<crate::timeline::Source>, Range<usize>)>,
    viewport: Viewport,
    zoom: Option<Zoom>,
    style: Style,
    presented: bool,
    texture: Texture,
    refine: Rc<RefCell<Vec<Range<usize>>>>,
}

impl EventEmitter<StyleChanged> for Waveform {}

impl Waveform {
    fn new(services: &Services, style: Style, cx: &mut Context<Self>) -> Self {
        cx.observe(&services.player, |panel, _, cx| {
            if panel.presented {
                cx.notify();
            }
        })
        .detach();
        cx.on_release(|panel, cx| panel.texture.clear(cx)).detach();
        Self {
            player: services.player.clone(),
            detail: services.player.read(cx).wave_view(),
            focus: cx.focus_handle(),
            bounds: Rc::default(),
            identity: None,
            viewport: Viewport::fit(0),
            zoom: None,
            style,
            presented: false,
            texture: Default::default(),
            refine: Default::default(),
        }
    }

    fn follow(&mut self, track: &WaveTrack) {
        let key = (track.generation, track.source.clone(), track.range.clone());
        if self.identity.as_ref() != Some(&key) {
            self.identity = Some(key);
            self.refine.borrow_mut().clear();
            self.viewport = Viewport::fit(track.range.len());
            self.zoom = None;
        }
    }

    fn tick(&mut self, now: Instant, frames: usize) {
        if let Some(zoom) = &self.zoom {
            let (view, complete) = zoom.at(now, frames);
            self.viewport = view;
            if complete {
                self.zoom = None;
            }
        }
    }

    fn position(&self, point: Point<Pixels>) -> f64 {
        let bounds = self.bounds.get();
        let width = f64::from(f32::from(bounds.size.width)).max(1.);
        (f64::from(f32::from(point.x - bounds.origin.x)) / width).clamp(0., 1.)
    }

    fn wheel(&mut self, e: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(track) = self.player.read(cx).wave_track() else {
            return;
        };
        self.follow(&track);
        let frames = track.range.len();
        let now = Instant::now();
        self.tick(now, frames);
        let delta = e.delta.pixel_delta(px(40.));
        let (dx, dy) = (f64::from(f32::from(delta.x)), f64::from(f32::from(delta.y)));
        if e.modifiers.shift || dx.abs() > dy.abs() {
            let input = if dx.abs() > dy.abs() { dx } else { dy };
            let width = f64::from(f32::from(self.bounds.get().size.width)).max(1.);
            self.viewport = self
                .viewport
                .pan(-input * self.viewport.span / width, frames);
            self.zoom = None;
        } else if dy != 0. {
            let anchor = self.position(e.position);
            let target = self
                .zoom
                .as_ref()
                .filter(|zoom| (zoom.end.span - zoom.from.span).signum() == -dy.signum());
            let span = target.map_or(self.viewport.span, |zoom| zoom.end.span);
            let end =
                self.viewport
                    .zoom(span * (-dy * 0.0035).clamp(-3., 3.).exp(), anchor, frames);
            self.zoom = Some(Zoom {
                from: self.viewport,
                end,
                anchor,
                started: now,
            });
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn surface(&self, track: &WaveTrack, viewport: Viewport) -> Surface {
        let detail = viewport.span < track.range.len() as f64
            && viewport.span <= f64::from(track.source.rate) * DETAIL_SECONDS;
        Surface {
            role: if detail { Role::Detail } else { Role::Overview },
            style: self.style.drawing(false),
            rate: track.source.rate,
            playable: track.range.clone(),
            viewport,
        }
    }

    fn keys(&self, track: &WaveTrack, viewport: Viewport, device_width: f64) -> Vec<Key> {
        let surface = self.surface(track, viewport);
        if surface.role == Role::Overview {
            return vec![];
        }
        let first = track.range.start + viewport.first.floor() as usize;
        let end = track.range.start + viewport.frame_at(1.).ceil() as usize;
        if viewport.span < device_width * 1.8 && viewport.span < 40_960. {
            return Key::samples(
                &track.source,
                first.saturating_sub(1)..end.saturating_add(1),
            );
        }
        let view = surface.view(device_width.round() as usize, 1);
        let range = if self.style == Style::Spectral {
            view.required_range()
        } else {
            first - track.range.start..end - track.range.start
        };
        let mut keys = Key::geometry(
            &track.source,
            track.range.clone(),
            range,
            device_width,
            viewport.span / f64::from(track.source.rate),
            self.style == Style::Spectral,
        );
        if self.style == Style::Spectral {
            let refine = self.refine.borrow();
            for range in refine
                .iter()
                .filter(|r| r.start < end - track.range.start && r.end > first - track.range.start)
            {
                for key in Key::refine(&track.source, &track.range, range.clone()) {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
        }
        keys
    }
}

impl PaneContent for Entity<Waveform> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus.clone()
    }
    fn title(&self, _: &App) -> SharedString {
        "Waveform".into()
    }
    fn set_presented(&self, presented: bool, cx: &mut App) {
        self.update(cx, |panel, cx| {
            panel.presented = presented;
            if !presented {
                panel.detail.clear();
            }
            cx.notify();
        });
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(self.read(cx).style).unwrap_or_default()
    }
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<Subscription> {
        vec![cx.subscribe(self, |_, _, _: &StyleChanged, cx| {
            cx.emit(PaneEvent::Changed)
        })]
    }
}

impl Render for Waveform {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let material = *materials(cx);
        let mut layout = v_flex()
            .size_full()
            .bg(material.canvas)
            .track_focus(&self.focus);
        let Some(track) = self.player.read(cx).wave_track() else {
            self.detail.clear();
            self.texture.clear(cx);
            self.identity = None;
            return layout.child(
                div()
                    .p_3()
                    .text_color(material.text_muted)
                    .child("Play a recording to see its waveform."),
            );
        };
        self.follow(&track);
        let frames = track.range.len();
        self.tick(Instant::now(), frames);
        if self.zoom.is_some() {
            let entity = cx.entity().entity_id();
            window.on_next_frame(move |_, cx| cx.notify(entity));
        }
        let device_width =
            f64::from(f32::from(self.bounds.get().size.width) * window.scale_factor()).max(1.);
        let view = self.viewport;
        let requested = self.zoom.as_ref().map_or(view, |zoom| zoom.end);
        let mut keys = self.keys(&track, view, device_width);
        for key in self.keys(&track, requested, device_width) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let tiles = if self.presented && window.visibility() == WindowVisibility::Visible {
            self.detail.tiles(track.generation, keys)
        } else {
            self.detail.clear();
            vec![]
        };
        let trace = view.span < device_width * 1.8 && samples_cover(&tiles, &track, view);
        let surface = self.surface(&track, view);
        let pages = if surface.role == Role::Overview {
            vec![track.overview.clone()]
        } else {
            tiles
                .iter()
                .filter_map(|tile| match tile.as_ref() {
                    Tile::Evidence(page) => Some(page.clone()),
                    Tile::Samples { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        let cursor = view.x(track.position as f64);
        let bounds = self.bounds.clone();
        let style = self.style;
        let panel = cx.weak_entity();
        let refining = self.refine.clone();
        let refinement_panel = panel.clone();
        let bounds_panel = panel.clone();
        let title = if trace {
            format!(
                "Stereo · {} samples",
                view.column_edge(1, 1) - view.column_edge(0, 1)
            )
        } else if pages.iter().all(|page| page.frame_count == 0) {
            "Preparing waveform".to_string()
        } else {
            format!("{:.3} s visible", view.span / f64::from(track.source.rate))
        };
        layout = layout.child(
            h_flex()
                .h(px(32.))
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(material.line)
                .child(
                    Button::new("waveform-style")
                        .ghost()
                        .xsmall()
                        .label(style.label())
                        .dropdown_menu(move |mut menu, _, _| {
                            for choice in Style::ALL {
                                let panel = panel.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(choice.label())
                                        .checked(choice == style)
                                        .on_click(move |_, _, cx| {
                                            let _ = panel.update(cx, |p, cx| {
                                                p.style = choice;
                                                cx.emit(StyleChanged);
                                                cx.notify();
                                            });
                                        }),
                                );
                            }
                            menu
                        }),
                )
                .child(
                    Button::new("waveform-fit")
                        .ghost()
                        .xsmall()
                        .label("Fit")
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.viewport = Viewport::fit(frames);
                            p.zoom = None;
                            cx.notify();
                        })),
                )
                .child(div().text_xs().text_color(material.text_muted).child(title)),
        );
        layout.child(
            div()
                .id("waveform-viewport")
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .cursor_pointer()
                .child(
                    canvas(
                        move |b, window, _| {
                            let previous = bounds.replace(b);
                            if previous.size != b.size {
                                let panel = bounds_panel.clone();
                                window.on_next_frame(move |_, cx| {
                                    let _ = panel.update(cx, |_, cx| cx.notify());
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(if trace {
                    paint_samples(tiles, track.range.clone(), view, material.wave)
                        .into_any_element()
                } else {
                    overview::paint(
                        pages,
                        surface,
                        self.texture.clone(),
                        move |ranges, window, _| {
                            let mut held = refining.borrow_mut();
                            if ranges.iter().any(|range| !held.contains(range)) {
                                let first = view.first.floor() as usize;
                                let end = view.frame_at(1.).ceil() as usize;
                                held.retain(|range| range.start < end && range.end > first);
                                for range in ranges {
                                    if !held.contains(range) {
                                        held.push(range.clone());
                                    }
                                }
                                let excess = held.len().saturating_sub(24);
                                held.drain(..excess);
                                let panel = refinement_panel.clone();
                                window.on_next_frame(move |_, cx| {
                                    let _ = panel.update(cx, |_, cx| cx.notify());
                                });
                            }
                        },
                    )
                    .into_any_element()
                })
                .when((0. ..=1.).contains(&cursor), |parent| {
                    parent.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .w(px(1.))
                            .left(relative(cursor as f32))
                            .bg(material.wave.cursor),
                    )
                })
                .on_scroll(cx.listener(|panel, e, _, cx| panel.wheel(e, cx)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|panel, event: &MouseDownEvent, window, cx| {
                        panel.focus.focus(window, cx);
                        let frame = panel
                            .viewport
                            .frame_at(panel.position(event.position))
                            .round() as usize;
                        panel
                            .player
                            .update(cx, |player, cx| player.seek_wave_track(frame, cx));
                    }),
                ),
        )
    }
}

fn samples_cover(tiles: &[Arc<Tile>], track: &WaveTrack, viewport: Viewport) -> bool {
    let first = track.range.start + viewport.first.floor() as usize;
    let end = (track.range.start + viewport.frame_at(1.).ceil() as usize).min(track.range.end);
    let mut cursor = first;
    for tile in tiles {
        if let Tile::Samples { first, samples } = tile.as_ref()
            && *first <= cursor
        {
            cursor = cursor.max(*first + samples.len());
        }
    }
    cursor >= end
}

fn paint_samples(
    tiles: Vec<Arc<Tile>>,
    range: Range<usize>,
    view: Viewport,
    colors: crate::theme::Wave,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let left = (range.start + view.first.floor() as usize)
                .saturating_sub(1)
                .max(range.start);
            let right = (range.start + view.frame_at(1.).ceil() as usize + 1).min(range.end);
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            for channel in 0..2 {
                let mut path = PathBuilder::stroke(px(1. / window.scale_factor()));
                let baseline = height * (0.25 + 0.5 * channel as f32);
                let mut previous = None;
                for tile in &tiles {
                    let Tile::Samples { first, samples } = tile.as_ref() else {
                        continue;
                    };
                    let start = (*first).max(left);
                    let end = (first + samples.len()).min(right);
                    for frame in start..end {
                        let sample = samples[frame - first];
                        let level = if channel == 0 {
                            sample.left
                        } else {
                            sample.right
                        };
                        let x = view.x((frame - range.start) as f64) as f32 * width;
                        let y = baseline - level.clamp(-1., 1.) * height * 0.22;
                        let next = bounds.origin + point(px(x), px(y));
                        if previous.is_some_and(|p| p + 1 == frame) {
                            path.line_to(next);
                        } else {
                            path.move_to(next);
                        }
                        previous = Some(frame);
                    }
                }
                if let Ok(path) = path.build() {
                    let color = if channel == 0 {
                        colors.played_body
                    } else {
                        colors.played_body.opacity(0.72)
                    };
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size_full()
}

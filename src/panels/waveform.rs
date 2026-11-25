//! The detail deck follows playback at a fixed centre. Panning holds a source
//! position for inspection until the listener seeks or returns to playback.
use super::{Home, Instances, PaneContent, PanelType, Services};
use crate::{
    overview::{self, Surface, Texture, Viewport},
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
        h_flex, v_flex,
    },
    prelude::*,
    *,
};
use std::{cell::Cell, ops::Range, rc::Rc, sync::Arc};

pub static TYPE: PanelType = PanelType {
    key: "disqro.waveform",
    title: "Waveform",
    icon: IconName::AudioLines,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |services, _, _, cx| Rc::new(cx.new(|cx| Waveform::new(services, cx))),
};

struct Waveform {
    player: Entity<Player>,
    detail: View,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    identity: Option<(u64, Arc<crate::timeline::Source>, Range<usize>)>,
    viewport: Viewport,
    inspecting: Option<f64>,
    presented: bool,
    texture: Texture,
    input_at: Option<std::time::Instant>,
}

impl Waveform {
    fn new(services: &Services, cx: &mut Context<Self>) -> Self {
        cx.observe(&services.player, |panel, _, cx| {
            if panel.presented {
                cx.notify();
            }
        })
        .detach();
        Self {
            player: services.player.clone(),
            detail: services.player.read(cx).wave_view(),
            focus: cx.focus_handle(),
            bounds: Rc::default(),
            identity: None,
            viewport: Viewport::fit(0),
            inspecting: None,
            presented: false,
            texture: Default::default(),
            input_at: None,
        }
    }

    fn follow(&mut self, track: &WaveTrack) {
        let key = (track.generation, track.source.clone(), track.range.clone());
        if self.identity.as_ref() != Some(&key) {
            let span = f64::from(track.source.rate) * 8.0;
            self.viewport.span = span.min(track.range.len().max(1) as f64);
            self.identity = Some(key);
            self.inspecting = None;
        }
        self.viewport = Viewport::centered(
            self.inspecting.unwrap_or_else(|| track.current_frame()),
            self.viewport.span,
        );
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
        self.input_at = crate::diagnostics::get().map(|_| std::time::Instant::now());
        let delta = e.delta.pixel_delta(px(40.));
        let (dx, dy) = (f64::from(f32::from(delta.x)), f64::from(f32::from(delta.y)));
        if e.modifiers.shift || dx.abs() > dy.abs() {
            let input = if dx.abs() > dy.abs() { dx } else { dy };
            let width = f64::from(f32::from(self.bounds.get().size.width)).max(1.);
            let moved = self.viewport.frame_at(0.5) - input * self.viewport.span / width;
            let centre = moved.clamp(0.0, track.range.len().saturating_sub(1) as f64);
            self.viewport = Viewport::centered(centre, self.viewport.span);
            self.inspecting = Some(centre);
        } else if dy != 0. {
            let anchor = if self.inspecting.is_some() {
                self.position(e.position)
            } else {
                0.5
            };
            let span = self.viewport.span * (-dy * 0.0035).clamp(-3., 3.).exp();
            let limit = track
                .range
                .len()
                .min((f64::from(track.source.rate) * DETAIL_SECONDS) as usize);
            self.viewport = self.viewport.zoom(span, anchor, limit, track.range.len());
            if self.inspecting.is_some() {
                self.inspecting = Some(self.viewport.frame_at(0.5));
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn keys(&self, track: &WaveTrack, surface: &Surface, width: f64) -> Vec<Key> {
        let view = surface.view(width.round() as usize, 1);
        let range = view.required_range();
        let mut keys = if self.viewport.span < width * 3.0 && self.viewport.span < 40_960.0 {
            let left = (self.viewport.first.floor() as usize).saturating_sub(2);
            let right = (self.viewport.frame_at(1.0).ceil() as usize + 2).min(track.range.len());
            Key::samples(
                &track.source,
                track.range.start + left..track.range.start + right,
            )
        } else {
            vec![]
        };
        keys.extend(Key::geometry(
            &track.source,
            track.range.clone(),
            range,
            width,
            self.viewport.span / f64::from(track.source.rate),
        ));
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
                panel.texture.clear();
            }
            cx.notify();
        });
    }
}

impl Render for Waveform {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let material = *materials(cx);
        let layout = v_flex()
            .size_full()
            .bg(material.canvas)
            .track_focus(&self.focus);
        let Some(track) = self.player.read(cx).wave_track() else {
            self.detail.clear();
            self.texture.clear();
            self.identity = None;
            return layout.child(
                div()
                    .p_3()
                    .text_color(material.text_muted)
                    .child("Play a recording to see its waveform."),
            );
        };
        self.follow(&track);
        let viewport = self.viewport;
        let surface = Surface {
            role: Role::Detail,
            rate: track.source.rate,
            playable: track.range.clone(),
            viewport,
        };
        let active = self.presented && window.visibility() == WindowVisibility::Visible;
        if active && track.playing && self.inspecting.is_none() {
            let entity = cx.entity().entity_id();
            window.on_next_frame(move |_, cx| cx.notify(entity));
        }
        let width =
            f64::from(f32::from(self.bounds.get().size.width) * window.scale_factor()).max(1.);
        let mut evidence = overview::Evidence {
            overview: Some(track.overview.clone()),
            input_at: self.input_at.take(),
            ..Default::default()
        };
        if active {
            // A newly laid-out panel schedules again with its measured width.
            for tile in self
                .detail
                .tiles(track.generation, self.keys(&track, &surface, width))
            {
                match tile.as_ref() {
                    Tile::Evidence(page) => evidence.pages.push(page.clone()),
                    Tile::Samples { .. } => evidence.samples.push(tile),
                }
            }
        } else {
            self.detail.clear();
        }
        let span_seconds = viewport.span / f64::from(surface.rate);
        let title = if span_seconds < 0.05 {
            format!("{} samples visible", viewport.span.round() as usize)
        } else {
            format!("{span_seconds:.3} s visible")
        };
        let bounds = self.bounds.clone();
        let panel = cx.weak_entity();
        let inspecting = self.inspecting.is_some();
        layout
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(material.line)
                    .child(div().text_xs().text_color(material.text_muted).child(title))
                    .when(inspecting, |header| {
                        header.child(
                            Button::new("follow-waveform")
                                .ghost()
                                .xsmall()
                                .label("Follow playback")
                                .on_click(cx.listener(|panel, _, _, cx| {
                                    panel.inspecting = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
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
                                    let panel = panel.clone();
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
                    .child(overview::paint(evidence, surface, self.texture.clone()))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .w(px(1.))
                            .left(relative(0.5))
                            .ml(px(-0.5))
                            .bg(if inspecting {
                                material.text_muted
                            } else {
                                material.wave.cursor
                            }),
                    )
                    .on_scroll(cx.listener(|panel, e, _, cx| panel.wheel(e, cx)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|panel, event: &MouseDownEvent, window, cx| {
                            panel.focus.focus(window, cx);
                            let frame = panel
                                .viewport
                                .frame_at(panel.position(event.position))
                                .round() as usize;
                            panel.inspecting = None;
                            panel
                                .player
                                .update(cx, |player, cx| player.seek_wave_track(frame, cx));
                            cx.notify();
                        }),
                    ),
            )
    }
}

//! The detail deck follows a physical source at a fixed centre. Each instance
//! owns its zoom and may hold a separate position for inspection.
use super::{Home, Instances, PaneContent, PanelType, Services};
use crate::{
    audio::{PlaybackIdentity, WaveSeekAnchor},
    overview::{self, Surface, Texture, Viewport},
    player::{Player, PlayerEvent, WaveTrack},
    theme::materials,
    timeline::Source,
    waveform_detail::{Coverage, Key, Tile, View},
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
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, atomic::Ordering::Relaxed},
    time::Instant,
};

pub static TYPE: PanelType = PanelType {
    key: "disqro.waveform",
    title: "Waveform",
    icon: IconName::AudioLines,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |services, _, _, cx| Rc::new(cx.new(|cx| Waveform::new(services, cx))),
};

/// All decks in the workspace use one native frame request. Motion updates
/// textures the scene already references. Layout owns bounds and hit testing.
#[derive(Clone, Default)]
pub(crate) struct Motion(Rc<RefCell<MotionState>>);

#[derive(Default)]
struct MotionState {
    panels: Vec<WeakEntity<Waveform>>,
    armed: bool,
}

impl Motion {
    fn request(&self, panel: WeakEntity<Waveform>, window: &mut Window) {
        let mut state = self.0.borrow_mut();
        if !state
            .panels
            .iter()
            .any(|held| held.entity_id() == panel.entity_id())
        {
            state.panels.push(panel);
        }
        if state.armed {
            return;
        }
        state.armed = true;
        drop(state);
        let start = crate::diagnostics::begin_stage();
        #[cfg(windows)]
        gpui_windows_gpu::present_retained_next_frame(window);
        let motion = self.clone();
        crate::diagnostics::stage(start, "wave_request", 0, 0);
        window.on_next_frame(move |window, cx| {
            let panels = std::mem::take(&mut *motion.0.borrow_mut()).panels;
            let now = Instant::now();
            let start = crate::diagnostics::begin_stage();
            let count = panels.len() as u64;
            for panel in panels {
                let _ = panel.update(cx, |deck, cx| {
                    if deck.animate(now, window, cx) {
                        motion.request(cx.weak_entity(), window);
                    }
                });
            }
            crate::diagnostics::stage(start, "wave_frame", count, 0);
        });
    }
}

#[derive(Clone, Copy, Default)]
struct Layout {
    bounds: Bounds<Pixels>,
    scale: f32,
}

impl Layout {
    fn fraction(self, x: Pixels) -> f64 {
        let width = f64::from(f32::from(self.bounds.size.width)).max(1.);
        (f64::from(f32::from(x - self.bounds.origin.x)) / width).clamp(0., 1.)
    }
}

struct Displayed {
    anchor: WaveSeekAnchor,
    viewport: Viewport,
    layout: Layout,
}

impl Displayed {
    fn seek(&self, x: Pixels) -> Option<(WaveSeekAnchor, usize)> {
        let frame = self.viewport.frame_at(self.layout.fraction(x)).round();
        if frame < 0. || !self.anchor.playable.contains(&(frame as usize)) {
            return None;
        }
        Some((self.anchor.clone(), frame as usize))
    }
}

struct DeckPosition {
    source: Option<Arc<Source>>,
    last: Option<(PlaybackIdentity, f64)>,
    seconds: f64,
    inspecting: Option<f64>,
    viewport: Viewport,
}

impl Default for DeckPosition {
    fn default() -> Self {
        Self {
            source: None,
            last: None,
            seconds: 8.,
            inspecting: None,
            viewport: Viewport::fit(0),
        }
    }
}

impl DeckPosition {
    fn update(&mut self, track: &WaveTrack, at: Instant) -> bool {
        if self.source.as_ref() != Some(&track.source) {
            self.source = Some(track.source.clone());
            self.inspecting = None;
            self.last = None;
        }
        let previous = self
            .last
            .filter(|(identity, _)| track.identity.continues(*identity));
        let sample = track.position(at);
        let mut frame = sample.map_or_else(
            || previous.map_or(track.frame as f64, |(_, frame)| frame),
            |p| p.frame,
        );
        if let Some((_, previous)) = previous {
            frame = frame.max(previous);
        }
        frame = frame.clamp(0., track.source.frames.saturating_sub(1) as f64);
        if let (Some(d), Some(sample)) = (crate::diagnostics::get(), sample) {
            d.wave_clock_age_us
                .fetch_max(sample.observation_age_ns / 1000, Relaxed);
        }
        self.last = Some((track.identity, frame));
        let rate = f64::from(track.source.rate);
        self.viewport = Viewport::centered(
            self.inspecting.unwrap_or(frame),
            (self.seconds * rate).clamp(32., DETAIL_SECONDS * rate),
        );
        self.inspecting.is_none() && (track.playing || sample.is_some_and(|p| p.advancing))
    }
}

struct Waveform {
    player: Entity<Player>,
    detail: View,
    motion: Motion,
    focus: FocusHandle,
    layout: Rc<Cell<Option<Layout>>>,
    displayed: Rc<RefCell<Option<Displayed>>>,
    position: DeckPosition,
    coverage: Option<&'static str>,
    presented: bool,
    texture: Texture,
    input_at: Option<Instant>,
    zoom_label: Option<Task<()>>,
}

impl Waveform {
    fn new(services: &Services, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&services.player, |panel, player, event, cx| {
            if panel.presented
                && (matches!(event, PlayerEvent::WaveChanged)
                    || (matches!(event, PlayerEvent::WaveEvidence)
                        && (!player.read(cx).status.playing
                            || panel.position.inspecting.is_some())))
            {
                if let Some(d) = crate::diagnostics::get() {
                    d.wave_invalidations.fetch_add(1, Relaxed);
                }
                cx.notify();
            }
        })
        .detach();
        Self {
            player: services.player.clone(),
            detail: services.player.read(cx).wave_view(),
            motion: services.wave_motion.clone(),
            focus: cx.focus_handle(),
            layout: Rc::default(),
            displayed: Rc::default(),
            position: DeckPosition::default(),
            coverage: None,
            presented: false,
            texture: Default::default(),
            input_at: None,
            zoom_label: None,
        }
    }

    fn wheel(&mut self, e: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(track), Some(layout)) = (self.player.read(cx).wave_track(), self.layout.get())
        else {
            return;
        };
        if !self.presented || layout.scale != window.scale_factor() {
            return;
        }
        self.position.update(&track, Instant::now());
        self.input_at = crate::diagnostics::get().map(|_| Instant::now());
        let delta = e.delta.pixel_delta(px(40.));
        let (dx, dy) = (f64::from(f32::from(delta.x)), f64::from(f32::from(delta.y)));
        let state = &mut self.position;
        let followed = state.inspecting.is_none();
        if e.modifiers.shift || dx.abs() > dy.abs() {
            let input = if dx.abs() > dy.abs() { dx } else { dy };
            let width = f64::from(f32::from(layout.bounds.size.width)).max(1.);
            let moved = state.viewport.frame_at(0.5) - input * state.viewport.span / width;
            state.inspecting = Some(moved.clamp(0., track.source.frames.saturating_sub(1) as f64));
        } else if dy != 0. {
            let anchor = if state.inspecting.is_some() {
                layout.fraction(e.position.x)
            } else {
                0.5
            };
            let rate = f64::from(track.source.rate);
            let span = state.viewport.span * (-dy * 0.0035).clamp(-3., 3.).exp();
            let zoomed = state.viewport.zoom(
                span,
                anchor,
                (rate * DETAIL_SECONDS) as usize,
                track.source.frames,
            );
            state.seconds = zoomed.span / rate;
            if state.inspecting.is_some() {
                state.inspecting = Some(zoomed.frame_at(0.5));
            }
            // Keep the status label current without rebuilding the scene for
            // every fractional wheel or trackpad event.
            if self.zoom_label.is_none() {
                let panel = cx.weak_entity();
                self.zoom_label = Some(window.spawn(cx, async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(60))
                        .await;
                    let _ = panel.update(cx, |deck, cx| {
                        deck.zoom_label = None;
                        cx.notify();
                    });
                }));
            }
        }
        cx.stop_propagation();
        if followed && state.inspecting.is_some() {
            cx.notify();
        }
        self.motion.request(cx.weak_entity(), window);
    }

    fn prepare(
        &self,
        track: &WaveTrack,
        layout: Option<Layout>,
        active: bool,
    ) -> (Surface, overview::Evidence, Coverage) {
        let viewport = self.position.viewport;
        let surface = Surface {
            role: Role::Detail,
            rate: track.source.rate,
            playable: 0..track.source.frames,
            viewport,
        };
        let mut evidence = overview::Evidence {
            overview: Some(track.overview.clone()),
            overview_start: track.range.start,
            ..Default::default()
        };
        let Some((_, view)) = layout
            .filter(|_| active)
            .and_then(|layout| surface.target(layout.bounds, layout.scale))
        else {
            self.detail.clear();
            return (surface, evidence, Coverage::default());
        };
        let physical = view.required_range();
        let width = view.width as f64;
        let mut keys = if viewport.span < width * 3. && viewport.span < 40_960. {
            let first = (viewport.first.floor() as usize).saturating_sub(4);
            let end = (viewport.frame_at(1.).ceil() as usize + 5).min(track.source.frames);
            Key::samples(&track.source, first..end)
        } else {
            vec![]
        };
        keys.extend(Key::geometry(
            &track.source,
            surface.playable.clone(),
            physical,
            width,
            viewport.span / f64::from(track.source.rate),
        ));
        let preview = (!track.overview.facts.is_empty())
            .then(|| track.range.start..track.range.start + track.overview.frame_count);
        let loaded = self.detail.tiles(track.generation, keys, preview);
        for tile in loaded.tiles {
            match tile.as_ref() {
                Tile::Evidence(page) => evidence.pages.push(page.clone()),
                Tile::Samples { .. } => evidence.samples.push(tile),
            }
        }
        (surface, evidence, loaded.coverage)
    }

    fn coverage(coverage: Coverage, available: bool) -> Option<&'static str> {
        if !available || coverage.unavailable > 0 {
            Some(if coverage.approximate > 0 {
                "Approximate · detail unavailable"
            } else {
                "Detail unavailable"
            })
        } else if coverage.pending > 0 {
            Some(if coverage.approximate > 0 {
                "Refining waveform"
            } else {
                "Loading detail"
            })
        } else {
            None
        }
    }

    fn release_surface(&mut self) {
        self.detail.clear();
        self.texture.clear();
        self.layout.set(None);
        self.displayed.borrow_mut().take();
        self.zoom_label = None;
        self.input_at = None;
    }

    fn animate(&mut self, now: Instant, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.presented
            || window.visibility() != WindowVisibility::Visible
            || self.texture.unavailable()
        {
            return false;
        }
        let (Some(track), Some(layout)) = (self.player.read(cx).wave_track(), self.layout.get())
        else {
            return false;
        };
        if layout.scale != window.scale_factor() {
            self.displayed.borrow_mut().take();
            cx.notify();
            return false;
        }
        let moving = self.position.update(&track, now);
        let start = crate::diagnostics::begin_stage();
        let (surface, evidence, coverage) = self.prepare(&track, Some(layout), true);
        let status = Self::coverage(coverage, track.source.available);
        crate::diagnostics::stage(
            start,
            "wave_evidence_projection",
            coverage.exact as u64,
            coverage.pending as u64,
        );
        let Some((bounds, view)) = surface.target(layout.bounds, layout.scale) else {
            return false;
        };
        #[cfg(windows)]
        {
            let ready = self
                .texture
                .repaint(window, cx, view, &evidence, surface.viewport.first);
            if !ready {
                *self.displayed.borrow_mut() = None;
            } else {
                if let (Some(input), Some(d)) = (self.input_at.take(), crate::diagnostics::get()) {
                    d.wave_input_us
                        .fetch_max(input.elapsed().as_micros() as u64, Relaxed);
                }
                crate::diagnostics::stage(
                    crate::diagnostics::begin_stage(),
                    "wave_playhead",
                    surface.viewport.frame_at(0.5).to_bits(),
                    surface.viewport.span.to_bits(),
                );
                *self.displayed.borrow_mut() = track.seek.map(|anchor| Displayed {
                    anchor,
                    viewport: surface.viewport,
                    layout: Layout { bounds, ..layout },
                });
            }
            if self.coverage != status || !ready {
                cx.notify();
            }
            moving && ready
        }
        #[cfg(not(windows))]
        {
            let _ = (bounds, view, evidence, status);
            cx.notify();
            moving
        }
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
                panel.release_surface();
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
            self.release_surface();
            return layout.child(
                div()
                    .p_3()
                    .text_color(material.text_muted)
                    .child("Play a recording to see its waveform."),
            );
        };
        let moving = self.position.update(&track, Instant::now());
        let active = self.presented && window.visibility() == WindowVisibility::Visible;
        let (surface, evidence, coverage) = self.prepare(&track, self.layout.get(), active);
        self.coverage = Self::coverage(coverage, track.source.available);
        let failed = self.texture.unavailable();
        if active && moving && !failed {
            self.motion.request(cx.weak_entity(), window);
        }
        let seconds = self.position.viewport.span / f64::from(track.source.rate);
        let title = if seconds < 0.05 {
            format!(
                "{} samples visible",
                self.position.viewport.span.round() as usize
            )
        } else {
            format!("{seconds:.3} s visible")
        };
        let bounds = self.layout.clone();
        let displayed = self.displayed.clone();
        let panel = cx.weak_entity();
        let texture = self.texture.clone();
        let input = self.input_at.take();
        let inspecting = self.position.inspecting.is_some();
        layout
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(material.line)
                    .child(div().text_xs().text_color(material.text_muted).child(title))
                    .when_some(
                        if failed {
                            Some("Waveform display unavailable")
                        } else {
                            self.coverage
                        },
                        |header, status| {
                            header.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(material.text_muted)
                                    .child(status),
                            )
                        },
                    )
                    .when(inspecting, |header| {
                        header.child(
                            Button::new("follow-waveform")
                                .ghost()
                                .xsmall()
                                .label("Follow playback")
                                .on_click(cx.listener(|panel, _, _, cx| {
                                    panel.position.inspecting = None;
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
                            |_, _, _| (),
                            move |b, _, window, cx| {
                                if !active {
                                    return;
                                }
                                let _timer = crate::diagnostics::Timer::measure(|d| &d.paint_us);
                                if let Some(d) = crate::diagnostics::get() {
                                    d.frames.fetch_add(1, Relaxed);
                                }
                                let scale = window.scale_factor();
                                let previous = bounds.replace(Some(Layout { bounds: b, scale }));
                                let Some((target, view)) = surface.target(b, scale) else {
                                    displayed.borrow_mut().take();
                                    return;
                                };
                                let start = crate::diagnostics::begin_stage();
                                #[cfg(windows)]
                                let ready = texture.paint(
                                    window,
                                    cx,
                                    target,
                                    view,
                                    &evidence,
                                    surface.viewport.first,
                                );
                                #[cfg(not(windows))]
                                let ready = {
                                    let _ = (&texture, cx, view, evidence);
                                    false
                                };
                                crate::diagnostics::stage(
                                    start,
                                    "wave_deck_paint",
                                    u64::from(ready),
                                    (scale * 96.).round() as u64,
                                );
                                *displayed.borrow_mut() = if ready {
                                    track.seek.map(|anchor| Displayed {
                                        anchor,
                                        viewport: surface.viewport,
                                        layout: Layout {
                                            bounds: target,
                                            scale,
                                        },
                                    })
                                } else {
                                    None
                                };
                                if let (true, Some(d), Some(input)) =
                                    (ready, crate::diagnostics::get(), input)
                                {
                                    d.wave_input_us
                                        .fetch_max(input.elapsed().as_micros() as u64, Relaxed);
                                }
                                if previous
                                    .is_none_or(|p| p.bounds.size != b.size || p.scale != scale)
                                    || failed != texture.unavailable()
                                {
                                    window.on_next_frame(move |_, cx| {
                                        let _ = panel.update(cx, |deck, cx| {
                                            if deck.presented {
                                                cx.notify();
                                            }
                                        });
                                    });
                                }
                            },
                        )
                        .size_full(),
                    )
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
                    .on_scroll(cx.listener(|panel, e, window, cx| panel.wheel(e, window, cx)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|panel, event: &MouseDownEvent, window, cx| {
                            if !panel.presented {
                                return;
                            }
                            panel.focus.focus(window, cx);
                            let target = panel
                                .displayed
                                .borrow()
                                .as_ref()
                                .filter(|shown| shown.layout.scale == window.scale_factor())
                                .and_then(|shown| shown.seek(event.position.x));
                            if let Some((anchor, frame)) = target
                                && panel.player.update(cx, |player, cx| {
                                    player.seek_wave_track(anchor, frame, cx)
                                })
                            {
                                panel.position.inspecting = None;
                                cx.notify();
                            }
                        }),
                    ),
            )
    }
}

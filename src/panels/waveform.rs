//! The detail deck follows a physical source at a fixed centre. Each instance
//! owns its zoom and may hold a separate position for inspection.
use super::{Home, Instances, Pane, PaneContent, PaneEvent, PanelType, Services};
use crate::{
    audio::{PlaybackIdentity, WaveSeekAnchor},
    overview::{self, Surface, Texture, Viewport},
    player::{Player, PlayerEvent, WaveTrack},
    theme::materials,
    timeline::Source,
    waveform_detail::{Coverage, Key, View},
    waveform_port_analysis::DETAIL_SECONDS,
    waveform_render::Role,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{ContextMenuExt, PopupMenuItem},
        v_flex,
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

#[path = "waveformInspection.rs"]
mod inspection;
#[cfg(all(windows, feature = "qualification"))]
#[path = "waveformInspectionJourney.rs"]
mod inspection_journey;
use waveform::inspection::SamplePoints;

#[path = "waveformScrub.rs"]
mod scrub;
#[path = "waveformZoom.rs"]
mod zoom;
use scrub::Press;

pub static TYPE: PanelType = PanelType {
    key: "disqro.waveform",
    title: "Waveform",
    icon: IconName::AudioLines,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |services, record, window, cx| {
        Rc::new(cx.new(|cx| Waveform::new(services, record.state(), window, cx)))
    },
};

/// The instant a frame's waveform motion shows: the compositor tick that
/// scheduled the frame, so successive frames sample the renderer clock one
/// refresh period apart however late the UI thread woke. Falls back to `now`
/// without a recent tick.
pub(crate) fn frame_time(now: Instant) -> Instant {
    #[cfg(windows)]
    if let Some(tick) = gpui_windows_gpu::frame_tick()
        && tick.at <= now
        && now.duration_since(tick.at) < tick.period * 2
    {
        return tick.at;
    }
    now
}

/// All decks in the workspace use one native frame request. Motion updates
/// textures the scene already references. Layout owns bounds and hit testing.
#[derive(Clone, Default)]
pub(crate) struct Motion(Rc<RefCell<MotionState>>);

#[derive(Default)]
struct MotionState {
    panels: Vec<WeakEntity<Waveform>>,
    presented: Vec<WeakEntity<Waveform>>,
    armed: bool,
}

impl Motion {
    fn set_presented(&self, panel: WeakEntity<Waveform>, presented: bool) {
        let mut state = self.0.borrow_mut();
        if presented {
            state.presented.push(panel);
        } else {
            state
                .presented
                .retain(|view| view.entity_id() != panel.entity_id());
            state
                .panels
                .retain(|view| view.entity_id() != panel.entity_id());
        }
    }

    fn follow_scrub(&self, owner: EntityId, window: &mut Window, cx: &App) {
        let presented = self.0.borrow().presented.clone();
        for panel in presented {
            if panel.entity_id() == owner
                || panel
                    .upgrade()
                    .is_some_and(|view| view.read(cx).position.inspecting.is_none())
            {
                self.request(panel, window);
            }
        }
    }

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
            let panels = {
                let mut state = motion.0.borrow_mut();
                state.armed = false;
                std::mem::take(&mut state.panels)
            };
            let woke = Instant::now();
            let now = frame_time(woke);
            let start = crate::diagnostics::begin_stage();
            let count = panels.len() as u64;
            for panel in panels {
                let _ = panel.update(cx, |deck, cx| {
                    if deck.animate(now, window, cx) {
                        motion.request(cx.weak_entity(), window);
                    }
                });
            }
            // The detail is the wake latency after the sampled tick, in µs.
            let lag = woke.saturating_duration_since(now).as_micros() as u64;
            crate::diagnostics::stage(start, "wave_frame", count, lag);
        });
    }
}

#[derive(Clone, Copy, Default)]
struct Layout {
    bounds: Bounds<Pixels>,
    scale: f32,
}

impl Layout {
    fn physical(self, point: Point<Pixels>) -> [f64; 2] {
        let delta = point - self.bounds.origin;
        [delta.x, delta.y].map(|p| f64::from(f32::from(p)) * f64::from(self.scale))
    }

    fn fraction(self, x: Pixels) -> f64 {
        let width = f64::from(f32::from(self.bounds.size.width)).max(1.);
        (f64::from(f32::from(x - self.bounds.origin.x)) / width).clamp(0., 1.)
    }
}

struct Displayed {
    anchor: WaveSeekAnchor,
    navigable: std::ops::Range<usize>,
    viewport: Viewport,
    layout: Layout,
}

impl Displayed {
    fn seek(&self, x: Pixels) -> Option<(WaveSeekAnchor, usize)> {
        let frame = self.viewport.frame_at(self.layout.fraction(x)).round();
        if frame < 0. || !self.navigable.contains(&(frame as usize)) {
            return None;
        }
        Some((self.anchor.clone(), frame as usize))
    }
}

struct DeckPosition {
    source: Option<Arc<Source>>,
    last: Option<(PlaybackIdentity, f64)>,
    zoom: zoom::Zoom,
    inspecting: Option<f64>,
    viewport: Viewport,
    velocity: waveform::planning::ViewVelocity,
}

impl Default for DeckPosition {
    fn default() -> Self {
        Self {
            source: None,
            last: None,
            zoom: zoom::Zoom::Seconds(8.),
            inspecting: None,
            viewport: Viewport::fit(0),
            velocity: Default::default(),
        }
    }
}

impl DeckPosition {
    fn update(&mut self, track: &WaveTrack, at: Instant, grabbed: bool) -> bool {
        if self.source.as_ref() != Some(&track.source) {
            self.source = Some(track.source.clone());
            self.inspecting = None;
            self.last = None;
        }
        let (frame, advancing, observed_frame) = if let Some(target) = track.scrub_frame {
            (target, false, target)
        } else {
            let previous = self
                .last
                .filter(|(identity, _)| track.identity.continues(*identity));
            let sample = track.position(at);
            let previous_frame = previous.map(|(_, frame)| frame);
            let mut frame =
                sample.map_or_else(|| previous_frame.unwrap_or(track.frame as f64), |p| p.frame);
            if let Some(previous) = previous_frame {
                frame = frame.max(previous);
            }
            frame = frame.clamp(0., track.source.frames.saturating_sub(1) as f64);
            if let (Some(d), Some(sample)) = (crate::diagnostics::get(), sample) {
                d.wave_clock_age_us
                    .fetch_max(sample.observation_age_ns / 1000, Relaxed);
            }
            self.last = Some((track.identity, frame));
            (
                frame,
                track.playing || sample.is_some_and(|p| p.advancing),
                sample.map_or(f64::NAN, |sample| sample.frame),
            )
        };
        let rate = f64::from(track.source.rate);
        self.viewport = Viewport::centered(
            if grabbed {
                frame
            } else {
                self.inspecting.unwrap_or(frame)
            },
            self.zoom
                .frames(track.source.rate)
                .clamp(32., DETAIL_SECONDS * rate),
        );
        self.velocity.observe(at, self.viewport.frame_at(0.5));
        if let Some(at) = crate::diagnostics::begin_stage() {
            crate::diagnostics::stage(
                Some(at),
                "wave_deck_position",
                self.viewport.frame_at(0.5).to_bits(),
                observed_frame.to_bits(),
            );
        }
        self.inspecting.is_none() && advancing
    }
}

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    show_sample_points: bool,
}

impl Settings {
    fn points(self, playing: bool, scrubbing: bool) -> waveform::inspection::SamplePoints {
        use waveform::inspection::SamplePoints;
        if self.show_sample_points && !playing && !scrubbing {
            SamplePoints::Visible
        } else {
            SamplePoints::Hidden
        }
    }
}

struct SettingsChanged;
impl EventEmitter<SettingsChanged> for Waveform {}

struct Waveform {
    settings: Settings,
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
    zoom_focus: FocusHandle,
    zoom_editor: Option<Entity<zoom::InputState>>,
    press: Option<Press>,
    capture_watch: Option<Task<()>>,
}

impl Waveform {
    fn new(
        services: &Services,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        #[cfg(all(windows, feature = "qualification"))]
        inspection_journey::start(window, cx);
        cx.observe_window_activation(window, |panel, window, cx| {
            if !window.is_window_active() {
                panel.cancel_press(window, cx);
                if panel.zoom_editor.take().is_some() {
                    cx.notify();
                }
            }
        })
        .detach();
        cx.subscribe_in(
            &services.player,
            window,
            |panel, player, event, window, cx| {
                if matches!(event, PlayerEvent::WaveChanged) && panel.scrub_ended() {
                    panel.cancel_press(window, cx);
                }
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
            },
        )
        .detach();
        Self {
            settings,
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
            zoom_focus: cx.focus_handle().tab_stop(true),
            zoom_editor: None,
            press: None,
            capture_watch: None,
        }
    }

    fn wheel(&mut self, e: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.press.is_some() {
            cx.stop_propagation();
            return;
        }
        let (Some(track), Some(layout)) = (self.player.read(cx).wave_track(), self.layout.get())
        else {
            return;
        };
        if !self.presented || layout.scale != window.scale_factor() {
            return;
        }
        self.position
            .update(&track, frame_time(Instant::now()), false);
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
            state.zoom = zoom::Zoom::frames_at(zoomed.span, track.source.rate);
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

    fn points(&self, track: &WaveTrack, cx: &App) -> waveform::inspection::SamplePoints {
        self.settings.points(
            track.playing
                || (self.settings.show_sample_points
                    && track.position(Instant::now()).is_some_and(|p| p.advancing)),
            self.player.read(cx).waveform_grabbed(),
        )
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
        let mut prefetch = vec![];
        let mut keys = if viewport.span < width * 3. && viewport.span < 40_960. {
            let first = (viewport.first.floor() as usize).saturating_sub(4);
            let end = (viewport.frame_at(1.).ceil() as usize + 5).min(track.source.frames);
            let velocity = self.position.velocity.current();
            if let Ok(visible) = waveform::FrameRange::new(first.min(end) as u64, end as u64) {
                for block in waveform::planning::sample_prefetch(
                    track.source.frames as u64,
                    visible,
                    velocity,
                ) {
                    let range = block.start as usize..block.end as usize;
                    prefetch.extend(Key::samples(&track.source, range));
                }
            }
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
        let preview = (!track.overview.facts().is_empty())
            .then(|| track.range.start..track.range.start + track.overview.frame_count());
        let loaded = self
            .detail
            .tiles_ahead(track.generation, keys, prefetch, preview);
        evidence.pages = loaded.pages;
        evidence.samples = loaded.samples;
        (surface, evidence, loaded.coverage)
    }

    fn coverage(coverage: Coverage, available: bool) -> Option<&'static str> {
        if !available || coverage.unavailable > 0 {
            Some(if coverage.approximate > 0 {
                "Approximate · detail unavailable"
            } else {
                "Detail unavailable"
            })
        } else if coverage.deferred > 0 {
            Some(if coverage.approximate > 0 {
                "Approximate · detail limit reached"
            } else {
                "Detail limit reached"
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
        self.zoom_editor = None;
        self.input_at = None;
        self.press = None;
        self.capture_watch = None;
    }

    fn animate(&mut self, now: Instant, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.presented
            || window.visibility() != WindowVisibility::Visible
            || self.texture.unavailable()
            || self.texture.limited()
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
        let grabbed = self.grabbed();
        let moving = self.position.update(&track, now, grabbed);
        // An inspected texture and its measurement must enter the same scene.
        if self.points(&track, cx) != SamplePoints::Hidden {
            cx.notify();
            return false;
        }
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
            self.texture.set_sample_points(SamplePoints::Hidden);
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
                // The continuous device origin this retained draw shows.
                crate::diagnostics::stage(
                    crate::diagnostics::begin_stage(),
                    "wave_motion",
                    view.origin().to_bits(),
                    view.width as u64,
                );
                if track.scrub_frame.is_some() && (self.position.inspecting.is_none() || grabbed) {
                    crate::diagnostics::stage(
                        crate::diagnostics::begin_stage(),
                        "wave_scrub_draw",
                        surface.viewport.frame_at(0.5).to_bits(),
                        cx.entity_id().as_u64(),
                    );
                }
                *self.displayed.borrow_mut() = track.seek.map(|anchor| Displayed {
                    anchor,
                    navigable: track.navigable.clone(),
                    viewport: surface.viewport,
                    layout: Layout { bounds, ..layout },
                });
            }
            if self.coverage != status || !ready {
                if track.scrub_frame.is_some() {
                    crate::diagnostics::stage(
                        crate::diagnostics::begin_stage(),
                        "wave_scrub_layout",
                        u64::from(ready),
                        0,
                    );
                }
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
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(self.read(cx).settings).unwrap_or_default()
    }
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<Subscription> {
        vec![cx.subscribe(self, |_, _, _: &SettingsChanged, cx| {
            cx.emit(PaneEvent::Changed)
        })]
    }
    fn set_presented(&self, presented: bool, cx: &mut App) {
        self.update(cx, |panel, cx| {
            panel.presented = presented;
            panel.motion.set_presented(cx.weak_entity(), presented);
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
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && (panel.cancel_zoom(window, cx) || panel.cancel_press(window, cx))
                {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }));
        let Some(track) = self.player.read(cx).wave_track() else {
            self.release_surface();
            return layout.child(
                div()
                    .p_3()
                    .text_color(material.text_muted)
                    .child("Play a recording to see its waveform."),
            );
        };
        let grabbed = self.grabbed();
        let moving = self
            .position
            .update(&track, frame_time(Instant::now()), grabbed);
        let active = self.presented && window.visibility() == WindowVisibility::Visible;
        let (surface, evidence, coverage) = self.prepare(&track, self.layout.get(), active);
        self.coverage = Self::coverage(coverage, track.source.available);
        let failed = self.texture.unavailable();
        let limited = self.texture.limited();
        if active && moving && !failed && !limited {
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
        let points = self.points(&track, cx);
        let inspector = (points != SamplePoints::Hidden).then(|| Rc::new(RefCell::new(None)));
        let tip = inspector.clone();
        let inspection_player = self.player.clone();
        let shown = self.settings.show_sample_points;
        let menu_panel = cx.weak_entity();
        let input = self.input_at.take();
        let inspecting = self.position.inspecting.is_some() && !grabbed;
        layout
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(material.line)
                    .child(self.zoom_control(title, cx))
                    .when_some(
                        if failed {
                            Some("Waveform display unavailable")
                        } else if limited {
                            Some("Waveform display limit reached")
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
                    .when(grabbed, |viewport| viewport.cursor_grabbing())
                    .when(!grabbed, |viewport| viewport.cursor_grab())
                    .child(
                        canvas(
                            move |b, w, _| {
                                (points != SamplePoints::Hidden)
                                    .then(|| w.insert_hitbox(b, HitboxBehavior::Normal))
                            },
                            move |b, hitbox, window, cx| {
                                if !active {
                                    return;
                                }
                                Self::listen_for_scrub(&panel, window);
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
                                let layout = Layout {
                                    bounds: target,
                                    scale,
                                };
                                let points = if points != SamplePoints::Hidden
                                    && !evidence.samples.is_empty()
                                    && inspection_player.read(cx).inspection_current(&track)
                                {
                                    points.for_view(&view)
                                } else {
                                    SamplePoints::Hidden
                                };
                                let hitbox = hitbox.filter(|_| points != SamplePoints::Hidden);
                                let hit = hitbox
                                    .as_ref()
                                    .filter(|h| h.is_hovered(window))
                                    .and_then(|_| {
                                        waveform::inspection::hit_sample(
                                            &view,
                                            surface.viewport.first,
                                            layout.physical(window.mouse_position()),
                                            |i| {
                                                evidence
                                                    .samples
                                                    .iter()
                                                    .find_map(|block| block.sample(i))
                                            },
                                        )
                                    });
                                texture.set_sample_points(
                                    hit.map_or(points, |h| SamplePoints::Highlight(h.frame)),
                                );
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
                                if ready && let Some(hitbox) = hitbox {
                                    inspection::listen(
                                        &panel,
                                        texture.clone(),
                                        layout,
                                        hitbox,
                                        hit,
                                        window,
                                    );
                                    if let (Some(hit), Some(tip)) = (hit, &tip) {
                                        let chapter = inspection_player
                                            .read(cx)
                                            .inspection_chapter(&track, hit.frame as usize);
                                        *tip.borrow_mut() = Some(inspection::Reading {
                                            hit,
                                            chapter,
                                            pointer: window.mouse_position(),
                                        });
                                    }
                                }
                                *displayed.borrow_mut() = if ready {
                                    track.seek.map(|anchor| Displayed {
                                        anchor,
                                        navigable: track.navigable.clone(),
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
                                    || limited != texture.limited()
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
                    .when_some(inspector, |viewport, reading| {
                        viewport.child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, cx| {
                                    if let Some(reading) = reading.borrow_mut().take() {
                                        reading.paint(bounds, window, cx);
                                    }
                                },
                            )
                            .absolute()
                            .size_full(),
                        )
                    })
                    .on_scroll(cx.listener(|panel, e, window, cx| panel.wheel(e, window, cx)))
                    .on_mouse_down(MouseButton::Left, cx.listener(Waveform::mouse_down))
                    .context_menu(move |menu, _, _| {
                        let panel = menu_panel.clone();
                        menu.item(
                            PopupMenuItem::new("Show Sample Points")
                                .checked(shown)
                                .on_click(move |_, _, cx| {
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.settings.show_sample_points =
                                            !panel.settings.show_sample_points;
                                        cx.emit(SettingsChanged);
                                        cx.notify();
                                    });
                                }),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod point_tests {
    use super::Settings;
    use waveform::inspection::SamplePoints::{Hidden, Visible};

    #[test]
    fn settings_restore_per_panel_and_gate_transport_transitions() {
        let other = Settings::default();
        let restored: Settings =
            serde_json::from_value(serde_json::json!({"showSamplePoints": true})).unwrap();
        for (playing, scrubbing, points) in [
            (false, false, Visible),
            (true, false, Hidden),
            (false, true, Hidden),
            (true, true, Hidden),
            (false, false, Visible),
        ] {
            assert_eq!(restored.points(playing, scrubbing), points);
            assert_eq!(other.points(playing, scrubbing), Hidden);
        }
        assert_eq!(
            serde_json::to_value(restored).unwrap()["showSamplePoints"],
            true
        );
        assert!(
            !serde_json::from_str::<Settings>("{}")
                .unwrap()
                .show_sample_points
        );
    }
}

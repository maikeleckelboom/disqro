//! The detail deck follows a physical source at a fixed centre. Each instance
//! owns its zoom and may hold a separate position for inspection.
use super::{Home, Instances, Pane, PaneContent, PaneEvent, PanelType, Services};
use crate::{
    audio::{PlaybackIdentity, WaveSeekAnchor},
    overview::{self, Surface, Texture, Viewport},
    player::{Player, PlayerEvent, WaveTrack},
    theme::materials,
    timeline::Source,
    waveform_detail::{Coverage, View},
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

/// Use the timing of this window's current native request. The scheduled
/// refresh slot removes immediate-mode wake jitter from the audio sample.
pub(crate) fn frame_time(now: Instant) -> Instant {
    #[cfg(windows)]
    if let Some(tick) = gpui_windows_gpu::frame_tick() {
        return tick.sample_at;
    }
    now
}

#[cfg(windows)]
fn signed_ns(a: Instant, b: Instant) -> i64 {
    a.saturating_duration_since(b).as_nanos() as i64
        - b.saturating_duration_since(a).as_nanos() as i64
}

/// All decks in the workspace, and the compact waveform's playhead, use one
/// native frame request. Motion updates textures the scene already
/// references. Layout owns bounds and hit testing.
#[derive(Clone, Default)]
pub(crate) struct Motion(Rc<RefCell<MotionState>>);

#[derive(Default)]
struct MotionState {
    panels: Vec<WeakEntity<Waveform>>,
    presented: Vec<WeakEntity<Waveform>>,
    spare: Vec<WeakEntity<Waveform>>,
    /// The compact waveform waiting for the next frame.
    strip: Option<WeakEntity<crate::player::WaveStrip>>,
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

    /// Modifier keys follow keyboard focus, which can be in another panel
    /// while the pointer inspects a sample. Redraw only a registered hit.
    pub(crate) fn inspection_modifiers(&self, window: &Window, cx: &mut App) {
        for panel in &self.0.borrow().presented {
            if let Some(entity) = panel.upgrade() {
                let deck = entity.read(cx);
                if deck.layout.get().is_some_and(|layout| {
                    deck.texture
                        .sample_hit(layout.physical(window.mouse_position()))
                        .is_some()
                }) {
                    cx.notify(panel.entity_id());
                }
            }
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
        drop(state);
        self.arm(window);
    }

    /// Moves the compact waveform's playhead on the next frame, together
    /// with every deck that moves.
    pub(crate) fn request_strip(
        &self,
        strip: WeakEntity<crate::player::WaveStrip>,
        window: &mut Window,
    ) {
        self.0.borrow_mut().strip = Some(strip);
        self.arm(window);
    }

    fn arm(&self, window: &mut Window) {
        let mut state = self.0.borrow_mut();
        if state.armed {
            return;
        }
        state.armed = true;
        drop(state);
        let requested = crate::diagnostics::begin_stage();
        #[cfg(windows)]
        gpui_windows_gpu::present_retained_next_frame(window);
        let motion = self.clone();
        crate::diagnostics::stage(requested, "wave_request", 0, 0);
        window.on_next_frame(move |window, cx| {
            let (panels, strip) = {
                let mut state = motion.0.borrow_mut();
                state.armed = false;
                let spare = std::mem::take(&mut state.spare);
                (
                    std::mem::replace(&mut state.panels, spare),
                    state.strip.take(),
                )
            };
            let mut panels = panels;
            let woke = Instant::now();
            let now = frame_time(woke);
            let start = crate::diagnostics::begin_stage();
            let count = panels.len() as u64;
            crate::diagnostics::stage(requested, "wave_request_to_dispatch", count, 0);
            #[cfg(windows)]
            if let Some(tick) = gpui_windows_gpu::frame_tick() {
                crate::diagnostics::stage(
                    start,
                    "wave_frame_target",
                    signed_ns(tick.intended_at, woke) as u64,
                    tick.period.as_nanos() as u64,
                );
                crate::diagnostics::stage(
                    start,
                    "wave_frame_signal",
                    woke.saturating_duration_since(tick.at).as_nanos() as u64,
                    signed_ns(tick.sample_at, tick.at) as u64,
                );
            }
            for panel in panels.drain(..) {
                let _ = panel.update(cx, |deck, cx| {
                    let panel = cx.entity_id().as_u64();
                    if crate::diagnostics::with_panel(panel, || deck.animate(now, window, cx)) {
                        motion.request(cx.weak_entity(), window);
                    }
                });
            }
            motion.0.borrow_mut().spare = panels;
            if let Some(strip) = strip {
                let _ = strip.update(cx, |strip, cx| {
                    if strip.animate(now, window, cx) {
                        motion.request_strip(cx.weak_entity(), window);
                    }
                });
            }
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
    viewport: Viewport,
    layout: Layout,
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
            self.velocity = Default::default();
        }
        let (frame, advancing, observed_frame) = if let Some(target) = track.scrub_frame {
            (target, false, target)
        } else {
            let previous = self
                .last
                .filter(|(identity, _)| track.identity.continues(*identity))
                .map(|(_, frame)| frame);
            let presented = track.presented_frame(at, previous);
            let (frame, sample) = (presented.frame, presented.sample);
            if let (Some(d), Some(sample)) = (crate::diagnostics::get(), sample) {
                d.wave_clock_age_us
                    .fetch_max(sample.observation_age_ns / 1000, Relaxed);
            }
            if let Some(sample) = sample {
                let start = crate::diagnostics::begin_stage();
                crate::diagnostics::stage(
                    start,
                    "wave_clock",
                    sample.phase_ns as u64,
                    sample.prediction_ns,
                );
                crate::diagnostics::stage(
                    start,
                    "wave_clock_position",
                    frame.to_bits(),
                    sample.observation_age_ns,
                );
            }
            self.last = Some((track.identity, frame));
            (
                frame,
                presented.advancing,
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

/// Per-panel preferences. Workspaces saved with the retired sample ruler
/// still load: unknown keys are ignored and dropped on the next save.
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    show_sample_points: bool,
}

/// What the deck draws over its waveform for the current transport state.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Overlays {
    /// Markers at decoded frame positions.
    sample_points: bool,
    /// Hover measurements, the selection ring and its guide.
    inspection: bool,
}

impl Settings {
    /// Markers stay visible while the user drags the waveform by hand and
    /// hide during ordinary playback. Measurements wait for the gesture to end.
    fn visible(self, playing: bool, scrubbing: bool) -> Overlays {
        let sample_points = self.show_sample_points && (scrubbing || !playing);
        Overlays {
            sample_points,
            inspection: sample_points && !scrubbing,
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
    channel_widths: Rc<RefCell<inspection::ChannelWidths>>,
    /// Markers the last deck paint asked the surface to draw.
    requested_points: Rc<Cell<SamplePoints>>,
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
            channel_widths: Default::default(),
            requested_points: Default::default(),
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

    fn overlays(&self, track: &WaveTrack, cx: &App) -> Overlays {
        self.settings.visible(
            track.playing
                || (self.settings.show_sample_points
                    && track.position(Instant::now()).is_some_and(|p| p.advancing)),
            self.player.read(cx).waveform_grabbed(),
        )
    }

    /// What the deck draws: only the frames a playhead can reach, so audio
    /// of the source beyond them is never shown as if it played next. The
    /// surface's viewport counts from their start.
    fn surface(&self, track: &WaveTrack) -> Surface {
        let reach = track.navigable.clone();
        let viewport = self.position.viewport;
        Surface {
            role: Role::Detail,
            rate: track.source.rate,
            viewport: waveform::Viewport {
                first: viewport.first - reach.start as f64,
                ..viewport
            },
            playable: reach,
        }
    }

    fn prepare(
        &self,
        track: &WaveTrack,
        surface: &Surface,
        view: Option<&waveform::View>,
    ) -> (overview::Evidence, Coverage) {
        // Evidence is read for the reachable frames only, counted from their
        // start, as the surface draws them.
        let reach = surface.playable.clone();
        let viewport = self.position.viewport;
        let mut evidence = overview::Evidence {
            overview: Some(track.overview.clone()),
            overview_start: track.range.start.saturating_sub(reach.start),
            ..Default::default()
        };
        let Some(view) = view else {
            self.detail.clear();
            return (evidence, Coverage::default());
        };
        let (true, Ok(rate)) = (
            track.source.available,
            waveform::SampleRate::new(track.source.rate),
        ) else {
            self.detail.clear();
            return (evidence, Coverage::default());
        };
        let preview = (!track.overview.facts().is_empty()).then(|| waveform::FrameRange {
            start: track.range.start as u64,
            end: (track.range.start + track.overview.frame_count()) as u64,
        });
        let loaded = self
            .detail
            .residency(track.generation, &track.source, preview, |latency| {
                waveform::planning::residency(
                    rate,
                    waveform::FrameRange {
                        start: reach.start as u64,
                        end: reach.end as u64,
                    },
                    view.required_range(),
                    viewport,
                    view.width as f64,
                    self.position.velocity,
                    latency,
                )
            });
        evidence.pages = loaded.pages;
        evidence.samples = loaded.samples;
        (evidence, loaded.coverage)
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
        // Sample markers and any measurement must use the same viewport as
        // the texture in this scene. Keep the retained path at wider zooms.
        let start = crate::diagnostics::begin_stage();
        let surface = self.surface(&track);
        let Some((bounds, view)) = surface.target(layout.bounds, layout.scale) else {
            return false;
        };
        crate::diagnostics::stage(start, "wave_projection", 0, 0);
        let overlays = self.overlays(&track, cx);
        if overlays.sample_points && sample_spacing(&view, layout.scale) {
            cx.notify();
            return false;
        }
        let start = crate::diagnostics::begin_stage();
        let (evidence, coverage) = self.prepare(&track, &surface, Some(&view));
        let status = Self::coverage(coverage, track.source.available);
        crate::diagnostics::stage(
            start,
            "wave_evidence_projection",
            coverage.exact as u64,
            coverage.pending as u64,
        );
        #[cfg(windows)]
        {
            let start = crate::diagnostics::begin_stage();
            self.texture.set_sample_points(SamplePoints::Hidden);
            let ready = self
                .texture
                .repaint(window, cx, view, &evidence, surface.viewport.first);
            crate::diagnostics::stage(start, "wave_retained_submit", u64::from(ready), 0);
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
                    self.position.viewport.frame_at(0.5).to_bits(),
                    self.position.viewport.span.to_bits(),
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
                        self.position.viewport.frame_at(0.5).to_bits(),
                        cx.entity_id().as_u64(),
                    );
                }
                *self.displayed.borrow_mut() = track.seek.map(|anchor| Displayed {
                    anchor,
                    viewport: self.position.viewport,
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
        let surface = self.surface(&track);
        let shown = self.position.viewport;
        let target = self
            .layout
            .get()
            .filter(|_| active)
            .and_then(|layout| surface.target(layout.bounds, layout.scale));
        let panel_id = cx.entity_id().as_u64();
        let (evidence, coverage) = crate::diagnostics::with_panel(panel_id, || {
            let start = crate::diagnostics::begin_stage();
            let (evidence, coverage) =
                self.prepare(&track, &surface, target.as_ref().map(|(_, view)| view));
            crate::diagnostics::stage(
                start,
                "wave_scene",
                self.position.viewport.span.to_bits(),
                coverage.approximate as u64,
            );
            (evidence, coverage)
        });
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
        let overlays = self.overlays(&track, cx);
        let inspector = overlays
            .inspection
            .then(|| Rc::new(RefCell::new(None::<inspection::Reading>)));
        let measurement = inspector.clone();
        let channel_widths = self.channel_widths.clone();
        let requested_points = self.requested_points.clone();
        let inspection_player = self.player.clone();
        let settings = self.settings;
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
                    })
                    .when_some(inspector, |header, reading| {
                        // Layout belongs to the header. Paint after the surface has
                        // confirmed this scene's PCM hit and render succeeded.
                        header.child(deferred(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, cx| {
                                    if let Some(reading) = reading.borrow_mut().take() {
                                        reading.paint(bounds, window, cx);
                                    }
                                },
                            )
                            .flex_1()
                            .min_w_0()
                            .h_full(),
                        ))
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
                                overlays
                                    .inspection
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
                                let points = if overlays.sample_points
                                    && sample_spacing(&view, scale)
                                    && !evidence.samples.is_empty()
                                    && inspection_player.read(cx).inspection_source(&track)
                                {
                                    SamplePoints::Visible
                                } else {
                                    SamplePoints::Hidden
                                };
                                let hitbox = hitbox.filter(|_| {
                                    points != SamplePoints::Hidden
                                        && inspection_player.read(cx).inspection_current(&track)
                                });
                                let hit = hitbox
                                    .as_ref()
                                    .filter(|h| h.is_hovered(window))
                                    .and_then(|_| {
                                        waveform::inspection::hit_sample(
                                            &view,
                                            f64::from(scale),
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
                                let requested =
                                    hit.map_or(points, |h| SamplePoints::Highlight(h.frame));
                                requested_points.set(requested);
                                texture.set_sample_points(requested);
                                let ready = crate::diagnostics::with_panel(panel_id, || {
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
                                        let _ = &texture;
                                        false
                                    };
                                    crate::diagnostics::stage(
                                        start,
                                        "wave_deck_paint",
                                        u64::from(ready),
                                        (scale * 96.).round() as u64,
                                    );
                                    if ready {
                                        crate::diagnostics::stage(
                                            start,
                                            "wave_scene_motion",
                                            view.origin().to_bits(),
                                            view.width as u64,
                                        );
                                        crate::diagnostics::stage(
                                            start,
                                            "wave_scene_playhead",
                                            shown.frame_at(0.5).to_bits(),
                                            shown.span.to_bits(),
                                        );
                                        if start.is_some() {
                                            // Client coordinates of the zoom control for private qualification.
                                            let x = ((f32::from(b.origin.x) + 30.) * scale).round()
                                                as u32;
                                            let y = ((f32::from(b.origin.y) - 16.) * scale).round()
                                                as u32;
                                            crate::diagnostics::stage(
                                                start,
                                                "wave_deck_layout",
                                                (u64::from(x) << 32) | u64::from(y),
                                                0,
                                            );
                                        }
                                    }
                                    ready
                                });
                                if ready && let Some(hitbox) = hitbox {
                                    inspection::listen(
                                        &panel,
                                        texture.clone(),
                                        layout,
                                        hitbox,
                                        hit,
                                        window,
                                    );
                                    if let (Some(hit), Some(measurement)) = (hit, &measurement) {
                                        let chapter = inspection_player
                                            .read(cx)
                                            .inspection_chapter(&track, hit.frame as usize);
                                        let frames = waveform::inspection::visible_frames(
                                            &view,
                                            surface.viewport.first,
                                        );
                                        let first = if frames.is_empty() {
                                            hit.frame
                                        } else {
                                            frames.start
                                        };
                                        let last = frames.end.saturating_sub(1).max(first);
                                        *measurement.borrow_mut() = Some(inspection::Reading {
                                            hit,
                                            chapter,
                                            channel_widths,
                                            samples: evidence.samples,
                                            first_frame: first,
                                            last_frame: last.max(hit.frame),
                                        });
                                    }
                                }
                                *displayed.borrow_mut() = if ready {
                                    track.seek.map(|anchor| Displayed {
                                        anchor,
                                        viewport: shown,
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
                    .on_scroll(cx.listener(|panel, e, window, cx| panel.wheel(e, window, cx)))
                    .on_mouse_down(MouseButton::Left, cx.listener(Waveform::mouse_down))
                    .context_menu(move |menu, _, _| {
                        let panel = menu_panel.clone();
                        menu.item(
                            PopupMenuItem::new("Show Sample Points")
                                .checked(settings.show_sample_points)
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

fn sample_spacing(view: &waveform::View, scale: f32) -> bool {
    SamplePoints::Visible.for_view(view, f64::from(scale)) == SamplePoints::Visible
}

#[cfg(test)]
mod point_tests {
    use super::{Overlays, Settings};

    #[test]
    fn settings_restore_per_panel_and_gate_transport_transitions() {
        let other = Settings::default();
        let restored: Settings =
            serde_json::from_value(serde_json::json!({"showSamplePoints": true})).unwrap();
        for (playing, scrubbing, points, inspection) in [
            (false, false, true, true),
            // Ordinary playback hides markers.
            (true, false, false, false),
            // Manual scrubbing keeps markers but suspends measurements.
            (false, true, true, false),
            (true, true, true, false),
            (false, false, true, true),
        ] {
            let visible = restored.visible(playing, scrubbing);
            assert_eq!(visible.sample_points, points);
            assert_eq!(visible.inspection, inspection);
            assert_eq!(other.visible(playing, scrubbing), Overlays::default());
        }
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::json!({"showSamplePoints": true})
        );
        assert!(
            !serde_json::from_str::<Settings>("{}")
                .unwrap()
                .show_sample_points
        );
    }

    #[test]
    fn workspaces_with_the_retired_sample_ruler_still_load() {
        for (saved, points) in [
            (serde_json::json!({"showSampleRuler": true}), false),
            (
                serde_json::json!({"showSamplePoints": true, "showSampleRuler": true}),
                true,
            ),
        ] {
            let settings: Settings = serde_json::from_value(saved).unwrap();
            assert_eq!(settings.show_sample_points, points);
            assert_eq!(
                serde_json::to_value(settings).unwrap(),
                serde_json::json!({"showSamplePoints": points})
            );
        }
    }
}

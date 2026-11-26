use crate::{
    analysis::{Analysis, Shape, Update},
    artwork,
    audio::{Command, Status, Transport},
    library::{DiscContext, clock},
    location::ActivateLocationControl,
    queue::{PlaylistOccurrence, Queue, Repeat},
    theme::materials,
    timeline::{Program, SECOND},
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Disableable, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
        tooltip::Tooltip,
        v_flex,
    },
    prelude::*,
    *,
};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};

#[path = "player_cursor.rs"]
mod cursor;
#[path = "player_wave.rs"]
mod wave;
use crate::level_meter::LevelMeter;
pub(crate) use wave::WaveStrip;
pub(crate) use wave::WaveTrack;

/// The perspective tabs above the timeline lane.
const PERSPECTIVE_HEIGHT: f32 = 20.;
/// The row holding the recording, transport and listening controls.
const CONTROL_ROW: f32 = 40.;

pub enum PlayerEvent {
    Locate(
        Arc<crate::library::Track>,
        DiscContext,
        crate::browser::Destination,
    ),
    ContinueCd(crate::queue::Entry),
    TrackChanged,
    Notice(String),
    PlaySelection,
    ToggleQueue,
    QueueChanged,
    WaveChanged,
    WaveEvidence,
    /// Play state, position jumps or availability of Previous and Next.
    StateChanged,
}
impl EventEmitter<PlayerEvent> for Player {}

pub(crate) fn reveal_items(
    mut menu: PopupMenu,
    player: &Entity<Player>,
    track: &Arc<crate::library::Track>,
    context: DiscContext,
    occurrence: Option<PlaylistOccurrence>,
) -> PopupMenu {
    use crate::browser::Destination;
    let destinations = occurrence
        .map(|occurrence| {
            (
                Destination::Playlist(occurrence),
                "Show in Playlist",
                IconName::ListMusic,
            )
        })
        .into_iter()
        .chain([
            (Destination::Library, "Show in Library", IconName::Library),
            (
                Destination::Locations,
                "Show in Locations",
                IconName::FolderOpen,
            ),
        ]);
    for (destination, label, icon) in destinations {
        let (player, track) = (player.clone(), track.clone());
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(icon)
                .on_click(move |_, _, cx| {
                    player.update(cx, |_, cx| {
                        cx.emit(PlayerEvent::Locate(track.clone(), context, destination))
                    });
                }),
        );
    }
    menu
}

/// What the system media overlay and the tray describe.
#[derive(Clone, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<Arc<Vec<u8>>>,
}

#[derive(Clone)]
struct Scrub;
impl Render for Scrub {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}
#[derive(Clone)]
struct LevelDrag;
impl Render for LevelDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct Player {
    pub spectra: crate::spectrum_worker::Hub,
    pub can_start: bool,
    pub play_request: crate::store::Ticket,
    transport: Transport,
    meter: Entity<LevelMeter>,
    wave_strip: Entity<wave::WaveStrip>,
    pub show_levels: bool,
    pub status: Status,
    queue: Arc<Queue>,
    pub volume: f32,
    muted_from: Option<f32>,
    level_bounds: Rc<Cell<Bounds<Pixels>>>,
    panel_height: Rc<Cell<Option<Pixels>>>,
    scrub: Option<wave::StripSeek>,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    source_info: artwork::MediaInfo,
    cover_path: Option<PathBuf>,
    cover: Option<Arc<Image>>,
    cover_bytes: Option<Arc<Vec<u8>>>,
    pub full_cd: bool,
    analysis: Analysis,
    generation: u64,
    view_trace: Rc<crate::diagnostics::ViewTrace>,
    program: Option<Arc<Program>>,
    segment: Option<usize>,
    shapes: Vec<Shape>,
    unavailable: Option<String>,
    waveform_scrub: Option<Arc<crate::audio::scrub::Gesture>>,
    visible: bool,
    drawn_position: f64,
    /// Whether the compact waveform moves its playhead by itself.
    cursor_retained: Rc<Cell<bool>>,
    /// Whether the trailing sidebar, home of Queue and History, is shown.
    /// The workspace keeps it current for the player's Queue button.
    pub queue_shown: bool,
}
impl Player {
    pub fn new(
        volume: f32,
        full_cd: bool,
        show_levels: bool,
        listening: Option<crate::listening::Writer>,
        motion: crate::panels::waveform::Motion,
        cx: &mut Context<Self>,
    ) -> Self {
        let transport = Transport::new(volume, listening);
        let meter = cx.new(|_| LevelMeter::new(transport.signal.clone(), Axis::Horizontal));
        let player = cx.entity();
        let cursor_retained = Rc::new(Cell::new(false));
        let retained = cursor_retained.clone();
        let wave_strip = cx.new(|cx| wave::WaveStrip::new(player, motion, retained, cx));
        let events = transport.changed.clone();
        cx.spawn(async move |this, cx| {
            while events.recv().await.is_ok() {
                if this.update(cx, |p, cx| p.sync(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let (analysis, updates) = Analysis::new();
        cx.spawn(async move |this, cx| {
            while let Ok((generation, event)) = updates.recv().await {
                if this
                    .update(cx, |p, cx| p.wave_update(generation, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            can_start: false,
            spectra: crate::spectrum_worker::Hub::new(transport.signal.pcm()),
            play_request: transport.sender().play_request,
            transport,
            meter,
            wave_strip,
            show_levels,
            status: Status::default(),
            queue: Arc::default(),
            volume,
            muted_from: None,
            level_bounds: Default::default(),
            panel_height: Default::default(),
            scrub: None,
            seek_bounds: Default::default(),
            source_info: Default::default(),
            cover_path: None,
            cover: None,
            cover_bytes: None,
            full_cd,
            analysis,
            generation: 0,
            view_trace: Default::default(),
            program: None,
            segment: None,
            shapes: vec![],
            unavailable: None,
            waveform_scrub: None,
            visible: true,
            drawn_position: 0.,
            cursor_retained,
            queue_shown: true,
        }
    }
    pub fn command(&mut self, c: Command, cx: &mut Context<Self>) {
        if matches!(c, Command::Toggle | Command::SetPlaying(true))
            && self.status.track.is_none()
            && self.queue.upcoming().is_empty()
        {
            cx.emit(PlayerEvent::PlaySelection);
        } else if let Err(e) = self.transport.send(c, self.status.entry) {
            self.status.error = Some(e.to_string());
            cx.notify();
        }
    }
    /// The volume to remember: muting is temporary.
    pub fn saved_volume(&self) -> f32 {
        self.muted_from.unwrap_or(self.volume)
    }
    pub fn queue(&self) -> &Arc<Queue> {
        &self.queue
    }
    pub fn playing_entry(&self) -> Option<&crate::queue::Entry> {
        self.queue
            .current_entry()
            .filter(|e| Some(e.id) == self.status.entry)
    }
    /// A sender for control paths outside the window, such as media keys.
    pub fn sender(&self) -> crate::audio::Commands {
        self.transport.sender()
    }
    /// Hidden windows suppress position-only redraws while playback continues.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.transport.set_quiet(!visible);
        self.sync_meter(cx);
        if visible {
            self.sync(cx);
            if self.program.is_none() {
                self.request_wave();
            }
            cx.notify();
        }
    }
    pub fn set_show_levels(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.show_levels = shown;
        self.sync_meter(cx);
        cx.notify();
    }

    fn sync_meter(&self, cx: &mut Context<Self>) {
        let active = self.visible && self.show_levels && self.status.playing;
        self.meter.update(cx, |m, cx| m.sample(active, cx));
    }
    pub fn now_playing(&self) -> Option<NowPlaying> {
        let t = self.status.track.as_deref()?;
        let info = (t.disc.is_none()).then_some(&self.source_info);
        Some(NowPlaying {
            title: info
                .and_then(|i| i.title.clone())
                .unwrap_or_else(|| t.title.clone()),
            artist: info
                .and_then(|i| i.artist.clone())
                .unwrap_or_else(|| t.artist.clone()),
            album: info
                .and_then(|i| i.album.clone())
                .unwrap_or_else(|| t.album.clone()),
            cover: self.cover_bytes.clone(),
        })
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (status, queue) = self.transport.state();
        let closed_scrub = self
            .waveform_scrub
            .as_ref()
            .is_some_and(|scrub| scrub.closed());
        if closed_scrub {
            self.waveform_scrub = None;
        }
        if status == self.status {
            if closed_scrub {
                cx.emit(PlayerEvent::WaveChanged);
            }
            return;
        }
        if status.scrub_error != self.status.scrub_error
            && let Some((_, message)) = &status.scrub_error
        {
            cx.emit(PlayerEvent::Notice(message.clone()));
        }
        if status.skipped != self.status.skipped
            && let Some((_, text)) = &status.skipped
        {
            cx.emit(PlayerEvent::Notice(text.clone()));
        }
        let old_context = self.playing_entry().map(|e| e.disc_context);
        let new_context = queue.current_entry().map(|e| e.disc_context);
        let context_changed = old_context != new_context
            && status.track.as_ref().is_some_and(|t| !t.has_disc_context());
        let queue_changed = status.queue_revision != self.status.queue_revision;
        if queue_changed {
            self.queue = queue;
            cx.emit(PlayerEvent::QueueChanged);
        }
        let path = status.track.as_ref().map(|t| &t.source);
        if path != self.cover_path.as_ref() {
            self.cover_path = path.cloned();
            self.cover = None;
            self.cover_bytes = None;
            self.source_info = Default::default();
            if let Some(path) = path.cloned() {
                cx.spawn(async move |this, cx| {
                    let file = path.clone();
                    let mut info = cx
                        .background_spawn(async move { artwork::for_source(&file) })
                        .await;
                    let _ = this.update(cx, |p, cx| {
                        if p.cover_path.as_ref() == Some(&path) {
                            let bytes = info.cover.take().map(Arc::new);
                            p.cover = bytes
                                .as_ref()
                                .map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b.to_vec())));
                            p.cover_bytes = bytes;
                            p.source_info = info;
                            cx.emit(PlayerEvent::TrackChanged);
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        let track_changed = context_changed
            || self.status.track.as_deref() != status.track.as_deref()
            || self.status.entry != status.entry
            || self.status.source_revision != status.source_revision
            || self.status.source_object != status.source_object;
        let state_changed = self.status.playback_state() != status.playback_state()
            || self.status.next != status.next
            || self.status.previous != status.previous
            || (self.status.position - status.position).abs() > 1.5;
        let position_only = !track_changed && {
            let mut a = self.status.clone();
            a.position = status.position;
            if a.scrub.map(|position| position.id) == status.scrub.map(|position| position.id) {
                a.scrub = status.scrub;
            }
            a == status
        };
        let _transition =
            track_changed.then(|| crate::diagnostics::Timer::measure(|d| &d.transition_us));
        let identity_changed = self.status.playback_identity() != status.playback_identity();
        let wave_changed = closed_scrub
            || track_changed
            || queue_changed
            || identity_changed
            || self.status.scrub.map(|position| position.id)
                != status.scrub.map(|position| position.id)
            || self.status.playing != status.playing
            || (!status.playing && self.status.position != status.position);
        self.status = status;
        if self.show_levels && self.visible {
            self.sync_meter(cx);
        }
        if track_changed {
            self.segment = self
                .program
                .as_ref()
                .zip(self.status.track.as_ref())
                .and_then(|(p, t)| {
                    p.index(t).filter(|&i| {
                        !context_changed
                            && self
                                .status
                                .source_revision
                                .as_ref()
                                .is_none_or(|r| r == &p.segments[i].source.stamp)
                            && self
                                .status
                                .source_object
                                .is_none_or(|file| Some(file) == p.segments[i].source.object)
                    })
                });
            if let Some(i) = self.segment {
                self.analysis.focus(i);
            } else {
                self.request_wave();
            }
            cx.emit(PlayerEvent::TrackChanged);
        }
        if identity_changed || track_changed || self.status.scrub.is_some() {
            self.scrub = None;
        }
        if state_changed {
            cx.emit(PlayerEvent::StateChanged);
        }
        if wave_changed {
            cx.emit(PlayerEvent::WaveChanged);
        }
        // Redraw for position alone only when the clock turns over. The
        // compact waveform moves its own playhead at the display's rate; only
        // where it cannot does position redraw whenever the playhead moves on
        // screen.
        let (shown_position, shown_duration) = self.wave_time();
        let redraw = !position_only || shown_position.floor() != self.drawn_position.floor() || {
            let width = f64::from(f32::from(self.seek_bounds.get().size.width)).max(1.);
            let moved = (shown_position - self.drawn_position).abs();
            !self.cursor_retained.get() && moved * width / shown_duration.max(1.) >= 0.75
        };
        if redraw && (self.visible || !position_only) {
            self.drawn_position = shown_position;
            cx.notify();
        }
    }
    fn set_volume(&mut self, v: f32, cx: &mut Context<Self>) {
        self.apply_volume(v.clamp(0., 1.), None, cx);
    }
    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if self.volume > 0. {
            self.apply_volume(0., Some(self.volume), cx);
        } else {
            self.apply_volume(self.muted_from.unwrap_or(0.65), None, cx);
        }
    }
    fn apply_volume(&mut self, volume: f32, muted_from: Option<f32>, cx: &mut Context<Self>) {
        match self
            .transport
            .send(Command::Volume(volume), self.status.entry)
        {
            Ok(()) => {
                self.volume = volume;
                self.muted_from = muted_from;
                cx.emit(PlayerEvent::StateChanged);
            }
            Err(e) => self.status.error = Some(e.to_string()),
        }
        cx.notify();
    }
    fn fraction_at(&self, x: Pixels, bounds: Bounds<Pixels>) -> f32 {
        let w = f32::from(bounds.size.width).max(1.);
        (f32::from(x - bounds.origin.x) / w).clamp(0., 1.)
    }
    fn key(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        enabled: bool,
        cmd: fn() -> Command,
        cx: &mut Context<Self>,
    ) -> Button {
        let m = materials(cx);
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(icon).size(px(17.)).text_color(m.text))
            .accessibility_label(label)
            .tooltip(label)
            .disabled(!enabled)
            .on_click(cx.listener(move |p, _, _, cx| p.command(cmd(), cx)))
    }
    fn mode(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        on: bool,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Button {
        let m = materials(cx);
        let cmd = Rc::new(Cell::new(Some(cmd)));
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(icon).size(px(15.)).text_color(if on {
                m.accent
            } else {
                m.text_faint
            }))
            .accessibility_label(label)
            .tooltip(label)
            .on_click(cx.listener(move |p, _, _, cx| {
                if let Some(c) = cmd.take() {
                    p.command(c, cx)
                }
            }))
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::diagnostics::Timer::measure(|d| &d.ui_us);
        crate::diagnostics::stage(crate::diagnostics::begin_stage(), "player_render", 0, 0);
        let m = *materials(cx);
        let s = &self.status;
        let active = s.track.is_some();
        let (position, duration) = self.wave_time();
        let shown_position = self.scrub_fraction().map_or(position, |f| f * duration);

        let mut art = div()
            .size(px(CONTROL_ROW))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(px(4.))
            .bg(m.control)
            .flex()
            .justify_center()
            .items_center();
        art = if let Some(cover) = &self.cover {
            art.child(img(cover.clone()).size_full().object_fit(ObjectFit::Cover))
        } else {
            art.child(
                Icon::new(IconName::Disc3)
                    .size(px(20.))
                    .text_color(m.text_faint),
            )
        };

        use crate::browser::Destination;
        let disc_context = self
            .playing_entry()
            .map_or_default(|entry| entry.disc_context);
        let occurrence = self
            .playing_entry()
            .and_then(|entry| entry.playlist_occurrence);
        let playing_destination = Destination::Current(occurrence);
        let art = art
            .id("playing-artwork")
            .relative()
            .when_some(s.track.clone(), |art, track| {
                let label = "Go to Playing Track";
                let keyboard_track = track.clone();
                art.role(Role::Button)
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .bottom_0()
                            .size(px(16.))
                            .bg(m.canvas.opacity(0.9))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::LocateFixed)
                                    .size(px(12.))
                                    .text_color(m.text),
                            ),
                    )
                    .aria_label(label)
                    .tab_index(0)
                    .key_context("LocationControl")
                    .focus_visible(|d| d.border_1().border_color(m.accent))
                    .cursor_pointer()
                    .tooltip(move |w, cx| Tooltip::new(label).build(w, cx))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(PlayerEvent::Locate(
                            track.clone(),
                            disc_context,
                            playing_destination,
                        ));
                    }))
                    .on_action(cx.listener(move |_, _: &ActivateLocationControl, _, cx| {
                        cx.emit(PlayerEvent::Locate(
                            keyboard_track.clone(),
                            disc_context,
                            playing_destination,
                        ));
                    }))
            });
        let reveal_menu = s.track.clone().map(|track| {
            let player = cx.entity();
            let source = self.queue.source.clone();
            let entry = self
                .playing_entry()
                .filter(|entry| entry.disc_context.allows(&entry.track))
                .cloned();
            Button::new("now-playing-menu")
                .ghost()
                .xsmall()
                .icon(
                    Icon::new(IconName::Ellipsis)
                        .size(px(16.))
                        .text_color(m.text_muted),
                )
                .accessibility_label("Now Playing actions")
                .tooltip("Now Playing actions")
                .dropdown_menu(move |menu, _, _| {
                    let menu = menu.when_some(source.as_ref(), |menu, label| {
                        menu.item(PopupMenuItem::label(format!("Started from {label}")))
                            .separator()
                    });
                    reveal_items(menu, &player, &track, disc_context, occurrence).when_some(
                        entry.clone(),
                        |menu, entry| {
                            let player = player.clone();
                            menu.item(PopupMenuItem::new("Continue This CD").on_click(
                                move |_, _, cx| {
                                    player.update(cx, |_, cx| {
                                        cx.emit(PlayerEvent::ContinueCd(entry.clone()))
                                    });
                                },
                            ))
                        },
                    )
                })
        });

        let now = self.now_playing();
        let title = now
            .as_ref()
            .map(|n| n.title.clone())
            .unwrap_or_else(|| "Nothing playing".into());
        let subtitle = s.error.clone().unwrap_or_else(|| match &now {
            Some(_) if s.scrub.is_some() => "Scrubbing".into(),
            None => "Double-click a track to start listening".into(),
            Some(n) => {
                let parts: Vec<&str> = [n.artist.as_str(), n.album.as_str()]
                    .into_iter()
                    .filter(|p| !p.is_empty())
                    .collect();
                if parts.is_empty() {
                    s.track.as_ref().map(|t| t.format()).unwrap_or_default()
                } else {
                    parts.join("  ·  ")
                }
            }
        });

        let play_will_pause = s.scrub.map_or(s.playing, |scrub| scrub.was_playing);
        let play = div()
            .id("play-pause")
            .role(Role::Button)
            .aria_label(if play_will_pause { "Pause" } else { "Play" })
            .size(px(34.))
            .flex_shrink_0()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(m.control)
            .text_color(m.text)
            .when(active || self.can_start, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(m.control_hover))
                    .on_click(cx.listener(|p, _, _, cx| p.command(Command::Toggle, cx)))
            })
            .when(!active && !self.can_start, |d| d.opacity(0.4))
            .tooltip(|w, cx| Tooltip::new("Play or pause · Space").build(w, cx))
            .child(
                Icon::new(if play_will_pause {
                    IconName::Pause
                } else {
                    IconName::Play
                })
                .size(px(16.)),
            );

        let transport = h_flex()
            .flex_shrink_0()
            .gap(px(2.))
            .items_center()
            .child(self.mode(
                "shuffle",
                IconName::Shuffle,
                if s.shuffle {
                    "Shuffle on"
                } else {
                    "Shuffle off"
                },
                s.shuffle,
                Command::Shuffle(!s.shuffle),
                cx,
            ))
            .child(self.key(
                "previous",
                IconName::SkipBack,
                "Previous",
                s.previous,
                || Command::Step(-1),
                cx,
            ))
            .child(div().px(px(4.)).child(play))
            .child(self.key(
                "next",
                IconName::SkipForward,
                "Next",
                s.next,
                || Command::Step(1),
                cx,
            ))
            .child(self.mode(
                "repeat",
                if s.repeat == Repeat::One {
                    IconName::Repeat1
                } else {
                    IconName::Repeat
                },
                match s.repeat {
                    Repeat::Off => "Repeat off",
                    Repeat::All => "Repeat all",
                    Repeat::One => "Repeat one",
                },
                s.repeat != Repeat::Off,
                Command::Repeat(s.repeat.cycle()),
                cx,
            ));

        let muted = self.volume <= 0.;
        let volume = h_flex()
            .flex_shrink_0()
            .gap_1()
            .items_center()
            .child(
                Button::new("mute")
                    .ghost()
                    .xsmall()
                    .icon(
                        Icon::new(if muted {
                            IconName::VolumeX
                        } else if self.volume < 0.5 {
                            IconName::Volume1
                        } else {
                            IconName::Volume2
                        })
                        .size(px(15.))
                        .text_color(m.text_muted),
                    )
                    .accessibility_label(if muted { "Unmute" } else { "Mute" })
                    .tooltip(if muted { "Unmute" } else { "Mute" })
                    .on_click(cx.listener(|p, _, _, cx| p.toggle_mute(cx))),
            )
            .child({
                let bounds = self.level_bounds.clone();
                let v = self.volume;
                div()
                    .id("volume")
                    .relative()
                    .w(px(84.))
                    .h(px(20.))
                    .cursor_pointer()
                    .tooltip(move |w, cx| {
                        Tooltip::new(format!("Volume {}%", (v * 100.).round())).build(w, cx)
                    })
                    .child(
                        canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(8.5))
                            .h(px(3.))
                            .rounded_full()
                            .bg(m.control_hover)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(v))
                                    .rounded_full()
                                    .bg(m.text_muted),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(5.))
                            .left(relative(v))
                            .ml(px(-5.))
                            .size(px(10.))
                            .rounded_full()
                            .bg(m.text),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|p, e: &MouseDownEvent, _, cx| {
                            let f = p.fraction_at(e.position.x, p.level_bounds.get());
                            p.set_volume(f, cx);
                        }),
                    )
                    .on_drag(LevelDrag, |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| LevelDrag)
                    })
                    .on_drag_move(cx.listener(|p, e: &DragMoveEvent<LevelDrag>, _, cx| {
                        let f = p.fraction_at(e.event.position.x, e.bounds);
                        p.set_volume(f, cx);
                    }))
                    .on_scroll(cx.listener(|p, e: &ScrollEvent, w, cx| {
                        // One wheel notch moves about 5%; touchpads move smoothly.
                        let dy = f32::from(e.delta.pixel_delta(w.line_height()).y);
                        let step = dy / (f32::from(w.line_height()) * 20.);
                        if step != 0. {
                            p.set_volume(p.volume + step, cx);
                        }
                    }))
            });

        let volume = v_flex()
            .flex_shrink_0()
            .child(volume)
            .when(self.show_levels, |d| d.child(self.meter.clone()));

        let strip = self.wave_strip.clone();
        let seek = div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(if window.is_a11y_active() {
                strip.into_any_element()
            } else {
                strip
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element()
            });
        let time = |t: f64| {
            div()
                .flex_shrink_0()
                .w(px(44.))
                .text_size(px(crate::theme::size::TEXT_SMALL))
                .text_color(m.text_faint)
                .child(clock(t))
        };
        let queue_shown = self.queue_shown;
        let queue_label = if queue_shown {
            "Hide Queue · Ctrl+Alt+Right"
        } else {
            "Show Queue · Ctrl+Alt+Right"
        };
        let queue = Button::new("toggle-queue")
            .ghost()
            .xsmall()
            .icon(
                Icon::new(IconName::ListMusic)
                    .size(px(16.))
                    .text_color(if queue_shown { m.accent } else { m.text_muted }),
            )
            .accessibility_label(queue_label)
            .tooltip(queue_label)
            .toggled(queue_shown)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PlayerEvent::ToggleQueue)));

        let height = self.panel_height.clone();
        let player = cx.entity_id();
        // The dock can clip its 100px minimum panel in a shorter window.
        // Measure the visible slot independently of the player content.
        let measure = canvas(
            move |b, w, _| {
                let h = b.intersect(&w.content_mask().bounds).size.height;
                if h > px(0.) && height.replace(Some(h)) != Some(h) {
                    w.on_next_frame(move |_, cx| cx.notify(player));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        // Three zones of equal flexibility keep the transport centred in the
        // window: the recording on the left, playback in the middle, and the
        // listening controls on the right. The timeline runs the full width
        // below them and takes whatever height the player is given.
        let identity = h_flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w_0()
            .gap(px(10.))
            .items_center()
            .child(art)
            .child(
                v_flex()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(px(6.))
                            .items_center()
                            .child(
                                div()
                                    .id("now-playing-title")
                                    .role(Role::Label)
                                    .aria_value(title.clone())
                                    .truncate()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(m.text)
                                    .child(title),
                            )
                            .children(reveal_menu),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(crate::theme::size::TEXT_SMALL))
                            .text_color(if s.error.is_some() {
                                m.danger
                            } else {
                                m.text_muted
                            })
                            .child(subtitle),
                    ),
            );
        let listening = h_flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w_0()
            .gap(px(6.))
            .items_center()
            .justify_end()
            .child(queue)
            .child(volume);
        let lane = h_flex()
            .w_full()
            .flex_1()
            .min_h(px(22.))
            .px(px(10.))
            .gap(px(8.))
            .items_center()
            .bg(m.canvas)
            .rounded(px(6.))
            .rounded_tl(px(0.))
            .child(time(shown_position).text_color(m.text_muted))
            .child(seek)
            .child(time(duration).text_right());
        let content = v_flex()
            .w_full()
            .h(self.panel_height.get().unwrap_or(px(100.)))
            .bg(m.raised)
            .px(px(12.))
            .pt(px(6.))
            .pb(px(8.))
            .child(
                h_flex()
                    .w_full()
                    .h(px(CONTROL_ROW))
                    .flex_shrink_0()
                    .gap(px(16.))
                    .items_center()
                    .child(identity)
                    .child(transport)
                    .child(listening),
            )
            .child(div().mt(px(4.)).flex_shrink_0().child(self.perspectives(
                self.overview(),
                &m,
                cx,
            )))
            .child(lane);
        div()
            .id("player")
            .relative()
            .size_full()
            .bg(m.raised)
            .child(measure)
            .child(content)
    }
}

use crate::{
    artwork,
    audio::{Command, Status, Transport},
    library::clock,
    overview::Bin,
    queue::{Queue, Repeat},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Disableable, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{ContextMenuExt, PopupMenuItem},
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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub enum PlayerEvent {
    Locate(Arc<crate::library::Track>, crate::browser::Destination),
    ContinueCd(crate::queue::Entry),
    TrackChanged,
    PlaySelection,
    QueueChanged,
    /// Play state, position jumps or availability of Previous and Next.
    StateChanged,
}
impl EventEmitter<PlayerEvent> for Player {}

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
    pub can_start: bool,
    pub play_request: crate::store::Ticket,
    transport: Transport,
    pub status: Status,
    queue: Arc<Queue>,
    pub volume: f32,
    muted_from: Option<f32>,
    level_bounds: Rc<Cell<Bounds<Pixels>>>,
    scrub: Option<f32>,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    source_info: artwork::MediaInfo,
    cover_path: Option<PathBuf>,
    cover: Option<Arc<Image>>,
    cover_bytes: Option<Arc<Vec<u8>>>,
    peaks: Option<Arc<[Bin]>>,
    analysis: Arc<AtomicU64>,
    analysed: Option<u64>,
    visible: bool,
    drawn_position: f64,
}
impl Player {
    pub fn new(volume: f32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        let transport = Transport::new(volume);
        let events = transport.changed.clone();
        cx.spawn(async move |this, cx| {
            while events.recv().await.is_ok() {
                if this.update(cx, |p, cx| p.sync(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            can_start: false,
            play_request: Default::default(),
            transport,
            status: Status::default(),
            queue: Arc::default(),
            volume,
            muted_from: None,
            level_bounds: Default::default(),
            scrub: None,
            seek_bounds: Default::default(),
            source_info: Default::default(),
            cover_path: None,
            cover: None,
            cover_bytes: None,
            peaks: None,
            analysis: Default::default(),
            analysed: None,
            visible: true,
            drawn_position: 0.,
        }
    }
    pub fn command(&mut self, c: Command, cx: &mut Context<Self>) {
        if matches!(
            c,
            Command::Play(..)
                | Command::PlayDisc(..)
                | Command::Jump(_)
                | Command::Stop
                | Command::Step(_)
        ) {
            self.play_request.next();
        }
        if matches!(c, Command::Toggle | Command::SetPlaying(true))
            && self.status.track.is_none()
            && self.queue.entries.is_empty()
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
    /// While hidden, the player stops drawing and analysing; playback and
    /// media controls carry on.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.transport.set_quiet(!visible);
        if !visible && self.peaks.is_none() {
            self.analysis.fetch_add(1, Ordering::Relaxed);
            self.analysed = None;
        }
        if visible {
            self.sync(cx);
            self.analyse(cx);
            cx.notify();
        }
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
        if status == self.status {
            return;
        }
        if status.queue_revision != self.status.queue_revision {
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
        let track_changed = self.status.track.as_deref() != status.track.as_deref()
            || self.status.entry != status.entry;
        let state_changed = self.status.playing != status.playing
            || self.status.next != status.next
            || self.status.previous != status.previous
            || (self.status.position - status.position).abs() > 1.5;
        let position_only = !track_changed && {
            let mut a = self.status.clone();
            a.position = status.position;
            a == status
        };
        self.status = status;
        if track_changed {
            self.analysis.fetch_add(1, Ordering::Relaxed);
            self.peaks = None;
            self.analysed = None;
            self.analyse(cx);
            cx.emit(PlayerEvent::TrackChanged);
        }
        if state_changed {
            cx.emit(PlayerEvent::StateChanged);
        }
        // Redraw for position alone only when the playhead moves on screen or
        // the clock turns over; a long track then redraws a few times a second
        // instead of on every transport tick.
        let redraw = !position_only || {
            let width = f64::from(f32::from(self.seek_bounds.get().size.width)).max(1.);
            let moved = (self.status.position - self.drawn_position).abs();
            let pixels = moved * width / self.status.duration.max(1.);
            pixels >= 0.75 || self.status.position.floor() != self.drawn_position.floor()
        };
        if redraw && (self.visible || !position_only) {
            self.drawn_position = self.status.position;
            cx.notify();
        }
    }
    fn analyse(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let Some(item) = self.status.track.clone() else {
            return;
        };
        let Some(entry) = self.status.entry else {
            return;
        };
        if self.analysed == Some(entry) {
            return;
        }
        self.analysed = Some(entry);
        let revision = self.analysis.fetch_add(1, Ordering::Relaxed) + 1;
        let generation = self.analysis.clone();
        let cache = crate::profile::location().and_then(|p| p.parent().map(|p| p.join("Cache")));
        cx.spawn(async move |this, cx| {
            let peaks = cx
                .background_spawn(async move {
                    crate::overview::load(&item, cache, &generation, revision)
                })
                .await;
            let _ = this.update(cx, |p, cx| {
                if p.analysis.load(Ordering::Relaxed) == revision {
                    p.peaks = peaks;
                    cx.notify();
                }
            });
        })
        .detach();
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let s = &self.status;
        let active = s.track.is_some();
        let progress = self.scrub.unwrap_or(if s.duration > 0. {
            (s.position / s.duration) as f32
        } else {
            0.
        });
        let shown_position = self.scrub.map_or(s.position, |f| f64::from(f) * s.duration);

        let mut art = div()
            .size(px(72.))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(px(6.))
            .bg(m.control)
            .flex()
            .justify_center()
            .items_center();
        art = if let Some(cover) = &self.cover {
            art.child(img(cover.clone()).size_full().object_fit(ObjectFit::Cover))
        } else {
            art.child(
                Icon::new(IconName::Disc3)
                    .size(px(28.))
                    .text_color(m.text_faint),
            )
        };

        let art = if let Some(track) = s.track.clone() {
            use crate::browser::Destination;
            let player = cx.entity();
            let click = track.clone();
            let entry = self
                .playing_entry()
                .filter(|e| e.disc_context.allows(&e.track))
                .cloned();
            let (destination, hint) = if entry.is_some() {
                (Destination::Disc, "Open playing CD")
            } else {
                (Destination::Collection, "Show in Collection")
            };
            art.id("open-playing-cd")
                .cursor_pointer()
                .tooltip(move |w, cx| Tooltip::new(hint).build(w, cx))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(PlayerEvent::Locate(click.clone(), destination))
                }))
                .context_menu(move |menu, _, _| {
                    let (p, t) = (player.clone(), track.clone());
                    menu.item(
                        PopupMenuItem::new("Show in Collection").on_click(move |_, _, cx| {
                            p.update(cx, |_, cx| {
                                cx.emit(PlayerEvent::Locate(t.clone(), Destination::Collection))
                            })
                        }),
                    )
                    .when_some(entry.clone(), |menu, entry| {
                        let p = player.clone();
                        menu.item(PopupMenuItem::new("Continue This CD").on_click(
                            move |_, _, cx| {
                                p.update(cx, |_, cx| {
                                    cx.emit(PlayerEvent::ContinueCd(entry.clone()))
                                })
                            },
                        ))
                    })
                })
                .into_any_element()
        } else {
            art.into_any_element()
        };
        let now = self.now_playing();
        let title = now
            .as_ref()
            .map(|n| n.title.clone())
            .unwrap_or_else(|| "Nothing playing".into());
        let subtitle = s.error.clone().unwrap_or_else(|| match &now {
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

        let play = div()
            .id("play-pause")
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
                Icon::new(if s.playing {
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

        let bounds = self.seek_bounds.clone();
        let seek = div()
            .id("seek")
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(34.))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .map(|d| match self.peaks.clone() {
                Some(peaks) => d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(crate::overview::paint(peaks, progress, m.wave)),
                ),
                // Before the overview is ready, a quiet progress line.
                None => d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(16.))
                        .h(px(2.))
                        .rounded_full()
                        .bg(m.wave.ahead_body)
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(relative(progress))
                                .bg(m.wave.played_body),
                        ),
                ),
            })
            .when(active, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(progress))
                        .w(px(2.))
                        .ml(px(-1.))
                        .rounded_full()
                        .bg(m.wave.cursor),
                )
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|p, e: &MouseDownEvent, _, cx| {
                        p.scrub = Some(p.fraction_at(e.position.x, p.seek_bounds.get()));
                        cx.notify();
                    }),
                )
                .on_drag(Scrub, |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| Scrub)
                })
                .on_drag_move(cx.listener(|p, e: &DragMoveEvent<Scrub>, _, cx| {
                    p.scrub = Some(p.fraction_at(e.event.position.x, e.bounds));
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|p, _, _, cx| {
                        if let Some(f) = p.scrub.take() {
                            p.command(Command::Seek(f64::from(f)), cx);
                        }
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|p, _, _, cx| {
                        if let Some(f) = p.scrub.take() {
                            p.command(Command::Seek(f64::from(f)), cx);
                        }
                    }),
                )
            });

        let time = |t: f64| {
            div()
                .flex_shrink_0()
                .w(px(44.))
                .text_size(px(11.5))
                .text_color(m.text_faint)
                .child(clock(t))
        };

        h_flex()
            .id("player")
            .size_full()
            .min_h(px(96.))
            .bg(m.raised)
            .px(px(16.))
            .gap(px(14.))
            .items_center()
            .child(art)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .w_full()
                            .gap(px(12.))
                            .items_center()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(1.))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(m.text)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(if s.error.is_some() {
                                                m.danger
                                            } else {
                                                m.text_muted
                                            })
                                            .child(subtitle),
                                    ),
                            )
                            .child(transport)
                            .child(volume),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap(px(8.))
                            .items_center()
                            .child(time(shown_position).text_color(m.text_muted))
                            .child(seek)
                            .child(time(s.duration).text_right()),
                    ),
            )
    }
}

use crate::{
    artwork,
    audio::{Command, Status, Transport},
    library::clock,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        slider::{Slider, SliderEvent, SliderState},
        v_flex,
    },
    prelude::*,
    *,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub enum PlayerEvent {
    TrackChanged,
    PlaySelection,
}
impl EventEmitter<PlayerEvent> for Player {}
pub struct Player {
    pub can_start: bool,
    transport: Transport,
    pub status: Status,
    pub volume: f32,
    level: Entity<SliderState>,
    seek: Entity<SliderState>,
    scrubbing: bool,
    source_info: artwork::MediaInfo,
    cover_path: Option<PathBuf>,
    cover: Option<Arc<Image>>,
    peaks: Option<Arc<[[f32; 2]]>>,
    analysis: Arc<AtomicU64>,
}
impl Player {
    pub fn new(volume: f32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let level = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(volume * 100.)
        });
        let seek = cx.new(|_| SliderState::new().min(0.).max(1000.).step(1.));
        cx.subscribe(&level, |p: &mut Self, _, e, cx| {
            if let SliderEvent::Change(v) = e {
                p.volume = v.start() / 100.;
                p.transport.send(Command::Volume(p.volume));
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&seek, window, |p: &mut Self, _, e, _, cx| match e {
            SliderEvent::Change(_) => p.scrubbing = true,
            SliderEvent::Release(v) => {
                p.scrubbing = false;
                p.command(Command::Seek(f64::from(v.start() / 1000.)), cx);
            }
        })
        .detach();
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
            transport,
            status: Status::default(),
            volume,
            level,
            seek,
            scrubbing: false,
            source_info: Default::default(),
            cover_path: None,
            cover: None,
            peaks: None,
            analysis: Default::default(),
        }
    }
    pub fn command(&mut self, c: Command, cx: &mut Context<Self>) {
        if matches!(c, Command::Toggle) && self.status.track.is_none() {
            cx.emit(PlayerEvent::PlaySelection);
        } else {
            self.transport.send(c);
        }
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let status = self.transport.status();
        if status == self.status {
            return;
        }
        let path = status.track.as_ref().map(|t| &t.source);
        if path != self.cover_path.as_ref() {
            self.cover_path = path.cloned();
            self.cover = None;
            self.source_info = Default::default();
            if let Some(path) = path.cloned() {
                cx.spawn(async move |this, cx| {
                    let file = path.clone();
                    let mut info = cx
                        .background_spawn(async move { artwork::for_source(&file) })
                        .await;
                    let _ = this.update(cx, |p, cx| {
                        if p.cover_path.as_ref() == Some(&path) {
                            p.cover = info
                                .cover
                                .take()
                                .map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b)));
                            p.source_info = info;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        if self.status.track.as_deref() != status.track.as_deref() {
            cx.emit(PlayerEvent::TrackChanged);
            self.peaks = None;
            let revision = self.analysis.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(item) = status.track.clone() {
                let generation = self.analysis.clone();
                let cache =
                    crate::profile::location().and_then(|p| p.parent().map(|p| p.join("Cache")));
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
        }
        self.status = status;
        cx.notify();
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
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(icon))
            .accessibility_label(label)
            .tooltip(label)
            .disabled(!enabled)
            .on_click(cx.listener(move |p, _, _, cx| p.command(cmd(), cx)))
    }
}
impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.status;
        let active = s.track.is_some();
        if !self.scrubbing && s.duration > 0. {
            let v = (s.position / s.duration * 1000.) as f32;
            if (self.seek.read(cx).value().start() - v).abs() > 0.25 {
                self.seek.update(cx, |s, cx| s.set_value(v, window, cx));
            }
        }
        let muted = cx.theme().muted_foreground;
        let mut art = div()
            .size(px(72.))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded_md()
            .bg(cx.theme().secondary)
            .flex()
            .justify_center()
            .items_center();
        art = if let Some(cover) = &self.cover {
            art.child(
                img(cover.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
        } else {
            art.child(Icon::new(IconName::Disc3).size(px(29.)).text_color(muted))
        };
        let track = s.track.as_deref();
        let info = track
            .filter(|t| t.disc.is_none())
            .map(|_| &self.source_info);
        let title = info
            .and_then(|i| i.title.as_deref())
            .or_else(|| track.map(|t| t.title.as_str()))
            .unwrap_or("Choose a recording")
            .to_owned();
        let subtitle = s.error.clone().unwrap_or_else(|| {
            track.map_or_else(
                || "Double-click a track to play".into(),
                |t| {
                    let mut parts = vec![];
                    let artist = info.and_then(|i| i.artist.as_ref()).unwrap_or(&t.artist);
                    let album = info.and_then(|i| i.album.as_ref()).unwrap_or(&t.album);
                    if !artist.is_empty() {
                        parts.push(artist.clone());
                    }
                    if !album.is_empty() {
                        parts.push(album.clone());
                    }
                    if parts.is_empty() {
                        parts.push(t.format());
                    }
                    parts.join("   ·   ")
                },
            )
        });
        let button = Button::new("play-pause")
            .primary()
            .small()
            .icon(Icon::new(if s.playing {
                IconName::Pause
            } else {
                IconName::Play
            }))
            .accessibility_label(if s.playing { "Pause" } else { "Play" })
            .tooltip("Play or pause · Space")
            .disabled(!active && !self.can_start)
            .on_click(cx.listener(|p, _, _, cx| p.command(Command::Toggle, cx)));
        v_flex()
            .size_full()
            .justify_center()
            .gap_3()
            .px_5()
            .py_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .child(art)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_lg()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(if s.error.is_some() {
                                        cx.theme().danger
                                    } else {
                                        muted
                                    })
                                    .truncate()
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(self.key(
                                "previous",
                                IconName::SkipBack,
                                "Previous",
                                s.previous,
                                || Command::Step(-1),
                                cx,
                            ))
                            .child(button)
                            .child(self.key(
                                "next",
                                IconName::SkipForward,
                                "Next",
                                s.next,
                                || Command::Step(1),
                                cx,
                            ))
                            .child(self.key(
                                "stop",
                                IconName::Square,
                                "Stop",
                                active,
                                || Command::Stop,
                                cx,
                            )),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(div().min_w(px(38.)).child(clock(s.position)))
                    .child(
                        div()
                            .flex_1()
                            .h(px(30.))
                            .relative()
                            .flex()
                            .items_center()
                            .when_some(self.peaks.clone(), |d, peaks| {
                                d.child(div().absolute().inset_0().child(crate::overview::paint(
                                    peaks,
                                    self.seek.read(cx).value().start() / 1000.,
                                )))
                            })
                            .child(
                                Slider::new(&self.seek)
                                    .when(self.peaks.is_some(), |s| s.bg(rgba(0x8bd9cf55)))
                                    .disabled(!active)
                                    .accessibility_label("Playback position"),
                            ),
                    )
                    .child(div().min_w(px(38.)).child(clock(s.duration)))
                    .child(Icon::new(IconName::Volume2).small())
                    .child(
                        div()
                            .w(px(80.))
                            .child(Slider::new(&self.level).accessibility_label("Volume")),
                    ),
            )
    }
}

use super::{Home, Instances, Pane, PaneContent, PaneEvent, PanelType, Services};
use crate::{
    player::{Player, PlayerEvent},
    spectral_history as spec,
    spectrum::{Mode, Range, Speed},
    spectrum_worker::{Delivery, Request, Subscription as Feed},
    stereo_field as field,
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
        v_flex,
    },
    prelude::*,
    *,
};
use std::rc::Rc;
mod plot;
use plot::Plot;

pub static SPECTROGRAM: PanelType = PanelType {
    key: "disqro.spectrogram",
    minimum_content: size(208., 128.),
    title: "Spectrogram",
    icon: IconName::Layers,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |s, r, w, cx| {
        Rc::new(cx.new(|cx| Instrument::new(s, Config::Spectral(r.state()), w, cx)))
    },
};
pub static STEREO_FIELD: PanelType = PanelType {
    key: "disqro.stereo-field",
    minimum_content: size(176., 128.),
    title: "Stereo Field",
    icon: IconName::Radar,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |s, r, w, cx| Rc::new(cx.new(|cx| Instrument::new(s, Config::Field(r.state()), w, cx))),
};
#[derive(Clone, Copy, PartialEq)]
enum Config {
    Spectral(spec::Settings),
    Field(field::Settings),
}
impl Config {
    fn request(self) -> Request {
        match self {
            Self::Spectral(s) => Request::Spectral(s.analysis),
            Self::Field(s) => Request::Field(s.analysis),
        }
    }
    fn delivery(self, field_side: usize) -> Delivery {
        match self {
            Self::Spectral(s) => Delivery {
                speed: s.speed,
                field_side,
                statistics: false,
            },
            Self::Field(s) => Delivery {
                speed: s.speed,
                field_side,
                statistics: s.statistics,
            },
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Spectral(_) => SPECTROGRAM.title,
            Self::Field(_) => STEREO_FIELD.title,
        }
    }
}
struct Changed;
struct Instrument {
    field_readouts: Entity<std::collections::HashSet<EntityId>>,
    player: Entity<Player>,
    plot: Entity<Plot>,
    focus: FocusHandle,
    config: Config,
    presented: bool,
    visible: bool,
    frozen: bool,
    stream: Option<(Feed, Task<()>)>,
}
impl EventEmitter<Changed> for Instrument {}
impl Instrument {
    fn new(s: &Services, config: Config, w: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_visibility(w, |s: &mut Self, v, _, cx| {
            s.visible = v == WindowVisibility::Visible;
            s.sync(cx);
        })
        .detach();
        cx.subscribe(&s.player, |s: &mut Self, _, e, cx| {
            if matches!(e, PlayerEvent::StateChanged | PlayerEvent::TrackChanged) {
                s.sync(cx);
            }
        })
        .detach();
        let id = cx.entity_id();
        cx.on_release(move |s, cx| {
            s.field_readouts.update(cx, |fields, cx| {
                if fields.remove(&id) {
                    cx.notify();
                }
            })
        })
        .detach();
        let plot = cx.new(|cx| Plot::new(config, cx));
        cx.subscribe(&plot, |s: &mut Self, _, &plot::FieldSize(side), _| {
            if let Some((sub, _)) = &s.stream {
                sub.set_delivery(s.config.delivery(side));
            }
        })
        .detach();
        Self {
            field_readouts: s.field_readouts.clone(),
            player: s.player.clone(),
            plot,
            focus: cx.focus_handle(),
            config,
            presented: false,
            visible: w.visibility() == WindowVisibility::Visible,
            frozen: false,
            stream: None,
        }
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        let shows_stats = self.presented
            && self.visible
            && matches!(self.config,Config::Field(s) if s.statistics);
        if self.field_readouts.read(cx).contains(&id) != shows_stats {
            self.field_readouts.update(cx, |fields, cx| {
                if shows_stats {
                    fields.insert(id);
                } else {
                    fields.remove(&id);
                }
                cx.notify();
            });
        }
        let p = self.player.read(cx);
        let active = self.presented && self.visible && p.status.playing && !self.frozen;
        if active == self.stream.is_some() {
            return;
        }
        if !active {
            self.stream = None;
            if !self.frozen {
                self.plot.update(cx, |plot, cx| plot.clear(cx));
            }
            return;
        }
        let capture = p.spectra.capture.clone();
        let Ok((sub, rx)) = p.spectra.visualize(
            self.config.request(),
            self.config.delivery(self.plot.read(cx).field_side.get()),
        ) else {
            self.plot.update(cx, |plot, cx| {
                plot.failed = true;
                cx.notify();
            });
            return;
        };
        let boundary = (capture.epoch(), capture.position());
        let plot = self.plot.downgrade();
        self.plot.update(cx, |plot, cx| {
            plot.clear(cx);
            plot.boundary = boundary;
        });
        let task = cx.spawn(async move |_, cx| {
            while let Ok(frame) = rx.recv().await {
                if plot
                    .update(cx, |p, cx| {
                        p.frame = frame.filter(|f| f.epoch == capture.epoch());
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.stream = Some((sub, task));
    }
    fn configure(&mut self, config: Config, cx: &mut Context<Self>) {
        if self.config == config {
            return;
        }
        let changed = self.config.request() != config.request();
        let old = changed.then(|| self.stream.take());
        self.config = config;
        if changed {
            self.frozen = false;
        }
        self.plot.update(cx, |p, cx| {
            p.config = config;
            if changed {
                p.clear(cx);
            }
            cx.notify();
        });
        if let Some((sub, _)) = &self.stream {
            sub.set_delivery(config.delivery(self.plot.read(cx).field_side.get()));
        }
        self.sync(cx);
        drop(old);
        cx.emit(Changed);
        cx.notify();
    }
    fn options(
        this: WeakEntity<Self>,
        config: Config,
        mut menu: PopupMenu,
        w: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        macro_rules! setting {
            ($variant:ident,$s:ident,$ty:path,$label:literal,$($field:ident).+) => {{
                let entity=this.clone();
                menu=menu.submenu($label,w,cx,move |mut list,_,_|{
                    for &value in <$ty>::ALL {
                        let entity=entity.clone();
                        list=list.item(PopupMenuItem::new(value.label()).checked($s.$($field).+ == value).on_click(move |_,_,cx|{
                            let _=entity.update(cx,|panel,cx|{if let Config::$variant(mut s)=panel.config {
                                s.$($field).+ = value; panel.configure(Config::$variant(s),cx);
                            }});
                        }));
                    }
                    list
                });
            }};
        }
        match config {
            Config::Spectral(s) => {
                setting!(Spectral, s, spec::View, "Presentation", view);
                setting!(Spectral, s, Mode, "Channels", analysis.mode);
                setting!(
                    Spectral,
                    s,
                    spec::WindowLength,
                    "Analysis window",
                    analysis.resolution
                );
                setting!(Spectral, s, spec::Span, "History", span);
                setting!(Spectral, s, Range, "Frequency range", analysis.range);
                setting!(
                    Spectral,
                    s,
                    spec::Mapping,
                    "Frequency scale",
                    analysis.mapping
                );
                setting!(Spectral, s, spec::Floor, "Intensity floor", floor);
                setting!(Spectral, s, spec::Ceiling, "Intensity ceiling", ceiling);
                setting!(Spectral, s, spec::Palette, "Color", palette);
                setting!(Spectral, s, Speed, "Refresh rate", speed);
            }
            Config::Field(s) => {
                setting!(Field, s, field::Axes, "Axes", analysis.axes);
                setting!(Field, s, field::Ink, "Drawing", analysis.ink);
                setting!(Field, s, field::Gain, "Display gain", analysis.gain);
                setting!(
                    Field,
                    s,
                    field::Persistence,
                    "Persistence half-life",
                    analysis.persistence
                );
                setting!(Field, s, Speed, "Refresh rate", speed);
                menu = menu.item(
                    PopupMenuItem::new("Stereo statistics")
                        .checked(s.statistics)
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |p, cx| {
                                if let Config::Field(mut s) = p.config {
                                    s.statistics = !s.statistics;
                                    p.configure(Config::Field(s), cx);
                                }
                            });
                        }),
                );
            }
        }
        menu
    }
}
impl PaneContent for Entity<Instrument> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn title(&self, cx: &App) -> SharedString {
        self.read(cx).config.title().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus.clone()
    }
    fn set_presented(&self, v: bool, cx: &mut App) {
        self.update(cx, |s, cx| {
            s.presented = v;
            s.sync(cx);
        });
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        match self.read(cx).config {
            Config::Spectral(s) => serde_json::to_value(s),
            Config::Field(s) => serde_json::to_value(s),
        }
        .unwrap_or_default()
    }
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<gpui_kit::Subscription> {
        vec![cx.subscribe(self, |_, _, _: &Changed, cx| cx.emit(PaneEvent::Changed))]
    }
}
impl Render for Instrument {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let config = self.config;
        let (name, detail) = match config {
            Config::Spectral(s) => (s.view.label(), s.analysis.mode.label()),
            Config::Field(s) => (s.analysis.ink.label(), s.analysis.axes.label()),
        };
        let label = format!("{} settings", config.title());
        let this = cx.weak_entity();
        let button = Button::new("instrument-settings")
            .ghost()
            .xsmall()
            .icon(Icon::new(IconName::SlidersHorizontal))
            .accessibility_label(label.clone())
            .tooltip(label)
            .dropdown_menu(move |menu, w, cx| Self::options(this.clone(), config, menu, w, cx));
        let freeze = if self.frozen {
            "Resume live history"
        } else {
            "Freeze history"
        };
        v_flex()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .bg(m.canvas)
            .child(
                super::header::panel_header()
                    // The tab already names the default view.
                    .when(name != config.title(), |row| {
                        row.child(super::header::summary(name, &m))
                    })
                    .child(super::header::detail(
                        if self.frozen { "Frozen" } else { detail },
                        &m,
                    ))
                    .when(matches!(config, Config::Spectral(_)), |row| {
                        row.child(
                            Button::new("history-freeze")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(if self.frozen {
                                    IconName::Play
                                } else {
                                    IconName::Pause
                                }))
                                .tooltip(freeze)
                                .accessibility_label(freeze)
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.frozen = !p.frozen;
                                    if !p.frozen {
                                        p.plot.update(cx, |plot, cx| plot.clear(cx));
                                    }
                                    p.sync(cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(button),
            )
            .child(div().flex_1().min_h_0().child(if w.is_a11y_active() {
                self.plot.clone().into_any_element()
            } else {
                self.plot
                    .clone()
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element()
            }))
    }
}

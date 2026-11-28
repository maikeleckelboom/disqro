//! One persisted dock instance. The worker only has a subscription while the
//! tab is displayed, its native window is visible and the transport is playing.
use super::{Home, Instances, Pane, PaneContent, PaneEvent, PanelType, Services};
use crate::spectrum::Style;
use crate::{
    player::{Player, PlayerEvent},
    spectrum::*,
    spectrum_worker,
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
        v_flex,
    },
    prelude::*,
    *,
};
use std::rc::Rc;
mod plot;
use plot::Plot;

pub static TYPE: PanelType = PanelType {
    key: "disqro.spectrum",
    title: "Spectrum",
    icon: IconName::AudioLines,
    instances: Instances::Multiple,
    home: Home::Main,
    build: |s, r, w, cx| Rc::new(cx.new(|cx| Spectrum::new(s, r.state(), w, cx))),
};

struct SettingsChanged;
struct Spectrum {
    player: Entity<Player>,
    focus: FocusHandle,
    settings: Settings,
    plot: Entity<Plot>,
    tab: bool,
    visible: bool,
    stream: Option<(spectrum_worker::Subscription, Task<()>)>,
}
impl EventEmitter<SettingsChanged> for Spectrum {}
impl Spectrum {
    fn new(s: &Services, settings: Settings, w: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self {
            player: s.player.clone(),
            settings,
            plot: cx.new(|cx| {
                cx.observe(&s.field_readouts, |_, _, cx| cx.notify())
                    .detach();
                Plot::new(settings, s.field_readouts.clone())
            }),
            focus: cx.focus_handle(),
            tab: false,
            visible: w.visibility() == WindowVisibility::Visible,
            stream: None,
        }
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let p = self.player.read(cx);
        let active = self.tab && self.visible && p.status.playing;
        if active == self.stream.is_some() {
            return;
        }
        if active {
            let Ok((sub, rx)) = p.spectra.subscribe(self.settings) else {
                self.plot.update(cx, |plot, cx| {
                    plot.failed = true;
                    cx.notify();
                });
                return;
            };
            let capture = p.spectra.capture.clone();
            let plot = self.plot.downgrade();
            let task = cx.spawn(async move |_, cx| {
                while let Ok(update) = rx.recv().await {
                    let update = update.filter(|r| r.epoch == capture.epoch());
                    if plot
                        .update(cx, |p, cx| {
                            p.failed = false;
                            p.reading = update;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
            self.stream = Some((sub, task));
        } else {
            self.stream = None;
            self.plot.update(cx, |p, cx| {
                p.reading = None;
                cx.notify();
            });
        }
    }
    fn configure(&mut self, settings: Settings, cx: &mut Context<Self>) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        let previous = self.stream.take();
        self.plot.update(cx, |p, cx| {
            p.settings = settings;
            p.reading = None;
            cx.notify();
        });
        self.sync(cx);
        drop(previous);
        cx.emit(SettingsChanged);
        cx.notify();
    }

    fn options(
        this: WeakEntity<Self>,
        settings: Settings,
        mut menu: PopupMenu,
        w: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        macro_rules! setting {
            ($field:ident,$ty:ident,$label:literal) => {{
                let entity = this.clone();
                menu = menu.submenu($label, w, cx, move |mut list, _, _| {
                    for &value in $ty::ALL {
                        let entity = entity.clone();
                        list = list.item(
                            PopupMenuItem::new(value.label())
                                .checked(settings.$field == value)
                                .on_click(move |_, _, cx| {
                                    let _ = entity.update(cx, |s, cx| {
                                        s.configure(
                                            Settings {
                                                $field: value,
                                                ..s.settings
                                            },
                                            cx,
                                        )
                                    });
                                }),
                        );
                    }
                    list
                });
            }};
        }
        setting!(style, Style, "Presentation");
        setting!(mode, Mode, "Channels");
        setting!(resolution, Resolution, "FFT window");
        setting!(range, Range, "Frequency range");
        if settings.style == Style::Trace {
            setting!(smoothing, Smoothing, "Smoothing");
            setting!(tilt, Tilt, "Display tilt");
        }
        setting!(hold, Hold, "Peak hold");
        setting!(release, Release, "Release");
        setting!(speed, Speed, "Update rate");
        menu
    }
}
impl PaneContent for Entity<Spectrum> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn title(&self, _: &App) -> SharedString {
        "Spectrum".into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus.clone()
    }
    fn set_presented(&self, active: bool, cx: &mut App) {
        self.update(cx, |s, cx| {
            s.tab = active;
            s.sync(cx);
        });
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(self.read(cx).settings).unwrap_or_default()
    }
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<Subscription> {
        vec![cx.subscribe(self, |_, _, _: &SettingsChanged, cx| {
            cx.emit(PaneEvent::Changed)
        })]
    }
}
impl Render for Spectrum {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let entity = cx.weak_entity();
        let s = self.settings;
        let unit = match (s.style, s.tilt) {
            (Style::Bands, _) => "Band RMS · dBFS".to_owned(),
            (Style::Trace, Tilt::Flat) => "Peak · dBFS".to_owned(),
            _ => format!("Peak · dBFS  {} tilt", s.tilt.label()),
        };
        v_flex()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .bg(m.canvas)
            .child(
                super::header::panel_header()
                    .child(super::header::title(s.mode.label(), &m))
                    .child(super::header::detail(unit, &m))
                    .when(s.hold != Hold::Off, |row| {
                        row.child(
                            h_flex()
                                .gap(px(4.))
                                .child(div().w(px(10.)).h(px(1.)).bg(m.text_muted))
                                .child(
                                    div()
                                        .text_size(px(crate::theme::size::TEXT_SMALL))
                                        .text_color(m.text_muted)
                                        .child("Hold"),
                                ),
                        )
                    })
                    .child(
                        Button::new("spectrum-settings")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::SlidersHorizontal))
                            .accessibility_label("Spectrum settings")
                            .tooltip("Spectrum settings")
                            .dropdown_menu(move |menu, w, cx| {
                                Self::options(entity.clone(), s, menu, w, cx)
                            }),
                    ),
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

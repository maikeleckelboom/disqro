use crate::{
    audio::Command,
    browser::Browser,
    library::{clock, display_name},
    player::Player,
    queue_view::QueueView,
    sidebar::Sidebar,
    table::ROW_HEIGHT,
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        button::*,
        dock::*,
        menu::{PopupMenu, PopupMenuItem},
        table::DataTable,
        *,
    },
    prelude::*,
    *,
};

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Kind {
    Folders,
    Tracks,
    Player,
    Queue,
}
impl Kind {
    pub const ALL: [Self; 4] = [Self::Folders, Self::Tracks, Self::Queue, Self::Player];
    pub fn name(self) -> &'static str {
        match self {
            Self::Folders => "Folders",
            Self::Tracks => "Recordings",
            Self::Player => "Now Playing",
            Self::Queue => "Queue",
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap()
    }
}

pub struct Pane {
    pub kind: Kind,
    pub browser: Entity<Browser>,
    pub player: Entity<Player>,
    pub sidebar: Entity<Sidebar>,
    pub queue: Entity<QueueView>,
    pub focus: FocusHandle,
}
impl EventEmitter<PanelEvent> for Pane {}
impl Focusable for Pane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.kind {
            Kind::Queue => self.queue.read(cx).focus_handle().clone(),
            _ => self.focus.clone(),
        }
    }
}
impl BasePanel for Pane {
    fn panel_name(&self) -> &'static str {
        self.kind.name()
    }
}
impl Panel for Pane {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let (title, detail): (String, String) = match self.kind {
            Kind::Tracks => {
                let b = self.browser.read(cx);
                let count = b.count;
                let total = b.duration.unwrap_or_default();
                let discs = b.discs.read(cx).delegate().items.len();
                let detail = if b.busy {
                    format!("Scanning · {count} tracks")
                } else if count == 0 {
                    String::new()
                } else if b.listing_discs() {
                    format!("{discs} discs · {count} tracks")
                } else if total > 0. {
                    format!(
                        "{count} {} · {}",
                        if count == 1 { "track" } else { "tracks" },
                        clock(total)
                    )
                } else {
                    format!("{count} {}", if count == 1 { "track" } else { "tracks" })
                };
                (
                    b.album.clone().unwrap_or_else(|| display_name(&b.path)),
                    if b.prefs.recursive {
                        if detail.is_empty() {
                            "Including subfolders".into()
                        } else {
                            format!("{detail} · Subfolders")
                        }
                    } else {
                        detail
                    },
                )
            }
            Kind::Queue => ("Queue".into(), self.queue.read(cx).summary(cx)),
            k => (k.name().into(), String::new()),
        };
        h_flex()
            .min_w_0()
            .gap(px(10.))
            .items_baseline()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(m.text)
                    .child(title),
            )
            .when(!detail.is_empty(), |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(m.text_faint)
                        .child(detail),
                )
            })
    }
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.kind.name().into())
    }
    fn title_style(&self, cx: &App) -> Option<TitleStyle> {
        let m = materials(cx);
        (self.kind == Kind::Queue).then_some(TitleStyle {
            background: m.chrome,
            foreground: m.text,
        })
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
    /// The sidebar and the player draw their own compact chrome.
    fn title_bar(&self, _: &App) -> bool {
        matches!(self.kind, Kind::Tracks | Kind::Queue)
    }
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        if self.kind != Kind::Tracks {
            return None;
        }
        let b = self.browser.read(cx);
        let albums = b.prefs.albums;
        let browser = self.browser.clone();
        let back = b.disc.is_some();
        Some(vec![
            Button::new("collection-view")
                .ghost()
                .xsmall()
                .label(if back {
                    "‹ Albums / CDs"
                } else if albums {
                    "All Tracks"
                } else {
                    "Albums / CDs"
                })
                .on_click(move |_, w, cx| {
                    browser.update(cx, |b, cx| b.view(back || !albums, w, cx))
                }),
        ])
    }
    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        match self.kind {
            Kind::Queue => self.queue.read(cx).panel_menu(menu, cx),
            Kind::Tracks => {
                let tracks: crate::library::TrackList =
                    self.browser.read(cx).table.read(cx).delegate().rows.clone();
                let empty = tracks.is_empty();
                let send = {
                    let player = self.player.clone();
                    move |c: Command, cx: &mut App| player.update(cx, |p, cx| p.command(c, cx))
                };
                let (s1, s2, s3) = (send.clone(), send.clone(), send);
                let (t1, t2, t3) = (tracks.clone(), tracks.clone(), tracks);
                let browser = self.browser.clone();
                let scope = browser.clone();
                let recursive = browser.read(cx).prefs.recursive;
                menu.item(
                    PopupMenuItem::new("Include Subfolders")
                        .checked(recursive)
                        .on_click(move |_, w, cx| {
                            scope.update(cx, |b, cx| {
                                b.prefs.recursive = !b.prefs.recursive;
                                b.reload(w, cx);
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Play All")
                        .icon(IconName::Play)
                        .disabled(empty)
                        .on_click(move |_, _, cx| {
                            s1(Command::Shuffle(false), cx);
                            s1(Command::Play(t1.clone(), 0), cx)
                        }),
                )
                .item(
                    PopupMenuItem::new("Shuffle All")
                        .icon(IconName::Shuffle)
                        .disabled(empty)
                        .on_click(move |_, _, cx| {
                            let start = fastrand::usize(..t2.len());
                            s2(Command::Shuffle(true), cx);
                            s2(Command::Play(t2.clone(), start), cx)
                        }),
                )
                .item(
                    PopupMenuItem::new("Add All to Queue")
                        .icon(IconName::ListEnd)
                        .disabled(empty)
                        .on_click(move |_, _, cx| s3(Command::Enqueue(t3.clone()), cx)),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Refresh")
                        .icon(IconName::RefreshCw)
                        .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.reload(w, cx))),
                )
            }
            _ => menu,
        }
    }
}
impl Pane {
    fn recordings(&self, cx: &App) -> impl IntoElement {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let mut view = v_flex().size_full().bg(m.canvas);
        if let Some(notice) = &b.notice {
            view = view.child(
                h_flex()
                    .mx(px(10.))
                    .mb(px(4.))
                    .px(px(10.))
                    .py(px(5.))
                    .gap(px(8.))
                    .rounded(px(5.))
                    .bg(m.warning.opacity(0.08))
                    .text_size(px(12.))
                    .text_color(m.warning)
                    .child(Icon::new(IconName::TriangleAlert).size(px(13.)))
                    .child(div().truncate().child(notice.clone())),
            );
        }
        if b.listing_discs() && !b.discs.read(cx).delegate().items.is_empty() {
            return view.child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&b.discs)
                        .accessibility_label("Albums and CDs")
                        .bordered(false)
                        .stripe(false)
                        .header_row_height(px(30.))
                        .with_size(gpui_kit::component::Size::Size(px(42.))),
                ),
            );
        }
        view.child(
            div().flex_1().min_h_0().text_size(px(13.)).child(
                DataTable::new(&b.table)
                    .accessibility_label("Recordings")
                    .bordered(false)
                    .stripe(false)
                    .header_row_height(px(30.))
                    .with_size(gpui_kit::component::Size::Size(px(ROW_HEIGHT))),
            ),
        )
    }
}
impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.kind {
            Kind::Folders => self.sidebar.clone().into_any_element(),
            Kind::Tracks => self.recordings(cx).into_any_element(),
            Kind::Player => self.player.clone().into_any_element(),
            Kind::Queue => self.queue.clone().into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .child(content)
    }
}

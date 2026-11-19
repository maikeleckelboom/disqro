use crate::{
    audio::Command,
    browser::{Browser, Location},
    catalog::Targets,
    library::{TrackList, clock},
    player::Player,
    queue_view::QueueView,
    sidebar::Sidebar,
    table::ROW_HEIGHT,
    table::queue_targets,
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
                let catalog = b.catalog.read(cx);
                let indexing = match &b.location {
                    Location::Library(Some(r)) => {
                        catalog.collection(*r).and_then(|c| c.status.clone())
                    }
                    Location::Library(None) => {
                        catalog.collections.iter().find_map(|c| c.status.clone())
                    }
                    _ => None,
                };
                let tracks = |n: usize| format!("{n} {}", if n == 1 { "track" } else { "tracks" });
                let detail = if b.busy {
                    format!("Scanning · {}", tracks(count))
                } else if count == 0 {
                    String::new()
                } else if b.listing_discs() {
                    format!("{discs} discs · {}", tracks(count))
                } else if total > 0. {
                    format!("{} · {}", tracks(count), clock(total))
                } else {
                    tracks(count)
                };
                let detail = match indexing {
                    Some(status) if detail.is_empty() => status,
                    Some(status) => format!("{detail} · {status}"),
                    None => detail,
                };
                let subfolders = b.location.is_folder() && b.prefs.recursive;
                (
                    b.title(cx),
                    if subfolders {
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
        let indexed_album = matches!(b.location, Location::Album { .. });
        if !b.can_list_discs() && !indexed_album {
            return None;
        }
        let albums = b.prefs.albums;
        let browser = self.browser.clone();
        let back = b.disc.is_some() || indexed_album;
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
                let b = self.browser.read(cx);
                let table = b.table.read(cx).delegate();
                let all = table.all_targets();
                let empty = all.is_empty();
                let location = b.location.clone();
                let catalog = b.catalog.clone();
                let in_library = catalog
                    .read(cx)
                    .collections
                    .iter()
                    .any(|c| b.path.starts_with(&c.root.path));
                let player = self.player.clone();
                let send = {
                    let (catalog, player) = (catalog.clone(), player.clone());
                    move |targets: Targets, shuffle: Option<bool>, cx: &mut App| {
                        let command: Box<dyn FnOnce(TrackList) -> Command> = match shuffle {
                            None => Box::new(Command::Enqueue),
                            Some(false) => Box::new(|t| Command::Play(t, 0)),
                            Some(true) => Box::new(|t: TrackList| {
                                let start = fastrand::usize(..t.len());
                                Command::Play(t, start)
                            }),
                        };
                        if let Some(on) = shuffle {
                            player.update(cx, |p, cx| p.command(Command::Shuffle(on), cx));
                        }
                        queue_targets(&catalog, &player, targets, command, cx);
                    }
                };
                let (s1, s2, s3) = (send.clone(), send.clone(), send);
                let (t1, t2, t3) = (all.clone(), all.clone(), all);
                let browser = self.browser.clone();
                let scope = browser.clone();
                let recursive = b.prefs.recursive;
                let folder = b.path.clone();
                let menu = match &location {
                    Location::Folder(_) => menu
                        .item(
                            PopupMenuItem::new("Include Subfolders")
                                .checked(recursive)
                                .on_click(move |_, w, cx| {
                                    scope.update(cx, |b, cx| {
                                        b.prefs.recursive = !b.prefs.recursive;
                                        b.reload(w, cx);
                                    });
                                }),
                        )
                        .separator(),
                    _ => menu,
                };
                let menu = menu
                    .item(
                        PopupMenuItem::new("Play All")
                            .icon(IconName::Play)
                            .disabled(empty)
                            .on_click(move |_, _, cx| s1(t1.clone(), Some(false), cx)),
                    )
                    .item(
                        PopupMenuItem::new("Shuffle All")
                            .icon(IconName::Shuffle)
                            .disabled(empty)
                            .on_click(move |_, _, cx| s2(t2.clone(), Some(true), cx)),
                    )
                    .item(
                        PopupMenuItem::new("Add All to Queue")
                            .icon(IconName::ListEnd)
                            .disabled(empty)
                            .on_click(move |_, _, cx| s3(t3.clone(), None, cx)),
                    )
                    .separator();
                match location {
                    Location::Folder(_) => menu
                        .when(!in_library, |menu| {
                            menu.item(
                                PopupMenuItem::new("Add Folder to Library")
                                    .icon(IconName::Library)
                                    .on_click(move |_, _, cx| {
                                        catalog.update(cx, |c, cx| c.add_root(folder.clone(), cx))
                                    }),
                            )
                        })
                        .item(
                            PopupMenuItem::new("Refresh")
                                .icon(IconName::RefreshCw)
                                .on_click(move |_, w, cx| {
                                    browser.update(cx, |b, cx| b.reload(w, cx))
                                }),
                        ),
                    Location::Library(root) | Location::Album { root, .. } => menu.item(
                        PopupMenuItem::new(if root.is_some() {
                            "Rescan Collection"
                        } else {
                            "Rescan Collections"
                        })
                        .icon(IconName::RefreshCw)
                        .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.reload(w, cx))),
                    ),
                    Location::Liked | Location::Playlist(_) => menu,
                }
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
        if b.listing_discs()
            && (!b.location.is_folder() || !b.discs.read(cx).delegate().items.is_empty())
        {
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
            div()
                .key_context("MusicTracks")
                .flex_1()
                .min_h_0()
                .text_size(px(13.))
                .child(
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

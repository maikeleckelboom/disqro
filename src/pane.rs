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
            Self::Folders => "Sidebar",
            Self::Tracks => "Recordings",
            Self::Player => "Now Playing",
            Self::Queue => "Queue",
        }
    }
    /// The name saved dock layouts know a panel by. The sidebar keeps the
    /// name it had when it held only the folder tree.
    pub fn key(self) -> &'static str {
        match self {
            Self::Folders => "Folders",
            kind => kind.name(),
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
        self.kind.key()
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
                let discs = b.section_count(cx);
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
                let detail = if b.busy && b.slow {
                    format!("Scanning · {}", tracks(count))
                } else if b.busy && count == 0 {
                    String::new()
                } else if b.busy {
                    tracks(count)
                } else if count == 0 {
                    String::new()
                } else if discs > 0 {
                    let plural = if discs == 1 { "disc" } else { "discs" };
                    format!("{discs} {plural} · {}", tracks(count))
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
                let subfolders = b.location.is_folder() && b.subfolders && b.page.is_none();
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
        let back = b.is_page();
        if !b.can_list_discs() && !back {
            return None;
        }
        let albums = b.prefs.albums;
        let browser = self.browser.clone();
        // A CD of a release goes back to the release.
        let release = back && b.membership.is_some();
        Some(vec![
            Button::new("collection-view")
                .ghost()
                .xsmall()
                .label(if release {
                    "‹ Release"
                } else if back {
                    "‹ Collection"
                } else if albums {
                    "All Tracks"
                } else {
                    "Albums / CDs"
                })
                .on_click(move |_, w, cx| {
                    browser.update(cx, |b, cx| {
                        if back {
                            b.up(w, cx)
                        } else {
                            b.view(!albums, w, cx)
                        }
                    })
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
                let subfolders = b.subfolders;
                let folder = b.path.clone();
                let menu = match &location {
                    Location::Folder(_) => menu
                        .item(
                            PopupMenuItem::new("Include Subfolders")
                                .checked(subfolders)
                                .on_click(move |_, w, cx| {
                                    scope.update(cx, |b, cx| b.set_subfolders(!subfolders, w, cx));
                                }),
                        )
                        .separator(),
                    _ => menu,
                };
                use crate::collection::{DiscId, Page};
                // The page shown: a CD, or every CD of a release.
                let page = match &b.location {
                    Location::Album { id, .. } => Some(Page::Disc(DiscId::Library(*id))),
                    Location::Release { id, .. } => Some(Page::Release(DiscId::Library(*id))),
                    _ => b.page.clone(),
                };
                let release = match &page {
                    Some(Page::Release(_)) => None,
                    _ => b.release().map(|(key, _)| Page::Release(key)),
                };
                let playing = self
                    .player
                    .read(cx)
                    .playing_entry()
                    .filter(|e| match &page {
                        Some(Page::Disc(id)) => table.page_contains(id, &e.track),
                        Some(Page::Release(_)) => table.lists(&e.track),
                        None => false,
                    })
                    .cloned();
                let cd_table = b.table.clone();
                let release_table = b.table.clone();
                let play_label = match &page {
                    Some(Page::Disc(_)) => "Play CD",
                    Some(Page::Release(_)) => "Play Release",
                    None if b.can_list_discs() => "Play Collection",
                    None => "Play All",
                };
                let menu = menu.when_some(playing, |menu, mut entry| {
                    entry.disc_context = crate::library::DiscContext::Selected;
                    let player = self.player.clone();
                    menu.item(
                        PopupMenuItem::new("Continue This CD").on_click(move |_, _, cx| {
                            player.update(cx, |_, cx| {
                                cx.emit(crate::player::PlayerEvent::ContinueCd(entry.clone()))
                            })
                        }),
                    )
                });
                let menu = menu
                    .item(
                        PopupMenuItem::new(play_label)
                            .icon(IconName::Play)
                            .disabled(empty && page.is_none())
                            .on_click(move |_, _, cx| match &page {
                                Some(page) => {
                                    cd_table.update(cx, |t, cx| t.delegate().play_page(page, cx))
                                }
                                None => s1(t1.clone(), Some(false), cx),
                            }),
                    )
                    .when_some(release, |menu, release| {
                        menu.item(
                            PopupMenuItem::new("Play Release").on_click(move |_, _, cx| {
                                release_table
                                    .update(cx, |t, cx| t.delegate().play_page(&release, cx))
                            }),
                        )
                    })
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
                    Location::Library(root)
                    | Location::Album { root, .. }
                    | Location::Release { root, .. } => menu.item(
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
            && (!b.location.is_folder() || !b.discs.read(cx).delegate().rows.is_empty())
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

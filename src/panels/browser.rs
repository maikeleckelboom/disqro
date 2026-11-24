//! The Music Browser: a content panel showing a folder, a collection, a CD
//! or release, Liked Tracks or a playlist.
//!
//! Every instance is its own [`Browser`] with its own location, history,
//! search, selection, scroll position, view mode and column layouts. The
//! catalog, the player and the Locations tree are shared.
use super::{Pane, PaneContent, PanelType, Router, Services};
use crate::{
    assets::FileIcon,
    audio::Command,
    browser::{Bookmark, Browser, Location, Route, SavedVisit, SequenceKind, music_home},
    catalog::Targets,
    collection::{Page, ViewPrefs},
    library::TrackList,
    location::LocationBar,
    player::Player,
    table::{Layouts, queue_targets},
    theme::{materials, size},
};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, ElementExt, FocusableExt as _, Icon, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
        table::DataTable,
        v_flex,
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, rc::Rc};

pub static TYPE: PanelType = PanelType {
    key: "disqro.browser",
    title: "Music Browser",
    icon: IconName::Library,
    instances: super::Instances::Multiple,
    home: super::Home::Main,
    build: |services, record, window, cx| {
        let state: BrowserState = record.state();
        Rc::new(BrowserPane::new(services, state, window, cx))
    },
};

/// What a Music Browser keeps between sessions.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserState {
    /// The view shown: a folder, a collection, Liked Tracks or a playlist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<Page>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_visits"
    )]
    pub(crate) back: Vec<SavedVisit>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_visits"
    )]
    pub(crate) forward: Vec<SavedVisit>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_visits"
    )]
    pub(crate) recent: Vec<SavedVisit>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_current"
    )]
    pub(crate) current: Option<SavedVisit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefs: Option<ViewPrefs>,
    /// The folder shown, or the last one while a library view is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// The Place Locations shows the folder under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Collections show Albums / CDs rather than All Tracks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub albums: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layouts: Option<Layouts>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub search: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub filter_open: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub scroll: f32,
}
fn is_zero(v: &f32) -> bool {
    *v == 0.
}
fn read_visits<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SavedVisit>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Vec<serde_json::Value>>(value)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|visit| serde_json::from_value(visit).ok())
        .collect())
}
fn read_current<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<SavedVisit>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).ok())
}

pub struct BrowserPane {
    pub browser: Entity<Browser>,
    router: Entity<Router>,
    _address: Entity<LocationBar>,
}

impl BrowserPane {
    fn new(services: &Services, state: BrowserState, window: &mut Window, cx: &mut App) -> Self {
        let template = services.router.read(cx).template.clone();
        let layouts = state
            .layouts
            .clone()
            .map(Layouts::validate)
            .unwrap_or(template.layouts);
        let mut prefs = state.prefs.unwrap_or(template.prefs);
        prefs.albums = state.albums.unwrap_or(prefs.albums);
        let folders = services.locations.read(cx).folders.clone();
        let (player, catalog) = (services.player.clone(), services.catalog.clone());
        let browser = cx.new(|cx| {
            let albums = prefs.albums;
            let mut b = Browser::new(player, catalog, folders, layouts, prefs, window, cx);
            b.path = state.folder.unwrap_or_else(music_home);
            b.tree_root = state.root.clone();
            let current = state.current.unwrap_or_else(|| {
                let location = state
                    .location
                    .unwrap_or_else(|| Location::Folder(b.path.clone()));
                let mut visit = SavedVisit::new(Route::new(location, state.page), albums);
                visit.root = state.root;
                visit.query = state.search;
                visit.bookmark = Bookmark::legacy(state.scroll);
                visit
            });
            b.restore_history(state.back, state.forward, state.recent);
            b.restore_visit(current, window, cx);
            b.filter_open = state.filter_open;
            b
        });
        // Each browser edits its own path, in its own header.
        let address = cx.new(|cx| crate::location::LocationBar::new(browser.clone(), window, cx));
        browser.update(cx, |b, _| {
            b.address = Some(address.downgrade());
        });
        Self {
            browser,
            router: services.router.clone(),
            _address: address,
        }
    }
    fn nav_button(
        id: &'static str,
        icon: IconName,
        label: &'static str,
        enabled: bool,
        cx: &App,
    ) -> Button {
        let m = materials(cx);
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(icon).size(px(16.)).text_color(m.text_muted))
            .accessibility_label(label)
            .tooltip(label)
            .disabled(!enabled)
    }
    /// What this instance saves, and what a new instance opened from it
    /// starts with.
    pub fn state(browser: &Browser, cx: &App) -> BrowserState {
        let (back, forward, recent) = browser.saved_history();
        BrowserState {
            back,
            forward,
            recent,
            current: Some(browser.saved_visit(cx)),
            folder: (!browser.path.as_os_str().is_empty()).then(|| browser.path.clone()),
            root: browser.tree_root.clone(),
            prefs: Some(browser.prefs.clone()),
            layouts: Some(browser.table.read(cx).delegate().layouts.clone()),
            filter_open: browser.filter_open,
            ..Default::default()
        }
    }
    pub fn new_tab_state(browser: &Browser, cx: &App) -> BrowserState {
        let mut visit = SavedVisit::new(browser.route(), browser.prefs.albums);
        visit.root = browser.tree_root.clone();
        visit.subfolders = browser.location.is_folder().then_some(browser.subfolders);
        visit.query = browser.search.read(cx).value().to_string();
        BrowserState {
            current: Some(visit),
            folder: (!browser.path.as_os_str().is_empty()).then(|| browser.path.clone()),
            root: browser.tree_root.clone(),
            prefs: Some(browser.prefs.clone()),
            layouts: Some(browser.table.read(cx).delegate().layouts.clone()),
            ..Default::default()
        }
    }
}

/// The panel menu: play or queue what the browser shows, and act on its view.
fn browser_menu(
    browser: &Entity<Browser>,
    player: &Entity<Player>,
    menu: PopupMenu,
    cx: &App,
) -> PopupMenu {
    use crate::collection::{DiscId, Page};
    let b = browser.read(cx);
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
    let menu = if b.sequence_kind() == Some(SequenceKind::Disc) {
        let mut menu = menu;
        for next in [false, true] {
            let kind = SequenceKind::Release;
            let Some(target) = b.sequence_label(kind, next) else {
                continue;
            };
            let browser = browser.clone();
            menu = menu.item(
                PopupMenuItem::new(format!("{} · {target}", kind.action(next))).on_click(
                    move |_, w, cx| {
                        browser.update(cx, |b, cx| b.step_sequence(Some(kind), next, w, cx));
                    },
                ),
            );
        }
        menu
    } else {
        menu
    };
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
    let refresh = browser.clone();
    let folder = b.path.clone();
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
    let playing = player
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
    let collection = b.can_list_discs();
    let play_label = match &page {
        Some(Page::Disc(_)) => "Play CD",
        Some(Page::Release(_)) => "Play Release",
        None if b.can_list_discs() => "Play Collection",
        None => "Play All",
    };
    let menu = menu.when_some(playing, |menu, mut entry| {
        entry.disc_context = crate::library::DiscContext::Selected;
        let player = player.clone();
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
                    Some(page) => cd_table.update(cx, |t, cx| t.delegate().play_page(page, cx)),
                    None if collection => {
                        cd_table.update(cx, |t, cx| t.delegate().play_collection(cx))
                    }
                    None => s1(t1.clone(), Some(false), cx),
                }),
        )
        .when_some(release, |menu, release| {
            menu.item(
                PopupMenuItem::new("Play Release").on_click(move |_, _, cx| {
                    release_table.update(cx, |t, cx| t.delegate().play_page(&release, cx))
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
                    .on_click(move |_, w, cx| refresh.update(cx, |b, cx| b.reload(w, cx))),
            ),
        Location::Library(root) | Location::Album { root, .. } | Location::Release { root, .. } => {
            menu.item(
                PopupMenuItem::new(if root.is_some() {
                    "Rescan Collection"
                } else {
                    "Rescan Collections"
                })
                .icon(IconName::RefreshCw)
                .on_click(move |_, w, cx| refresh.update(cx, |b, cx| b.reload(w, cx))),
            )
        }
        Location::Liked | Location::Playlist(_) | Location::Collections | Location::Playlists => {
            menu
        }
    }
}

/// What the browser is doing, when it is worth saying: a slow folder scan or
/// a collection being indexed.
fn status(b: &Browser, cx: &App) -> Option<String> {
    let catalog = b.catalog.read(cx);
    let indexing = match &b.location {
        Location::Library(Some(r)) => catalog.collection(*r).and_then(|c| c.status.clone()),
        Location::Library(None) => catalog.collections.iter().find_map(|c| c.status.clone()),
        _ => None,
    };
    if b.busy && b.slow {
        let n = b.count;
        Some(format!(
            "Scanning · {n} {}",
            if n == 1 { "track" } else { "tracks" }
        ))
    } else {
        indexing
    }
}

impl PaneContent for BrowserPane {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.browser.clone().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.browser.read(cx).list_focus(cx)
    }
    fn title(&self, cx: &App) -> SharedString {
        self.browser.read(cx).title(cx).into()
    }
    fn icon(&self, _: &'static PanelType, cx: &App) -> Icon {
        match &self.browser.read(cx).location {
            Location::Folder(_) => FileIcon::Folder.icon(),
            Location::Library(None) | Location::Collections => Icon::new(IconName::Library),
            Location::Library(Some(_)) | Location::Album { .. } | Location::Release { .. } => {
                Icon::new(IconName::Disc3)
            }
            Location::Liked => Icon::default().path(crate::assets::HEART_FILL),
            Location::Playlist(_) | Location::Playlists => Icon::new(IconName::ListMusic),
        }
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(BrowserPane::state(self.browser.read(cx), cx)).unwrap_or_default()
    }
    fn focused(&self, _: &mut Window, cx: &mut App) {
        let browser = self.browser.clone();
        self.router
            .update(cx, |r, cx| r.set_active(Some(&browser), cx));
    }
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<Subscription> {
        let pane = cx.weak_entity();
        self.browser.update(cx, |b, _| b.pane = Some(pane));
        vec![
            cx.observe(&self.browser, |pane, _, cx| pane.refresh(cx)),
            cx.subscribe(
                &self.browser,
                |_, _, _: &crate::browser::ViewChanged, cx| cx.emit(super::PaneEvent::Changed),
            ),
        ]
    }
}

const HEADER_INSET: f32 = 8.;
const BAR_INSET: f32 = 4.;
const NAV_BUTTON: f32 = 26.;
const NAV_GAP: f32 = 1.;
/// The location keeps room for a short name before the controls compress.
const IDENTITY_MIN: f32 = 140.;

const VIEW_TAB: f32 = 36.;
const VIEW_TAB_HEIGHT: f32 = 29.;
const ICON_BUTTON: f32 = 28.;
const SCOPE_GAP: f32 = 3.;
const FILTER_COMPACT: f32 = 100.;
const FILTER_FULL: f32 = 175.;
const VIEW_CHOICES: [(bool, &str, IconName); 2] = [
    (false, "All Tracks", IconName::ListMusic),
    (true, "Albums / CDs", IconName::Disc3),
];
const SCOPE_CHOICES: [(bool, &str, IconName); 2] = [
    (false, "This folder only", IconName::Folder),
    (true, "Include subfolders", IconName::Folders),
];

#[derive(Clone, Copy)]
pub(crate) struct BrowserHeaderWidths {
    /// Inline view tabs, scope and the current filter, including the drop target.
    pub minimum: Pixels,
    /// The preferred filter width with the other controls, before metadata.
    pub full: Pixels,
}

#[derive(Clone, Copy)]
struct BrowserViews {
    /// The selected presentation, when both presentations are available.
    albums: Option<bool>,
    /// The current scope of a physical folder.
    subfolders: Option<bool>,
}

impl BrowserViews {
    fn widths(self, editing: bool) -> BrowserHeaderWidths {
        let mut fixed = 2. * HEADER_INSET + size::TAB_DROP;
        if self.albums.is_some() {
            fixed += 2. * VIEW_TAB;
        }
        if self.subfolders.is_some() {
            fixed += ICON_BUTTON + SCOPE_GAP;
        }
        BrowserHeaderWidths {
            minimum: px(fixed + if editing { FILTER_COMPACT } else { ICON_BUTTON }),
            full: px(fixed + if editing { FILTER_FULL } else { ICON_BUTTON }),
        }
    }
}

#[derive(Clone)]
struct ViewFocus {
    tracks: FocusHandle,
    albums: FocusHandle,
    folders: FocusHandle,
}

impl ViewFocus {
    fn choice(&self, albums: bool) -> &FocusHandle {
        if albums { &self.albums } else { &self.tracks }
    }
}

impl Browser {
    /// The browser's header: its history, where it is, what it holds, and
    /// how it is shown. Every browser has one, whether it is alone in its
    /// group or one of several tabs, so its controls never move.
    fn bar(&self, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let widths = self.header_widths(cx);
        let nav = px(3. * NAV_BUTTON
            + 2. * NAV_GAP
            + BAR_INSET
            + ICON_BUTTON
            + if self.sequence_kind().is_some() {
                64.
            } else {
                0.
            });
        // The location keeps a readable minimum; the controls take what they
        // need up to their comfortable width and give way first.
        let controls = (width - nav - px(IDENTITY_MIN))
            .min(widths.full)
            .max(widths.minimum.min(width - nav))
            .max(px(0.));
        let identity = (width - nav - controls).max(px(0.));
        let browser = cx.entity();
        let player = self.table.read(cx).delegate().player.clone();
        h_flex()
            .id("browser-bar")
            .w(width)
            .h(px(size::BAR))
            .flex_shrink_0()
            .items_center()
            .pl(px(BAR_INSET))
            .relative()
            .bg(m.canvas)
            // The bottom rule is drawn first, so the selected view tab can
            // open through it into the table.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(m.line),
            )
            .child(self.history(cx))
            .child(
                h_flex()
                    .w(identity)
                    .h_full()
                    .min_w_0()
                    .items_center()
                    .children(self.address.as_ref().and_then(WeakEntity::upgrade)),
            )
            .when_some(self.sequence_kind(), |bar, kind| {
                bar.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap(px(2.))
                        .children([false, true].map(|next| {
                            let browser = browser.clone();
                            let target = self.sequence_label(kind, next);
                            let action = kind.action(next);
                            let shortcut = if next { "Alt+Page Down" } else { "Alt+Page Up" };
                            let tooltip = target.as_ref().map_or_else(
                                || action.to_owned(),
                                |title| format!("{action} · {title} · {shortcut}"),
                            );
                            BrowserPane::nav_button(
                                if next {
                                    "sequence-next"
                                } else {
                                    "sequence-previous"
                                },
                                if next {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronLeft
                                },
                                action,
                                target.is_some(),
                                cx,
                            )
                            .tooltip(tooltip)
                            .on_click(move |_, w, cx| {
                                browser.update(cx, |b, cx| b.step_sequence(Some(kind), next, w, cx))
                            })
                        })),
                )
            })
            .child(
                div()
                    .w(controls)
                    .h_full()
                    .flex_shrink_0()
                    .child(self.header_controls(controls, widths, window, cx)),
            )
            .child(
                Button::new("browser-menu")
                    .ghost()
                    .xsmall()
                    .w(px(ICON_BUTTON))
                    .icon(IconName::Ellipsis)
                    .accessibility_label("Panel menu")
                    .tooltip("Panel menu")
                    .dropdown_menu(move |menu, w, cx| {
                        let menu = browser_menu(&browser, &player, menu, cx);
                        let pane = browser.read(cx).pane.as_ref().and_then(WeakEntity::upgrade);
                        match (pane, cx.try_global::<super::PaneMenus>().cloned()) {
                            (Some(pane), Some(menus)) => (menus.0)(&pane, menu.separator(), w, cx),
                            _ => menu,
                        }
                    }),
            )
            .into_any_element()
    }

    fn history(&self, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let button = |id: &'static str, icon: IconName, label: &'static str, enabled: bool| {
            Button::new(id)
                .ghost()
                .xsmall()
                .w(px(NAV_BUTTON))
                .h(px(NAV_BUTTON))
                .icon(Icon::new(icon).size(px(15.)).text_color(m.text_muted))
                .accessibility_label(label)
                .tooltip(label)
                .tab_stop(true)
                .disabled(!enabled)
        };
        h_flex()
            .id("browser-history")
            .role(Role::Group)
            .aria_label("History")
            .flex_shrink_0()
            .gap(px(NAV_GAP))
            .child(
                button(
                    "back",
                    IconName::ArrowLeft,
                    "Back · Alt+Left",
                    self.can_go_back(),
                )
                .on_click(cx.listener(|b, _, w, cx| b.back(w, cx))),
            )
            .child(
                button(
                    "forward",
                    IconName::ArrowRight,
                    "Forward · Alt+Right",
                    self.can_go_forward(),
                )
                .on_click(cx.listener(|b, _, w, cx| b.forward(w, cx))),
            )
            .child(
                button("up", IconName::ArrowUp, "Up · Alt+Up", self.can_go_up())
                    .on_click(cx.listener(|b, _, w, cx| b.up(w, cx))),
            )
            .into_any_element()
    }

    fn header_views(&self) -> BrowserViews {
        BrowserViews {
            albums: (!self.is_page() && self.can_list_discs()).then_some(self.prefs.albums),
            subfolders: (self.location.is_folder() && self.page.is_none())
                .then_some(self.subfolders),
        }
    }

    pub(crate) fn header_widths(&self, cx: &App) -> BrowserHeaderWidths {
        let editing = self.filter_open || !self.search.read(cx).value().is_empty();
        self.header_views().widths(editing)
    }

    pub(crate) fn header_controls(
        &self,
        width: Pixels,
        widths: BrowserHeaderWidths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let browser = cx.entity();
        let compact = width < widths.full;
        let tight = width < widths.minimum;
        let views = self.header_views();
        let focus = (views.albums.is_some() || views.subfolders.is_some()).then(|| {
            window
                .use_keyed_state(("view-focus", browser.entity_id()), cx, |_, cx| ViewFocus {
                    albums: cx.focus_handle(),
                    tracks: cx.focus_handle(),
                    folders: cx.focus_handle(),
                })
                .read(cx)
                .clone()
        });
        let m = *materials(cx);
        let folded = tight && views.albums.is_some();
        let mode = views.albums.zip(focus.as_ref()).map(|(albums, focus)| {
            if folded {
                // Keep the focused control's handle when the tabs and scope
                // collapse into one menu in a narrow split.
                let handle = [&focus.tracks, &focus.albums]
                    .into_iter()
                    .chain(views.subfolders.map(|_| &focus.folders))
                    .find(|handle| handle.is_focused(window))
                    .unwrap_or_else(|| focus.choice(albums));
                view_menu(&browser, views, handle, m)
            } else {
                view_tabs(&browser, albums, focus, m)
            }
        });
        let scope = views
            .subfolders
            .zip(focus.as_ref())
            .filter(|_| !folded)
            .map(|(include, focus)| {
                h_flex().mr(px(SCOPE_GAP)).flex_shrink_0().child(view_menu(
                    &browser,
                    BrowserViews {
                        albums: None,
                        subfolders: Some(include),
                    },
                    &focus.folders,
                    m,
                ))
            });
        // The filter sits nearest the table it narrows; how the view is
        // shown closes the row.
        h_flex()
            .id("browser-controls")
            .w_full()
            .h_full()
            .px(px(if tight { 2. } else { HEADER_INSET }))
            .gap(px(SCOPE_GAP))
            .items_center()
            .justify_end()
            .child(self.filter_control(compact, tight, window, cx))
            .children(scope)
            .children(mode)
            .into_any_element()
    }

    /// The filter button, or the field once it is open or holds a query.
    fn filter_control(
        &self,
        compact: bool,
        tight: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *materials(cx);
        let input = self.search.clone();
        let empty = input.read(cx).value().is_empty();
        if !self.filter_open && empty {
            return Button::new("open-filter")
                .ghost()
                .xsmall()
                .icon(
                    Icon::new(IconName::ListFilter)
                        .size(px(14.))
                        .text_color(m.text_muted),
                )
                .w(px(ICON_BUTTON))
                .h(px(24.))
                .tab_stop(true)
                .text_color(m.text_muted)
                .accessibility_label("Filter this view")
                .tooltip("Filter this view · Ctrl+F")
                .on_click(cx.listener(|b, _, w, cx| b.set_filter_open(true, w, cx)))
                .into_any_element();
        }
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        h_flex()
            .id("browser-filter")
            .flex_shrink(1.)
            .min_w_0()
            .w(px(if compact { FILTER_COMPACT } else { FILTER_FULL }))
            .h(px(24.))
            .pl(px(if tight { 2. } else { 7. }))
            .pr(px(2.))
            .gap(px(5.))
            .items_center()
            .rounded(px(5.))
            .border_1()
            .map(|d| {
                if focused {
                    d.bg(m.canvas).border_color(m.accent.opacity(0.6))
                } else {
                    d.bg(m.field())
                        .border_color(transparent_black())
                        .hover(|d| d.border_color(m.control_line))
                }
            })
            .when(!tight, |d| {
                d.child(
                    Icon::new(IconName::ListFilter)
                        .size(px(13.))
                        .flex_shrink_0()
                        // A filter that hides rows stays visible, even unfocused.
                        .text_color(if empty { m.text_faint } else { m.accent }),
                )
            })
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&input)
                        .xsmall()
                        .appearance(false)
                        .cleanable(!tight)
                        .aria_label("Filter this view"),
                ),
            )
            .capture_key_down(cx.listener(|b, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    b.dismiss_filter(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }
}

fn view_tabs(
    browser: &Entity<Browser>,
    albums: bool,
    focus: &ViewFocus,
    m: crate::theme::Materials,
) -> AnyElement {
    // Connected tabs: the selected view takes the table's surface and opens
    // into it through the header's bottom rule, like the dock's own tabs.
    h_flex()
        .id("browser-views")
        .h_full()
        .items_end()
        .flex_shrink_0()
        .role(Role::Group)
        .aria_label("View")
        .children(VIEW_CHOICES.map(|(value, name, icon)| {
            let browser = browser.clone();
            let selected = albums == value;
            Button::new(name)
                .ghost()
                .xsmall()
                .w(px(VIEW_TAB))
                .h(px(VIEW_TAB_HEIGHT))
                .rounded(px(0.))
                .rounded_t(px(size::TAB_RADIUS + 1.))
                .px(px(0.))
                .selected(selected)
                .toggled(selected)
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .border_color(if selected {
                    m.line
                } else {
                    transparent_black()
                })
                .when(selected, |b| b.bg(m.canvas).hover(|b| b.bg(m.canvas)))
                .child(Icon::new(icon).size(px(15.)).text_color(if selected {
                    m.text
                } else {
                    m.text_faint
                }))
                .accessibility_label(name)
                .tooltip(name)
                .track_focus(focus.choice(value))
                .tab_stop(true)
                .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.view(value, w, cx)))
        }))
        .into_any_element()
}

/// A standalone folder scope menu, or a shared picker when the view tabs
/// cannot fit beside the scope and filter.
fn view_menu(
    browser: &Entity<Browser>,
    views: BrowserViews,
    focus: &FocusHandle,
    m: crate::theme::Materials,
) -> AnyElement {
    let browser = browser.clone();
    let (id, icon, description, anchor, width) = if let Some(albums) = views.albums {
        let (_, name, icon) = VIEW_CHOICES[usize::from(albums)];
        let action = if views.subfolders.is_some() {
            "Choose view or folder scope"
        } else {
            "Choose view"
        };
        (
            "browser-view-menu",
            icon,
            format!("{action} · {name}"),
            Anchor::TopLeft,
            VIEW_TAB,
        )
    } else {
        let include = views.subfolders.unwrap_or_default();
        let (_, name, icon) = SCOPE_CHOICES[usize::from(include)];
        (
            "browser-folder-scope",
            icon,
            format!("Folder scope · {name}"),
            Anchor::TopRight,
            ICON_BUTTON,
        )
    };
    Button::new(id)
        .ghost()
        .small()
        .w(px(width))
        .dropdown_caret(views.albums.is_some())
        .icon(Icon::new(icon).text_color(m.text_muted))
        .track_focus(focus)
        .tab_stop(true)
        .accessibility_label(description.clone())
        .tooltip(description)
        .dropdown_menu_with_anchor(anchor, move |mut menu, _, _| {
            if let Some(albums) = views.albums {
                for (value, name, icon) in VIEW_CHOICES {
                    let browser = browser.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name)
                            .icon(Icon::new(icon))
                            .checked(albums == value)
                            .on_click(move |_, w, cx| {
                                browser.update(cx, |b, cx| b.view(value, w, cx))
                            }),
                    );
                }
                if views.subfolders.is_some() {
                    menu = menu.separator();
                }
            }
            if let Some(include) = views.subfolders {
                menu = menu.label("Folder scope");
                for (value, name, icon) in SCOPE_CHOICES {
                    let browser = browser.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name)
                            .icon(Icon::new(icon))
                            .checked(include == value)
                            .on_click(move |_, w, cx| {
                                browser.update(cx, |b, cx| {
                                    // Reapplying a scope would clear descendant overrides.
                                    if b.subfolders != value {
                                        b.set_subfolders(value, w, cx);
                                    }
                                })
                            }),
                    );
                }
            }
            menu
        })
        .into_any_element()
}

/// The table of recordings, or of albums and CDs.
impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        // Rows follow the Cover column, also while its boundary is dragged:
        // the table's changes re-render this view.
        let table = self.table.clone();
        let row_height = table.update(cx, |t, _| crate::table::MusicTable::sync_rows(t));
        let mut view = v_flex()
            .size_full()
            .bg(m.canvas)
            .capture_any_mouse_down(cx.listener(|b, e: &MouseDownEvent, _, cx| {
                if matches!(e.button, MouseButton::Left | MouseButton::Right) {
                    b.cancel_context_for_input(cx);
                }
            }))
            .on_scroll(cx.listener(|b, _: &ScrollEvent, _, cx| {
                b.cancel_context_for_input(cx);
            }))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|b, _, w, cx| b.back(w, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|b, _, w, cx| b.forward(w, cx)),
            );
        let browser = cx.entity();
        view = view.child(
            container_query(move |size, window, cx| {
                browser.update(cx, |b, cx| b.bar(size.width, window, cx))
            })
            .w_full()
            .h(px(size::BAR))
            .flex_shrink_0(),
        );
        if let Some(message) = status(self, cx) {
            view = view.child(
                div()
                    .px(px(10.))
                    .text_size(px(size::TEXT_SMALL))
                    .text_color(m.text_muted)
                    .child(message),
            );
        }
        if let Some(notice) = &self.notice {
            view = view.child(
                gpui_kit::component::h_flex()
                    .mx(px(10.))
                    .my(px(4.))
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
        if self.waiting_for_context() {
            return view;
        }
        if self.listing_shelf() {
            return view.child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.discs)
                        .accessibility_label(
                            self.discs
                                .read(cx)
                                .delegate()
                                .directory_label
                                .unwrap_or("Albums and CDs"),
                        )
                        .bordered(false)
                        .stripe(false)
                        .header_row_height(px(30.))
                        .with_size(gpui_kit::component::Size::Size(px(42.))),
                ),
            );
        }
        let fit = table.clone();
        let focused = keyboard_focus(self.table.read(cx).focus_handle(cx), window);
        view.child(
            div()
                .id("tracks-viewport")
                .relative()
                .when_some(
                    match self.location {
                        Location::Playlist(id) => self
                            .catalog
                            .read(cx)
                            .playlist(id)
                            .map(|p| (id, p.name.clone())),
                        _ => None,
                    },
                    |d, (id, name)| {
                        crate::table::playlist_target(
                            d.border_1().border_color(m.canvas),
                            id,
                            name.into(),
                            self.catalog.clone(),
                            cx,
                        )
                    },
                )
                // Keep the outermost header grips clear of the dock splitters.
                .mx(px(4.))
                .on_prepaint(move |bounds, w, cx| {
                    if let Some(d) = crate::diagnostics::get() {
                        d.browser_prepaints
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let width = f32::from(bounds.size.width).floor();
                    let changed =
                        fit.update(cx, |t, cx| crate::table::MusicTable::fit(t, width, cx));
                    // Text columns follow the width, so headings laid out
                    // before it was known are drawn again on the next frame.
                    if changed {
                        let fit = fit.clone();
                        w.on_next_frame(move |_, cx| {
                            fit.update(cx, |t, cx| {
                                t.refresh(cx);
                                cx.notify();
                            })
                        });
                    }
                })
                .key_context("MusicTracks")
                .flex_1()
                .min_h_0()
                .text_size(px(13.))
                .child(
                    DataTable::new(&self.table)
                        .accessibility_label("Recordings")
                        .focus_ring(false)
                        .fixed_cols_border(false)
                        .bordered(false)
                        .stripe(false)
                        .header_row_height(px(size::HEADER))
                        .with_size(gpui_kit::component::Size::Size(px(row_height))),
                )
                .children(focus_outline(focused, &m)),
        )
    }
}

/// Whether the keyboard brought focus to a table, as `:focus-visible` would.
fn keyboard_focus(handle: FocusHandle, window: &Window) -> bool {
    window.last_input_was_keyboard() && handle.is_focused(window)
}

/// The shared keyboard-focus outline, drawn inside a table's viewport so it
/// is never clipped and never moves the rows.
fn focus_outline(focused: bool, m: &crate::theme::Materials) -> Option<Div> {
    focused.then(|| {
        div()
            .absolute()
            .inset_0()
            .rounded(px(5.))
            .border_1()
            .border_color(m.focus_line())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BrowserState, BrowserViews, FILTER_COMPACT, FILTER_FULL, ICON_BUTTON, SCOPE_GAP, VIEW_TAB,
        px,
    };
    use crate::browser::Location;
    use std::path::PathBuf;

    #[test]
    fn header_widths_budget_the_two_view_tabs_and_folder_scope_independently() {
        let library = BrowserViews {
            albums: Some(false),
            subfolders: None,
        };
        let folder = BrowserViews {
            subfolders: Some(true),
            ..library
        };
        let direct = BrowserViews {
            albums: None,
            subfolders: Some(false),
        };
        let flat = BrowserViews {
            albums: None,
            subfolders: None,
        };
        for editing in [false, true] {
            let flat = flat.widths(editing);
            let library = library.widths(editing);
            let folder = folder.widths(editing);
            let direct = direct.widths(editing);
            assert_eq!(library.minimum - flat.minimum, px(2. * VIEW_TAB));
            assert_eq!(
                folder.minimum - library.minimum,
                px(ICON_BUTTON + SCOPE_GAP)
            );
            assert_eq!(direct.minimum - flat.minimum, px(ICON_BUTTON + SCOPE_GAP));
            assert_eq!(folder.minimum - direct.minimum, px(2. * VIEW_TAB));
        }
    }

    #[test]
    fn header_reserves_filter_editing_room() {
        let folder = BrowserViews {
            albums: Some(true),
            subfolders: Some(true),
        };
        let closed = folder.widths(false);
        let open = folder.widths(true);
        assert_eq!(
            open.minimum - closed.minimum,
            px(FILTER_COMPACT - ICON_BUTTON)
        );
        assert_eq!(open.full - open.minimum, px(FILTER_FULL - FILTER_COMPACT));
    }

    #[test]
    fn a_saved_browser_keeps_its_view_and_a_new_one_starts_fresh() {
        let state = BrowserState {
            location: Some(Location::Playlist(3)),
            folder: Some(PathBuf::from("C:\\Music")),
            albums: Some(true),
            scroll: 120.,
            ..Default::default()
        };
        let json = serde_json::to_value(&state).unwrap();
        assert!(json.get("root").is_none());
        assert!(json.get("layouts").is_none());
        assert!(json.get("back").is_none() && json.get("forward").is_none());
        let back: BrowserState = serde_json::from_value(json).unwrap();
        assert_eq!(back.location, Some(Location::Playlist(3)));
        assert_eq!(back.albums, Some(true));
        assert_eq!(back.scroll, 120.);
        // A fresh instance has no saved view and takes the template's.
        let fresh: BrowserState =
            serde_json::from_value(serde_json::Value::Null).unwrap_or_default();
        assert!(fresh.location.is_none() && fresh.albums.is_none());
    }

    #[test]
    fn a_folder_cd_page_and_both_navigation_stacks_round_trip() {
        let state = serde_json::json!({
            "location":{"Folder":"C:/Music"},
            "page":{"Disc":{"Cue":"C:/Music/Disc.cue"}},
            "back":[{"location":"Liked","albums":false,"query":"mix"}],
            "forward":[{"location":{"Playlist":4},"albums":false,"query":"set"}]
        });
        let restored: BrowserState = serde_json::from_value(state.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), state);
    }

    #[test]
    fn obsolete_visits_and_row_keys_do_not_discard_usable_browser_state() {
        let restored: BrowserState = serde_json::from_value(serde_json::json!({
            "back":[{"location":"ObsoleteView"}, {"location":"Liked","query":"mix"}],
            "current":{
                "location":{"Playlist":4},
                "sort":["ObsoleteColumn",true],
                "query":"Retained",
                "bookmark":{"selected":{"ObsoleteRow":42}}
            }
        }))
        .unwrap();
        assert_eq!(restored.back.len(), 1);
        assert_eq!(restored.back[0].query, "mix");
        let current = restored.current.unwrap();
        assert_eq!(current.route.location, Location::Playlist(4));
        assert_eq!(current.query, "Retained");
    }

    #[test]
    fn two_docked_browser_records_restore_their_own_routes_and_history() {
        use crate::panels::PanelRecord;
        let pinned = PanelRecord::with_state(serde_json::json!({
            "current":{"location":{"Playlist":2},"query":"Evening","bookmark":{"selected":{"Key":42}}},
            "back":[{"location":"Liked"}],
            "prefs":{"albums":false,"subfolders":[]}
        }));
        let browse = PanelRecord::with_state(serde_json::json!({
            "current":{"location":{"Folder":"C:/Collection"},"albums":true,"subfolders":true},
            "forward":[{"location":{"Library":11},"albums":true}],
            "recent":[{"location":{"Playlist":2},"query":"Morning"}],
            "prefs":{"albums":true,"subfolders":[{"path":"C:/Collection","include":true}]}
        }));
        let on_disk = serde_json::to_vec(&[pinned, browse]).unwrap();
        let [pinned, browse]: [PanelRecord; 2] = serde_json::from_slice(&on_disk).unwrap();
        let pinned: BrowserState = pinned.state();
        let browse: BrowserState = browse.state();
        assert_eq!(pinned.current.unwrap().query, "Evening");
        assert_eq!(pinned.back[0].route.location, Location::Liked);
        assert!(pinned.forward.is_empty() && browse.back.is_empty());
        assert_eq!(
            browse.forward[0].route.location,
            Location::Library(Some(11))
        );
        assert_eq!(browse.recent[0].query, "Morning");
        assert_eq!(
            browse.current.unwrap().route.location,
            Location::Folder(PathBuf::from("C:/Collection"))
        );
        assert_ne!(pinned.prefs.unwrap().albums, browse.prefs.unwrap().albums);
    }
}

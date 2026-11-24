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
    browser::{Bookmark, Browser, Location, Route, SavedVisit, music_home},
    catalog::Targets,
    collection::{Page, ViewPrefs},
    library::TrackList,
    location::{LocationBar, NAV_FIELD_HEIGHT},
    player::Player,
    table::{Layouts, queue_targets},
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, ElementExt, Icon, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        menu::{PopupMenu, PopupMenuItem},
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
    toolbar: true,
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
    pub location: Entity<LocationBar>,
    pub browser: Entity<Browser>,
    router: Entity<Router>,
    player: Entity<Player>,
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
            b
        });
        let location = cx.new(|cx| LocationBar::new(browser.clone(), window, cx));
        Self {
            location,
            browser,
            router: services.router.clone(),
            player: services.player.clone(),
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
    /// Navigation and search for this instance, at its dock group’s width.
    pub fn address_bar(&self, available: Pixels, cx: &App) -> AnyElement {
        let m = *materials(cx);
        let browser = self.browser.clone();
        let location = self.location.clone();
        let router = self.router.clone();
        let owner = browser.clone();
        let b = browser.read(cx);
        let (can_back, can_forward, has_parent) =
            (b.can_go_back(), b.can_go_forward(), b.can_go_up());
        let search = b.search.clone();
        let (back, forward, up) = (browser.clone(), browser.clone(), browser);
        h_flex()
            .id("address-bar")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .capture_any_mouse_down(move |_, _, cx| {
                router.update(cx, |r, cx| r.set_active(Some(&owner), cx))
            })
            .px(px(3.))
            .gap(px(3.))
            .items_center()
            .border_b_1()
            .border_color(if b.active {
                m.accent.opacity(0.55)
            } else {
                m.line
            })
            .bg(if b.active {
                m.chrome.blend(m.accent.opacity(0.065))
            } else {
                m.chrome
            })
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .child(
                        Self::nav_button(
                            "back",
                            IconName::ArrowLeft,
                            "Back · Alt+Left",
                            can_back,
                            cx,
                        )
                        .on_click(move |_, w, cx| back.update(cx, |b, cx| b.back(w, cx))),
                    )
                    .child(
                        Self::nav_button(
                            "forward",
                            IconName::ArrowRight,
                            "Forward · Alt+Right",
                            can_forward,
                            cx,
                        )
                        .on_click(move |_, w, cx| forward.update(cx, |b, cx| b.forward(w, cx))),
                    )
                    .child(
                        Self::nav_button("up", IconName::ArrowUp, "Up · Alt+Up", has_parent, cx)
                            .on_click(move |_, w, cx| up.update(cx, |b, cx| b.up(w, cx))),
                    ),
            )
            .child(div().flex_1().min_w_0().child(location))
            .child(
                div()
                    .flex_shrink_0()
                    .w((available * 0.23).clamp(px(44.), px(180.)))
                    .child(
                        Styled::h(Input::new(&search).small(), px(NAV_FIELD_HEIGHT))
                            .bordered(false)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .size(px(14.))
                                    .text_color(m.text_faint),
                            )
                            .cleanable(true)
                            .aria_label("Find in current view"),
                    ),
            )
            .into_any_element()
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
            ..Default::default()
        }
    }
    pub fn new_tab_state(browser: &Browser, cx: &App) -> BrowserState {
        BrowserState {
            location: Some(browser.location.clone()),
            page: browser.page.clone(),
            folder: (!browser.path.as_os_str().is_empty()).then(|| browser.path.clone()),
            root: browser.tree_root.clone(),
            albums: Some(browser.prefs.albums),
            prefs: Some(browser.prefs.clone()),
            layouts: Some(browser.table.read(cx).delegate().layouts.clone()),
            search: browser.search.read(cx).value().to_string(),
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
    fn toolbar(&self, _: &mut Window, cx: &mut App) -> Vec<AnyElement> {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let mut items = Vec::new();
        if !b.is_page() && b.can_list_discs() {
            let albums = b.prefs.albums;
            for (id, label, value) in [
                ("view-tracks", "All Tracks", false),
                ("view-albums", "Albums / CDs", true),
            ] {
                let browser = self.browser.clone();
                items.push(
                    Button::new(id)
                        .ghost()
                        .xsmall()
                        .label(label)
                        .selected(albums == value)
                        .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.view(value, w, cx)))
                        .into_any_element(),
                );
            }
        }
        if b.location.is_folder() && b.page.is_none() {
            let include = b.subfolders;
            let browser = self.browser.clone();
            items.push(
                Button::new("view-subfolders")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::FolderTree).size(px(14.)))
                    .label("Subfolders")
                    .selected(include)
                    .tooltip(if include {
                        "Showing subfolders · click to show this folder only"
                    } else {
                        "Include subfolders"
                    })
                    .on_click(move |_, w, cx| {
                        browser.update(cx, |b, cx| b.set_subfolders(!include, w, cx))
                    })
                    .into_any_element(),
            );
        }
        if let Some(status) = status(b, cx) {
            items.push(
                div()
                    .ml(px(6.))
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(m.text_faint)
                    .child(status)
                    .into_any_element(),
            );
        }
        items
    }
    fn menu(&self, menu: PopupMenu, _: &mut Window, cx: &mut App) -> PopupMenu {
        browser_menu(&self.browser, &self.player, menu, cx)
    }
    fn focused(&self, _: &mut Window, cx: &mut App) {
        let browser = self.browser.clone();
        self.router
            .update(cx, |r, cx| r.set_active(Some(&browser), cx));
    }
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<Subscription> {
        vec![
            cx.observe(&self.browser, |pane, _, cx| pane.refresh(cx)),
            cx.subscribe(
                &self.browser,
                |_, _, _: &crate::browser::ViewChanged, cx| cx.emit(super::PaneEvent::Changed),
            ),
        ]
    }
}

/// The table of recordings, or of albums and CDs.
impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        // Rows follow the Cover column, also while its boundary is dragged:
        // the table's changes re-render this view.
        let table = self.table.clone();
        let row_height = table.update(cx, |t, _| crate::table::MusicTable::sync_rows(t));
        let mut view = v_flex()
            .size_full()
            .bg(m.canvas)
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|b, _, w, cx| b.back(w, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|b, _, w, cx| b.forward(w, cx)),
            );
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
        view.child(
            div()
                .id("tracks-viewport")
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
                .on_prepaint(move |bounds, _, cx| {
                    fit.update(cx, |t, cx| {
                        crate::table::MusicTable::fit(t, f32::from(bounds.size.width).floor(), cx)
                    });
                })
                .key_context("MusicTracks")
                .flex_1()
                .min_h_0()
                .text_size(px(13.))
                .child(
                    DataTable::new(&self.table)
                        .accessibility_label("Recordings")
                        .fixed_cols_border(false)
                        .bordered(false)
                        .stripe(false)
                        .header_row_height(px(30.))
                        .with_size(gpui_kit::component::Size::Size(px(row_height))),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::BrowserState;
    use crate::browser::Location;
    use std::path::PathBuf;

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

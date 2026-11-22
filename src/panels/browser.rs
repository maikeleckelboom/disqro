//! The Music Browser: a content panel showing a folder, a collection, a CD
//! or release, Liked Tracks or a playlist.
//!
//! Every instance is its own [`Browser`] with its own location, history,
//! search, selection, scroll position, view mode and column layouts. The
//! catalog, the player and the Locations tree are shared.
use super::{Pane, PaneContent, PanelType, Router, Services};
use crate::{
    assets::FileIcon,
    browser::{Browser, Location, SavedVisit, music_home},
    collection::Page,
    table::Layouts,
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        ElementExt, Icon, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{DropdownMenu, PopupMenuItem},
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) back: Vec<SavedVisit>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) forward: Vec<SavedVisit>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_view: Option<crate::collection::AlbumView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_selection: Option<Page>,
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

pub struct BrowserPane {
    pub browser: Entity<Browser>,
    router: Entity<Router>,
}

impl BrowserPane {
    fn new(services: &Services, state: BrowserState, window: &mut Window, cx: &mut App) -> Self {
        let template = services.router.read(cx).template.clone();
        let layouts = state
            .layouts
            .clone()
            .map(Layouts::validate)
            .unwrap_or(template.layouts);
        let mut prefs = template.prefs;
        prefs.albums = state.albums.unwrap_or(prefs.albums);
        let folders = services.locations.read(cx).folders.clone();
        let (player, catalog) = (services.player.clone(), services.catalog.clone());
        let browser = cx.new(|cx| {
            let mut b = Browser::new(player, catalog, folders, layouts, prefs, window, cx);
            if let Some(view) = state.album_view {
                b.discs.update(cx, |t, cx| {
                    t.delegate_mut().view = view.validate();
                    t.refresh(cx);
                });
            }
            let folder = state.folder.clone().unwrap_or_else(music_home);
            match state.location.clone() {
                Some(Location::Folder(path)) => {
                    b.restore_folder(path, state.root.clone(), window, cx)
                }
                Some(location) => {
                    b.path = folder;
                    b.show(location, window, cx);
                }
                None => b.restore_folder(folder, state.root.clone(), window, cx),
            }
            if b.location.is_folder() {
                b.page = state.page;
            }
            b.restore_history(state.back, state.forward);
            if !state.search.is_empty() {
                b.restore_search(state.search, window, cx);
            }
            b.filter_open = state.filter_open;
            b.restore_scroll(state.scroll, state.album_selection);
            b
        });
        Self {
            browser,
            router: services.router.clone(),
        }
    }
    /// What this instance saves, and what a new instance opened from it
    /// starts with.
    pub fn state(browser: &Browser, cx: &App) -> BrowserState {
        let (back, forward) = browser.saved_history();
        BrowserState {
            back,
            forward,
            scroll: browser.scroll_offset(cx),
            album_selection: browser
                .listing_discs()
                .then(|| {
                    let table = browser.discs.read(cx);
                    table
                        .delegate()
                        .rows
                        .get(table.selected_row()?)
                        .map(|r| match r {
                            crate::collection::Shelf::Disc(d) => Page::Disc(d.id.clone()),
                            crate::collection::Shelf::Release(r) => Page::Release(r.key.clone()),
                        })
                })
                .flatten(),
            ..Self::new_tab_state(browser, cx)
        }
    }
    pub fn new_tab_state(browser: &Browser, cx: &App) -> BrowserState {
        BrowserState {
            location: Some(browser.location.clone()),
            page: browser.page.clone(),
            folder: (!browser.path.as_os_str().is_empty()).then(|| browser.path.clone()),
            root: browser.tree_root.clone(),
            albums: Some(browser.prefs.albums),
            album_view: Some(browser.discs.read(cx).delegate().view.clone()),
            layouts: Some(browser.table.read(cx).delegate().layouts.clone()),
            search: browser.search.read(cx).value().to_string(),
            filter_open: browser.filter_open,
            ..Default::default()
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
            Location::Library(None) => Icon::new(IconName::Library),
            Location::Library(Some(_)) | Location::Album { .. } | Location::Release { .. } => {
                Icon::new(IconName::Disc3)
            }
            Location::Liked => Icon::default().path(crate::assets::HEART_FILL),
            Location::Playlist(_) => Icon::new(IconName::ListMusic),
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
        vec![
            cx.observe(&self.browser, |pane, _, cx| pane.refresh(cx)),
            cx.subscribe(
                &self.browser,
                |_, _, _: &crate::browser::ViewChanged, cx| cx.emit(super::PaneEvent::Changed),
            ),
        ]
    }
}

impl Browser {
    fn controls(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = *materials(cx);
        let b = self;
        let mut items = Vec::new();
        if !b.is_page() && b.can_list_discs() {
            let albums = b.prefs.albums;
            let browser = cx.entity();
            items.push(
                container_query(move |size, w, _| {
                    let choices = [("All Tracks", false), ("Albums / CDs", true)];
                    if size.width < px(290.) || w.viewport_size().width < px(800.) {
                        let browser = browser.clone();
                        Button::new("browser-view")
                            .ghost()
                            .xsmall()
                            .label(if albums {
                                "View: Albums / CDs"
                            } else {
                                "View: All Tracks"
                            })
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, value) in choices {
                                    let browser = browser.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(name).checked(albums == value).on_click(
                                            move |_, w, cx| {
                                                browser.update(cx, |b, cx| b.view(value, w, cx))
                                            },
                                        ),
                                    );
                                }
                                menu
                            })
                            .into_any_element()
                    } else {
                        h_flex()
                            .gap(px(3.))
                            .children(choices.map(|(name, value)| {
                                let browser = browser.clone();
                                Button::new(name)
                                    .ghost()
                                    .xsmall()
                                    .label(name)
                                    .selected(albums == value)
                                    .on_click(move |_, w, cx| {
                                        browser.update(cx, |b, cx| b.view(value, w, cx))
                                    })
                            }))
                            .into_any_element()
                    }
                })
                .flex_1()
                .h(px(25.))
                .into_any_element(),
            );
        }
        if b.location.is_folder() && b.page.is_none() {
            let include = b.subfolders;
            let browser = cx.entity();
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
        let controls = self.controls(cx);
        if !controls.is_empty() {
            view = view.child(
                h_flex()
                    .px(px(8.))
                    .gap(px(4.))
                    .h(px(29.))
                    .items_center()
                    .flex_shrink_0()
                    .children(controls),
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
        if self.listing_discs()
            && (!self.location.is_folder() || !self.discs.read(cx).delegate().rows.is_empty())
        {
            let albums = self.discs.clone();
            return view.child(
                div()
                    .id("album-viewport")
                    .mx(px(4.))
                    .flex_1()
                    .min_h_0()
                    .on_prepaint(move |b, _, cx| {
                        albums.update(cx, |t, cx| {
                            crate::collection::Discs::fit(t, f32::from(b.size.width).floor(), cx)
                        });
                    })
                    .child(
                        DataTable::new(&self.discs)
                            .accessibility_label("Albums and CDs")
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
}

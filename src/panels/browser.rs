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
    theme::{materials, size},
};
use gpui_kit::{
    assets::IconName,
    component::{
        ElementExt, FocusableExt as _, Icon, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
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

const HEADER_INSET: f32 = 6.;
const VIEW_INSET: f32 = 7.;
const VIEW_GAP: f32 = 3.;
const COUNT_GAP: f32 = 10.;
const ICON_BUTTON: f32 = 24.;
const FILTER_COMPACT: f32 = 100.;
const FILTER_FULL: f32 = 175.;

#[derive(Clone, Copy)]
pub(crate) struct BrowserHeaderWidths {
    /// Compact menu and the current filter control, including the drop target.
    pub minimum: Pixels,
    /// Room to edit a filter without moving tab identities.
    pub editable: Pixels,
    /// Labeled controls and the preferred filter width, before metadata.
    pub full: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderDensity {
    Labeled,
    Compact,
    Minimal,
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

impl BrowserHeaderWidths {
    fn density(self, width: Pixels) -> HeaderDensity {
        if width < self.minimum {
            HeaderDensity::Minimal
        } else if width < self.full {
            HeaderDensity::Compact
        } else {
            HeaderDensity::Labeled
        }
    }

    fn fits_summary(self, available: Pixels, text: Pixels) -> bool {
        available >= self.full + px(COUNT_GAP) + text
    }
}

impl Browser {
    /// Only release and disc pages need context beyond their own tab title.
    fn parent_trail(&self, cx: &Context<Self>) -> AnyElement {
        let (parent, library) = match &self.location {
            Location::Album { root, .. } | Location::Release { root, .. } => (
                root.and_then(|r| self.catalog.read(cx).collection(r))
                    .map_or_else(|| "All Music".into(), |r| r.root.name.clone()),
                Some(Location::Library(*root)),
            ),
            _ => (crate::library::display_name(&self.path), None),
        };
        let browser = cx.entity();
        let release = self.release().filter(|(key, _)| !self.shows_release(key));
        // The tab supplies the leaf title. Add a short disc number only when
        // release membership supplies a more specific identity.
        let disc = self.membership.as_ref().map(|_| self.crumb_title(cx));
        let m = *materials(cx);
        h_flex()
            .id("browser-parents")
            .role(Role::Group)
            .aria_label("Parent locations")
            .w_full()
            .min_w_0()
            .h(px(24.))
            .flex_shrink_0()
            .px(px(4.))
            .gap(px(2.))
            .overflow_x_scroll()
            .child(
                Button::new("collection-parent")
                    .ghost()
                    .xsmall()
                    .tab_stop(true)
                    .label(parent)
                    .on_click(cx.listener(move |b, _, w, cx| {
                        if let Some(location) = &library {
                            b.show(location.clone(), w, cx)
                        } else {
                            b.show_collection(w, cx)
                        }
                    })),
            )
            .when_some(release, |d, (key, title)| {
                d.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.))
                        .text_color(m.text_faint),
                )
                .child(
                    Button::new("release-parent")
                        .ghost()
                        .xsmall()
                        .tab_stop(true)
                        .label(title.clone())
                        .on_click(move |_, w, cx| {
                            browser.update(cx, |b, cx| b.open_release(&key, title.clone(), w, cx))
                        }),
                )
            })
            .when_some(disc, |d, label| {
                d.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.))
                        .text_color(m.text_faint),
                )
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_size(px(size::TEXT_SMALL))
                        .text_color(m.text_muted)
                        .child(label),
                )
            })
            .into_any_element()
    }

    pub(crate) fn header_widths(&self, window: &Window, cx: &App) -> BrowserHeaderWidths {
        let view = !self.is_page() && self.can_list_discs();
        let folders = self.location.is_folder() && self.page.is_none();
        let text = |label: &'static str| {
            crate::theme::label_width(label.into(), window.rem_size() * 0.75, window)
        };
        let mut compact = px(HEADER_INSET * 2. + size::TAB_DROP);
        let mut full = compact;
        // Kit's xsmall text buttons use a quarter-rem inset and icon gap.
        let decorated = window.rem_size() * 0.75 + px(16.);
        if view {
            compact += text("Albums / CDs").max(text("All Tracks")) + decorated;
            full += text("All Tracks") + text("Albums / CDs") + px(4. * VIEW_INSET + VIEW_GAP);
        }
        if folders {
            compact += px(ICON_BUTTON);
            full += text("Subfolders") + decorated;
            if view {
                compact += px(VIEW_GAP);
                full += px(VIEW_GAP);
            }
        }
        let editing = self.filter_open || !self.search.read(cx).value().is_empty();
        BrowserHeaderWidths {
            minimum: compact + px(if editing { FILTER_COMPACT } else { ICON_BUTTON }),
            editable: compact + px(FILTER_COMPACT),
            full: full
                + if editing {
                    px(FILTER_FULL)
                } else {
                    text("Filter") + decorated
                },
        }
    }

    pub(crate) fn header_controls(
        &self,
        width: Pixels,
        widths: BrowserHeaderWidths,
        drop_space: Stateful<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let browser = cx.entity();
        let density = widths.density(width);
        let compact = density != HeaderDensity::Labeled;
        let tight = density == HeaderDensity::Minimal;
        let view = !self.is_page() && self.can_list_discs();
        let folders = self.location.is_folder() && self.page.is_none();
        let count = (!compact)
            .then(|| status(self, cx).or_else(|| summary(self, cx)))
            .flatten()
            .filter(|text| {
                let text =
                    crate::theme::label_width(text.clone().into(), px(size::TEXT_SMALL), window);
                widths.fits_summary(width, text)
            });
        let m = *materials(cx);
        h_flex()
            .id("browser-controls")
            .w_full()
            .h(px(size::HEADER))
            .px(px(if tight { 2. } else { HEADER_INSET }))
            .items_center()
            .when(view || folders, |d| {
                // A focused choice keeps its handle when it folds into a menu.
                let focus = window
                    .use_keyed_state(("view-focus", browser.entity_id()), cx, |_, cx| ViewFocus {
                        albums: cx.focus_handle(),
                        tracks: cx.focus_handle(),
                        folders: cx.focus_handle(),
                    })
                    .read(cx)
                    .clone();
                let menu_focus = [&focus.tracks, &focus.albums]
                    .into_iter()
                    .chain((tight && folders).then_some(&focus.folders))
                    .find(|handle| handle.is_focused(window))
                    .unwrap_or(focus.choice(self.prefs.albums));
                d.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap(px(VIEW_GAP))
                        .when(view, |d| {
                            d.child(view_switch(
                                &browser,
                                self.prefs.albums,
                                density,
                                (tight && folders).then_some(self.subfolders),
                                &focus,
                                menu_focus,
                            ))
                        })
                        .when(folders && !(view && tight), |d| {
                            d.child(subfolder_toggle(
                                &browser,
                                self.subfolders,
                                compact,
                                &focus.folders,
                            ))
                        }),
                )
            })
            .when_some(count, |d, text| {
                d.child(
                    div()
                        .when(view || folders, |d| d.ml(px(COUNT_GAP)))
                        .whitespace_nowrap()
                        .text_size(px(size::TEXT_SMALL))
                        .text_color(m.text_faint)
                        .child(text),
                )
            })
            .child(drop_space)
            .child(self.filter_control(compact, tight, window, cx))
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
                .when(!compact, |b| b.label("Filter"))
                .when(compact, |b| b.w(px(ICON_BUTTON)))
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

/// All Tracks or Albums / CDs: side by side while there is room, otherwise a
/// menu that names the current view.
fn view_switch(
    browser: &Entity<Browser>,
    albums: bool,
    density: HeaderDensity,
    subfolders: Option<bool>,
    focus: &ViewFocus,
    menu_focus: &FocusHandle,
) -> AnyElement {
    let choices = [("All Tracks", false), ("Albums / CDs", true)];
    if density != HeaderDensity::Labeled {
        let browser = browser.clone();
        return Button::new("browser-view")
            .ghost()
            .xsmall()
            .label(if density == HeaderDensity::Minimal {
                "View"
            } else if albums {
                "Albums / CDs"
            } else {
                "All Tracks"
            })
            .dropdown_caret(true)
            .track_focus(menu_focus)
            .tab_stop(true)
            .accessibility_label(if albums {
                "View: Albums / CDs"
            } else {
                "View: All Tracks"
            })
            .tooltip("Choose how this view is shown")
            .dropdown_menu(move |mut menu, _, _| {
                for (name, value) in choices {
                    let browser = browser.clone();
                    menu = menu.item(PopupMenuItem::new(name).checked(albums == value).on_click(
                        move |_, w, cx| browser.update(cx, |b, cx| b.view(value, w, cx)),
                    ));
                }
                if let Some(include) = subfolders {
                    let browser = browser.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Include subfolders")
                            .checked(include)
                            .on_click(move |_, w, cx| {
                                browser.update(cx, |b, cx| b.set_subfolders(!include, w, cx))
                            }),
                    );
                }
                menu
            })
            .into_any_element();
    }
    h_flex()
        .id("browser-views")
        .flex_shrink_0()
        .gap(px(VIEW_GAP))
        .role(Role::Group)
        .aria_label("View")
        .children(choices.map(|(name, value)| {
            let browser = browser.clone();
            Button::new(name)
                .ghost()
                .xsmall()
                .px(px(VIEW_INSET))
                .label(name)
                .selected(albums == value)
                .track_focus(focus.choice(value))
                .tab_stop(true)
                .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.view(value, w, cx)))
        }))
        .into_any_element()
}

fn subfolder_toggle(
    browser: &Entity<Browser>,
    include: bool,
    compact: bool,
    focus: &FocusHandle,
) -> AnyElement {
    let browser = browser.clone();
    Button::new("view-subfolders")
        .ghost()
        .xsmall()
        .icon(Icon::new(IconName::FolderTree).size(px(14.)))
        .when(!compact, |b| b.label("Subfolders"))
        .when(compact, |b| b.w(px(ICON_BUTTON)))
        .accessibility_label("Include subfolders")
        .track_focus(focus)
        .tab_stop(true)
        .selected(include)
        .tooltip(if include {
            "Showing subfolders · click to show this folder only"
        } else {
            "Include subfolders"
        })
        .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.set_subfolders(!include, w, cx)))
        .into_any_element()
}

/// What the view holds once it is ready: its albums, or its tracks and their
/// total time.
fn summary(b: &Browser, cx: &App) -> Option<String> {
    let plural = |n: usize, one: &str, many: &str| {
        format!("{} {}", grouped(n), if n == 1 { one } else { many })
    };
    if b.listing_discs() {
        let n = b.discs.read(cx).delegate().rows.len();
        return (n > 0).then(|| plural(n, "album", "albums"));
    }
    if b.busy || b.count == 0 {
        return None;
    }
    let mut text = plural(b.count, "track", "tracks");
    if let Some(total) = b.duration.filter(|d| *d > 0.) {
        text.push_str(" · ");
        text.push_str(&span(total));
    }
    Some(text)
}

/// A count with thousands grouped: "2,123".
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A total listening time as people say it: "31 min", "134 h 5 min".
fn span(seconds: f64) -> String {
    let minutes = (seconds / 60.).round().max(1.) as u64;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
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
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|b, _, w, cx| b.back(w, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|b, _, w, cx| b.forward(w, cx)),
            );
        if self.is_page() {
            view = view.child(self.parent_trail(cx));
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
            let row_height = albums.update(cx, |t, _| crate::collection::Discs::sync_rows(t));
            let focused = keyboard_focus(self.discs.read(cx).focus_handle(cx), window);
            return view.child(
                div()
                    .id("album-viewport")
                    .relative()
                    .mx(px(4.))
                    .flex_1()
                    .min_h_0()
                    .on_prepaint(move |b, w, cx| {
                        let width = f32::from(b.size.width).floor();
                        let changed =
                            albums.update(cx, |t, cx| crate::collection::Discs::fit(t, width, cx));
                        // The headings were laid out with the old widths: draw
                        // the table again once the new ones are known.
                        if changed {
                            let albums = albums.clone();
                            w.on_next_frame(move |_, cx| {
                                albums.update(cx, |t, cx| {
                                    t.refresh(cx);
                                    cx.notify();
                                })
                            });
                        }
                    })
                    .child(
                        DataTable::new(&self.discs)
                            .accessibility_label("Albums and CDs")
                            .focus_ring(false)
                            .bordered(false)
                            .stripe(false)
                            .header_row_height(px(size::HEADER))
                            .with_size(gpui_kit::component::Size::Size(px(row_height))),
                    )
                    .children(focus_outline(focused, &m)),
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
    use super::{BrowserHeaderWidths, BrowserState, HeaderDensity, grouped, px, span};
    use crate::browser::Location;
    use std::path::PathBuf;

    #[test]
    fn browser_header_drops_metadata_before_compacting_essential_controls() {
        let widths = BrowserHeaderWidths {
            minimum: px(144.),
            editable: px(220.),
            full: px(245.),
        };
        assert!(widths.fits_summary(px(306.), px(51.)));
        assert!(!widths.fits_summary(px(305.), px(51.)));
        assert_eq!(widths.density(px(305.)), HeaderDensity::Labeled);
        assert_eq!(widths.density(px(245.)), HeaderDensity::Labeled);
        assert_eq!(widths.density(px(244.)), HeaderDensity::Compact);
        assert_eq!(widths.density(px(144.)), HeaderDensity::Compact);
        assert_eq!(widths.density(px(143.)), HeaderDensity::Minimal);
    }

    #[test]
    fn summaries_read_as_people_say_them() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(2123), "2,123");
        assert_eq!(grouped(100_089), "100,089");
        assert_eq!(span(20.), "1 min");
        assert_eq!(span(30. * 60. + 55.), "31 min");
        assert_eq!(span(2. * 3600.), "2 h");
        assert_eq!(span(134. * 3600. + 4. * 60. + 33.), "134 h 5 min");
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
}

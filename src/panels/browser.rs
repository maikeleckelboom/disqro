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
        Disableable, ElementExt, FocusableExt as _, Icon, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
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
        // Each browser edits its own path, in its own header.
        let address = cx.new(|cx| crate::location::LocationBar::new(browser.clone(), window, cx));
        browser.update(cx, |b, cx| {
            cx.observe(&address, |_, _, cx| cx.notify()).detach();
            b.address = Some(address);
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
const IDENTITY_ICON: f32 = 15.;
const IDENTITY_GAP: f32 = 4.;
/// The location keeps room for a short name before the controls compress.
const IDENTITY_MIN: f32 = 140.;

/// A parent place in a page's header trail.
#[derive(Clone)]
enum Crumb {
    /// The collection or folder holding the page.
    Parent(Option<Location>),
    /// The release a disc belongs to.
    Release(crate::collection::DiscId, String),
}
const COUNT_GAP: f32 = 10.;
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
    /// The places above the one shown, each a button, for release and disc
    /// pages. The last entry is the page itself.
    fn trail(&self, cx: &App) -> Vec<(SharedString, Option<Crumb>)> {
        if !self.is_page() {
            return vec![(self.title(cx).into(), None)];
        }
        let (parent, library) = match &self.location {
            Location::Album { root, .. } | Location::Release { root, .. } => (
                root.and_then(|r| self.catalog.read(cx).collection(r))
                    .map_or_else(|| "All Music".into(), |r| r.root.name.clone()),
                Some(Location::Library(*root)),
            ),
            _ => (crate::library::display_name(&self.path), None),
        };
        let mut trail = vec![(parent.into(), Some(Crumb::Parent(library)))];
        if let Some((key, title)) = self.release().filter(|(key, _)| !self.shows_release(key)) {
            trail.push((title.clone().into(), Some(Crumb::Release(key, title))));
        }
        // A disc of a release is named by its number below the release.
        let leaf = match &self.membership {
            Some(_) => self.crumb_title(cx),
            None => self.title(cx),
        };
        trail.push((leaf.into(), None));
        trail
    }

    /// The browser's header: its history, where it is, what it holds, and
    /// how it is shown. Every browser has one, whether it is alone in its
    /// group or one of several tabs, so its controls never move.
    fn bar(&self, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let widths = self.header_widths(cx);
        let nav = px(3. * NAV_BUTTON + 2. * NAV_GAP + BAR_INSET);
        // The location keeps a readable minimum; the controls take what they
        // need up to their comfortable width and give way first.
        let controls = (width - nav - px(IDENTITY_MIN))
            .min(widths.full)
            .max(widths.minimum.min(width - nav))
            .max(px(0.));
        let identity = (width - nav - controls).max(px(0.));
        let editing = self
            .address
            .as_ref()
            .is_some_and(|a| a.read(cx).is_editing());
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
                    .map(|d| match (editing, &self.address) {
                        (true, Some(address)) => d.pl(px(6.)).pr(px(6.)).child(address.clone()),
                        _ => d.child(self.identity(identity, window, cx)),
                    }),
            )
            .child(
                div()
                    .w(controls)
                    .h_full()
                    .flex_shrink_0()
                    .child(self.header_controls(controls, widths, window, cx)),
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

    /// Where the browser is: an icon, the places above it, its own name and
    /// what it holds. The count gives way first, then the parents.
    fn identity(&self, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let trail = self.trail(cx);
        let status = status(self, cx).or_else(|| summary(self, cx));
        let measure = |text: &SharedString, size: f32| {
            crate::theme::label_width(text.clone(), px(size), window)
        };
        let leaf = trail[trail.len() - 1].0.clone();
        let fixed = px(IDENTITY_ICON + 2. * IDENTITY_GAP + 8.) + measure(&leaf, size::TEXT);
        let parents: Pixels = trail[..trail.len() - 1]
            .iter()
            .map(|(label, _)| measure(label, size::TEXT_SMALL) + px(30.))
            .sum();
        let count = status.map(SharedString::from).filter(|text| {
            fixed + parents + px(COUNT_GAP) + measure(text, size::TEXT_SMALL) <= width
        });
        let show_parents = fixed + parents <= width;
        let browser = cx.entity();
        let icon = match &self.location {
            Location::Folder(_) => FileIcon::Folder.icon(),
            Location::Library(None) => Icon::new(IconName::Library),
            Location::Library(Some(_)) | Location::Album { .. } | Location::Release { .. } => {
                Icon::new(IconName::Disc3)
            }
            Location::Liked => Icon::default().path(crate::assets::HEART_FILL),
            Location::Playlist(_) => Icon::new(IconName::ListMusic),
        };
        let pane = self.pane.clone();
        let mut row = h_flex()
            .id("browser-location")
            .role(Role::Group)
            .aria_label("Location")
            .h_full()
            .min_w_0()
            .flex_1()
            .pl(px(6.))
            .gap(px(IDENTITY_GAP))
            .items_center()
            .child(
                icon.size(px(IDENTITY_ICON))
                    .flex_shrink_0()
                    .text_color(if self.active { m.accent } else { m.text_faint }),
            );
        if show_parents {
            for (ix, (label, crumb)) in trail[..trail.len() - 1].iter().cloned().enumerate() {
                let Some(crumb) = crumb else { continue };
                let browser = browser.clone();
                row = row
                    .child(
                        Button::new(("crumb", ix))
                            .ghost()
                            .xsmall()
                            .flex_shrink_0()
                            .tab_stop(true)
                            .label(label)
                            .text_color(m.text_muted)
                            .on_click(move |_, w, cx| {
                                browser.update(cx, |b, cx| match &crumb {
                                    Crumb::Parent(Some(location)) => {
                                        b.show(location.clone(), w, cx)
                                    }
                                    Crumb::Parent(None) => b.show_collection(w, cx),
                                    Crumb::Release(key, title) => {
                                        b.open_release(key, title.clone(), w, cx)
                                    }
                                })
                            }),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(12.))
                            .flex_shrink_0()
                            .text_color(m.text_faint),
                    );
            }
        }
        let tip = leaf.clone();
        row.child(
            div()
                .id("browser-title")
                .min_w_0()
                .truncate()
                .text_size(px(size::TEXT))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(m.text)
                .child(leaf)
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                }),
        )
        .when_some(count, |d, text| {
            d.child(
                div()
                    .ml(px(COUNT_GAP - IDENTITY_GAP))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_size(px(size::TEXT_SMALL))
                    .text_color(m.text_faint)
                    .child(text),
            )
        })
        // A browser alone in its group has no tab; its header offers the
        // tab's commands instead.
        .context_menu(move |menu, w, cx| {
            let pane = pane.as_ref().and_then(WeakEntity::upgrade);
            match (pane, cx.try_global::<super::PaneMenus>().cloned()) {
                (Some(pane), Some(menus)) => (menus.0)(&pane, menu, w, cx),
                _ => menu,
            }
        })
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
        let browser = cx.entity();
        view = view.child(
            container_query(move |size, window, cx| {
                browser.update(cx, |b, cx| b.bar(size.width, window, cx))
            })
            .w_full()
            .h(px(size::BAR))
            .flex_shrink_0(),
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
                .on_prepaint(move |bounds, w, cx| {
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
        grouped, px, span,
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

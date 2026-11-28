//! The Music Browser: a content panel showing a folder, a collection, a CD
//! or release, Favorites or a playlist.
//!
//! Every instance is its own [`Browser`] with its own location, history,
//! search, selection, scroll position, view mode and column layouts. The
//! catalog, the player and the Locations tree are shared.
use super::{Pane, PaneContent, PanelType, Router, Services};
use crate::{
    assets::FileIcon,
    audio::Command,
    browser::{Browser, Location, Route, SavedVisit, SequenceKind, music_home},
    collection::{ViewMode, ViewPrefs},
    location::LocationBar,
    player::Player,
    table::{Layouts, queue_targets},
    theme::{materials, size},
};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, ElementExt, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layouts: Option<Layouts>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub filter_open: bool,
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
        let prefs = state.prefs.unwrap_or(template.prefs);
        let folders = services.locations.read(cx).folders.clone();
        let (player, catalog) = (services.player.clone(), services.catalog.clone());
        let browser = cx.new(|cx| {
            let mode = prefs.mode;
            let mut b = Browser::new(player, catalog, folders, layouts, prefs, window, cx);
            b.path = state.folder.unwrap_or_else(music_home);
            b.tree_root = state.root.clone();
            let current = state.current.unwrap_or_else(|| {
                let mut visit =
                    SavedVisit::new(Route::new(Location::Folder(b.path.clone()), None), mode);
                visit.root = state.root;
                visit
            });
            b.restore_history(state.back, state.forward, state.recent);
            b.restore_visit(current, window, cx);
            b.filter_open = state.filter_open;
            b
        });
        // Each browser edits its own path, in its own header.
        let address = cx.new(|cx| LocationBar::new(browser.clone(), window, cx));
        browser.update(cx, |b, cx| {
            b.address = Some(address.downgrade());
            b.tree.update(cx, |t, _| {
                t.delegate_mut().router = Some(services.router.clone())
            });
            b.discs.update(cx, |t, _| {
                t.delegate_mut().router = Some(services.router.clone())
            });
        });
        Self {
            browser,
            router: services.router.clone(),
            _address: address,
        }
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
            layouts: Some(browser_layouts(browser, cx)),
            filter_open: browser.filter_open,
        }
    }
    pub fn new_tab_state(browser: &Browser, cx: &App) -> BrowserState {
        let mut visit = SavedVisit::new(browser.route(), browser.prefs.mode);
        visit.root = browser.tree_root.clone();
        visit.subfolders = browser
            .folder_scope()
            .or_else(|| browser.location.is_folder().then_some(browser.subfolders));
        visit.query = browser.search.read(cx).value().to_string();
        BrowserState {
            current: Some(visit),
            folder: (!browser.path.as_os_str().is_empty()).then(|| browser.path.clone()),
            root: browser.tree_root.clone(),
            prefs: Some(browser.prefs.clone()),
            layouts: Some(browser_layouts(browser, cx)),
            ..Default::default()
        }
    }
}

fn browser_layouts(browser: &Browser, cx: &App) -> Layouts {
    let mut layouts = browser.table.read(cx).delegate().layouts.clone();
    layouts.shelf = browser.discs.read(cx).delegate().shelf_layout.clone();
    layouts.artists = browser.discs.read(cx).delegate().artist_layout.clone();
    layouts.tree = browser.tree.read(cx).delegate().layout.clone();
    layouts
}

/// The panel menu: play or queue what the browser shows, and act on its view.
fn browser_menu(
    browser: &Entity<Browser>,
    player: &Entity<Player>,
    mut menu: PopupMenu,
    cx: &App,
) -> PopupMenu {
    use crate::collection::{DiscId, Page};
    let b = browser.read(cx);
    let table = b.table.read(cx).delegate();
    let empty = table.is_empty();
    let location = b.location.clone();
    let catalog = b.catalog.clone();
    let in_library = catalog
        .read(cx)
        .collections
        .iter()
        .any(|c| b.path.starts_with(&c.root.path));
    if let Some(target) = b.up_in_toolbar().then(|| b.up_label(cx)) {
        let up = browser.clone();
        let label = target.map_or_else(|| "Up".to_owned(), |target| format!("Up · {target}"));
        menu = menu
            .item(
                PopupMenuItem::new(label)
                    .icon(IconName::ArrowUp)
                    .disabled(!b.can_go_up())
                    .on_click(move |_, w, cx| up.update(cx, |b, cx| b.up(w, cx))),
            )
            .separator();
    }
    if b.listing_tree() {
        let targets = b.tree_visible_tracks(cx);
        let empty = targets.is_empty();
        for (label, icon, shuffle) in [
            ("Play Visible Tracks", IconName::Play, false),
            ("Shuffle Visible Tracks", IconName::Shuffle, true),
        ] {
            let (targets, catalog, player) = (targets.clone(), catalog.clone(), player.clone());
            menu = menu.item(
                PopupMenuItem::new(label)
                    .icon(icon)
                    .disabled(empty)
                    .on_click(move |_, _, cx| {
                        queue_targets(
                            &catalog,
                            &player,
                            targets.clone(),
                            move |selection| Command::PlayList(selection, shuffle),
                            cx,
                        );
                    }),
            );
        }
        let (catalog, player) = (catalog.clone(), player.clone());
        menu = menu.item(
            PopupMenuItem::new("Add Visible Tracks to Queue")
                .icon(IconName::ListPlus)
                .disabled(empty)
                .on_click(move |_, _, cx| {
                    queue_targets(&catalog, &player, targets.clone(), Command::Enqueue, cx)
                }),
        );
        let refresh = browser.clone();
        return menu.separator().item(
            PopupMenuItem::new("Refresh")
                .icon(IconName::RefreshCw)
                .on_click(move |_, w, cx| refresh.update(cx, |b, cx| b.reload(w, cx))),
        );
    }
    let axis = b.sequence_kind();
    let extra_axis = axis
        .filter(|kind| *kind == SequenceKind::Disc)
        .map(|_| SequenceKind::Release);
    for kind in axis.into_iter().chain(extra_axis) {
        for next in [false, true] {
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
    }
    let shuffle_table = b.table.clone();
    let add_table = b.table.clone();
    let add_player = player.clone();
    let add_catalog = catalog.clone();
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
        _ => b.release_key().map(Page::Release),
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
    let collection =
        b.can_list_discs() && !matches!(b.location, Location::Artist { .. } | Location::Releases);
    let play_label = match &page {
        Some(Page::Disc(_)) => "Play CD",
        Some(Page::Release(_)) => "Play Release",
        None if collection => "Play Collection",
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
                    None => cd_table.update(cx, |t, cx| t.delegate().play_all(false, cx)),
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
                .on_click(move |_, _, cx| {
                    shuffle_table.update(cx, |t, cx| t.delegate().play_all(true, cx))
                }),
        )
        .item(
            PopupMenuItem::new("Add All to Queue")
                .icon(IconName::ListEnd)
                .disabled(empty)
                .on_click(move |_, _, cx| {
                    queue_targets(
                        &add_catalog,
                        &add_player,
                        add_table.read(cx).delegate().all_targets(),
                        Command::Enqueue,
                        cx,
                    )
                }),
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
        Location::Liked
        | Location::RecentlyPlayed
        | Location::Playlist(_)
        | Location::Collections
        | Location::Playlists
        | Location::Artists
        | Location::Artist { .. }
        | Location::Releases => menu,
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
            Location::RecentlyPlayed => Icon::new(IconName::Clock),
            Location::Playlist(_) | Location::Playlists => Icon::new(IconName::ListMusic),
            Location::Artists | Location::Artist { .. } => Icon::new(IconName::MicVocal),
            Location::Releases => Icon::new(IconName::Disc3),
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

/// The space between the toolbar's edge and its first and last controls.
const BAR_INSET: f32 = 6.;
/// The space between neighboring toolbar controls.
const CONTROL_GAP: f32 = 2.;
/// The space between the breadcrumb and the controls on either side.
const ADDRESS_GAP: f32 = size::GAP;
const ICON_BUTTON: f32 = size::ICON_BUTTON;
/// The view picker's chrome around its label: padding, icon, gaps and caret.
const PICKER_CHROME: f32 = 58.;
const VIEW_CHOICES: [(ViewMode, &str, IconName); 3] = [
    (ViewMode::Tracks, "Tracks", IconName::ListMusic),
    (ViewMode::Albums, "Releases", IconName::Disc3),
    (ViewMode::Tree, "Tree", IconName::ListTree),
];
const SCOPE_CHOICES: [(bool, &str, IconName); 2] = [
    (false, "This folder only", IconName::Folder),
    (true, "Include subfolders", IconName::Folders),
];

#[derive(Clone, Copy)]
struct BrowserViews {
    mode: Option<ViewMode>,
    subfolders: Option<bool>,
    /// The presentations the destination offers.
    choices: &'static [ViewMode],
}

impl BrowserViews {
    /// The views the destination offers, in their usual order.
    fn offered(self) -> impl Iterator<Item = (usize, (ViewMode, &'static str, IconName))> {
        VIEW_CHOICES
            .into_iter()
            .enumerate()
            .filter(move |(_, (mode, _, _))| self.choices.contains(mode))
    }

    /// Whether the destination has a view or folder scope to choose.
    fn has_picker(self) -> bool {
        self.mode.is_some() || self.subfolders.is_some()
    }

    fn menu(self, mut menu: PopupMenu, browser: &Entity<Browser>) -> PopupMenu {
        if let Some(mode) = self.mode {
            for (_, (value, name, icon)) in self.offered() {
                let browser = browser.clone();
                menu = menu.item(
                    PopupMenuItem::new(name)
                        .icon(Icon::new(icon))
                        .checked(mode == value)
                        .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.view(value, w, cx))),
                );
            }
            if self.subfolders.is_some() {
                menu = menu.separator();
            }
        }
        if let Some(include) = self.subfolders {
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
    }
}

/// The Music Browser toolbar has one composition at every width:
///
/// ```text
/// Back  Forward  Breadcrumb ……………………  Filter  View ▾  More
/// ```
///
/// Width changes sizes, never the controls. The breadcrumb takes every
/// pixel the controls leave: it folds its earliest ancestors into a menu,
/// then truncates the current location. Once the breadcrumb reaches
/// [`ADDRESS_MIN`], so the current location stays identifiable, the view
/// picker's label truncates instead, down to [`PICKER_MIN`]. Below that the
/// toolbar keeps its composition and the breadcrumb gives way, and in a
/// panel narrower than the controls themselves, Back is the first to clip.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ToolbarLayout {
    /// The breadcrumb's width.
    address: Pixels,
    /// The view picker's width, when the destination offers one.
    picker: Option<Pixels>,
}

/// The narrowest breadcrumb before the view picker gives way: room for the
/// Earlier locations button and the start of the current location.
const ADDRESS_MIN: f32 = 132.;
/// The narrowest view picker: its icon, caret and the start of its label.
const PICKER_MIN: f32 = PICKER_CHROME + 20.;

impl ToolbarLayout {
    /// Everything but the breadcrumb and the view picker: insets, gaps,
    /// Back and Forward, Filter and the panel menu.
    fn fixed(picker: bool) -> Pixels {
        let history = px(2. * ICON_BUTTON + CONTROL_GAP);
        let controls =
            px(2. * ICON_BUTTON + CONTROL_GAP) + if picker { px(CONTROL_GAP) } else { px(0.) };
        px(2. * BAR_INSET + 2. * ADDRESS_GAP) + history + controls
    }

    /// `picker` is the view picker's full width, when the destination
    /// offers one.
    fn new(width: Pixels, picker: Option<Pixels>) -> Self {
        let available = (width - Self::fixed(picker.is_some())).max(px(0.));
        let picker =
            picker.map(|full| (available - px(ADDRESS_MIN)).clamp(px(PICKER_MIN).min(full), full));
        Self {
            address: (available - picker.unwrap_or_default()).max(px(0.)),
            picker,
        }
    }

    /// The controls to the right of the breadcrumb.
    fn controls(self) -> Pixels {
        px(2. * ICON_BUTTON + CONTROL_GAP)
            + self
                .picker
                .map_or(px(0.), |picker| picker + px(CONTROL_GAP))
    }
}

#[derive(Clone)]
struct ViewFocus {
    view: FocusHandle,
    filter: FocusHandle,
    back: FocusHandle,
    forward: FocusHandle,
}

impl Browser {
    /// The view picker's width, the same for every view, so choosing one
    /// never moves the controls beside it.
    fn picker_width(window: &Window) -> Pixels {
        VIEW_CHOICES
            .iter()
            .map(|(_, name, _)| {
                crate::theme::label_width((*name).into(), px(size::TEXT_SMALL), window)
            })
            .fold(px(0.), Pixels::max)
            + px(PICKER_CHROME)
    }

    /// The shared toolbar for standalone and tabbed browsers.
    fn bar(&self, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let browser = cx.entity();
        let focus = window
            .use_keyed_state(("view-focus", browser.entity_id()), cx, |_, cx| ViewFocus {
                view: cx.focus_handle(),
                filter: cx.focus_handle(),
                back: cx.focus_handle(),
                forward: cx.focus_handle(),
            })
            .read(cx)
            .clone();
        let views = self.header_views();
        let picker = views.has_picker().then(|| Self::picker_width(window));
        let layout = ToolbarLayout::new(width, picker);
        if let Some(address) = self.address.as_ref().and_then(WeakEntity::upgrade) {
            address.update(cx, |bar, cx| bar.set_width(layout.address, cx));
        }
        let address = self.address.as_ref().and_then(WeakEntity::upgrade);
        let player = self.table.read(cx).delegate().player.clone();
        let filter_applied = !self.search.read(cx).value().is_empty();
        let filter_shown = self.filter_shown(cx);
        h_flex()
            .id("browser-bar")
            .w(width)
            .h(px(size::BAR))
            .flex_shrink_0()
            .items_center()
            .px(px(BAR_INSET))
            .gap(px(ADDRESS_GAP))
            // Narrower than its composition, the toolbar keeps its right end,
            // where the panel menu offers everything, and clips its left.
            .justify_end()
            .overflow_hidden()
            .bg(m.canvas)
            .when(!filter_shown, |bar| bar.border_b_1().border_color(m.line))
            .child(self.history(&focus, cx))
            .child(
                h_flex()
                    .w(layout.address)
                    .flex_shrink_0()
                    .h_full()
                    .min_w_0()
                    .items_center()
                    .children(address),
            )
            .child(
                h_flex()
                    .id("browser-controls")
                    .ml_auto()
                    .w(layout.controls())
                    .flex_shrink_0()
                    .gap(px(CONTROL_GAP))
                    .items_center()
                    .child(
                        Button::new("open-filter")
                            .ghost()
                            .w(px(ICON_BUTTON))
                            .h(px(ICON_BUTTON))
                            .icon(
                                Icon::new(IconName::ListFilter)
                                    .size(px(size::ICON))
                                    // A filter that hides rows stays visible.
                                    .text_color(if filter_applied {
                                        m.accent
                                    } else {
                                        m.text_muted
                                    }),
                            )
                            // Pressed while the filter row is shown; the name
                            // stays the same in every state.
                            .toggled(filter_shown)
                            .track_focus(&focus.filter)
                            .tab_stop(true)
                            .accessibility_label("Filter this view")
                            .tooltip(if filter_applied {
                                "Filter this view · A filter hides rows · Ctrl+F"
                            } else {
                                "Filter this view · Ctrl+F"
                            })
                            .on_click(cx.listener(|b, _, w, cx| b.set_filter_open(true, w, cx))),
                    )
                    .when_some(layout.picker, |row, picker| {
                        row.child(view_picker(&browser, views, picker, &focus.view, m))
                    })
                    .child(
                        Button::new("browser-menu")
                            .ghost()
                            .w(px(ICON_BUTTON))
                            .h(px(ICON_BUTTON))
                            .icon(
                                Icon::new(IconName::Ellipsis)
                                    .size(px(size::ICON))
                                    .text_color(m.text_muted),
                            )
                            .accessibility_label("Panel menu")
                            .tab_stop(true)
                            .tooltip("Panel menu")
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, w, cx| {
                                let menu = browser_menu(&browser, &player, menu, cx);
                                let pane =
                                    browser.read(cx).pane.as_ref().and_then(WeakEntity::upgrade);
                                match (pane, cx.try_global::<super::PaneMenus>().cloned()) {
                                    (Some(pane), Some(menus)) => {
                                        (menus.0)(&pane, menu.separator(), w, cx)
                                    }
                                    _ => menu,
                                }
                            }),
                    ),
            )
            .into_any_element()
    }

    /// Whether the filter row is shown: once asked for, and for as long as
    /// a filter hides rows.
    fn filter_shown(&self, cx: &App) -> bool {
        self.filter_open || !self.search.read(cx).value().is_empty()
    }

    fn history(&self, focus: &ViewFocus, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let button = |id: &'static str,
                      icon: IconName,
                      action: &'static str,
                      shortcut: &'static str,
                      target: Option<String>| {
            let enabled = target.is_some();
            let label = target.map_or_else(
                || format!("{action} · {shortcut}"),
                |target| format!("{action} · {target} · {shortcut}"),
            );
            Button::new(id)
                .ghost()
                .w(px(ICON_BUTTON))
                .h(px(ICON_BUTTON))
                .icon(
                    Icon::new(icon)
                        .size(px(size::ICON))
                        .text_color(m.text_muted),
                )
                .accessibility_label(label.clone())
                .tooltip(label)
                .tab_stop(true)
                .disabled(!enabled)
        };
        h_flex()
            .id("browser-history")
            .role(Role::Group)
            .aria_label("History")
            .flex_shrink_0()
            .gap(px(CONTROL_GAP))
            .child(
                button(
                    "back",
                    IconName::ArrowLeft,
                    "Back",
                    "Alt+Left",
                    self.history_label(false, cx),
                )
                .track_focus(&focus.back)
                .on_click(cx.listener(|b, _, w, cx| b.back(w, cx))),
            )
            .child(
                button(
                    "forward",
                    IconName::ArrowRight,
                    "Forward",
                    "Alt+Right",
                    self.history_label(true, cx),
                )
                .track_focus(&focus.forward)
                .on_click(cx.listener(|b, _, w, cx| b.forward(w, cx))),
            )
            .into_any_element()
    }

    fn header_views(&self) -> BrowserViews {
        BrowserViews {
            mode: self
                .has_presentations()
                .then(|| self.presentation())
                .flatten(),
            subfolders: self.folder_scope(),
            choices: self.presentations(),
        }
    }

    /// The filter field, in its own row below the toolbar, so opening it
    /// never moves the toolbar's controls.
    fn filter_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.filter_shown(cx) {
            return None;
        }
        let m = *materials(cx);
        let input = self.search.clone();
        let empty = input.read(cx).value().is_empty();
        let scope = self.tree_filter_scope();
        Some(
            h_flex()
                .id("browser-filter")
                .w_full()
                .h(px(size::HEADER))
                .flex_shrink_0()
                .pl(px(size::INSET))
                .pr(px(BAR_INSET))
                .gap(px(size::GAP))
                .items_center()
                .bg(m.canvas)
                .border_b_1()
                .border_color(m.line)
                .when_some(scope, |d, scope| {
                    d.tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(scope).build(w, cx)
                    })
                })
                .child(
                    Icon::new(IconName::ListFilter)
                        .size(px(size::ICON_SMALL))
                        .flex_shrink_0()
                        .text_color(if empty { m.text_faint } else { m.accent }),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&input)
                            .small()
                            .appearance(false)
                            .aria_label(scope.unwrap_or("Filter this view")),
                    ),
                )
                .child(
                    Button::new("close-filter")
                        .ghost()
                        .w(px(ICON_BUTTON))
                        .h(px(ICON_BUTTON))
                        .icon(
                            Icon::new(IconName::X)
                                .size(px(size::ICON_SMALL))
                                .text_color(m.text_muted),
                        )
                        .accessibility_label("Clear and close filter")
                        .tooltip("Clear and close filter · Esc")
                        .on_click(cx.listener(|b, _, w, cx| b.dismiss_filter(w, cx))),
                )
                .capture_key_down(cx.listener(|b, e: &KeyDownEvent, w, cx| {
                    if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                        b.dismiss_filter(w, cx);
                        w.prevent_default();
                        cx.stop_propagation();
                    }
                }))
                .into_any_element(),
        )
    }
}

/// The view picker: the current view's name, and a menu of the views and
/// folder scope this destination offers. Its width never changes.
fn view_picker(
    browser: &Entity<Browser>,
    views: BrowserViews,
    width: Pixels,
    focus: &FocusHandle,
    m: crate::theme::Materials,
) -> AnyElement {
    let browser = browser.clone();
    let (name, icon) = match views.mode {
        Some(mode) => VIEW_CHOICES
            .into_iter()
            .find(|(choice, _, _)| *choice == mode)
            .map(|(_, name, icon)| (name, icon))
            .unwrap_or(("Tracks", IconName::ListMusic)),
        None => {
            let (_, name, icon) = SCOPE_CHOICES[usize::from(views.subfolders.unwrap_or_default())];
            (name, icon)
        }
    };
    let description = if views.mode.is_some() {
        format!("View · {name}")
    } else {
        format!("Folder scope · {name}")
    };
    Button::new("browser-view")
        .ghost()
        .w(width)
        .h(px(ICON_BUTTON))
        .px(px(8.))
        .justify_start()
        .gap(px(6.))
        .icon(
            Icon::new(icon)
                .size(px(size::ICON))
                .text_color(m.text_muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(size::TEXT_SMALL))
                .text_color(m.text)
                .child(if views.mode.is_some() { name } else { "Scope" }),
        )
        .dropdown_caret(true)
        .track_focus(focus)
        .tab_stop(true)
        .accessibility_label(description.clone())
        .tooltip(description)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            views.menu(menu, &browser)
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
        if let Some(row) = self.filter_row(cx) {
            view = view.child(row);
        }
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
                    .text_size(px(crate::theme::size::LABEL))
                    .text_color(m.warning)
                    .child(Icon::new(IconName::TriangleAlert).size(px(13.)))
                    .child(div().truncate().child(notice.clone())),
            );
        }
        if self.waiting_for_context() {
            return view;
        }
        if self.listing_tree() {
            let table = self.tree.clone();
            let focused = keyboard_focus(table.read(cx).focus_handle(cx), window);
            return view.child(
                div()
                    .id("tree-viewport")
                    .relative()
                    .mx(px(4.))
                    .flex_1()
                    .min_h_0()
                    .on_prepaint(move |bounds, w, cx| {
                        crate::table::fit_columns(
                            &table,
                            f32::from(bounds.size.width).floor(),
                            w,
                            cx,
                        );
                    })
                    .capture_key_down(
                        cx.listener(|b, e: &KeyDownEvent, w, cx| b.tree_key_down(e, w, cx)),
                    )
                    .capture_any_mouse_down({
                        // A right click below the rows offers the whole tree's
                        // actions; a row claims the click for itself first.
                        let table = self.tree.clone();
                        move |event, _, cx| {
                            if event.button == MouseButton::Right {
                                table.update(cx, |t, cx| {
                                    t.set_right_clicked_row(Some(usize::MAX), cx)
                                });
                            }
                        }
                    })
                    .child(crate::table::browser_table(
                        &self.tree,
                        crate::table::ROW_HEIGHT,
                        "Music tree",
                    ))
                    .children(focus_outline(focused, &m)),
            );
        }
        if self.listing_shelf() {
            let table = self.discs.clone();
            let state = table.read(cx);
            let label = state.delegate().directory_label.unwrap_or("Releases");
            let height = state.delegate().row_height();
            let focused = keyboard_focus(state.focus_handle(cx), window);
            return view.child(
                div()
                    .id("albums-viewport")
                    .relative()
                    .mx(px(4.))
                    .flex_1()
                    .min_h_0()
                    .on_prepaint(move |bounds, w, cx| {
                        crate::table::fit_columns(
                            &table,
                            f32::from(bounds.size.width).floor(),
                            w,
                            cx,
                        );
                    })
                    .on_key_down(cx.listener(|b, e: &KeyDownEvent, w, cx| {
                        if e.keystroke.key == "enter"
                            && !e.keystroke.modifiers.modified()
                            && let Some(row) = b.discs.read(cx).selected_row()
                        {
                            b.show_disc(row, w, cx);
                            w.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    .child(crate::table::browser_table(&self.discs, height, label))
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
                    if let Some(d) = crate::diagnostics::get() {
                        d.browser_prepaints
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    crate::table::fit_columns(&fit, f32::from(bounds.size.width).floor(), w, cx);
                })
                .key_context("MusicTracks")
                .flex_1()
                .min_h_0()
                .text_size(px(crate::theme::size::TEXT))
                .child(crate::table::browser_table(
                    &self.table,
                    row_height,
                    "Recordings",
                ))
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
    use super::{ADDRESS_MIN, BrowserState, PICKER_MIN, ToolbarLayout, ViewMode, px};
    use crate::browser::Location;
    use crate::browser::{Route, SavedVisit};
    use std::path::PathBuf;

    #[test]
    fn every_width_keeps_the_same_controls_and_narrowing_only_shrinks_them() {
        for full in [None, Some(px(130.))] {
            let mut previous: Option<ToolbarLayout> = None;
            for width in (100..=2400).rev() {
                let layout = ToolbarLayout::new(px(width as f32), full);
                // The same controls at every width: the picker never leaves.
                assert_eq!(layout.picker.is_some(), full.is_some(), "width {width}");
                if let Some(previous) = previous {
                    assert!(layout.address <= previous.address, "width {width}");
                    assert!(layout.picker <= previous.picker, "width {width}");
                }
                previous = Some(layout);
            }
        }
    }

    #[test]
    fn the_breadcrumb_keeps_the_current_location_before_the_picker_gives_way() {
        let full = Some(px(130.));
        let wide = ToolbarLayout::new(px(900.), full);
        let narrower = ToolbarLayout::new(px(800.), full);
        assert_eq!(wide.picker, full);
        assert_eq!(wide.address - narrower.address, px(100.));
        for width in 100..900 {
            let layout = ToolbarLayout::new(px(width as f32), full);
            if layout.picker < full {
                // The picker shrinks only once the breadcrumb is at its minimum.
                assert!(
                    layout.address >= px(ADDRESS_MIN) || layout.picker == Some(px(PICKER_MIN)),
                    "width {width}: {layout:?}"
                );
            }
        }
        let tight = ToolbarLayout::new(px(10.), full);
        assert_eq!(
            (tight.address, tight.picker),
            (px(0.), Some(px(PICKER_MIN)))
        );
    }

    #[test]
    fn a_saved_browser_keeps_its_view_and_a_new_one_starts_fresh() {
        let state = BrowserState {
            current: Some(SavedVisit::new(
                Route::new(Location::Playlist(3), None),
                ViewMode::Tree,
            )),
            folder: Some(PathBuf::from("C:\\Music")),
            prefs: Some(crate::collection::ViewPrefs {
                mode: ViewMode::Tree,
                ..Default::default()
            }),
            ..Default::default()
        };
        let json = serde_json::to_value(&state).unwrap();
        assert!(json.get("root").is_none());
        assert!(json.get("layouts").is_none());
        assert!(json.get("back").is_none() && json.get("forward").is_none());
        let back: BrowserState = serde_json::from_value(json).unwrap();
        assert_eq!(back.current.unwrap().route.location, Location::Playlist(3));
        assert_eq!(back.prefs.unwrap().mode, ViewMode::Tree);
        // A fresh instance has no saved view and takes the template's.
        let fresh: BrowserState =
            serde_json::from_value(serde_json::Value::Null).unwrap_or_default();
        assert!(fresh.current.is_none() && fresh.prefs.is_none());
    }

    #[test]
    fn a_folder_cd_page_and_both_navigation_stacks_round_trip() {
        let state = serde_json::json!({
            "current":{"location":{"Folder":"C:/Music"},"page":{"Disc":{"Cue":"C:/Music/Disc.cue"}}},
            "back":[{"location":"Liked","mode":"tracks","query":"mix"}],
            "forward":[{"location":{"Playlist":4},"mode":"tracks","query":"set"}]
        });
        let restored: BrowserState = serde_json::from_value(state).unwrap();
        assert_eq!(restored.back[0].mode, ViewMode::Tracks);
        assert_eq!(restored.forward[0].mode, ViewMode::Tracks);
        let saved = serde_json::to_value(restored).unwrap();
        assert_eq!(saved["back"][0]["mode"], "tracks");
        assert_eq!(saved["forward"][0]["mode"], "tracks");
        assert!(saved["back"][0].get("albums").is_none());
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
            "prefs":{"mode":"tracks","subfolders":[]}
        }));
        let browse = PanelRecord::with_state(serde_json::json!({
            "current":{"location":{"Folder":"C:/Collection"},"mode":"albums","subfolders":true},
            "forward":[{"location":{"Library":11},"mode":"albums"}],
            "recent":[{"location":{"Playlist":2},"query":"Morning"}],
            "prefs":{"mode":"albums","subfolders":[{"path":"C:/Collection","include":true}]}
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
        assert_ne!(pinned.prefs.unwrap().mode, browse.prefs.unwrap().mode);
    }
}

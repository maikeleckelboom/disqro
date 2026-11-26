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
    location::{LocationBar, TrailWidths},
    player::Player,
    table::{Layouts, queue_targets},
    theme::{materials, size},
};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, ElementExt, Icon, Selectable, Sizable,
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
    inline_sequence: bool,
    compact: bool,
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
    if compact {
        menu = compact_header_menu(b, browser, menu, cx);
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
    for kind in axis
        .filter(|_| !inline_sequence)
        .into_iter()
        .chain(extra_axis)
    {
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

fn compact_header_menu(
    browser: &Browser,
    entity: &Entity<Browser>,
    mut menu: PopupMenu,
    cx: &App,
) -> PopupMenu {
    let steps = [
        (
            "Back",
            browser.history_label(false, cx),
            Browser::back as fn(&mut Browser, &mut Window, &mut Context<Browser>),
        ),
        ("Forward", browser.history_label(true, cx), Browser::forward),
    ];
    let up = browser
        .up_in_toolbar()
        .then(|| ("Up", browser.up_label(cx), Browser::up as _));
    for (name, target, step) in steps.into_iter().chain(up) {
        let browser = entity.clone();
        let enabled = target.is_some();
        let label = target.map_or_else(|| name.to_owned(), |target| format!("{name} · {target}"));
        menu = menu.item(
            PopupMenuItem::new(label)
                .disabled(!enabled)
                .on_click(move |_, w, cx| browser.update(cx, |b, cx| step(b, w, cx))),
        );
    }
    let filter = entity.clone();
    menu = menu.separator().item(
        PopupMenuItem::new("Filter this view")
            .on_click(move |_, w, cx| filter.update(cx, |b, cx| b.set_filter_open(true, w, cx))),
    );
    if !browser.search.read(cx).value().is_empty() {
        let filter = entity.clone();
        menu = menu.item(
            PopupMenuItem::new("Clear filter")
                .on_click(move |_, w, cx| filter.update(cx, |b, cx| b.dismiss_filter(w, cx))),
        );
    }
    browser.header_views().menu(menu, entity).separator()
}

const HEADER_INSET: f32 = 8.;
const BAR_INSET: f32 = 4.;
const NAV_BUTTON: f32 = 26.;
const NAV_GAP: f32 = 1.;
const SEQUENCE_RAIL: f32 = 64.;
const VIEW_TAB: f32 = 36.;
const VIEW_TAB_HEIGHT: f32 = 29.;
const ICON_BUTTON: f32 = 28.;
const SCOPE_GAP: f32 = 3.;
const FILTER_COMPACT: f32 = 100.;
const FILTER_FULL: f32 = 175.;
const VIEW_CHOICES: [(ViewMode, &str, IconName); 3] = [
    (ViewMode::Tracks, "Tracks", IconName::ListMusic),
    (ViewMode::Albums, "Releases", IconName::Disc3),
    (ViewMode::Tree, "Tree", IconName::ListTree),
];
const VIEW_LABEL_SIZE: f32 = 12.;
const VIEW_LABEL_CHROME: f32 = 39.;
const PICKER_WIDTH: f32 = 46.;
const SCOPE_CHOICES: [(bool, &str, IconName); 2] = [
    (false, "This folder only", IconName::Folder),
    (true, "Include subfolders", IconName::Folders),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderDensity {
    Labels,
    Icons,
    Picker,
    Menu,
}

#[derive(Clone, Copy)]
struct BrowserViews {
    mode: Option<ViewMode>,
    subfolders: Option<bool>,
    /// The presentations the destination offers.
    choices: &'static [ViewMode],
}

impl BrowserViews {
    /// The view tabs the destination offers, in their usual order.
    fn offered(self) -> impl Iterator<Item = (usize, (ViewMode, &'static str, IconName))> {
        VIEW_CHOICES
            .into_iter()
            .enumerate()
            .filter(move |(_, (mode, _, _))| self.choices.contains(mode))
    }
}

#[derive(Clone, Copy)]
struct ControlWidths {
    labels: Pixels,
    icons: Pixels,
    picker: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HeaderFilter {
    Field(Pixels),
    Button,
    InMenu,
}

#[derive(Debug)]
struct HeaderLayout {
    density: HeaderDensity,
    address: Pixels,
    filter: HeaderFilter,
    controls: Pixels,
    sequence: bool,
}

impl ControlWidths {
    fn layout(
        self,
        width: Pixels,
        history: Pixels,
        trail: TrailWidths,
        editing: bool,
        has_sequence: bool,
    ) -> HeaderLayout {
        let row = (width - px(BAR_INSET + ICON_BUTTON)).max(px(0.));
        let available = row - history;
        let minimum_field = px(FILTER_COMPACT);
        let preferred_field = px(FILTER_FULL);
        let sequence_rail = px(SEQUENCE_RAIL);
        let sequence = has_sequence
            && available >= self.labels + trail.preferred + preferred_field + sequence_rail;
        let sequence_width = if sequence { sequence_rail } else { px(0.) };
        let picker_filter = if editing {
            minimum_field
        } else {
            px(ICON_BUTTON)
        };

        let (density, fixed, filter_width, address_limit) =
            if available >= self.labels + trail.preferred + minimum_field {
                let width = (available - sequence_width - self.labels - trail.preferred)
                    .min(preferred_field);
                let limit = if has_sequence && !sequence {
                    trail.preferred
                } else {
                    row
                };
                (HeaderDensity::Labels, self.labels, width, limit)
            } else if available >= self.icons + trail.minimum + minimum_field {
                (
                    HeaderDensity::Icons,
                    self.icons,
                    minimum_field,
                    trail.preferred,
                )
            } else if available >= self.picker + trail.minimum + picker_filter {
                (
                    HeaderDensity::Picker,
                    self.picker,
                    picker_filter,
                    trail.minimum,
                )
            } else {
                return HeaderLayout {
                    density: HeaderDensity::Menu,
                    address: row.min(trail.minimum),
                    filter: HeaderFilter::InMenu,
                    controls: px(0.),
                    sequence: false,
                };
            };
        let controls = fixed + filter_width;
        HeaderLayout {
            density,
            // Folding controls can release space all at once. Cap the trail
            // at the width it had at that boundary so hidden ancestors never
            // return during a continuous shrink.
            address: (available - sequence_width - controls).min(address_limit),
            filter: if density == HeaderDensity::Picker && !editing {
                HeaderFilter::Button
            } else {
                HeaderFilter::Field(filter_width)
            },
            controls,
            sequence,
        }
    }
}

impl BrowserViews {
    fn widths(self, labels: [Pixels; VIEW_CHOICES.len()]) -> ControlWidths {
        let labels: Pixels = self.offered().map(|(i, _)| labels[i]).sum();
        let overhead = px(2. * HEADER_INSET + size::TAB_DROP)
            + if self.subfolders.is_some() {
                px(ICON_BUTTON + SCOPE_GAP)
            } else {
                px(0.)
            }
            + if self.mode.is_some() {
                px(SCOPE_GAP)
            } else {
                px(0.)
            };
        let (text, icons, picker) = if self.mode.is_some() {
            (
                labels,
                px(self.offered().count() as f32 * VIEW_TAB),
                px(PICKER_WIDTH),
            )
        } else {
            (px(0.), px(0.), px(0.))
        };
        ControlWidths {
            labels: overhead + text,
            icons: overhead + icons,
            picker: overhead + picker,
        }
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

#[derive(Clone)]
struct ViewFocus {
    tracks: FocusHandle,
    albums: FocusHandle,
    tree: FocusHandle,
    folders: FocusHandle,
    filter: FocusHandle,
    back: FocusHandle,
    forward: FocusHandle,
    up: FocusHandle,
}

impl ViewFocus {
    fn choice(&self, mode: ViewMode) -> &FocusHandle {
        match mode {
            ViewMode::Tracks => &self.tracks,
            ViewMode::Tree => &self.tree,
            ViewMode::Albums => &self.albums,
        }
    }

    fn focused(&self, window: &Window) -> Option<&FocusHandle> {
        [
            &self.tracks,
            &self.albums,
            &self.tree,
            &self.folders,
            &self.filter,
            &self.back,
            &self.forward,
            &self.up,
        ]
        .into_iter()
        .find(|handle| handle.is_focused(window))
    }
}

impl Browser {
    /// The shared header for standalone and tabbed browsers. History and
    /// view controls fold into the panel menu as the dock narrows.
    fn bar(&self, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let browser = cx.entity();
        let focus = window
            .use_keyed_state(("view-focus", browser.entity_id()), cx, |_, cx| ViewFocus {
                tracks: cx.focus_handle(),
                albums: cx.focus_handle(),
                tree: cx.focus_handle(),
                folders: cx.focus_handle(),
                filter: cx.focus_handle(),
                back: cx.focus_handle(),
                forward: cx.focus_handle(),
                up: cx.focus_handle(),
            })
            .read(cx)
            .clone();
        let label_widths = VIEW_CHOICES.map(|(_, name, _)| {
            crate::theme::label_width(name.into(), px(VIEW_LABEL_SIZE), window)
                + px(VIEW_LABEL_CHROME)
        });
        let input = self.search.read(cx);
        let filter_focused = input.focus_handle(cx).is_focused(window);
        let filter_applied = !input.value().is_empty();
        let history = px(2. * NAV_BUTTON + NAV_GAP)
            + if self.up_in_toolbar() {
                px(NAV_BUTTON + NAV_GAP)
            } else {
                px(0.)
            };
        let sequence = self.sequence_kind();
        let layout = self.header_views().widths(label_widths).layout(
            width,
            history,
            TrailWidths::measure(self, window, cx),
            filter_focused || self.filter_open || filter_applied,
            sequence.is_some(),
        );
        let density = layout.density;
        let compact = density == HeaderDensity::Menu;
        let sequence = sequence.filter(|_| layout.sequence);
        let address = self.address.as_ref().and_then(WeakEntity::upgrade);
        if let Some(address) = &address {
            address.update(cx, |bar, cx| bar.set_width(layout.address, cx));
        }
        let edit_width = (width - px(BAR_INSET + ICON_BUTTON)).clamp(px(0.), px(FILTER_COMPACT));
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
            .when(!compact, |bar| bar.child(self.history(&focus, cx)))
            .child(
                h_flex()
                    .w(if compact && filter_focused {
                        edit_width
                    } else {
                        layout.address
                    })
                    .flex_shrink_0()
                    .h_full()
                    .min_w_0()
                    .items_center()
                    .map(|d| {
                        if compact && filter_focused {
                            d.child(self.filter_control(
                                HeaderFilter::Field(edit_width),
                                true,
                                &focus.filter,
                                window,
                                cx,
                            ))
                        } else {
                            d.children(address)
                        }
                    }),
            )
            .when_some(sequence, |bar, kind| {
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
            .when(!compact, |bar| {
                bar.child(
                    div()
                        .ml_auto()
                        .w(layout.controls)
                        .h_full()
                        .flex_shrink_0()
                        .child(self.header_controls(&layout, label_widths, &focus, window, cx)),
                )
            })
            .child(
                Button::new("browser-menu")
                    .when(compact, |button| button.ml_auto())
                    .ghost()
                    .xsmall()
                    .w(px(ICON_BUTTON))
                    .icon(IconName::Ellipsis)
                    .accessibility_label("Panel menu")
                    .tab_stop(true)
                    .when_some(
                        compact.then(|| focus.focused(window)).flatten(),
                        |button, handle| button.track_focus(handle).tab_stop(true),
                    )
                    .when(compact && filter_applied, |button| {
                        button.text_color(m.accent)
                    })
                    .tooltip(if compact && filter_applied {
                        "Panel menu · Filter active"
                    } else {
                        "Panel menu"
                    })
                    .dropdown_menu(move |menu, w, cx| {
                        let menu =
                            browser_menu(&browser, &player, menu, sequence.is_some(), compact, cx);
                        let pane = browser.read(cx).pane.as_ref().and_then(WeakEntity::upgrade);
                        match (pane, cx.try_global::<super::PaneMenus>().cloned()) {
                            (Some(pane), Some(menus)) => (menus.0)(&pane, menu.separator(), w, cx),
                            _ => menu,
                        }
                    }),
            )
            .into_any_element()
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
                .xsmall()
                .w(px(NAV_BUTTON))
                .h(px(NAV_BUTTON))
                .icon(Icon::new(icon).size(px(15.)).text_color(m.text_muted))
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
            .gap(px(NAV_GAP))
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
            .when(self.up_in_toolbar(), |bar| {
                bar.child(
                    button("up", IconName::ArrowUp, "Up", "Alt+Up", self.up_label(cx))
                        .track_focus(&focus.up)
                        .on_click(cx.listener(|b, _, w, cx| b.up(w, cx))),
                )
            })
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

    fn header_controls(
        &self,
        layout: &HeaderLayout,
        label_widths: [Pixels; VIEW_CHOICES.len()],
        focus: &ViewFocus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let browser = cx.entity();
        let views = self.header_views();
        let density = layout.density;
        let m = *materials(cx);
        let mode = views.mode.map(|mode| {
            if density == HeaderDensity::Picker {
                let handle = VIEW_CHOICES
                    .iter()
                    .map(|(choice, _, _)| focus.choice(*choice))
                    .find(|handle| handle.is_focused(window))
                    .unwrap_or_else(|| focus.choice(mode));
                view_menu(
                    &browser,
                    BrowserViews {
                        mode: Some(mode),
                        subfolders: None,
                        choices: views.choices,
                    },
                    handle,
                    m,
                )
            } else {
                view_tabs(&browser, views, mode, focus, density, label_widths, m)
            }
        });
        let scope = views.subfolders.map(|include| {
            view_menu(
                &browser,
                BrowserViews {
                    mode: None,
                    subfolders: Some(include),
                    choices: &[],
                },
                &focus.folders,
                m,
            )
        });
        h_flex()
            .id("browser-controls")
            .w_full()
            .h_full()
            .px(px(HEADER_INSET))
            .gap(px(SCOPE_GAP))
            .items_center()
            .justify_end()
            .child(self.filter_control(
                layout.filter,
                density == HeaderDensity::Picker,
                &focus.filter,
                window,
                cx,
            ))
            .children(scope)
            .children(mode)
            .into_any_element()
    }

    /// The filter button, or the field once it is open or holds a query.
    fn filter_control(
        &self,
        filter: HeaderFilter,
        tight: bool,
        button_focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *materials(cx);
        let input = self.search.clone();
        let empty = input.read(cx).value().is_empty();
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let HeaderFilter::Field(width) = filter else {
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
                .track_focus(button_focus)
                .tab_stop(true)
                .text_color(m.text_muted)
                .accessibility_label("Filter this view")
                .tooltip("Filter this view · Ctrl+F")
                .on_click(cx.listener(|b, _, w, cx| b.set_filter_open(true, w, cx)))
                .into_any_element();
        };
        // Preserve the keyboard target if its icon becomes an inline field.
        if button_focus.is_focused(window) && !focused {
            let browser = cx.entity();
            cx.defer_in(window, move |_, window, cx| {
                browser.update(cx, |b, cx| b.set_filter_open(true, window, cx));
            });
        }
        let scope = self.tree_filter_scope(cx);
        h_flex()
            .id("browser-filter")
            .when_some(scope, |d, scope| {
                d.tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(scope).build(w, cx)
                })
            })
            .flex_shrink(1.)
            .min_w_0()
            .w(width)
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
                        .aria_label(scope.unwrap_or("Filter this view")),
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
    views: BrowserViews,
    mode: ViewMode,
    focus: &ViewFocus,
    density: HeaderDensity,
    label_widths: [Pixels; VIEW_CHOICES.len()],
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
        .children(
            views
                .offered()
                .map(|(i, choice)| (choice, label_widths[i]))
                .map(|((value, name, icon), label_width)| {
                    let browser = browser.clone();
                    let selected = mode == value;
                    let labeled = density == HeaderDensity::Labels;
                    Button::new(name)
                        .ghost()
                        .xsmall()
                        .w(if labeled { label_width } else { px(VIEW_TAB) })
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
                        .when(selected, |b| b.bg(m.canvas))
                        .child(Icon::new(icon).size(px(15.)).text_color(if selected {
                            m.text
                        } else {
                            m.text_faint
                        }))
                        .when(labeled, |b| b.text_size(px(VIEW_LABEL_SIZE)).child(name))
                        .accessibility_label(name)
                        .tooltip(name)
                        .track_focus(focus.choice(value))
                        .tab_stop(true)
                        .on_click(move |_, w, cx| browser.update(cx, |b, cx| b.view(value, w, cx)))
                }),
        )
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
    let (id, icon, description, anchor, width) = if let Some(mode) = views.mode {
        let (_, name, icon) = VIEW_CHOICES
            .into_iter()
            .find(|(choice, _, _)| *choice == mode)
            .unwrap_or(VIEW_CHOICES[0]);
        (
            "browser-view-menu",
            icon,
            format!("Choose view · {name}"),
            Anchor::TopLeft,
            PICKER_WIDTH,
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
        .dropdown_caret(views.mode.is_some())
        .icon(Icon::new(icon).text_color(m.text_muted))
        .track_focus(focus)
        .tab_stop(true)
        .accessibility_label(description.clone())
        .tooltip(description)
        .dropdown_menu_with_anchor(anchor, move |menu, _, _| views.menu(menu, &browser))
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
                .text_size(px(13.))
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
    use super::{
        BrowserState, BrowserViews, HeaderDensity, HeaderFilter, TrailWidths, ViewMode, px,
    };
    use crate::browser::Location;
    use crate::browser::{Route, SavedVisit};
    use std::path::PathBuf;

    #[test]
    fn favorites_shrinks_the_filter_before_folding_the_location() {
        let views = BrowserViews {
            mode: None,
            subfolders: None,
            choices: &[],
        };
        let layout = |width| {
            views.widths([px(0.); 3]).layout(
                px(width),
                px(53.),
                TrailWidths {
                    preferred: px(180.),
                    minimum: px(115.),
                },
                false,
                false,
            )
        };
        let wide = layout(490.);
        let middle = layout(430.);
        let narrow = layout(415.);
        assert!(wide.address >= middle.address);
        assert_eq!(middle.address, narrow.address);
        assert_eq!(narrow.address, px(180.));
        let [
            HeaderFilter::Field(wide),
            HeaderFilter::Field(middle),
            HeaderFilter::Field(narrow),
        ] = [wide.filter, middle.filter, narrow.filter]
        else {
            panic!("filter should stay inline");
        };
        assert!(wide > middle && middle > narrow);
    }

    #[test]
    fn folding_any_toolbar_control_never_gives_the_breadcrumb_more_room() {
        for mode in [None, Some(ViewMode::Albums)] {
            for subfolders in [None, Some(false)] {
                for editing in [false, true] {
                    for sequence in [false, true] {
                        let views = BrowserViews {
                            mode,
                            subfolders,
                            choices: &[ViewMode::Tracks, ViewMode::Albums, ViewMode::Tree],
                        };
                        let mut previous = px(2000.);
                        let mut previous_filter = px(2000.);
                        for width in (120..=1000).rev() {
                            let layout = views.widths([px(235. / 3.); 3]).layout(
                                px(width as f32),
                                px(53.),
                                TrailWidths {
                                    preferred: px(184.),
                                    minimum: px(115.),
                                },
                                editing,
                                sequence,
                            );
                            assert!(layout.address <= previous, "width {width}, {layout:?}");
                            let filter = match layout.filter {
                                HeaderFilter::Field(width) => width,
                                HeaderFilter::Button => px(super::ICON_BUTTON),
                                HeaderFilter::InMenu => px(0.),
                            };
                            assert!(filter <= previous_filter, "width {width}, {layout:?}");
                            previous_filter = filter;
                            assert!(layout.address >= px(0.));
                            if editing {
                                assert_ne!(layout.filter, HeaderFilter::Button);
                            }
                            if layout.density == HeaderDensity::Menu {
                                assert!(!layout.sequence);
                            }
                            previous = layout.address;
                        }
                    }
                }
            }
        }
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

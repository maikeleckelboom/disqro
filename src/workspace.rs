//! The window owns the dock, the panel picker and the persistent player.
//! The shell addresses the browser most recently activated in the dock.
//!
//! Every view in the dock is a [`Pane`]: an instance of a registered
//! [`PanelType`](crate::panels::PanelType). The workspace creates, places,
//! closes, reopens, saves and restores instances without knowing their types,
//! and coordinates the one thing that spans them, the active Music Browser.
//! The player is not a dock panel: it is part of the window, so no
//! arrangement of panels can close it or interrupt playback.
mod recovery;
mod search;
mod sidebar;
mod skin;

use crate::{
    audio::Command,
    browser::{Browser, Reveal, music_home},
    catalog::{Catalog, CatalogEvent, Targets},
    panels::{
        self, Destination, Home, InstanceId, Instances, Pane, PaneEvent, PanelRecord, PanelType,
        Router, RouterEvent, Services, Template,
        browser::{BrowserPane, BrowserState},
        locations::{Locations, LocationsChanged},
    },
    player::{Player, PlayerEvent},
    profile::{self, ClosedPanel, Profile, SavedQueue, WindowPrefs, WorkspaceState},
    store::Scope,
    system::{System, SystemEvent},
    table::Layouts,
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        button::*,
        dock::*,
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
        *,
    },
    prelude::*,
    *,
};
use skin::{Skin, SkinState, pane_of};
use std::{collections::HashMap, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

gpui_kit::actions!(
    disqro,
    [
        OpenFolder,
        FindTrack,
        SearchLibrary,
        ToggleLeftSidebar,
        ToggleRightSidebar,
        PlayPause,
        ToggleWaveform,
        PlaySelected,
        OpenDirectory,
        GoBack,
        GoForward,
        GoUp,
        FocusLocation,
        Refresh,
        NextTrack,
        PreviousTrack,
        ShowQueue,
        HideToTray,
        Quit,
        ToggleLike,
        ActivateTrackLike,
        RateOne,
        RateTwo,
        RateThree,
        RateFour,
        RateFive,
        ClearRating,
        ExtendUp,
        ExtendDown,
        MovePlaylistUp,
        MovePlaylistDown,
        RemoveFromPlaylist,
        NewPlaylist,
        AddMusicFolder,
        NewBrowserTab,
        CloseTab
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-w", ToggleWaveform, Some("Disqro && !Input")),
        KeyBinding::new("ctrl-o", OpenFolder, Some("Disqro")),
        KeyBinding::new("ctrl-alt-left", ToggleLeftSidebar, Some("Disqro")),
        KeyBinding::new("ctrl-alt-right", ToggleRightSidebar, Some("Disqro")),
        KeyBinding::new("ctrl-k", SearchLibrary, Some("Disqro")),
        KeyBinding::new("ctrl-f", FindTrack, Some("Disqro")),
        KeyBinding::new("ctrl-l", FocusLocation, Some("Disqro")),
        KeyBinding::new("alt-d", FocusLocation, Some("Disqro")),
        KeyBinding::new("f4", FocusLocation, Some("Disqro")),
        KeyBinding::new("ctrl-t", NewBrowserTab, Some("Disqro")),
        KeyBinding::new("ctrl-w", CloseTab, Some("Disqro")),
        KeyBinding::new(
            "space",
            PlayPause,
            Some("Disqro && !Input && !LibrarySection"),
        ),
        KeyBinding::new("enter", PlaySelected, Some("DataTable")),
        KeyBinding::new("enter", ActivateTrackLike, Some("TrackLike")),
        KeyBinding::new("space", ActivateTrackLike, Some("TrackLike")),
        KeyBinding::new("enter", OpenDirectory, Some("Tree")),
        KeyBinding::new("alt-left", GoBack, Some("Disqro")),
        KeyBinding::new("backspace", GoBack, Some("Disqro && !Input")),
        KeyBinding::new("alt-right", GoForward, Some("Disqro")),
        KeyBinding::new("alt-up", GoUp, Some("Disqro && !Queue")),
        KeyBinding::new("f5", Refresh, Some("Disqro")),
        KeyBinding::new("ctrl-right", NextTrack, Some("Disqro && !Input")),
        KeyBinding::new("ctrl-left", PreviousTrack, Some("Disqro && !Input")),
        KeyBinding::new("ctrl-q", Quit, Some("Disqro")),
        KeyBinding::new("alt-l", ToggleLike, Some("Disqro && !Input")),
        KeyBinding::new("alt-1", RateOne, Some("Disqro && !Input")),
        KeyBinding::new("alt-2", RateTwo, Some("Disqro && !Input")),
        KeyBinding::new("alt-3", RateThree, Some("Disqro && !Input")),
        KeyBinding::new("alt-4", RateFour, Some("Disqro && !Input")),
        KeyBinding::new("alt-5", RateFive, Some("Disqro && !Input")),
        KeyBinding::new("alt-0", ClearRating, Some("Disqro && !Input")),
        KeyBinding::new(
            "delete",
            RemoveFromPlaylist,
            Some("MusicTracks > DataTable"),
        ),
        KeyBinding::new("shift-up", ExtendUp, Some("MusicTracks > DataTable")),
        KeyBinding::new("shift-down", ExtendDown, Some("MusicTracks > DataTable")),
        KeyBinding::new(
            "ctrl-shift-up",
            MovePlaylistUp,
            Some("MusicTracks > DataTable"),
        ),
        KeyBinding::new(
            "ctrl-shift-down",
            MovePlaylistDown,
            Some("MusicTracks > DataTable"),
        ),
        KeyBinding::new("ctrl-n", NewPlaylist, Some("Disqro")),
    ]);
}

/// Layouts written with another version are replaced by the default layout;
/// the rest of the profile is kept.
const LAYOUT_VERSION: usize = 3;
const PLAYER_HEIGHT: f32 = 104.;
const PLAYER_MIN: f32 = 84.;
const PLAYER_MAX: f32 = 200.;
const LEADING_WIDTH: f32 = 232.;
const TRAILING_WIDTH: f32 = 300.;

/// A live pane and the subscription that carries its requests here.
struct Tracked {
    pane: Entity<Pane>,
    _events: Subscription,
}

/// Where a new pane goes.
enum Place {
    Group(NodeId),
    Beside(NodeId, gpui_kit::base::Placement, Option<Pixels>),
}

/// A saved layout this build lays out, or `None` for one written with another
/// version, which the default layout replaces.
fn current_layout(layout: Option<DockAreaState>) -> Option<DockAreaState> {
    layout.filter(|layout| layout.version == Some(LAYOUT_VERSION))
}

/// The player's top edge being dragged.
#[derive(Clone)]
struct PlayerResize;
impl Render for PlayerResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct Workspace {
    services: Services,
    dock: Entity<DockArea>,
    skin: Rc<SkinState>,
    panes: HashMap<PanelId, Tracked>,
    undo: Option<recovery::Checkpoint>,
    drag_checkpoint: Option<(PanelId, NodeId, usize, recovery::Checkpoint)>,
    /// Recent closed instances, with their former group companions.
    closed: Vec<ClosedPanel>,
    address: Option<Entity<crate::location::LocationBar>>,
    search: Entity<search::LibrarySearch>,
    search_visible: bool,
    left_hidden: bool,
    right_hidden: bool,
    player_height: f32,
    profile: Option<Arc<profile::ProfileFile>>,
    save_revision: u64,
    quitting: bool,
    save_dirty: bool,
    window: AnyWindowHandle,
    prefs: WindowPrefs,
    system: Option<System>,
    hwnd: isize,
    window_visible: bool,
    hidden_to_tray: bool,
    save_pending: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

fn native_handle(window: &Window) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

/// The Music Browser a pane shows, if it is one.
fn browser_of(pane: &Entity<Pane>, cx: &App) -> Option<Entity<Browser>> {
    pane.read(cx)
        .content
        .as_any()
        .downcast_ref::<BrowserPane>()
        .map(|p| p.browser.clone())
}

impl Workspace {
    pub fn new(arg: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (file, mut saved, profile_notice) = match profile::location() {
            Some(path) => {
                let (file, saved, notice) = profile::ProfileFile::open(path);
                (Some(Arc::new(file)), saved, notice)
            }
            None => (
                None,
                Profile::default(),
                Some("Workspace storage is unavailable.".into()),
            ),
        };
        let volume = saved.volume.clamp(0., 1.);
        let player = cx.new(|cx| Player::new(volume, saved.full_cd, window, cx));
        let catalog = cx.new(Catalog::new);
        let layouts = saved
            .table_layouts
            .take()
            .map(Layouts::validate)
            .unwrap_or_else(|| Layouts::migrate(&saved.columns, saved.columns_version));
        let router = cx.new(|_| {
            Router::new(Template {
                layouts,
                prefs: saved.collection.clone(),
            })
        });
        if saved.expanded.is_empty() {
            saved.expanded = std::mem::take(&mut saved.legacy_navigation.sidebar.expanded);
        }
        let locations = cx
            .new(|cx| Locations::new(saved.places.take(), std::mem::take(&mut saved.expanded), cx));
        let services = Services {
            player: player.clone(),
            catalog: catalog.clone(),
            locations: locations.clone(),
            router: router.clone(),
        };
        // Every type can be rebuilt from a saved layout by its key.
        for ty in panels::TYPES.iter().copied() {
            let services = services.clone();
            register_panel(cx, ty.key, move |context, window, cx| {
                let record = PanelRecord::read(context.info());
                let id = record.id.unwrap_or_else(|| InstanceId::fresh(|_| false));
                let pane = cx.new(|cx| Pane::new(ty, id, &record, &services, window, cx));
                Arc::new(pane)
            });
        }
        let skin = SkinState::new(cx.weak_entity());
        let dock = {
            let skin = skin.clone();
            cx.new(|cx| {
                let inner = DockSkin::new(cx);
                DockArea::new("listening", Some(LAYOUT_VERSION), window, cx)
                    .with_renderer(Rc::new(Skin::new(inner, skin)))
            })
        };
        let (events_tx, events_rx) = async_channel::unbounded();
        let system = System::new(events_tx, player.read(cx).sender());
        let subscriptions = vec![
            cx.subscribe_in(&dock, window, |s: &mut Self, _, e, w, cx| {
                if let DockEvent::LayoutChanged = e {
                    s.layout_changed(w, cx)
                }
            }),
            cx.subscribe(&locations, |s: &mut Self, _, _: &LocationsChanged, cx| {
                s.schedule_save(cx)
            }),
            cx.observe_in(&router, window, |s: &mut Self, _, w, cx| {
                s.bind_address(w, cx);
                s.schedule_save(cx);
                cx.notify();
            }),
            cx.subscribe_in(&router, window, |s: &mut Self, _, e, w, cx| match e {
                RouterEvent::Open(destination) => {
                    s.open_browser(Some(destination.clone()), None, None, w, cx);
                }
                RouterEvent::Beside(destination) => {
                    let place = s
                        .active_browser_pane(cx)
                        .and_then(|p| s.node_of(&p, cx))
                        .map(|node| {
                            if s.skin.can_open_beside(node) {
                                Place::Beside(node, gpui_kit::base::Placement::Right, None)
                            } else {
                                Place::Group(node)
                            }
                        });
                    s.open_browser(Some(destination.clone()), None, place, w, cx);
                }
                RouterEvent::Reveal => s.reveal_active_browser(w, cx),
            }),
            cx.subscribe_in(&catalog, window, |_, _, e, window, cx| {
                if let CatalogEvent::Notice(text) = e {
                    window.push_notification(text.clone(), cx);
                }
            }),
            cx.subscribe_in(&player, window, |s: &mut Self, _, e, w, cx| match e {
                PlayerEvent::Locate(track, dest) => {
                    let browser = s.ensure_browser(w, cx);
                    s.reveal_active_browser(w, cx);
                    browser.update(cx, |b, cx| b.locate(track.clone(), *dest, w, cx));
                }
                PlayerEvent::ContinueCd(entry) => match s.browser(cx) {
                    Some(b) => b.update(cx, |b, cx| b.continue_cd(entry.clone(), cx)),
                    None => crate::browser::continue_cd(
                        &s.services.catalog,
                        &s.services.player,
                        entry.clone(),
                        cx,
                    ),
                },
                PlayerEvent::QueueChanged
                | PlayerEvent::TrackChanged
                | PlayerEvent::StateChanged => {
                    s.update_system(cx);
                    s.schedule_save(cx);
                }
                PlayerEvent::PlaySelection => {}
            }),
            cx.observe_window_visibility(window, |s: &mut Self, visibility, _, cx| {
                let visible = visibility == WindowVisibility::Visible;
                s.window_visible = visible;
                s.sync_panel_display(cx);
                s.services
                    .player
                    .update(cx, |p, cx| p.set_visible(visible, cx));
                if visible {
                    s.hidden_to_tray = false;
                }
            }),
        ];
        let workspace = std::mem::take(&mut saved.workspace);
        let owner = cx.weak_entity();
        let search = cx.new(|cx| search::LibrarySearch::new(owner, catalog.clone(), window, cx));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let mut app = Self {
            services,
            dock,
            skin,
            panes: HashMap::new(),
            undo: None,
            drag_checkpoint: None,
            address: None,
            search,
            search_visible: false,
            left_hidden: workspace.left_hidden,
            right_hidden: workspace.right_hidden,
            closed: workspace.closed,
            player_height: workspace
                .player_height
                .unwrap_or(PLAYER_HEIGHT)
                .clamp(PLAYER_MIN, PLAYER_MAX),
            profile: file,
            save_revision: 0,
            quitting: false,
            save_dirty: false,
            window: window.window_handle(),
            prefs: saved.window,
            system,
            hwnd: native_handle(window),
            window_visible: window.visibility() == WindowVisibility::Visible,
            hidden_to_tray: false,
            save_pending: false,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        let loaded = current_layout(saved.layout.take()).is_some_and(|layout| {
            app.dock
                .update(cx, |d, cx| d.load(layout, window, cx))
                .is_ok()
        });
        if loaded {
            app.sync_panes(window, cx);
            let active = workspace
                .active
                .and_then(|id| app.pane_with(id, cx))
                .and_then(|p| browser_of(&p, cx));
            if let Some(browser) = active {
                app.services
                    .router
                    .update(cx, |r, cx| r.set_active(Some(&browser), cx));
            }
        } else {
            let legacy = saved.legacy_navigation;
            app.reset(
                Some(BrowserState {
                    folder: legacy.folder,
                    location: legacy.location,
                    ..Default::default()
                }),
                window,
                cx,
            );
        }
        app.set_customizing(false, window, cx);
        app.sync_active_browser(cx);
        app.bind_address(window, cx);
        if let Some(saved) = saved.queue.take()
            && !saved.queue.entries.is_empty()
        {
            app.services.player.update(cx, |p, cx| {
                p.command(Command::Restore(Box::new(saved.queue), saved.position), cx)
            });
        }
        if let Some(target) = arg {
            app.open_argument(target, window, cx);
        }
        if let Some(browser) = app.browser(cx) {
            browser.read(cx).list_focus(cx).focus(window, cx);
        }
        if let Some(notice) = profile_notice {
            // The component notification layer is installed after the workspace constructor.
            window.on_next_frame(move |window, cx| window.push_notification(notice, cx));
        }
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                if this
                    .update_in(cx, |s, w, cx| s.system_event(event, w, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |s, cx| {
                if s.prefs.close_to_tray && s.tray_available() {
                    s.hide_to_tray(window, cx);
                    false
                } else {
                    s.quit(window, cx);
                    false
                }
            })
            .unwrap_or(true)
        });
        app
    }

    /// A file or folder passed on the command line opens in the active
    /// browser, playing the file.
    fn open_argument(&mut self, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let is_file = crate::library::is_audio(&target)
            || target
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cue"));
        let folder = if is_file {
            target
                .parent()
                .map(|p| p.to_owned())
                .unwrap_or_else(music_home)
        } else {
            target.clone()
        };
        let browser = self.ensure_browser(window, cx);
        browser.update(cx, |b, cx| {
            b.open_revealing(folder, is_file.then_some(Reveal::Play(target)), window, cx)
        });
    }

    // ----- Panes ---------------------------------------------------------

    fn center(&self, cx: &App) -> PaneTree {
        self.dock
            .read(cx)
            .layout(DockPlacement::Center)
            .cloned()
            .unwrap_or_else(|| PaneTree::new(RootKind::Split))
    }
    /// The panes in the dock, in layout order.
    fn ordered(&self, cx: &App) -> Vec<Entity<Pane>> {
        self.center(cx)
            .panels()
            .filter_map(|id| self.panes.get(&id).map(|t| t.pane.clone()))
            .collect()
    }
    fn pane_with(&self, id: InstanceId, cx: &App) -> Option<Entity<Pane>> {
        self.panes
            .values()
            .find(|t| t.pane.read(cx).id == id)
            .map(|t| t.pane.clone())
    }
    fn pane_of_type(&self, ty: &PanelType, cx: &App) -> Option<Entity<Pane>> {
        self.ordered(cx).into_iter().find(|p| p.read(cx).ty == ty)
    }
    fn is_open(&self, ty: &PanelType, cx: &App) -> bool {
        self.panes.values().any(|t| t.pane.read(cx).ty == ty)
    }
    fn panel_id(pane: &Entity<Pane>) -> PanelId {
        PanelId::from(pane.entity_id())
    }
    /// The tab group holding a pane.
    fn node_of(&self, pane: &Entity<Pane>, cx: &App) -> Option<NodeId> {
        self.center(cx).find_panel_node(Self::panel_id(pane))
    }
    /// The panes sharing a group, in tab order.
    fn group_members(&self, node: NodeId, cx: &App) -> Vec<Entity<Pane>> {
        match self.center(cx).find_node(node).map(|n| n.kind()) {
            Some(PaneRef::Tabs { panels, .. }) => panels
                .iter()
                .filter_map(|id| self.panes.get(id).map(|t| t.pane.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }
    /// Whether a pane is the tab its group shows.
    fn is_displayed(&self, pane: &Entity<Pane>, cx: &App) -> bool {
        let id = Self::panel_id(pane);
        let tree = self.center(cx);
        tree.find_panel_node(id)
            .and_then(|node| tree.find_node(node))
            .is_some_and(|node| match node.kind() {
                PaneRef::Tabs { panels, active_ix } => panels.get(active_ix) == Some(&id),
                PaneRef::Split { .. } => false,
            })
    }

    /// Follows the dock after any edit: tracks new panes, forgets closed
    /// ones, and keeps navigation directed at an open browser.
    fn layout_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_panel_drag(cx);
        self.skin.retain_groups(&self.center(cx));
        self.sync_panes(window, cx);
        self.sync_active_browser(cx);
        self.schedule_save(cx);
        cx.notify();
    }
    fn sync_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<PanelId> = self.center(cx).panels().collect();
        for id in &ids {
            if self.panes.contains_key(id) {
                continue;
            }
            let Some(pane) = self.dock.read(cx).panel(*id).and_then(pane_of) else {
                continue;
            };
            let events = cx.subscribe_in(&pane, window, |s: &mut Self, pane, e, w, cx| {
                s.pane_event(pane.clone(), e, w, cx)
            });
            self.panes.insert(
                *id,
                Tracked {
                    pane,
                    _events: events,
                },
            );
        }
        self.panes.retain(|id, _| ids.contains(id));
        self.sync_panel_display(cx);
        // An identity appears once, even in a layout edited by hand.
        let mut seen = Vec::new();
        for pane in self.ordered(cx) {
            let id = pane.read(cx).id;
            if seen.contains(&id) {
                let fresh = InstanceId::fresh(|id| seen.contains(&id));
                pane.update(cx, |p, _| p.id = fresh);
                seen.push(fresh);
            } else {
                seen.push(id);
            }
        }
    }
    fn pane_event(
        &mut self,
        pane: Entity<Pane>,
        e: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            PaneEvent::Changed => {
                if let Some(browser) = browser_of(&pane, cx) {
                    self.remember_browser(&browser, cx);
                }
                self.schedule_save(cx);
                cx.notify();
            }
            PaneEvent::Close => {
                self.close_pane(&pane, window, cx);
            }
            PaneEvent::Duplicate => self.duplicate(&pane, window, cx),
        }
    }
    /// New browsers start like the one last changed; the folders shown with
    /// their subfolders are the same in every browser.
    fn remember_browser(&mut self, browser: &Entity<Browser>, cx: &mut Context<Self>) {
        let b = browser.read(cx);
        let layouts = b.table.read(cx).delegate().layouts.clone();
        let prefs = b.prefs.clone();
        self.services.router.update(cx, |r, _| {
            r.template.layouts = layouts;
            r.template.prefs = prefs.clone();
        });
        for pane in self.ordered(cx) {
            if let Some(other) = browser_of(&pane, cx)
                && other != *browser
                && other.read(cx).prefs.subfolders != prefs.subfolders
            {
                let rules = prefs.subfolders.clone();
                other.update(cx, |b, _| b.prefs.subfolders = rules);
            }
        }
    }

    fn new_pane(
        &mut self,
        ty: &'static PanelType,
        mut record: PanelRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let taken: Vec<InstanceId> = self.panes.values().map(|t| t.pane.read(cx).id).collect();
        let id = record
            .id
            .filter(|id| !taken.contains(id))
            .unwrap_or_else(|| InstanceId::fresh(|id| taken.contains(&id)));
        record.id = Some(id);
        let services = self.services.clone();
        cx.new(|cx| Pane::new(ty, id, &record, &services, window, cx))
    }
    /// Where a type goes when nothing else decides: with the main content,
    /// or beside the panels of its side, or at that edge of the workspace.
    fn home_of(&self, ty: &PanelType, cx: &App) -> Option<Place> {
        let tree = self.center(cx);
        let root = tree.root().id();
        let group_of = |home: Home| {
            self.ordered(cx)
                .into_iter()
                .filter(|p| p.read(cx).ty.home == home)
                .find_map(|p| self.node_of(&p, cx))
        };
        use gpui_kit::base::Placement::{Left, Right};
        Some(match ty.home {
            Home::Main => match self
                .active_browser_pane(cx)
                .and_then(|p| self.node_of(&p, cx))
                .or_else(|| group_of(Home::Main))
            {
                Some(node) => Place::Group(node),
                None => match (group_of(Home::Trailing), group_of(Home::Leading)) {
                    (Some(trailing), _) => Place::Beside(trailing, Left, None),
                    (None, Some(leading)) => Place::Beside(leading, Right, None),
                    (None, None) => Place::Beside(root, Right, None),
                },
            },
            Home::Leading => match group_of(Home::Leading) {
                Some(node) => Place::Group(node),
                None => Place::Beside(root, Left, Some(px(LEADING_WIDTH))),
            },
            Home::Trailing => match group_of(Home::Trailing) {
                Some(node) => Place::Group(node),
                None => Place::Beside(root, Right, Some(px(TRAILING_WIDTH))),
            },
        })
    }
    /// Puts a new pane in the dock and shows it.
    fn place(
        &mut self,
        pane: &Entity<Pane>,
        place: Option<Place>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = Self::panel_id(pane);
        // GPUI registers new panels in the first tab group. When the new
        // panel moves elsewhere, restore the tab that group originally showed.
        let tree = self.center(cx);
        let first_group = tree.panels().next().and_then(|p| tree.find_panel_node(p));
        let previous = first_group.and_then(|node| match tree.find_node(node)?.kind() {
            PaneRef::Tabs { panels, active_ix } => panels.get(active_ix).copied(),
            PaneRef::Split { .. } => None,
        });
        self.dock.update(cx, |d, cx| {
            d.add_panel(pane.clone(), DockPlacement::Center, None, window, cx);
            if first_group.is_some() {
                match place {
                    Some(Place::Group(node)) => d.move_panel(
                        id,
                        InsertTarget::Tabs {
                            node,
                            ix: None,
                            activate: true,
                        },
                        window,
                        cx,
                    ),
                    Some(Place::Beside(node, placement, size)) => d.move_panel(
                        id,
                        InsertTarget::Split {
                            node,
                            placement,
                            size,
                        },
                        window,
                        cx,
                    ),
                    None => {}
                }
            }
            if let Some(previous) = previous
                && d.layout(DockPlacement::Center)
                    .and_then(|layout| layout.find_panel_node(id))
                    != first_group
            {
                d.select_panel(previous, window, cx);
            }
            d.select_panel(id, window, cx);
        });
        self.layout_changed(window, cx);
        pane.read(cx).focus_handle(cx).focus(window, cx);
    }
    /// Opens a panel of `ty`: shows the open one of a single-instance type,
    /// otherwise adds one to `node`, to where it was closed, or to its home.
    pub fn open_panel(
        &mut self,
        ty: &'static PanelType,
        node: Option<NodeId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        if self.sidebar_hidden(ty.home) {
            self.toggle_sidebar(ty.home, window, cx);
        }
        if ty.instances == Instances::Single
            && let Some(pane) = self.pane_of_type(ty, cx)
        {
            self.reveal(&pane, window, cx);
            return pane;
        }
        self.remember_layout(None, cx);
        let closed = (ty.instances == Instances::Single)
            .then(|| {
                let ix = self.closed.iter().position(|c| c.key == ty.key)?;
                Some(self.closed.remove(ix))
            })
            .flatten();
        self.open_record(ty, node, closed, window, cx)
    }
    fn open_record(
        &mut self,
        ty: &'static PanelType,
        node: Option<NodeId>,
        closed: Option<ClosedPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let record = closed
            .as_ref()
            .map(|c| c.record.clone())
            .unwrap_or_default();
        let pane = self.new_pane(ty, record, window, cx);
        let beside = closed
            .into_iter()
            .flat_map(|c| c.beside)
            .find_map(|id| self.pane_with(id, cx).and_then(|p| self.node_of(&p, cx)));
        let place = node
            .map(Place::Group)
            .or(beside.map(Place::Group))
            .or_else(|| self.home_of(ty, cx));
        self.place(&pane, place, window, cx);
        if let Some(browser) = browser_of(&pane, cx) {
            self.services
                .router
                .update(cx, |r, cx| r.set_active(Some(&browser), cx));
        }
        pane
    }
    /// Opens a Music Browser, in `node` or the active content group, showing
    /// `destination` or the view `state` describes.
    fn open_browser(
        &mut self,
        destination: Option<Destination>,
        state: Option<BrowserState>,
        place: Option<Place>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Browser> {
        self.remember_layout(None, cx);
        let record = state.map(PanelRecord::with_state).unwrap_or_default();
        let pane = self.new_pane(&panels::browser::TYPE, record, window, cx);
        let place = place.or_else(|| self.home_of(&panels::browser::TYPE, cx));
        self.place(&pane, place, window, cx);
        let browser = browser_of(&pane, cx).expect("a Music Browser pane shows a browser");
        self.services
            .router
            .update(cx, |r, cx| r.set_active(Some(&browser), cx));
        if let Some(destination) = destination {
            browser.update(cx, |b, cx| destination.apply(b, window, cx));
        }
        browser
    }
    /// The active browser, opening one when none is open.
    fn ensure_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Browser> {
        match self.browser(cx) {
            Some(browser) => browser,
            None => self.open_browser(None, None, None, window, cx),
        }
    }
    fn duplicate(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let node = self.node_of(pane, cx);
        match browser_of(pane, cx) {
            Some(browser) => {
                let state = BrowserPane::new_tab_state(browser.read(cx), cx);
                self.open_browser(None, Some(state), node.map(Place::Group), window, cx);
            }
            None => {
                self.remember_layout(None, cx);
                let p = pane.read(cx);
                let (ty, mut record) = (p.ty, p.record(cx));
                record.id = None;
                let copy = self.new_pane(ty, record, window, cx);
                self.place(&copy, node.map(Place::Group), window, cx);
            }
        }
    }
    fn reveal(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let id = Self::panel_id(pane);
        self.dock.update(cx, |d, cx| d.select_panel(id, window, cx));
        pane.read(cx).focus_handle(cx).focus(window, cx);
    }
    fn active_browser_pane(&self, cx: &App) -> Option<Entity<Pane>> {
        let browser = self.browser(cx)?;
        self.ordered(cx)
            .into_iter()
            .find(|p| browser_of(p, cx).as_ref() == Some(&browser))
    }
    fn reveal_active_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_browser_pane(cx);
        if let Some(pane) = pane
            && !self.is_displayed(&pane, cx)
        {
            let id = Self::panel_id(&pane);
            self.dock.update(cx, |d, cx| d.select_panel(id, window, cx));
        }
    }
    /// Closing is one recoverable layout edit.
    fn close_pane(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_layout(Some(pane.read(cx).id), cx);
        self.remove_pane(pane, window, cx);
    }
    fn remove_pane(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let p = pane.read(cx);
        let beside = self
            .node_of(pane, cx)
            .map(|node| self.group_members(node, cx))
            .unwrap_or_default()
            .into_iter()
            .filter(|other| other != pane)
            .map(|other| other.read(cx).id)
            .collect();
        let closed = ClosedPanel {
            key: p.ty.key.to_string(),
            record: p.record(cx),
            beside,
        };
        // Keep the most recently closed instance of each type in the picker.
        self.closed.retain(|c| c.key != closed.key);
        self.closed.push(closed);
        self.dock
            .update(cx, |d, cx| d.remove_panel(pane.clone(), window, cx));
    }
    fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closed) = self.closed.last() else {
            return;
        };
        let Some(ty) = panels::TYPES
            .iter()
            .copied()
            .find(|ty| ty.key == closed.key)
        else {
            return;
        };
        if closed.record.id.is_some()
            && self
                .undo
                .as_ref()
                .is_some_and(|u| u.closed_id == closed.record.id)
        {
            self.undo_layout(window, cx);
            return;
        }
        self.remember_layout(None, cx);
        let closed = self.closed.pop();
        self.open_record(ty, None, closed, window, cx);
    }
    /// Closes the panel behind a tab; `false` when it is not a pane, such as
    /// a placeholder for a type this build does not know.
    pub fn close_panel(
        &mut self,
        id: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.panes.get(&id).map(|t| t.pane.clone()) else {
            return false;
        };
        self.close_pane(&pane, window, cx);
        true
    }
    fn split(
        &mut self,
        pane: &Entity<Pane>,
        placement: gpui_kit::base::Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.node_of(pane, cx) else {
            return;
        };
        self.remember_layout(None, cx);
        let id = Self::panel_id(pane);
        // A move, not `split_at`: the tab leaves its group for the new one.
        let target = InsertTarget::Split {
            node,
            placement,
            size: None,
        };
        self.dock
            .update(cx, |d, cx| d.move_panel(id, target, window, cx));
    }
    /// The pane holding keyboard focus.
    fn focused_pane(&self, window: &Window, cx: &App) -> Option<Entity<Pane>> {
        self.ordered(cx)
            .into_iter()
            .find(|p| p.read(cx).contains_focus(window, cx))
    }

    /// The default arrangement: Library and Locations, one Music Browser,
    /// Queue and History. Open instances keep their state; other panels close.
    fn reset(
        &mut self,
        initial: Option<BrowserState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use panels::{browser, library, locations, queue};
        if initial.is_none() {
            self.remember_layout(None, cx);
            if let Some(before) = &mut self.undo {
                before.sidebar_visibility = Some((self.left_hidden, self.right_hidden));
            }
            self.left_hidden = false;
            self.right_hidden = false;
        }
        let mut initial = initial;
        let mut get = |s: &mut Self, ty: &'static PanelType, cx: &mut Context<Self>| {
            let open = if ty == &browser::TYPE {
                let active = s.browser(cx);
                s.ordered(cx)
                    .into_iter()
                    .find(|p| active.is_some() && browser_of(p, cx) == active)
                    .or_else(|| s.pane_of_type(ty, cx))
            } else {
                s.pane_of_type(ty, cx)
            };
            open.unwrap_or_else(|| {
                let record = s
                    .closed
                    .iter()
                    .find(|c| c.key == ty.key)
                    .map(|c| c.record.clone())
                    .or_else(|| {
                        if ty == &browser::TYPE {
                            initial.take().map(PanelRecord::with_state)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                s.new_pane(ty, record, window, cx)
            })
        };
        let library = get(self, &library::TYPE, cx);
        let locations = get(self, &locations::TYPE, cx);
        let browser = get(self, &browser::TYPE, cx);
        let queue = get(self, &queue::QUEUE, cx);
        let history = get(self, &queue::HISTORY, cx);
        self.closed.retain(|c| {
            ![&library, &locations, &browser, &queue, &history]
                .iter()
                .any(|p| {
                    c.record.id == Some(p.read(cx).id)
                        || (p.read(cx).ty.instances == Instances::Single
                            && c.key == p.read(cx).ty.key)
                })
        });
        let layout = DockLayout::h_split()
            .child(
                DockLayout::tabs().panel(library).panel(locations),
                Some(px(LEADING_WIDTH)),
            )
            .child(DockLayout::tabs().panel(browser.clone()), None)
            .child(
                DockLayout::tabs().panel(queue).panel(history),
                Some(px(TRAILING_WIDTH)),
            );
        self.dock
            .update(cx, |d, cx| d.set_center(layout, window, cx));
        self.sync_panes(window, cx);
        if let Some(b) = browser_of(&browser, cx) {
            self.services
                .router
                .update(cx, |r, cx| r.set_active(Some(&b), cx));
        }
        self.schedule_save(cx);
        cx.notify();
    }

    // ----- The active Music Browser ----------------------------------------

    fn browser(&self, cx: &App) -> Option<Entity<Browser>> {
        self.services.router.read(cx).active()
    }
    /// Keeps the active browser among the open ones.
    fn sync_active_browser(&mut self, cx: &mut Context<Self>) {
        let open: Vec<Entity<Browser>> = self
            .ordered(cx)
            .iter()
            .filter_map(|p| browser_of(p, cx))
            .collect();
        let active = self.browser(cx);
        if active.as_ref().is_some_and(|b| open.contains(b)) {
            return;
        }
        let next = open.first().cloned();
        self.services
            .router
            .update(cx, |r, cx| r.set_active(next.as_ref(), cx));
    }
    fn snapshot(&self, cx: &App) -> Profile {
        let p = self.services.player.read(cx);
        let queue = p.queue();
        let router = self.services.router.read(cx);
        let locations = self.services.locations.read(cx);
        let active = router.active().and_then(|browser| {
            self.ordered(cx)
                .into_iter()
                .find(|p| browser_of(p, cx).as_ref() == Some(&browser))
                .map(|p| p.read(cx).id)
        });
        Profile {
            volume: p.saved_volume(),
            full_cd: p.full_cd,
            layout: Some(self.dock.read(cx).dump(cx)),
            workspace: WorkspaceState {
                left_hidden: self.left_hidden,
                right_hidden: self.right_hidden,
                active,
                closed: self.closed.clone(),
                player_height: Some(self.player_height),
            },
            columns: vec![],
            table_layouts: Some(router.template.layouts.clone()),
            places: Some(locations.places.state()),
            queue: (!queue.entries.is_empty()).then(|| SavedQueue {
                queue: (**queue).clone(),
                position: p.status.position,
            }),
            window: self.prefs,
            collection: router.template.prefs.clone(),
            expanded: locations.expanded(cx),
            columns_version: 0,
            legacy_navigation: Default::default(),
        }
    }
    /// What a library action applies to: the table's selection, or the
    /// playing track when nothing is selected.
    fn targets(&self, cx: &App) -> Targets {
        let playing = || match &self.services.player.read(cx).status.track {
            Some(track) => Targets::Tracks(vec![track.clone()]),
            None => Targets::Tracks(vec![]),
        };
        let Some(browser) = self.browser(cx) else {
            return playing();
        };
        let t = browser.read(cx).table.read(cx);
        let targets = t.delegate().targets(None, t.selected_row());
        if !targets.is_empty()
            || t.selected_row()
                .is_some_and(|i| t.delegate().header(i).is_some())
        {
            return targets;
        }
        match &self.services.player.read(cx).status.track {
            Some(track) => Targets::Tracks(vec![track.clone()]),
            None => targets,
        }
    }
    fn extend_selection(&self, direction: isize, cx: &mut App) {
        let Some(browser) = self.browser(cx) else {
            return;
        };
        browser.read(cx).table.clone().update(cx, |t, cx| {
            let primary = t.selected_row();
            if let Some(row) = t.delegate_mut().extend_selection(primary, direction) {
                t.clear_selection(cx);
                t.scroll_to_row(row, cx);
            }
        });
    }
    fn move_selection(&self, down: bool, cx: &mut App) {
        let Some(browser) = self.browser(cx) else {
            return;
        };
        browser.read(cx).table.clone().update(cx, |t, cx| {
            t.delegate().move_selection(t.selected_row(), down, cx)
        });
    }
    fn rate(&mut self, stars: u8, cx: &mut Context<Self>) {
        let targets = self.targets(cx);
        self.services
            .catalog
            .update(cx, |c, cx| c.rate(targets, stars, cx));
    }
    fn add_music_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add to Library".into()),
        });
        let catalog = self.services.catalog.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = result.await
                && let Some(path) = paths.into_iter().next()
            {
                catalog.update(cx, |c, cx| c.add_root(path, cx));
            }
        })
        .detach();
    }
    /// Keep at most one debounced write in flight. A change during the write
    /// asks for another snapshot after it completes.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.profile.is_none() || self.quitting {
            return;
        }
        self.save_dirty = true;
        if self.save_pending {
            return;
        }
        self.save_pending = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let Ok(Some((file, revision, snapshot))) = this.update(cx, |s, cx| {
                    if s.quitting {
                        return None;
                    }
                    s.save_dirty = false;
                    s.save_revision += 1;
                    s.profile
                        .clone()
                        .map(|file| (file, s.save_revision, s.snapshot(cx)))
                }) else {
                    return;
                };
                let result = cx
                    .background_spawn(async move { file.commit(revision, &snapshot) })
                    .await;
                let again = this.update(cx, |s, cx| {
                    if let Err(e) = result {
                        let _ = s.window.update(cx, |_, w, cx| {
                            w.push_notification(format!("Workspace save failed: {e}"), cx)
                        });
                    }
                    s.save_pending = s.save_dirty && !s.quitting;
                    s.save_pending
                });
                if !again.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }
    fn update_system(&mut self, cx: &mut Context<Self>) {
        let Some(system) = &mut self.system else {
            return;
        };
        let p = self.services.player.read(cx);
        system.update(p.now_playing().as_ref(), &p.status);
    }
    fn system_event(&mut self, e: SystemEvent, window: &mut Window, cx: &mut Context<Self>) {
        let send = |s: &Self, c: Command, cx: &mut Context<Self>| {
            s.services.player.update(cx, |p, cx| p.command(c, cx))
        };
        match e {
            SystemEvent::TrayUnavailable => {
                if self.hidden_to_tray {
                    self.show_window(window, cx);
                }
                cx.notify();
            }
            SystemEvent::ShowWindow => self.show_window(window, cx),
            SystemEvent::PlayPause => self.play_pause(cx),
            SystemEvent::Previous => send(self, Command::Step(-1), cx),
            SystemEvent::Next => send(self, Command::Step(1), cx),
            SystemEvent::ShowQueue => {
                self.show_window(window, cx);
                self.open_panel(&panels::queue::QUEUE, None, window, cx);
            }
            SystemEvent::Quit => self.quit(window, cx),
        }
    }
    fn show_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::system::show_window(self.hwnd);
        window.activate_window();
        self.hidden_to_tray = false;
        cx.notify();
    }
    fn tray_available(&self) -> bool {
        self.system.as_ref().is_some_and(System::tray_available)
    }
    fn hide_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.system.as_ref().is_some_and(System::confirm_tray) {
            window.minimize_window();
            return;
        }
        self.hidden_to_tray = true;
        crate::system::hide_window(self.hwnd);
        self.services
            .player
            .update(cx, |p, cx| p.set_visible(false, cx));
        self.schedule_save(cx);
    }
    fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        let Some(file) = self.profile.clone() else {
            cx.quit();
            return;
        };
        self.quitting = true;
        self.save_revision += 1;
        let revision = self.save_revision;
        let mut snapshot = self.snapshot(cx);
        let commands = self.services.player.read(cx).sender();
        // All personal edits issued before Quit must reach SQLite before process exit.
        let flushed = self.services.catalog.read(cx).handle().map(|s| s.drain());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if let Some(flushed) = flushed {
                        flushed.await?;
                    }
                    snapshot.queue = commands.snapshot()?;
                    file.commit(revision, &snapshot)
                })
                .await;
            if let Err(error) = result {
                let Ok(answer) = this.update_in(cx, |s, w, cx| {
                    if s.hidden_to_tray {
                        s.show_window(w, cx);
                    }
                    w.prompt(
                        PromptLevel::Warning,
                        "The workspace could not be saved.",
                        Some(&error.to_string()),
                        &["Keep open", "Quit without saving"],
                        cx,
                    )
                }) else {
                    return;
                };
                if answer.await != Ok(1) {
                    let _ = this.update(cx, |s, _| s.quitting = false);
                    return;
                }
            }
            let _ = this.update(cx, |s, cx| {
                s.system = None;
                cx.quit();
            });
        })
        .detach();
    }
    fn play_selected(&self, cx: &mut App) {
        let Some(browser) = self.browser(cx) else {
            return;
        };
        let t = browser.read(cx).table.read(cx);
        if let Some(ix) = t
            .selected_row()
            .filter(|&i| t.delegate().header(i).is_none())
            .or_else(|| (!t.delegate().is_empty()).then(|| t.delegate().first_track()))
        {
            browser
                .read(cx)
                .table
                .clone()
                .update(cx, |t, cx| t.delegate().play(ix, cx));
        }
    }
    fn play_pause(&self, cx: &mut App) {
        let p = self.services.player.read(cx);
        if p.status.track.is_none() && p.queue().entries.is_empty() {
            self.play_selected(cx);
        } else {
            self.services
                .player
                .update(cx, |p, cx| p.command(Command::Toggle, cx));
        }
    }
    fn set_customizing(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_checkpoint = None;
        self.dock
            .update(cx, |dock, cx| dock.set_locked(!enabled, window, cx));
        cx.notify();
    }
    fn workspace_menu(&self, cx: &Context<Self>) -> AnyElement {
        let shell = cx.weak_entity();
        let prefs = self.prefs;
        let tray = self.tray_available();
        let locked = self.dock.read(cx).is_locked();
        let can_undo = self.undo.is_some();
        let can_reopen = !self.closed.is_empty();
        Button::new("workspace")
            .ghost()
            .small()
            .icon(Icon::new(IconName::Ellipsis).text_color(materials(cx).text_muted))
            .accessibility_label("Workspace")
            .tooltip("Workspace")
            .dropdown_menu(move |mut menu, _, cx| {
                menu = menu
                    .menu("Go to Folder or File…", Box::new(FocusLocation))
                    .menu("Open Folder…", Box::new(OpenFolder))
                    .menu("Add Music Folder to Library…", Box::new(AddMusicFolder))
                    .menu("New Playlist", Box::new(NewPlaylist))
                    .separator()
                    .menu("New Music Browser Tab", Box::new(NewBrowserTab));
                for ty in panels::TYPES
                    .iter()
                    .copied()
                    .filter(|t| t.instances == Instances::Single)
                {
                    let s = shell.clone();
                    let open = shell.upgrade().is_some_and(|w| w.read(cx).is_open(ty, cx));
                    menu = menu.item(
                        PopupMenuItem::new(format!("Show {}", ty.title))
                            .checked(open)
                            .on_click(move |_, w, cx| {
                                let _ = s.update(cx, |s, cx| s.open_panel(ty, None, w, cx));
                            }),
                    );
                }
                let (s1, s3, s4, s5) = (shell.clone(), shell.clone(), shell.clone(), shell.clone());
                let customize = shell.clone();
                let undo = shell.clone();
                let reopen = shell.clone();
                menu.item(
                    PopupMenuItem::new("Reopen Closed Panel")
                        .disabled(!can_reopen)
                        .on_click(move |_, w, cx| {
                            let _ = reopen.update(cx, |s, cx| s.reopen_closed(w, cx));
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if locked { "Customize Layout" } else { "Done" }).on_click(
                        move |_, w, cx| {
                            let _ = customize.update(cx, |s, cx| s.set_customizing(locked, w, cx));
                        },
                    ),
                )
                .item(
                    PopupMenuItem::new("Undo Layout Change")
                        .disabled(!can_undo)
                        .on_click(move |_, w, cx| {
                            let _ = undo.update(cx, |s, cx| s.undo_layout(w, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Restore Default Layout").on_click(move |_, w, cx| {
                        let _ = s1.update(cx, |s, cx| s.reset(None, w, cx));
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Close to Tray")
                        .checked(prefs.close_to_tray)
                        .disabled(!tray)
                        .on_click(move |_, _, cx| {
                            let _ = s3.update(cx, |s, cx| {
                                s.prefs.close_to_tray = !s.prefs.close_to_tray;
                                s.schedule_save(cx);
                                cx.notify();
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("Hide to Tray").disabled(!tray).on_click(
                    move |_, w, cx| {
                        let _ = s4.update(cx, |s, cx| s.hide_to_tray(w, cx));
                    },
                ))
                .item(PopupMenuItem::new("Quit Disqro").on_click(move |_, w, cx| {
                    let _ = s5.update(cx, |s, cx| s.quit(w, cx));
                }))
            })
            .into_any_element()
    }
    fn bind_address(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        match (self.browser(cx), &self.address) {
            (Some(browser), Some(bar)) => bar.update(cx, |bar, cx| bar.bind(browser, w, cx)),
            (Some(browser), None) => {
                self.address = Some(cx.new(|cx| crate::location::LocationBar::new(browser, w, cx)))
            }
            (None, _) => self.address = None,
        }
    }
    fn title_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let navigation = self.address.as_ref().map_or_else(
            || {
                Button::new("open-browser-nav")
                    .ghost()
                    .small()
                    .label("Open Music Browser")
                    .on_click(cx.listener(|s, _, w, cx| {
                        s.ensure_browser(w, cx);
                    }))
                    .into_any_element()
            },
            |bar| bar.clone().into_any_element(),
        );
        // Kit marks its underlying title bar as native caption. Occlude only
        // the client controls, leaving the flexible space between them draggable.
        let client = || {
            h_flex()
                .h_full()
                .items_center()
                .gap(px(4.))
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let leading = client()
            .min_w_0()
            .child(self.sidebar_button(Home::Leading, cx))
            .child(navigation);
        let trailing = client()
            .flex_shrink_0()
            .child(self.search_field(window, cx))
            .child(self.sidebar_button(Home::Trailing, cx))
            .when(!self.dock.read(cx).is_locked(), |d| {
                d.child(
                    div().occlude().child(
                        Button::new("finish-layout")
                            .small()
                            .label("Done")
                            .icon(IconName::Check)
                            .tooltip("Finish customizing layout")
                            .on_click(cx.listener(|s, _, w, cx| s.set_customizing(false, w, cx))),
                    ),
                )
            })
            .child(div().occlude().child(self.workspace_menu(cx)));

        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(6.))
            .child(
                h_flex()
                    .w(px(42.))
                    .h_full()
                    .flex_shrink_0()
                    .items_center()
                    .child(img("brand/compact.png").size(px(18.))),
            )
            .child(leading)
            .child(div().h_full().flex_1().min_w(px(24.)))
            .child(trailing)
            .into_any_element()
    }
    /// Recovery when the center contains no visible panels.
    fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let workspace = cx.weak_entity();
        v_flex()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .bg(m.canvas)
            .child(
                Icon::new(IconName::LayoutPanelLeft)
                    .size(px(28.))
                    .text_color(m.text_faint),
            )
            .child(div().text_size(px(13.)).text_color(m.text_muted).child(
                if self.panes.is_empty() {
                    "No panels are open"
                } else {
                    "No panels are visible"
                },
            ))
            .child(
                Button::new("empty-open")
                    .small()
                    .label("Open a Panel")
                    .dropdown_menu(move |menu, _, cx| picker_menu(&workspace, None, menu, cx)),
            )
            .into_any_element()
    }
}

/// The panel picker: every type this build provides. A single-instance type
/// that is open is shown where it is; any other pick adds an instance.
fn picker_menu(
    workspace: &WeakEntity<Workspace>,
    node: Option<NodeId>,
    mut menu: PopupMenu,
    cx: &App,
) -> PopupMenu {
    for ty in panels::TYPES.iter().copied() {
        let label = match ty.instances {
            Instances::Multiple => format!("New {}", ty.title),
            Instances::Single => ty.title.to_string(),
        };
        let workspace = workspace.clone();
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(ty.icon)
                .on_click(move |_, w, cx| {
                    let _ = workspace.update(cx, |s, cx| s.open_panel(ty, node, w, cx));
                }),
        );
    }
    let can_reopen = workspace
        .upgrade()
        .is_some_and(|w| !w.read(cx).closed.is_empty());
    let workspace = workspace.clone();
    menu.separator().item(
        PopupMenuItem::new("Reopen Closed Panel")
            .disabled(!can_reopen)
            .on_click(move |_, w, cx| {
                let _ = workspace.update(cx, |s, cx| s.reopen_closed(w, cx));
            }),
    )
}

/// A tab's menu: the panel's own actions, then splitting and closing.
fn tab_menu(
    workspace: &WeakEntity<Workspace>,
    pane: &Entity<Pane>,
    menu: PopupMenu,
    _: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let editing = workspace
        .upgrade()
        .is_some_and(|w| !w.read(cx).dock.read(cx).is_locked());
    let alone = workspace.upgrade().is_none_or(|w| {
        let w = w.read(cx);
        w.node_of(pane, cx)
            .is_none_or(|node| w.group_members(node, cx).len() < 2)
    });
    let (a, b) = (workspace.clone(), workspace.clone());
    let (p1, p2) = (pane.clone(), pane.clone());
    let id = Workspace::panel_id(pane);
    let arrangement = workspace
        .upgrade()
        .and_then(|w| w.read(cx).sidebar_action(id, cx));
    let sidebar = workspace.clone();
    let instance = pane.read(cx).ty.instances == Instances::Multiple;
    let address = browser_of(pane, cx).and_then(|b| {
        let b = b.read(cx);
        crate::location::physical_address(&b.location, b.page.as_ref())
            .map(|p| p.display().to_string())
    });
    Pane::menu(pane, menu, cx)
        .when_some(address, |menu, path| {
            menu.item(
                PopupMenuItem::new("Copy Address")
                    .icon(IconName::Copy)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()))
                    }),
            )
        })
        .when(instance, |menu| menu.separator())
        .when_some(arrangement, |menu, action| {
            menu.item(
                PopupMenuItem::new(action.label()).on_click(move |_, w, cx| {
                    let _ = sidebar.update(cx, |s, cx| s.arrange_sidebar(id, action, w, cx));
                }),
            )
        })
        .when(editing, |menu| {
            menu.item(
                PopupMenuItem::new("Split Right")
                    .icon(IconName::SquareSplitHorizontal)
                    .disabled(alone)
                    .on_click(move |_, w, cx| {
                        let _ = a.update(cx, |s, cx| {
                            s.split(&p1, gpui_kit::base::Placement::Right, w, cx)
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Split Down")
                    .icon(IconName::SquareSplitVertical)
                    .disabled(alone)
                    .on_click(move |_, w, cx| {
                        let _ = b.update(cx, |s, cx| {
                            s.split(&p2, gpui_kit::base::Placement::Bottom, w, cx)
                        });
                    }),
            )
        })
        .when(instance || arrangement.is_some() || editing, |menu| {
            menu.separator()
        })
        .item(Pane::close_item(pane, cx))
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let empty = self.dock.read(cx).is_empty(DockPlacement::Center, cx);
        let title = self.title_bar(window, cx);
        let player_height = self.player_height;
        let dock = AnyView::from(self.dock.clone());
        // Cached drawing omits descendants from subsequent accessibility trees.
        let dock = if window.is_a11y_active() {
            dock.into_any_element()
        } else {
            dock.cached(StyleRefinement::default().size_full())
                .into_any_element()
        };
        v_flex()
            .key_context("Disqro")
            .track_focus(&self.focus)
            .size_full()
            .bg(m.canvas)
            .text_color(m.text)
            .on_action(cx.listener(|s, _: &OpenFolder, w, cx| {
                let browser = s.ensure_browser(w, cx);
                browser.update(cx, |b, cx| b.choose(w, cx))
            }))
            .on_action(cx.listener(|s, _: &GoBack, w, cx| {
                if let Some(b) = s.browser(cx) {
                    s.reveal_active_browser(w, cx);
                    b.update(cx, |b, cx| b.back(w, cx))
                }
            }))
            .on_action(cx.listener(|s, _: &GoForward, w, cx| {
                if let Some(b) = s.browser(cx) {
                    s.reveal_active_browser(w, cx);
                    b.update(cx, |b, cx| b.forward(w, cx))
                }
            }))
            .on_action(cx.listener(|s, _: &GoUp, w, cx| {
                if let Some(b) = s.browser(cx) {
                    s.reveal_active_browser(w, cx);
                    b.update(cx, |b, cx| b.up(w, cx))
                }
            }))
            .on_action(cx.listener(|s, _: &Refresh, w, cx| {
                if let Some(b) = s.browser(cx) {
                    b.update(cx, |b, cx| b.reload(w, cx))
                }
            }))
            .on_action(
                cx.listener(|s, _: &ToggleLeftSidebar, w, cx| {
                    s.toggle_sidebar(Home::Leading, w, cx)
                }),
            )
            .on_action(cx.listener(|s, _: &ToggleRightSidebar, w, cx| {
                s.toggle_sidebar(Home::Trailing, w, cx)
            }))
            .on_action(cx.listener(|s, _: &FocusLocation, w, cx| {
                s.hide_suggestions(cx);
                s.ensure_browser(w, cx);
                s.reveal_active_browser(w, cx);
                s.bind_address(w, cx);
                if let Some(bar) = &s.address {
                    bar.update(cx, |bar, cx| bar.edit(w, cx));
                }
            }))
            .on_action(cx.listener(|s, _: &SearchLibrary, w, cx| s.open_search(w, cx)))
            .on_action(cx.listener(|s, _: &FindTrack, w, cx| {
                s.open_filter(w, cx);
            }))
            .on_action(cx.listener(|s, _: &NewBrowserTab, w, cx| {
                let state = s
                    .browser(cx)
                    .map(|b| BrowserPane::new_tab_state(b.read(cx), cx));
                s.open_browser(None, state, None, w, cx);
            }))
            .on_action(cx.listener(|s, _: &CloseTab, w, cx| {
                if let Some(pane) = s.focused_pane(w, cx) {
                    s.close_pane(&pane, w, cx);
                }
            }))
            .on_action(cx.listener(|s, _: &ToggleWaveform, _, cx| {
                s.services
                    .player
                    .update(cx, |p, cx| p.set_perspective(!p.full_cd, cx))
            }))
            .on_action(cx.listener(|s, _: &PlayPause, _, cx| s.play_pause(cx)))
            .on_action(cx.listener(|s, _: &NextTrack, _, cx| {
                s.services
                    .player
                    .update(cx, |p, cx| p.command(Command::Step(1), cx))
            }))
            .on_action(cx.listener(|s, _: &PreviousTrack, _, cx| {
                s.services
                    .player
                    .update(cx, |p, cx| p.command(Command::Step(-1), cx))
            }))
            .on_action(cx.listener(|s, _: &ShowQueue, w, cx| {
                s.open_panel(&panels::queue::QUEUE, None, w, cx);
            }))
            .on_action(cx.listener(|s, _: &ToggleLike, _, cx| {
                let targets = s.targets(cx);
                s.services
                    .catalog
                    .update(cx, |c, cx| c.toggle_like(targets, cx));
            }))
            .on_action(cx.listener(|s, _: &RateOne, _, cx| s.rate(1, cx)))
            .on_action(cx.listener(|s, _: &RateTwo, _, cx| s.rate(2, cx)))
            .on_action(cx.listener(|s, _: &RateThree, _, cx| s.rate(3, cx)))
            .on_action(cx.listener(|s, _: &RateFour, _, cx| s.rate(4, cx)))
            .on_action(cx.listener(|s, _: &RateFive, _, cx| s.rate(5, cx)))
            .on_action(cx.listener(|s, _: &ClearRating, _, cx| s.rate(0, cx)))
            .on_action(cx.listener(|s, _: &ExtendUp, _, cx| s.extend_selection(-1, cx)))
            .on_action(cx.listener(|s, _: &ExtendDown, _, cx| s.extend_selection(1, cx)))
            .on_action(cx.listener(|s, _: &MovePlaylistUp, _, cx| s.move_selection(false, cx)))
            .on_action(cx.listener(|s, _: &MovePlaylistDown, _, cx| s.move_selection(true, cx)))
            .on_action(cx.listener(|s, _: &RemoveFromPlaylist, _, cx| {
                let Some(b) = s.browser(cx) else { return };
                let t = b.read(cx).table.read(cx);
                if let Targets::Keys(Scope::Playlist(p), entries) =
                    t.delegate().targets(None, t.selected_row())
                    && !entries.is_empty()
                {
                    s.services
                        .catalog
                        .update(cx, |c, cx| c.remove_entries(p, entries, cx));
                }
            }))
            .on_action(cx.listener(|s, _: &NewPlaylist, _, cx| {
                s.services
                    .catalog
                    .update(cx, |c, cx| c.new_playlist(Targets::Tracks(vec![]), cx));
            }))
            .on_action(cx.listener(|s, _: &AddMusicFolder, w, cx| s.add_music_folder(w, cx)))
            .on_action(cx.listener(|s, _: &HideToTray, w, cx| s.hide_to_tray(w, cx)))
            .on_action(cx.listener(|s, _: &Quit, w, cx| s.quit(w, cx)))
            .on_action(cx.listener(|s, _: &PlaySelected, w, cx| {
                let Some(browser) = s.browser(cx) else { return };
                if browser.read(cx).listing_discs() {
                    browser.update(cx, |b, cx| {
                        let ix = b.discs.read(cx).selected_row().unwrap_or(0);
                        b.show_disc(ix, w, cx);
                    });
                } else {
                    browser.update(cx, |b, cx| {
                        let t = b.table.read(cx);
                        b.activate(
                            t.selected_row()
                                .unwrap_or_else(|| t.delegate().first_track()),
                            w,
                            cx,
                        );
                    });
                }
            }))
            .on_drag_move(cx.listener(|s, e: &DragMoveEvent<PlayerResize>, w, cx| {
                let bottom = w.viewport_size().height;
                let height = f32::from(bottom - e.event.position.y).clamp(PLAYER_MIN, PLAYER_MAX);
                if (height - s.player_height).abs() >= 1. {
                    s.player_height = height;
                    s.schedule_save(cx);
                    cx.notify();
                }
            }))
            .child(TitleBar::new().h(px(40.)).pl(px(10.)).child(title))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(dock)
                    .when(
                        self.search_visible && self.search.read(cx).full_results(),
                        |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .occlude()
                                    .bg(m.canvas)
                                    .child(self.search.clone()),
                            )
                        },
                    )
                    .when(empty, |d| d.child(self.empty_state(cx))),
            )
            .child(
                div()
                    .id("player")
                    .h(px(player_height))
                    .flex_shrink_0()
                    .relative()
                    .border_t_1()
                    .border_color(m.line)
                    .child(self.services.player.clone())
                    .child(
                        // The player's top edge resizes it.
                        div()
                            .id("player-edge")
                            .absolute()
                            .top(px(-3.))
                            .left_0()
                            .right_0()
                            .h(px(6.))
                            .cursor_row_resize()
                            .on_drag(PlayerResize, |_, _, _, cx| cx.new(|_| PlayerResize)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{LAYOUT_VERSION, current_layout};
    use gpui_kit::component::dock::DockAreaState;

    #[test]
    fn only_a_layout_of_this_version_is_restored() {
        let layout = |version| DockAreaState {
            version,
            ..Default::default()
        };
        assert!(current_layout(Some(layout(Some(LAYOUT_VERSION)))).is_some());
        // Layouts of the fixed Sidebar, Recordings, Queue and Now Playing
        // panels were version 2; they give way to the default layout.
        assert!(current_layout(Some(layout(Some(2)))).is_none());
        assert!(current_layout(Some(layout(None))).is_none());
        assert!(current_layout(None).is_none());
    }
}

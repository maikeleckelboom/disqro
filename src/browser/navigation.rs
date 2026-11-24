//! Per-browser navigation routes, history, and view restoration.
use super::*;
use crate::table::{Selection, Viewport};
use std::collections::HashMap;

const HISTORY_SELECTION_KEYS: usize = 128 * 1024;

fn read_or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Route {
    pub location: Location,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<Page>,
}

impl Route {
    pub fn new(location: Location, page: Option<Page>) -> Self {
        Self { location, page }
    }

    fn same(&self, other: &Self) -> bool {
        self.page == other.page
            && match (&self.location, &other.location) {
                (Location::Folder(a), Location::Folder(b)) => crate::places::same_path(a, b),
                (Location::Album { id: a, .. }, Location::Album { id: b, .. })
                | (Location::Release { id: a, .. }, Location::Release { id: b, .. }) => a == b,
                (a, b) => a == b,
            }
    }

    fn parent(&self, membership: Option<&Membership>) -> Option<(Route, Option<RowKey>)> {
        let (location, page, child) = match (&self.location, &self.page) {
            (location @ Location::Folder(_), Some(Page::Disc(id))) => (
                location.clone(),
                membership.map(|m| Page::Release(m.key.clone())),
                Some(RowKey::Disc(id.clone())),
            ),
            (location @ Location::Folder(_), Some(Page::Release(id))) => {
                (location.clone(), None, Some(RowKey::Release(id.clone())))
            }
            (Location::Folder(path), None) => (
                Location::Folder(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())?
                        .to_owned(),
                ),
                None,
                None,
            ),
            (Location::Album { root, id, .. }, _) => (
                membership
                    .and_then(|m| {
                        Some(Location::Release {
                            root: *root,
                            id: m.key.album()?,
                            title: m.title.clone(),
                        })
                    })
                    .unwrap_or(Location::Library(*root)),
                None,
                Some(RowKey::Disc(DiscId::Library(*id))),
            ),
            (Location::Release { root, id, .. }, _) => (
                Location::Library(*root),
                None,
                Some(RowKey::Release(DiscId::Library(*id))),
            ),
            (Location::Library(Some(root)), _) => (
                Location::Collections,
                None,
                Some(RowKey::Link(ShelfLink::Collection(*root))),
            ),
            (Location::Playlist(id), _) => (
                Location::Playlists,
                None,
                Some(RowKey::Link(ShelfLink::Playlist(*id))),
            ),
            (Location::Liked, _) => (Location::Library(None), None, None),
            _ => return None,
        };
        Some((Route::new(location, page), child))
    }
}

/// Compact, serializable context shared by the current view, history, and
/// previously visited routes. No listings or hydrated tracks are retained.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedVisit {
    #[serde(flatten)]
    pub route: Route,
    #[serde(default)]
    pub albums: bool,
    #[serde(default)]
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subfolders: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_or_default"
    )]
    sort: Option<(Field, bool)>,
    #[serde(
        default,
        skip_serializing_if = "Bookmark::is_empty",
        deserialize_with = "read_or_default"
    )]
    pub bookmark: Bookmark,
}

impl SavedVisit {
    pub fn new(route: Route, albums: bool) -> Self {
        Self {
            route,
            albums,
            query: String::new(),
            root: None,
            child: None,
            subfolders: None,
            sort: None,
            bookmark: Bookmark::default(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Bookmark {
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<RowKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    marked: Option<Arc<Selection>>,
    viewport: Viewport,
}

impl Bookmark {
    pub fn legacy(scroll: f32) -> Self {
        Self {
            viewport: Viewport::legacy(scroll),
            ..Self::default()
        }
    }

    fn has_context(&self) -> bool {
        self.selected.is_some() || self.marked.is_some() || !self.viewport.is_at_start()
    }
    fn is_empty(&self) -> bool {
        !self.has_context()
    }
}

#[derive(Default)]
pub(super) struct History {
    back: Vec<SavedVisit>,
    forward: Vec<SavedVisit>,
    recent: Vec<SavedVisit>,
}

fn push_bounded(visits: &mut Vec<SavedVisit>, visit: SavedVisit) {
    if visits.len() == HISTORY {
        visits.remove(0);
    }
    visits.push(visit);
}

impl History {
    pub(super) fn can_step(&self, forward: bool) -> bool {
        if forward {
            !self.forward.is_empty()
        } else {
            !self.back.is_empty()
        }
    }
    fn remember(&mut self, visit: SavedVisit) {
        self.recent.retain(|entry| !entry.route.same(&visit.route));
        push_bounded(&mut self.recent, visit);
    }

    fn find(&self, route: &Route) -> Option<SavedVisit> {
        self.recent
            .iter()
            .rev()
            .find(|entry| entry.route.same(route))
            .cloned()
    }

    fn record(&mut self, current: SavedVisit) {
        self.remember(current.clone());
        push_bounded(&mut self.back, current);
        self.forward.clear();
        self.limit_selections();
    }

    fn step(&mut self, current: SavedVisit, forward: bool) -> Option<SavedVisit> {
        let next = if forward {
            self.forward.pop()?
        } else {
            self.back.pop()?
        };
        self.remember(current.clone());
        push_bounded(
            if forward {
                &mut self.back
            } else {
                &mut self.forward
            },
            current,
        );
        self.limit_selections();
        Some(next)
    }

    fn limit_selections(&mut self) {
        let mut decisions = HashMap::new();
        let mut retained = 0usize;
        // Prefer the latest context for each location. Share identical in-memory
        // snapshots between the last visit and the history entry for that visit.
        let visits = self
            .recent
            .iter_mut()
            .rev()
            .chain(self.back.iter_mut().rev())
            .chain(self.forward.iter_mut().rev());
        for visit in visits {
            if let Some(marked) = &visit.bookmark.marked {
                let keep = *decisions.entry(Arc::as_ptr(marked)).or_insert_with(|| {
                    if retained == 0
                        || marked.len() <= HISTORY_SELECTION_KEYS.saturating_sub(retained)
                    {
                        retained = retained.saturating_add(marked.len());
                        true
                    } else {
                        false
                    }
                });
                if !keep {
                    visit.bookmark.marked = None;
                }
            }
        }
    }
}

pub(super) struct Pending {
    bookmark: Bookmark,
    target: Option<RowKey>,
}

pub(super) enum Arrival {
    Regular,
    Place(PathBuf),
    File(Reveal),
    Row(RowKey),
    FolderChild(PathBuf),
}

const SHELF_ROW_HEIGHT: f32 = 42.;

impl Browser {
    pub(crate) fn route(&self) -> Route {
        Route::new(self.location.clone(), self.page.clone())
    }

    fn bookmark(&self, cx: &App) -> Bookmark {
        if let Some(pending) = &self.pending {
            return pending.bookmark.clone();
        }
        if self.listing_shelf() {
            let table = self.discs.read(cx);
            let key = |i| table.delegate().rows.get(i).map(Shelf::key);
            Bookmark {
                selected: table.selected_row().and_then(key),
                marked: None,
                viewport: Viewport::capture(table, SHELF_ROW_HEIGHT, key),
            }
        } else {
            let table = self.table.read(cx);
            let key = |i| table.delegate().selected_key(i);
            Bookmark {
                selected: table
                    .selected_row()
                    .and_then(key)
                    .or_else(|| self.selected.clone()),
                marked: table.delegate().marked_selection().map(Arc::new),
                viewport: Viewport::capture(table, table.delegate().row_height(), key),
            }
        }
    }

    pub(crate) fn saved_visit(&self, cx: &App) -> SavedVisit {
        let table = self.table.read(cx);
        SavedVisit {
            route: self.route(),
            albums: self.prefs.albums,
            query: self.search.read(cx).value().to_string(),
            root: self.tree_root.clone(),
            child: self.folder_child.clone(),
            subfolders: self.location.is_folder().then_some(self.subfolders),
            sort: table
                .delegate()
                .sort
                .map(|(f, d)| (f, d == ColumnSort::Descending)),
            bookmark: self.bookmark(cx),
        }
    }

    pub(crate) fn saved_history(&self) -> (Vec<SavedVisit>, Vec<SavedVisit>, Vec<SavedVisit>) {
        (
            self.history.back.clone(),
            self.history.forward.clone(),
            self.history.recent.clone(),
        )
    }

    pub(crate) fn restore_history(
        &mut self,
        back: Vec<SavedVisit>,
        forward: Vec<SavedVisit>,
        recent: Vec<SavedVisit>,
    ) {
        let bounded = |mut visits: Vec<SavedVisit>| {
            let skip = visits.len().saturating_sub(HISTORY);
            visits.drain(..skip);
            visits
        };
        self.history = History {
            back: bounded(back),
            forward: bounded(forward),
            recent: bounded(recent),
        };
        self.history.limit_selections();
    }

    pub(super) fn cancel_pending(&mut self) {
        self.navigation += 1;
        self.pending = None;
    }

    pub(crate) fn waiting_for_context(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.target.is_some() || p.bookmark.has_context())
    }

    pub(super) fn move_to(
        &mut self,
        route: Route,
        arrival: Arrival,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.saved_visit(cx);
        let same = current.route.same(&route);
        if same && matches!(arrival, Arrival::Regular) {
            self.navigation += 1;
            return;
        }
        if (!same || matches!(arrival, Arrival::Row(_) | Arrival::File(_)))
            && !matches!(&current.route.location, Location::Folder(p) if p.as_os_str().is_empty())
        {
            self.history.record(current.clone());
        }
        let place = self.tree_root.as_ref().filter(|root| {
            matches!(&route.location, Location::Folder(path) if library::within(path, root))
        }).cloned();
        let mut visit = if same {
            current
        } else {
            self.history.find(&route).unwrap_or_else(|| {
                let mut visit = SavedVisit::new(route.clone(), self.prefs.albums);
                visit.root = place;
                if matches!(&route.location, Location::Folder(path)
                    if self.location.is_folder() && crate::places::same_path(path, &self.path))
                {
                    visit.subfolders = Some(self.subfolders);
                }
                visit
            })
        };
        // Names may change between visits to the same destination.
        visit.route = route;
        let (file, target) = match arrival {
            Arrival::Place(root) => {
                visit.root = Some(root);
                (None, None)
            }
            Arrival::FolderChild(child) => {
                visit.child = Some(child);
                (None, None)
            }
            Arrival::File(file) => (Some(file), None),
            Arrival::Row(key) => {
                if self.subfolders
                    && self.page.is_some()
                    && matches!(&visit.route.location, Location::Folder(path) if crate::places::same_path(path, &self.path))
                    && matches!(key, RowKey::Disc(_) | RowKey::Release(_))
                {
                    visit.subfolders = Some(true);
                }
                (None, Some(key))
            }
            Arrival::Regular => (None, None),
        };
        self.apply_visit(visit, TreeReveal::Ancestors, (file, target), window, cx);
    }

    pub(super) fn up_to_collection(
        &mut self,
        location: Location,
        albums: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let route = Route::new(location, None);
        let current = self.saved_visit(cx);
        self.history.record(current);
        let mut visit = self
            .history
            .find(&route)
            .unwrap_or_else(|| SavedVisit::new(route, albums));
        if visit.albums != albums {
            visit.albums = albums;
            visit.bookmark = Bookmark::default();
        }
        self.restore_visit(visit, window, cx);
    }

    pub(crate) fn restore_visit(
        &mut self,
        visit: SavedVisit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_visit(
            visit,
            TreeReveal::PreserveExpansion,
            (None, None),
            window,
            cx,
        );
    }

    fn apply_visit(
        &mut self,
        mut visit: SavedVisit,
        tree_reveal: TreeReveal,
        (file, target): (Option<Reveal>, Option<RowKey>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let had_list_focus = self.list_focus(cx).is_focused(window);
        self.cancel_pending();
        self.page = visit
            .route
            .page
            .filter(|_| visit.route.location.is_folder());
        self.prefs.albums = visit.albums;
        self.folder_child = visit.child.take();
        let search = if file.is_some() {
            String::new()
        } else {
            visit.query
        };
        self.query = search.trim().to_lowercase();
        self.search
            .update(cx, |s, cx| s.set_value(search, window, cx));
        self.reset_table(cx);
        self.table.update(cx, |t, _| {
            t.delegate_mut().sort = visit.sort.map(|(field, descending)| {
                (
                    field,
                    if descending {
                        ColumnSort::Descending
                    } else {
                        ColumnSort::Ascending
                    },
                )
            });
        });
        self.pending = Some(Pending {
            bookmark: visit.bookmark,
            target,
        });
        let location = visit.route.location;
        self.membership = None;
        self.membership_pending = false;
        self.notice = None;
        if let Location::Folder(path) = location {
            let recursive = visit
                .subfolders
                .unwrap_or_else(|| self.prefs.includes_subfolders(&path));
            if (self.folder_ready || self.busy)
                && self.location.is_folder()
                && crate::places::same_path(&self.path, &path)
                && self.subfolders == recursive
                && file.is_none()
            {
                self.location = Location::Folder(path);
                self.tree_root = visit.root;
                self.sync_tree_with(tree_reveal, cx);
                self.filter(cx);
                if !self.busy {
                    self.finish_listing(cx);
                }
            } else {
                self.subfolders = recursive;
                self.load_from(path, file, visit.root, tree_reveal, window, cx);
            }
        } else {
            self.generation += 1;
            if self.busy {
                self.folder_ready = false;
            }
            self.task.take();
            self.busy = false;
            self.count = 0;
            self.duration = None;
            self.location = location;
            self.sync_tree_with(TreeReveal::PreserveExpansion, cx);
            if let Location::Album { id, .. } = self.location {
                self.load_membership(id, cx);
            }
            self.filter(cx);
            let catalog = self.catalog.read(cx);
            let unavailable = self.location.unavailable(catalog).or_else(|| {
                catalog
                    .handle()
                    .is_none()
                    .then_some("The Library is unavailable.")
            });
            if let Some(notice) = unavailable {
                self.notice = Some(notice.into());
                self.finish_listing(cx);
            }
        }
        if had_list_focus {
            self.list_focus(cx).focus(window, cx);
        }
        cx.emit(ViewChanged);
        cx.notify();
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.saved_visit(cx);
        if let Some(next) = self.history.step(current, false) {
            self.restore_visit(next, window, cx);
        }
    }

    pub fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.saved_visit(cx);
        if let Some(next) = self.history.step(current, true) {
            self.restore_visit(next, window, cx);
        }
    }

    pub(crate) fn ancestors(&self) -> Vec<Route> {
        let mut routes = vec![self.route()];
        while let Some((parent, _)) = routes.last().unwrap().parent(self.membership.as_deref()) {
            routes.push(parent);
        }
        routes.reverse();
        routes
    }

    pub fn can_go_up(&self) -> bool {
        !self.membership_pending && self.route().parent(self.membership.as_deref()).is_some()
    }

    pub(crate) fn open_ancestor(
        &mut self,
        route: Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.membership_pending
            && self
                .route()
                .parent(self.membership.as_deref())
                .is_some_and(|(parent, _)| parent.same(&route))
        {
            self.up(window, cx);
        } else {
            self.move_to(route, Arrival::Regular, window, cx);
        }
    }

    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.membership_pending {
            return;
        }
        let Some((parent, target)) = self.route().parent(self.membership.as_deref()) else {
            return;
        };
        let arrival = match (&self.location, &parent.location, &self.page) {
            (Location::Folder(path), Location::Folder(_), None) => {
                Arrival::FolderChild(path.clone())
            }
            _ => target.map_or(Arrival::Regular, Arrival::Row),
        };
        self.move_to(parent, arrival, window, cx);
    }

    pub(super) fn finish_listing(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let listing_shelf = self.listing_shelf();
        let shelf_position =
            |discs: &Discs, key: &RowKey| discs.rows.iter().position(|r| r.key() == *key);
        let target = pending.target.as_ref().and_then(|key| {
            if listing_shelf {
                shelf_position(self.discs.read(cx).delegate(), key)
            } else {
                let table = self.table.read(cx).delegate();
                table.position(key).or_else(|| match key {
                    RowKey::Track(source, Some(disc)) => {
                        let mut disc = disc.clone();
                        disc.sheet = self.equivalent_sheets.get(&disc.sheet)?.clone();
                        table.position(&RowKey::Track(source.clone(), Some(disc)))
                    }
                    RowKey::Disc(id)
                        if self.location.is_folder()
                            && self.page.is_none()
                            && !self.can_list_discs() =>
                    {
                        table.first_on_disc(id)
                    }
                    _ => None,
                })
            }
        });
        if let Some(key) = &pending.target
            && target.is_none()
            && self.notice.is_none()
        {
            let view = match key {
                RowKey::Release(_) if self.can_list_discs() && !listing_shelf => Some(true),
                RowKey::Disc(_) if self.location == Location::Library(None) && !listing_shelf => {
                    Some(true)
                }
                RowKey::Key(_) | RowKey::Track(..) if listing_shelf => Some(false),
                _ => None,
            };
            let clear_sort = matches!(key, RowKey::Disc(_))
                && !listing_shelf
                && view.is_none()
                && self.table.read(cx).delegate().sort.is_some();
            let clear_search = view.is_none() && !clear_sort && !self.query.is_empty();
            if view.is_some() || clear_sort || clear_search {
                self.pending = Some(pending);
                let browser = cx.entity();
                let navigation = self.navigation;
                let window = self.window;
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        browser.update(cx, |b, cx| {
                            if b.navigation != navigation || b.pending.is_none() {
                                return;
                            }
                            if clear_search {
                                b.clear_search(window, cx);
                            }
                            if let Some(albums) = view {
                                b.prefs.albums = albums;
                            }
                            if clear_sort {
                                b.table.update(cx, |t, _| t.delegate_mut().sort = None);
                            }
                            b.filter(cx);
                            if b.location.is_folder() && !b.busy {
                                b.finish_listing(cx);
                            }
                        });
                    });
                });
                return;
            }
        }
        let mark = pending.bookmark;
        let reveal = |scroll: &UniformListScrollHandle, row| {
            scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        };
        if listing_shelf {
            self.discs.update(cx, |t, cx| {
                let selected = target.or_else(|| {
                    mark.selected
                        .as_ref()
                        .and_then(|key| shelf_position(t.delegate(), key))
                });
                reselect(t, selected, cx);
                mark.viewport
                    .restore(t, SHELF_ROW_HEIGHT, |key| shelf_position(t.delegate(), key));
                if let Some(row) = target {
                    reveal(&t.vertical_scroll_handle, row);
                }
                cx.notify();
            });
        } else {
            self.table.update(cx, |t, cx| {
                let height = MusicTable::sync_rows(t);
                let selected = target.or_else(|| {
                    mark.selected
                        .as_ref()
                        .and_then(|key| t.delegate().position(key))
                });
                reselect(t, selected, cx);
                if target.is_none()
                    && let Some(marked) = mark.marked
                {
                    t.delegate_mut()
                        .restore_marks(Arc::unwrap_or_clone(marked), !self.query.is_empty());
                } else {
                    t.delegate_mut().clear_marks();
                }
                mark.viewport
                    .restore(t, height, |key| t.delegate().position(key));
                if let Some(row) = target {
                    reveal(&t.vertical_scroll_handle, row);
                }
                self.selected = selected.and_then(|row| t.delegate().selected_key(row));
                if target.is_some() || !t.delegate().has_selection_cursor() {
                    t.delegate_mut().set_primary(self.selected.clone());
                }
                cx.notify();
            });
        }
        if target.is_none()
            && matches!(pending.target, Some(RowKey::Key(_) | RowKey::Track(..)))
            && self.notice.is_none()
        {
            self.notice = Some("This recording is unavailable.".into());
        }
        cx.emit(ViewChanged);
        cx.notify();
    }

    pub fn locate(
        &mut self,
        track: Arc<Track>,
        dest: Destination,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if dest == Destination::Locations {
            self.reveal_folder(track, w, cx);
            return;
        }
        let request = self.navigation + 1;
        self.navigation = request;
        let lookup = self.catalog.read(cx).handle().map(|store| {
            let target = track.clone();
            store.call(move |s| s.registered_disc(&target))
        });
        cx.spawn_in(w, async move |this, cx| {
            let result = match lookup {
                Some(lookup) => lookup.await,
                None => Ok(None),
            };
            let _ = this.update_in(cx, |b, w, cx| {
                if b.navigation != request {
                    return;
                }
                match result {
                    Ok(Some(disc)) => {
                        let location = Location::Album {
                            root: Some(disc.root),
                            id: disc.album,
                            title: disc.title,
                        };
                        b.move_to(
                            Route::new(location, None),
                            Arrival::Row(RowKey::Key(disc.track)),
                            w,
                            cx,
                        );
                    }
                    Ok(None) => w.push_notification("This recording is outside the Library.", cx),
                    Err(_) => {
                        w.push_notification("This recording is unavailable in the Library.", cx)
                    }
                }
            });
        })
        .detach();
    }

    fn reveal_folder(&mut self, track: Arc<Track>, w: &mut Window, cx: &mut Context<Self>) {
        let source = track
            .disc
            .as_ref()
            .map_or(&track.source, |disc| &disc.sheet);
        let Some(parent) = source.parent() else {
            return;
        };
        let disc = DiscId::of(&track);
        let page = track.has_disc_context().then(|| Page::Disc(disc.clone()));
        let origin = std::iter::once(self.route())
            .chain(self.history.recent.iter().rev().map(|v| v.route.clone()))
            .find(|r| {
                r.page == page
                    && matches!(&r.location, Location::Folder(path)
                if crate::places::same_path(parent, path) || library::within(parent, path))
            });
        let route = origin.unwrap_or_else(|| {
            let disc_folder = parent
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::release::disc_folder);
            Route::new(
                Location::Folder(parent.to_owned()),
                if disc_folder.is_some() { None } else { page },
            )
        });
        let key = RowKey::Track(track.source.clone(), track.disc.clone());
        self.move_to(route, Arrival::Row(key), w, cx);
    }

    pub fn continue_cd(&self, request: crate::queue::Entry, cx: &mut Context<Self>) {
        let request = match self.continue_from_folder(request, cx) {
            Some(request) => request,
            None => return,
        };
        let player = self.table.read(cx).delegate().player.clone();
        continue_cd(&self.catalog, &player, request, cx);
    }
    /// Continues a CD whose folder this browser has open, without asking the
    /// library. Hands the request back when it cannot.
    fn continue_from_folder(
        &self,
        request: crate::queue::Entry,
        cx: &mut App,
    ) -> Option<crate::queue::Entry> {
        if !request.disc_context.allows(&request.track) {
            return None;
        }
        let player = self.table.read(cx).delegate().player.clone();
        if request.track.library_id.is_none()
            && self.location.is_folder()
            && !self.busy
            && let Some(disc) = self.programs.iter().find(|d| d.id.contains(&request.track))
        {
            let program = disc.tracks.clone().into();
            let (entry, context) = (request.id, request.disc_context);
            player.update(cx, |p, cx| {
                p.command(
                    crate::audio::Command::ContinueDisc(entry, program, context),
                    cx,
                )
            });
            return None;
        }
        Some(request)
    }
}

/// Continues the CD of a queue entry from the program Disqro reads for it,
/// for when no open browser lists that CD.
pub fn continue_cd(
    catalog: &Entity<Catalog>,
    player: &Entity<crate::player::Player>,
    request: crate::queue::Entry,
    cx: &mut App,
) {
    let crate::queue::Entry {
        id: entry,
        track,
        disc_context: context,
    } = request;
    if !context.allows(&track) {
        return;
    }
    let (catalog, player) = (catalog.clone(), player.clone());
    cx.spawn(async move |cx| {
        let result = Catalog::program(catalog.clone(), track.clone(), context, cx).await;
        match result {
            Ok(tracks) if tracks.iter().any(|t| t.same_item(&track)) => {
                player.update(cx, |p, cx| {
                    p.command(
                        crate::audio::Command::ContinueDisc(entry, tracks, context),
                        cx,
                    )
                });
            }
            other => catalog.update(cx, |_, cx| {
                cx.emit(CatalogEvent::Notice(other.err().map_or_else(
                    || "This CD is unavailable.".into(),
                    |e| format!("{e:#}"),
                )))
            }),
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{
        DiscId, Field, HISTORY, History, Location, Membership, Page, PathBuf, PlaylistId, Route,
        RowKey, SavedVisit, ShelfLink,
    };
    use crate::library::DiscTrack;

    fn playlist(id: PlaylistId) -> SavedVisit {
        SavedVisit::new(Route::new(Location::Playlist(id), None), false)
    }

    #[test]
    fn history_steps_restore_snapshots_while_direct_navigation_uses_the_latest_visit() {
        let mut history = History::default();
        let mut first = playlist(1);
        first.query = "Earlier filter".into();
        first.bookmark.selected = Some(RowKey::Key(12));
        first.sort = Some((Field::Title, true));
        history.record(first.clone());
        let mut second = playlist(2);
        second.bookmark.selected = Some(RowKey::Key(22));
        let mut previous = history.step(second.clone(), false).unwrap();
        assert_eq!(previous.query, first.query);
        assert_eq!(previous.sort, first.sort);
        assert_eq!(previous.bookmark.selected, Some(RowKey::Key(12)));
        previous.query = "Later filter".into();
        let forward = history.step(previous.clone(), true).unwrap();
        assert_eq!(forward.bookmark.selected, Some(RowKey::Key(22)));
        assert_eq!(history.find(&first.route).unwrap().query, previous.query);
        assert_eq!(history.step(forward, false).unwrap().query, previous.query);
        history.record(playlist(3));
        assert!(!history.can_step(true));
        assert!(!History::default().can_step(false));
    }

    #[test]
    fn up_uses_the_release_and_directory_hierarchy() {
        let child = Route::new(
            Location::Album {
                root: Some(3),
                id: 22,
                title: "Album CD 2".into(),
            },
            None,
        );
        let membership = Membership {
            key: DiscId::Library(21),
            title: "Album".into(),
            number: 2,
            subtitle: String::new(),
        };
        let (release, reveal) = child.parent(Some(&membership)).unwrap();
        assert_eq!(reveal, Some(RowKey::Disc(DiscId::Library(22))));
        assert_eq!(
            release.location,
            Location::Release {
                root: Some(3),
                id: 21,
                title: "Album".into()
            }
        );
        let (collection, reveal) = release.parent(None).unwrap();
        assert_eq!(reveal, Some(RowKey::Release(DiscId::Library(21))));
        let (directory, reveal) = collection.parent(None).unwrap();
        assert_eq!(directory.location, Location::Collections);
        assert_eq!(reveal, Some(RowKey::Link(ShelfLink::Collection(3))));

        let (directory, reveal) = playlist(8).route.parent(None).unwrap();
        assert_eq!(directory.location, Location::Playlists);
        assert_eq!(reveal, Some(RowKey::Link(ShelfLink::Playlist(8))));
    }

    #[test]
    fn cue_chapters_on_one_file_remain_distinct_through_workspace_serialization() {
        let directory = PathBuf::from(r"C:\Collection");
        let source = directory.join(r"CD 2\program.wav");
        let sheet = source.with_extension("cue");
        let key = |number, start| {
            RowKey::Track(
                source.clone(),
                Some(DiscTrack {
                    sheet: sheet.clone(),
                    number,
                    start,
                    end: Some(start + 60),
                }),
            )
        };
        let mut visit = SavedVisit::new(
            Route::new(
                Location::Folder(directory),
                Some(Page::Disc(DiscId::Cue(sheet.clone()))),
            ),
            true,
        );
        visit.bookmark.selected = Some(key(32, 1860));
        visit.subfolders = Some(true);
        let encoded = serde_json::to_vec(&visit).unwrap();
        let restored: SavedVisit = serde_json::from_slice(&encoded).unwrap();
        assert!(visit.route.same(&restored.route));
        assert_eq!(restored.subfolders, Some(true));
        assert_eq!(restored.bookmark.selected, Some(key(32, 1860)));
        assert_ne!(restored.bookmark.selected, Some(key(33, 1920)));
    }

    #[test]
    fn history_and_last_visit_cache_are_bounded_independently_of_other_browsers() {
        let mut a = History::default();
        let mut b = History::default();
        b.record(playlist(0));
        for id in 0..HISTORY as i64 + 5 {
            a.record(playlist(id));
        }
        assert_eq!(a.back.len(), HISTORY);
        assert_eq!(a.recent.len(), HISTORY);
        assert!(a.find(&playlist(0).route).is_none());
        assert!(b.find(&playlist(0).route).is_some());
    }
}

//! Per-browser navigation routes, history, and view restoration.
use super::*;
use crate::panels::{Router, RouterEvent};
use crate::table::{Selection, Viewport};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelf: Option<ShelfOrigin>,
}

#[derive(Clone, Debug, Hash, Serialize, Deserialize)]
pub(crate) enum ShelfOrigin {
    Releases,
    Artist(crate::store::ArtistId, String),
}

impl ShelfOrigin {
    fn of(location: &Location) -> Option<Self> {
        match location {
            Location::Artist { id, name } => Some(Self::Artist(*id, name.clone())),
            Location::Releases => Some(Self::Releases),
            _ => None,
        }
    }

    pub(super) fn location(&self) -> Location {
        match self {
            Self::Artist(id, name) => Location::Artist {
                id: *id,
                name: name.clone(),
            },
            Self::Releases => Location::Releases,
        }
    }
}

impl Route {
    pub fn new(location: Location, page: Option<Page>) -> Self {
        Self {
            location,
            page,
            shelf: None,
        }
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        let same_page = match (&self.page, &other.page) {
            (Some(Page::Disc(a)), Some(Page::Disc(b)))
            | (Some(Page::Release(a)), Some(Page::Release(b))) => a.same(b),
            (a, b) => a == b,
        };
        same_page
            && match (&self.shelf, &other.shelf) {
                (Some(ShelfOrigin::Artist(a, _)), Some(ShelfOrigin::Artist(b, _))) => a == b,
                (None, None) | (Some(ShelfOrigin::Releases), Some(ShelfOrigin::Releases)) => true,
                _ => false,
            }
            && match (&self.location, &other.location) {
                (Location::Folder(a), Location::Folder(b)) => crate::places::same_path(a, b),
                (
                    Location::Album {
                        root: ar, id: a, ..
                    },
                    Location::Album {
                        root: br, id: b, ..
                    },
                )
                | (
                    Location::Release {
                        root: ar, id: a, ..
                    },
                    Location::Release {
                        root: br, id: b, ..
                    },
                ) => a == b && ar == br,
                (Location::Artist { id: a, .. }, Location::Artist { id: b, .. }) => a == b,
                (a, b) => a == b,
            }
    }

    /// An identity for focus and element state across relabeling and reflow.
    pub(crate) fn control_id(&self) -> u64 {
        let mut hash = DefaultHasher::new();
        std::mem::discriminant(&self.location).hash(&mut hash);
        match &self.location {
            Location::Folder(path) => crate::places::path_key(path).hash(&mut hash),
            Location::Library(root) => root.hash(&mut hash),
            Location::Artist { id, .. } => id.hash(&mut hash),
            Location::Playlist(id) | Location::Album { id, .. } | Location::Release { id, .. } => {
                id.hash(&mut hash);
            }
            _ => {}
        }
        self.shelf.hash(&mut hash);
        std::mem::discriminant(&self.page).hash(&mut hash);
        if let Some(page) = &self.page {
            std::mem::discriminant(page).hash(&mut hash);
            let (Page::Disc(id) | Page::Release(id)) = page;
            std::mem::discriminant(id).hash(&mut hash);
            match id {
                DiscId::Cue(path) => crate::places::path_key(path).hash(&mut hash),
                DiscId::Album(path, title, number) => {
                    crate::places::path_key(path).hash(&mut hash);
                    title.hash(&mut hash);
                    number.hash(&mut hash);
                }
                DiscId::Library(id) => id.hash(&mut hash),
            }
        }
        hash.finish()
    }

    pub(super) fn program(&self) -> Option<(DiscId, SequenceKind)> {
        match (&self.location, &self.page) {
            (Location::Album { id, .. }, _) => Some((DiscId::Library(*id), SequenceKind::Disc)),
            (Location::Release { id, .. }, _) => {
                Some((DiscId::Library(*id), SequenceKind::Release))
            }
            (_, Some(Page::Disc(id))) => Some((id.clone(), SequenceKind::Disc)),
            (_, Some(Page::Release(id))) => Some((id.clone(), SequenceKind::Release)),
            _ => None,
        }
    }

    /// Keep the nearest useful child when a breadcrumb skips the release.
    /// Folder ancestors instead reveal their immediate physical child.
    fn arrival_at(&self, destination: &Route, membership: Option<&Membership>) -> Option<Arrival> {
        let mut child = self.clone();
        let mut disc = None;
        while let Some((parent, target)) = child.parent(membership) {
            if let Some(RowKey::Disc(id)) = &target {
                disc = Some(id.clone());
            }
            if parent.same(destination) {
                return Some(match (&child.location, &child.page, target) {
                    (Location::Folder(path), None, None) => Arrival::FolderChild(path.clone()),
                    (_, _, Some(release @ RowKey::Release(_))) => {
                        Arrival::Row(disc.map_or(release, RowKey::Disc))
                    }
                    (_, _, Some(key)) => Arrival::Row(key),
                    _ => Arrival::Regular,
                });
            }
            child = parent;
        }
        None
    }

    fn has_toolbar_parent(&self) -> bool {
        match (&self.location, &self.page) {
            (Location::Folder(path), None) => path
                .parent()
                .is_some_and(|parent| !parent.as_os_str().is_empty()),
            (Location::Folder(_), Some(_))
            | (Location::Album { .. } | Location::Release { .. } | Location::Artist { .. }, _) => {
                true
            }
            _ => false,
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
                    .unwrap_or_else(|| {
                        self.shelf
                            .as_ref()
                            .map_or(Location::Library(*root), ShelfOrigin::location)
                    }),
                None,
                Some(RowKey::Disc(DiscId::Library(*id))),
            ),
            (Location::Release { root, id, .. }, _) => (
                self.shelf
                    .as_ref()
                    .map_or(Location::Library(*root), ShelfOrigin::location),
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
            (Location::Liked | Location::RecentlyPlayed, _) => {
                (Location::Library(None), None, None)
            }
            (Location::Artist { id, .. }, _) => {
                (Location::Artists, None, Some(RowKey::Artist(*id)))
            }
            _ => return None,
        };
        let mut parent = Route::new(location, page);
        if matches!(parent.location, Location::Release { .. }) {
            parent.shelf.clone_from(&self.shelf);
        }
        Some((parent, child))
    }
}

/// Compact, serializable context shared by the current view, history, and
/// previously visited routes. No listings or hydrated tracks are retained.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedVisit {
    #[serde(flatten)]
    pub route: Route,
    #[serde(default)]
    pub mode: ViewMode,
    #[serde(default)]
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subfolders: Option<bool>,
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
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_or_default"
    )]
    tree_expanded: Vec<TreeId>,
    /// Subtrees expanded throughout; `null` is the whole tree.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_or_default"
    )]
    tree_deep: Vec<Option<TreeId>>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_or_default"
    )]
    tree_closed: Vec<TreeId>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_or_default"
    )]
    tree_links: Vec<(TreeId, Option<TreeId>)>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_or_default"
    )]
    reveal: Option<RowKey>,
}

impl SavedVisit {
    pub fn new(route: Route, mode: ViewMode) -> Self {
        Self {
            route,
            mode,
            query: String::new(),
            root: None,
            child: None,
            subfolders: None,
            sort: None,
            bookmark: Bookmark::default(),
            tree_expanded: Vec::new(),
            tree_deep: Vec::new(),
            tree_closed: Vec::new(),
            tree_links: Vec::new(),
            reveal: None,
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
    fn destination(&self, forward: bool) -> Option<&SavedVisit> {
        if forward {
            self.forward.last()
        } else {
            self.back.last()
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
                    if marked.len() <= HISTORY_SELECTION_KEYS.saturating_sub(retained) {
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
    recording: bool,
}

impl Pending {
    pub(super) fn shelf_keys(&self) -> Vec<RowKey> {
        self.target
            .iter()
            .chain(&self.bookmark.viewport.anchor)
            .chain(&self.bookmark.selected)
            .cloned()
            .collect()
    }
    pub(super) fn is_recording(&self) -> bool {
        self.recording
    }

    pub(super) fn indexed_child(&self) -> Option<Scope> {
        match self.target.as_ref()? {
            RowKey::Disc(DiscId::Library(id)) => Some(Scope::Album(*id)),
            RowKey::Release(DiscId::Library(id)) => Some(Scope::Release(*id)),
            _ => None,
        }
    }
}

pub(super) enum Arrival {
    Regular,
    Place(PathBuf),
    File(Reveal),
    Row(RowKey),
    Recording(RowKey, ViewMode),
    FolderChild(PathBuf),
    Sequence(bool),
}

#[derive(Default)]
struct RecordingSite {
    row: Option<Arc<Track>>,
    registration: Option<crate::store::query::RegisteredDisc>,
    in_scope: Option<i64>,
    in_playlist: Option<i64>,
}

impl Browser {
    pub(crate) fn route(&self) -> Route {
        Route {
            location: self.location.clone(),
            page: self.page.clone(),
            shelf: self.shelf_origin.clone(),
        }
    }

    pub(super) fn open_indexed_program(
        &mut self,
        location: Location,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut route = Route::new(location, None);
        route.shelf = ShelfOrigin::of(&self.location).or_else(|| self.shelf_origin.clone());
        self.move_to(route, Arrival::Regular, w, cx);
    }

    pub(crate) fn navigation_revision(&self) -> u64 {
        self.navigation
    }

    fn bookmark(&self, cx: &App) -> Bookmark {
        if let Some(pending) = &self.pending {
            return pending.bookmark.clone();
        }
        if self.listing_tree() {
            let tree = self.tree.read(cx);
            return Bookmark {
                selected: tree.delegate().selected_key(),
                marked: None,
                viewport: Viewport::capture(tree, crate::table::ROW_HEIGHT, |index| {
                    tree.delegate().row_key(index)
                }),
            };
        }
        if self.listing_shelf() {
            let table = self.discs.read(cx);
            let key = |i| table.delegate().key_at(i);
            Bookmark {
                selected: table.selected_row().and_then(key),
                marked: None,
                viewport: Viewport::capture(table, table.delegate().row_height(), key),
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
        let expansion = self.tree_expansion(cx);
        SavedVisit {
            route: self.route(),
            mode: self.prefs.mode,
            query: self.search.read(cx).value().to_string(),
            root: self.tree_root.clone(),
            child: self.folder_child.clone(),
            subfolders: self
                .folder_scope()
                .or_else(|| self.location.is_folder().then_some(self.subfolders)),
            sort: if self.listing_tree() {
                self.tree.read(cx).delegate().sort
            } else if self.listing_shelf() {
                self.discs.read(cx).delegate().sort
            } else {
                table.delegate().sort
            }
            .map(|(f, d)| (f, d == ColumnSort::Descending)),
            bookmark: self.bookmark(cx),
            tree_expanded: expansion.open,
            tree_deep: expansion.deep,
            tree_closed: expansion.closed,
            tree_links: expansion.links,
            reveal: self.pending.as_ref().and_then(|p| p.target.clone()),
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
        self.intent.next();
        self.navigation += 1;
        self.pending = None;
    }

    /// Supersedes destination lookups at the point of interaction. A history
    /// bookmark still belongs to its visit when focus moves to another control.
    pub fn begin_navigation(&mut self, cx: &mut Context<Self>) -> u64 {
        let revision = self.intent.next();
        if self.pending.as_ref().is_some_and(|p| {
            p.recording || matches!(p.target, Some(RowKey::Key(_) | RowKey::Track(..)))
        }) {
            self.navigation += 1;
            self.pending = None;
            cx.notify();
        }
        revision
    }

    pub(crate) fn cancel_context_for_input(&mut self, cx: &mut Context<Self>) {
        if self.waiting_for_context() {
            self.cancel_pending();
            cx.notify();
        } else {
            self.intent.next();
            self.navigation += 1;
        }
    }

    pub(crate) fn waiting_for_context(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            p.recording || !self.listing_tree() && (p.target.is_some() || p.bookmark.has_context())
        })
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
            self.begin_navigation(cx);
            return;
        }
        if (!same
            || matches!(
                arrival,
                Arrival::Row(_) | Arrival::Recording(..) | Arrival::File(_)
            ))
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
                let mut visit = SavedVisit::new(route.clone(), self.prefs.mode);
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
        let recording = matches!(arrival, Arrival::Recording(..));
        let (file, target) = match arrival {
            Arrival::Place(root) => {
                visit.root = Some(root);
                (None, None)
            }
            Arrival::FolderChild(child) => {
                visit.child = Some(child);
                (None, None)
            }
            Arrival::Sequence(recursive) => {
                visit.subfolders = Some(recursive);
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
            Arrival::Recording(key, mode) => {
                visit.mode = mode;
                (None, Some(key))
            }
            Arrival::Regular => (None, None),
        };
        self.apply_visit(
            visit,
            TreeReveal::Ancestors,
            (file, target, recording),
            window,
            cx,
        );
    }

    pub(super) fn up_to_collection(
        &mut self,
        location: Location,
        mode: ViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let route = Route::new(location, None);
        let current_route = self.route();
        let current = self.saved_visit(cx);
        self.history.record(current);
        let mut visit = self
            .history
            .find(&route)
            .unwrap_or_else(|| SavedVisit::new(route, mode));
        if visit.mode != mode {
            visit.mode = mode;
            visit.bookmark = Bookmark::default();
        }
        let target = match current_route.arrival_at(&visit.route, self.membership.as_deref()) {
            Some(Arrival::Row(key)) => Some(key),
            _ => None,
        };
        self.apply_visit(
            visit,
            TreeReveal::Ancestors,
            (None, target, false),
            window,
            cx,
        );
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
            (None, None, false),
            window,
            cx,
        );
    }

    fn apply_visit(
        &mut self,
        mut visit: SavedVisit,
        tree_reveal: TreeReveal,
        (file, target, recording): (Option<Reveal>, Option<RowKey>, bool),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let had_list_focus = self.list_focus(cx).is_focused(window);
        self.cancel_pending();
        self.page = visit
            .route
            .page
            .filter(|_| visit.route.location.is_folder());
        self.shelf_origin = visit.route.shelf;
        self.prefs.mode = if visit.mode == ViewMode::Tree && file.is_some() {
            ViewMode::Tracks
        } else {
            visit.mode
        };
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
        let expansion = Expansion {
            open: std::mem::take(&mut visit.tree_expanded),
            deep: std::mem::take(&mut visit.tree_deep),
            closed: std::mem::take(&mut visit.tree_closed),
            links: std::mem::take(&mut visit.tree_links),
        };
        self.reset_tree(expansion, cx);
        let sort = visit.sort.map(|(field, descending)| {
            (
                field,
                if descending {
                    ColumnSort::Descending
                } else {
                    ColumnSort::Ascending
                },
            )
        });
        let shelf = self.page.is_none()
            && (visit.route.location.is_directory()
                || visit.route.location.presentation(visit.mode) == Some(ViewMode::Albums));
        if visit.mode == ViewMode::Tree
            && self.page.is_none()
            && matches!(
                visit.route.location,
                Location::Folder(_) | Location::Library(_)
            )
        {
            self.tree.update(cx, |t, _| t.delegate_mut().sort = sort);
        } else if shelf {
            self.discs.update(cx, |t, _| t.delegate_mut().sort = sort);
        } else {
            self.table.update(cx, |t, _| t.delegate_mut().sort = sort);
        }
        let target = target.or_else(|| if file.is_none() { visit.reveal } else { None });
        self.pending = Some(Pending {
            bookmark: if target.is_some() {
                Bookmark::default()
            } else {
                visit.bookmark
            },
            target,
            recording,
        });
        let location = visit.route.location;
        self.membership = None;
        self.membership_pending = false;
        self.notice = None;
        if let Location::Folder(path) = location {
            self.indexed_sequence.cancel();
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
                    if !self.page_available() {
                        self.notice = Some("This CD or release is unavailable.".into());
                    }
                    self.refresh_folder_traversal(cx);
                    self.finish_listing(cx);
                }
            } else {
                self.subfolders = recursive;
                self.load_from(path, file, visit.root, tree_reveal, window, cx);
            }
        } else {
            if let Location::Library(Some(root)) = location {
                self.subfolders = visit
                    .subfolders
                    .unwrap_or_else(|| self.prefs.collection_includes_subfolders(root));
            }
            self.folder_traversal = Default::default();
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
            self.load_indexed_sequence(cx);
            self.filter(cx);
            if self.listing_tree() {
                self.finish_listing(cx);
            }
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

    pub(crate) fn history_label(&self, forward: bool, cx: &App) -> Option<String> {
        let visit = self.history.destination(forward)?;
        let mut label = self.route_label(&visit.route, cx);
        if visit.route.page.is_none()
            && matches!(
                visit.route.location,
                Location::Folder(_) | Location::Library(_)
            )
            && let Some(RowKey::Disc(id) | RowKey::Release(id)) = visit
                .bookmark
                .viewport
                .anchor
                .as_ref()
                .or(visit.bookmark.selected.as_ref())
            && let Some(name) = self.disc_label(id)
        {
            label = format!("{label} › {name}");
        }
        Some(label)
    }

    fn disc_label(&self, id: &DiscId) -> Option<String> {
        match id {
            DiscId::Cue(sheet) => sheet.file_stem().map(|s| s.to_string_lossy().into_owned()),
            DiscId::Album(folder, title, _) => Some(if title.is_empty() {
                library::display_name(folder)
            } else {
                title.clone()
            }),
            DiscId::Library(_) => None,
        }
    }

    pub(crate) fn route_label(&self, route: &Route, cx: &App) -> String {
        self.navigation_label(route, false, cx)
    }

    pub(crate) fn breadcrumb_label(&self, route: &Route, cx: &App) -> String {
        self.navigation_label(route, true, cx)
    }

    fn navigation_label(&self, route: &Route, compact: bool, cx: &App) -> String {
        self.program_title(route, compact)
            .or_else(|| match route.page.as_ref()? {
                Page::Disc(id) | Page::Release(id) => self.disc_label(id),
            })
            .unwrap_or_else(|| self.location_label(&route.location, cx))
    }

    pub(crate) fn up_in_toolbar(&self) -> bool {
        self.route().has_toolbar_parent()
    }

    pub(crate) fn up_label(&self, cx: &App) -> Option<String> {
        if !self.can_go_up() {
            return None;
        }
        let (route, _) = self.route().parent(self.membership.as_deref())?;
        Some(self.route_label(&route, cx))
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.saved_visit(cx);
        if let Some(next) = self.history.step(current, false) {
            self.restore_visit(next, window, cx);
        } else {
            self.begin_navigation(cx);
        }
    }

    pub fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.saved_visit(cx);
        if let Some(next) = self.history.step(current, true) {
            self.restore_visit(next, window, cx);
        } else {
            self.begin_navigation(cx);
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
        !(self.membership_pending && self.membership.is_none()
            || self.busy && matches!(self.page, Some(Page::Disc(_))))
            && self.route().parent(self.membership.as_deref()).is_some()
    }

    pub(crate) fn open_breadcrumb(
        &mut self,
        route: Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let arrival = self
            .route()
            .arrival_at(&route, self.membership.as_deref())
            .unwrap_or_else(|| self.sequence_arrival(&route));
        self.move_to(route, arrival, window, cx);
    }

    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_up()
            && let Some((parent, _)) = self.route().parent(self.membership.as_deref())
        {
            self.open_breadcrumb(parent, window, cx);
        } else {
            self.begin_navigation(cx);
        }
    }

    pub(super) fn finish_listing(&mut self, cx: &mut Context<Self>) {
        if self.listing_tree() {
            if let Some(pending) = self.pending.take() {
                let center = pending.target.is_some();
                let selected = pending.target.or(pending.bookmark.selected);
                match selected {
                    Some(key) if let Some(target) = tree::recording_identity(&key) => {
                        self.finish_tree_reveal(target, key, pending.bookmark.viewport, center, cx);
                    }
                    selected => self.tree.update(cx, |t, cx| {
                        TreeTable::restore_bookmark(
                            t,
                            selected.as_ref(),
                            pending.bookmark.viewport,
                            cx,
                        )
                    }),
                }
            }
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        let listing_shelf = self.listing_shelf();
        let shelf_position = |discs: &Discs, key: &RowKey| discs.position(key);
        let target = pending.target.as_ref().and_then(|key| {
            if listing_shelf {
                let shelf = self.discs.read(cx).delegate();
                if pending.recording
                    && let RowKey::Disc(id) = key
                {
                    shelf.recording_disc(id)
                } else {
                    shelf_position(shelf, key)
                }
            } else {
                let table = self.table.read(cx).delegate();
                table.position(key).or_else(|| match key {
                    RowKey::Track(source, Some(disc)) => {
                        let sheet = self.alternate_sheet(&disc.sheet).unwrap_or(&disc.sheet);
                        (0..table.len()).find(|&i| {
                            table.track(i).is_some_and(|t| {
                                crate::places::same_path(source, &t.source)
                                    && t.disc.as_ref().is_some_and(|d| {
                                        (disc.number, disc.start, disc.end)
                                            == (d.number, d.start, d.end)
                                            && crate::places::same_path(sheet, &d.sheet)
                                    })
                            })
                        })
                    }
                    RowKey::Track(source, None) => (0..table.len()).find(|&i| {
                        table.track(i).is_some_and(|t| {
                            t.disc.is_none() && crate::places::same_path(source, &t.source)
                        })
                    }),
                    RowKey::Disc(id) | RowKey::Release(id) => table.child_position(key, id),
                    _ => None,
                })
            }
        });
        if pending.target.is_some() && target.is_none() && !self.query.is_empty() {
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
                        b.clear_search(window, cx);
                        b.filter(cx);
                        if b.location.is_folder() && !b.busy {
                            b.finish_listing(cx);
                        }
                    });
                });
            });
            return;
        }
        let mark = pending.bookmark;
        let reveal = |scroll: &UniformListScrollHandle, row| {
            scroll.scroll_to_item(
                row,
                if matches!(pending.target, Some(RowKey::Disc(_) | RowKey::Release(_))) {
                    ScrollStrategy::Top
                } else {
                    ScrollStrategy::Center
                },
            );
        };
        if listing_shelf {
            self.discs.update(cx, |t, cx| {
                let selected = target.or_else(|| {
                    mark.selected
                        .as_ref()
                        .and_then(|key| shelf_position(t.delegate(), key))
                });
                reselect(t, selected, cx);
                if pending.target.is_none() {
                    mark.viewport.restore(t, t.delegate().row_height(), |key| {
                        shelf_position(t.delegate(), key)
                    });
                }
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
                if let Some(marked) = mark.marked {
                    // Toolkit row selection emits SelectRow, which clears the
                    // delegate's selection. A range owns its own cursor.
                    reselect(t, None, cx);
                    t.delegate_mut()
                        .restore_marks(Arc::unwrap_or_clone(marked), !self.query.is_empty());
                } else {
                    reselect(t, selected, cx);
                    t.delegate_mut().clear_marks();
                }
                if pending.target.is_none() {
                    mark.viewport
                        .restore(t, height, |key| t.delegate().position(key));
                }
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
            && let Some(key) = pending.target
            && self.notice.is_none()
        {
            self.notice = Some(
                match key {
                    RowKey::Key(_) | RowKey::Track(..) => "This recording is unavailable.",
                    _ => "This item is no longer in the collection.",
                }
                .into(),
            );
        }
        cx.emit(ViewChanged);
        cx.notify();
    }

    pub fn show_search_track(
        &mut self,
        id: crate::store::TrackId,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self.begin_navigation(cx);
        let Some(store) = self.catalog.read(cx).handle() else {
            return;
        };
        let lookup = store.query(&self.intent, request, move |s| {
            s.rows(Scope::Library(None), &[id])
        });
        cx.spawn_in(w, async move |browser, cx| {
            let result = lookup.await;
            let _ = browser.update_in(cx, |b, w, cx| {
                if b.navigation_is_current(request)
                    && let Ok(rows) = result
                    && let Some(row) = rows.into_iter().find(|row| row.available)
                {
                    b.locate(
                        row.track,
                        library::DiscContext::Inferred,
                        Destination::Library,
                        w,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(crate) fn reveal_recording(
        &mut self,
        id: crate::store::TrackId,
        router: &Entity<Router>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self.begin_navigation(cx);
        let Some(store) = self.catalog.read(cx).handle() else {
            w.push_notification("The Library is unavailable.", cx);
            return;
        };
        let lookup = store.query(&self.intent, request, move |s| {
            let mut files = crate::store::DetachedFiles::default();
            Ok(s.rows(Scope::Library(None), &[id])?
                .pop()
                .filter(|row| {
                    row.available
                        && row.track.source.is_file()
                        && row.track.disc.as_ref().is_none_or(|d| d.sheet.is_file())
                        && files.available(row)
                })
                .map(|row| row.track))
        });
        self.resolve_recording_reveal(request, lookup, router.clone(), w, cx)
            .detach();
    }

    fn resolve_recording_reveal(
        &self,
        request: u64,
        lookup: impl Future<Output = anyhow::Result<Option<Arc<Track>>>> + 'static,
        router: Entity<Router>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn_in(w, async move |this, cx| {
            let found = lookup.await;
            let _ = this.update_in(cx, |b, w, cx| {
                let target = cx.entity_id();
                if !b.navigation_is_current(request)
                    || !router
                        .read(cx)
                        .active()
                        .is_some_and(|b| b.entity_id() == target)
                {
                    return;
                }
                match found {
                    Ok(Some(track)) => {
                        b.locate(
                            track,
                            library::DiscContext::Inferred,
                            Destination::Locations,
                            w,
                            cx,
                        );
                        router.update(cx, |_, cx| cx.emit(RouterEvent::Reveal(target)));
                    }
                    Ok(None) => w.push_notification("This recording is unavailable.", cx),
                    Err(error) => w.push_notification(error.to_string(), cx),
                }
            });
        })
    }

    pub fn locate(
        &mut self,
        track: Arc<Track>,
        context: library::DiscContext,
        dest: Destination,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self.begin_navigation(cx);
        let scope = match &self.location {
            Location::Library(root) if self.listing_tree() => Some(Scope::Library(*root)),
            _ => self.scope(),
        };
        let on_shelf = self.listing_discs();
        if matches!(dest, Destination::Current(_))
            && self.location.is_folder()
            && !on_shelf
            && !self.listing_tree()
            && let Some(key) = self
                .table
                .read(cx)
                .delegate()
                .recording_key(&track, track.library_id)
        {
            self.reveal_row(self.route(), key, ViewMode::Tracks, w, cx);
            return;
        }
        let occurrence = match dest {
            Destination::Current(occurrence) => occurrence,
            Destination::Playlist(occurrence) => Some(occurrence),
            _ => None,
        };
        let lookup = self.catalog.read(cx).handle().map(|store| {
            let track = track.clone();
            store.query(&self.intent, request, move |s| {
                let id = match track.library_id {
                    Some(id) => Some(id),
                    None => s.find(&crate::store::Locator::of(&track))?,
                };
                let Some(id) = id else {
                    return Ok(RecordingSite::default());
                };
                let row = s
                    .rows(Scope::Library(None), &[id])?
                    .pop()
                    .filter(|row| row.available);
                let registration = match &row {
                    Some(row) => s.registered_disc(&row.track)?,
                    None => None,
                };
                let in_scope = scope
                    .map(|scope| s.reveal_key(scope, id, occurrence))
                    .transpose()?
                    .flatten();
                let in_playlist = if let Some(entry) = occurrence {
                    s.reveal_key(Scope::Playlist(entry.playlist), id, Some(entry))?
                } else {
                    None
                };
                Ok(RecordingSite {
                    row: row.map(|r| r.track),
                    registration,
                    in_scope,
                    in_playlist,
                })
            })
        });
        if let Some(lookup) = lookup {
            cx.spawn_in(w, async move |this, cx| {
                let result = lookup.await;
                let _ = this.update_in(cx, |b, w, cx| {
                    if !b.navigation_is_current(request) {
                        return;
                    }
                    match result {
                        Ok(site) => b.arrive_recording(track, context, dest, site, w, cx),
                        Err(err) if !crate::store::is_canceled(&err) => {
                            if matches!(dest, Destination::Library | Destination::Playlist(_)) {
                                w.push_notification(err.to_string(), cx);
                            } else {
                                b.arrive_recording(
                                    track,
                                    context,
                                    dest,
                                    RecordingSite::default(),
                                    w,
                                    cx,
                                );
                            }
                        }
                        Err(_) => {}
                    }
                });
            })
            .detach();
        } else {
            self.arrive_recording(track, context, dest, RecordingSite::default(), w, cx);
        }
    }

    fn arrive_recording(
        &mut self,
        original: Arc<Track>,
        context: library::DiscContext,
        dest: Destination,
        site: RecordingSite,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let RecordingSite {
            row,
            registration,
            in_scope,
            in_playlist,
        } = site;
        let track = row.unwrap_or(original);
        let disc = DiscId::of(&track);
        let Some(folder) = disc.folder() else { return };
        let current = self.route();
        let mode = self.presentation().unwrap_or(ViewMode::Tracks);
        let library_view = matches!(
            self.location,
            Location::Library(_)
                | Location::Album { .. }
                | Location::Release { .. }
                | Location::Artist { .. }
                | Location::Releases
        );
        let physical_key = || RowKey::Track(track.source.clone(), track.disc.clone());
        if matches!(dest, Destination::Current(_)) || dest == Destination::Library && library_view {
            if let Some(key) = in_scope {
                let key = if self.listing_discs() {
                    registration
                        .as_ref()
                        .map_or_else(|| RowKey::Disc(disc.clone()), |reg| reg.shelf_key())
                } else {
                    RowKey::Key(key)
                };
                self.reveal_row(current, key, mode, w, cx);
                return;
            }
            if self.location.is_folder() && matches!(dest, Destination::Current(_)) {
                let contains = match &self.page {
                    Some(Page::Disc(shown)) => shown.same(&disc),
                    Some(Page::Release(shown)) => self.programs.iter().any(|program| {
                        program.id.same(&disc)
                            && program
                                .release
                                .as_ref()
                                .is_some_and(|release| release.key.same(shown))
                    }),
                    None => {
                        crate::places::same_path(folder, &self.path)
                            || (self.subfolders || mode == ViewMode::Tree)
                                && library::within(folder, &self.path)
                    }
                };
                if contains {
                    let key = if mode == ViewMode::Albums {
                        RowKey::Disc(disc.clone())
                    } else {
                        physical_key()
                    };
                    self.reveal_row(current, key, mode, w, cx);
                    return;
                }
            }
        }
        let playlist = match dest {
            Destination::Current(playlist) => playlist,
            Destination::Playlist(playlist) => Some(playlist),
            _ => None,
        };
        if let Some((playlist, key)) = playlist.zip(in_playlist) {
            self.reveal_row(
                Route::new(Location::Playlist(playlist.playlist), None),
                RowKey::Key(key),
                ViewMode::Tracks,
                w,
                cx,
            );
            return;
        }
        if matches!(dest, Destination::Playlist(_)) {
            w.push_notification("This entry is no longer in the playlist.", cx);
            return;
        }
        if dest != Destination::Locations {
            if let Some(reg) = registration {
                let (location, key) = match mode {
                    ViewMode::Tree => (Location::Library(Some(reg.root)), RowKey::Key(reg.track)),
                    ViewMode::Albums => (Location::Library(Some(reg.root)), reg.shelf_key()),
                    ViewMode::Tracks => (
                        Location::Album {
                            root: Some(reg.root),
                            id: reg.album,
                            title: reg.title,
                        },
                        RowKey::Key(reg.track),
                    ),
                };
                self.reveal_row(Route::new(location, None), key, mode, w, cx);
                return;
            }
            if dest == Destination::Library {
                w.push_notification("This recording is outside the Library.", cx);
                return;
            }
        }
        let page =
            (mode == ViewMode::Tracks && context.allows(&track)).then(|| Page::Disc(disc.clone()));
        let key = if mode == ViewMode::Albums {
            RowKey::Disc(disc.clone())
        } else {
            physical_key()
        };
        self.reveal_row(
            Route::new(Location::Folder(folder.to_owned()), page),
            key,
            mode,
            w,
            cx,
        );
    }

    fn reveal_row(
        &mut self,
        route: Route,
        key: RowKey,
        mode: ViewMode,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.route().same(&route) {
            self.history.record(self.saved_visit(cx));
            self.cancel_pending();
            self.pending = Some(Pending {
                bookmark: Bookmark::default(),
                target: Some(key),
                recording: true,
            });
            if !self.busy
                && !self.shelf_loading
                && (self.listing_shelf()
                    || self.listing_tree()
                    || !self.table.read(cx).delegate().is_loading())
            {
                self.finish_listing(cx);
            }
        } else {
            self.move_to(route, Arrival::Recording(key, mode), w, cx);
        }
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

#[cfg(all(test, windows))]
mod native_tests;

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
        ..
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
        Arrival, DiscId, Field, HISTORY, History, Location, Membership, Page, PathBuf, PlaylistId,
        Route, RowKey, SavedVisit, ShelfLink,
    };
    use crate::collection::ViewMode;
    use crate::library::DiscTrack;

    fn playlist(id: PlaylistId) -> SavedVisit {
        SavedVisit::new(Route::new(Location::Playlist(id), None), ViewMode::Tracks)
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
        assert!(history.destination(true).is_none());
        assert!(History::default().destination(false).is_none());
    }

    #[test]
    fn toolbar_up_is_contextual_without_removing_route_parents() {
        for location in [
            Location::Liked,
            Location::RecentlyPlayed,
            Location::Library(Some(3)),
            Location::Playlist(8),
        ] {
            let route = Route::new(location, None);
            assert!(!route.has_toolbar_parent());
            assert!(route.parent(None).is_some());
        }
        for location in [
            Location::Library(None),
            Location::Collections,
            Location::Playlists,
            Location::Folder(PathBuf::from(r"C:\")),
        ] {
            let route = Route::new(location, None);
            assert!(!route.has_toolbar_parent());
            assert!(route.parent(None).is_none());
        }
        let path = PathBuf::from(r"C:\Music");
        assert!(Route::new(Location::Folder(path.clone()), None).has_toolbar_parent());
        assert!(
            Route::new(
                Location::Folder(path.clone()),
                Some(Page::Disc(DiscId::Cue(path.join("album.cue"))))
            )
            .has_toolbar_parent()
        );
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
    fn breadcrumb_to_collection_retains_the_departing_disc_across_a_release() {
        let folder = PathBuf::from(r"C:\Music\Collection");
        let disc = DiscId::Cue(folder.join(r"VII\CD2\disc.cue"));
        let member = Membership {
            key: DiscId::Cue(folder.join(r"VII\CD1\disc.cue")),
            title: "VII".into(),
            number: 2,
            subtitle: String::new(),
        };
        let root = Route::new(Location::Folder(folder.clone()), None);
        let child = Route::new(root.location.clone(), Some(Page::Disc(disc.clone())));
        assert!(
            matches!(child.arrival_at(&root, Some(&member)), Some(Arrival::Row(RowKey::Disc(id))) if id == disc)
        );
        let release = child.parent(Some(&member)).unwrap().0;
        assert!(
            matches!(release.arrival_at(&root, None), Some(Arrival::Row(RowKey::Release(id))) if id == member.key)
        );
        let parent = root.parent(None).unwrap().0;
        assert!(
            matches!(child.arrival_at(&parent, Some(&member)), Some(Arrival::FolderChild(path)) if path == folder)
        );
        assert!(child.arrival_at(&playlist(1).route, None).is_none());
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
            ViewMode::Albums,
        );
        visit.bookmark.selected = Some(key(32, 1860));
        visit.subfolders = Some(true);
        visit.reveal = Some(key(33, 1920));
        let encoded = serde_json::to_vec(&visit).unwrap();
        let restored: SavedVisit = serde_json::from_slice(&encoded).unwrap();
        assert!(visit.route.same(&restored.route));
        assert_eq!(restored.subfolders, Some(true));
        assert_eq!(restored.reveal, Some(key(33, 1920)));
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

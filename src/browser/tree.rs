use super::{Arrival, Browser, Location, Route, ViewChanged};
use crate::{
    audio::Command,
    catalog::{Catalog, ListeningList},
    collection::{Disc, DiscId, Discs, Page, Release, Shelf, ViewMode, tree_releases},
    library::{Track, TrackList},
    panels::Destination,
    store::{RootId, Scope, Ticket},
    table::{Field, RowKey, Viewport, play_library, reselect},
};
use gpui_kit::{
    component::table::{ColumnSort, TableState},
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

mod table;
pub(crate) use table::TreeTable;

const EXPANSION_LIMIT: usize = 128;
const BRANCH_LIMIT: usize = 4096;
const PARALLEL_BRANCHES: usize = 4;
const LOADED_NODE_LIMIT: usize = 32_768;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TreeId {
    Collection(RootId),
    Folder(Option<RootId>, String),
    Release(DiscId),
    Disc(DiscId),
    IndexedTrack(i64),
    File {
        source: String,
        chapter: Option<(String, u8, u64)>,
    },
}

impl TreeId {
    fn track(track: &Track) -> Self {
        match track.library_id {
            Some(id) => Self::IndexedTrack(id),
            None => Self::File {
                source: crate::store::path_key(&track.source),
                chapter: track
                    .disc
                    .as_ref()
                    .map(|d| (crate::store::path_key(&d.sheet), d.number, d.start)),
            },
        }
    }
}

#[derive(Clone)]
enum Item {
    Collection {
        id: RootId,
        path: PathBuf,
        name: String,
        online: bool,
    },
    Folder {
        root: Option<RootId>,
        path: PathBuf,
    },
    Release {
        release: Arc<Release>,
        folder: PathBuf,
    },
    Disc {
        disc: Arc<Disc>,
        folder: PathBuf,
    },
    Track {
        track: Arc<Track>,
        disc: Option<Arc<Disc>>,
    },
}

#[derive(Clone)]
struct Node {
    id: TreeId,
    item: Item,
}

impl Node {
    fn folder(root: Option<RootId>, path: PathBuf) -> Self {
        Self {
            id: TreeId::Folder(root, crate::store::path_key(&path)),
            item: Item::Folder { root, path },
        }
    }

    fn track(track: Arc<Track>, disc: Option<Arc<Disc>>) -> Self {
        Self {
            id: TreeId::track(&track),
            item: Item::Track { track, disc },
        }
    }

    fn disc(disc: Arc<Disc>, folder: PathBuf) -> Self {
        Self {
            id: TreeId::Disc(disc.id.clone()),
            item: Item::Disc { disc, folder },
        }
    }

    fn release(release: Arc<Release>, folder: PathBuf) -> Self {
        Self {
            id: TreeId::Release(release.key.clone()),
            item: Item::Release { release, folder },
        }
    }

    fn expandable(&self) -> bool {
        !matches!(self.item, Item::Track { .. })
    }

    fn value(&self, field: Field) -> String {
        match &self.item {
            Item::Collection { name, path, .. } => match field {
                Field::Title => name.clone(),
                Field::Path => path.display().to_string(),
                _ => String::new(),
            },
            Item::Folder { path, .. } => match field {
                Field::Title => crate::library::folder_name(path),
                Field::Path => path.display().to_string(),
                _ => String::new(),
            },
            Item::Release { release, .. } => release.label(field),
            Item::Disc { disc, .. } => disc.label(field),
            Item::Track { track, .. } => field.value(track, crate::store::Mark::default()),
        }
    }

    fn matches(
        &self,
        terms: &[crate::store::Term],
        marks: &dyn Fn(&Track) -> crate::store::Mark,
    ) -> bool {
        match &self.item {
            Item::Track { track, .. } => crate::collection::matches(track, terms, || marks(track)),
            _ => {
                let title = self.value(Field::Title).to_lowercase();
                terms.iter().all(
                    |term| matches!(term, crate::store::Term::Any(word) if title.contains(word)),
                )
            }
        }
    }

    fn destination(&self, place: Option<PathBuf>) -> Option<Destination> {
        Some(match &self.item {
            Item::Collection { id, .. } => Destination::View(Location::Library(Some(*id))),
            Item::Folder { path, .. } => Destination::Folder {
                path: path.clone(),
                root: place,
            },
            Item::Release { release, folder } => {
                if let Some(id) = release.key.album() {
                    Destination::View(Location::Release {
                        root: release.discs.first().and_then(|disc| disc.root),
                        id,
                        title: release.title.clone(),
                    })
                } else {
                    Destination::FolderPage {
                        path: folder.clone(),
                        page: Page::Release(release.key.clone()),
                    }
                }
            }
            Item::Disc { disc, folder } => {
                if let Some(id) = disc.id.album() {
                    Destination::View(Location::Album {
                        root: disc.root,
                        id,
                        title: disc.title.clone(),
                    })
                } else {
                    Destination::FolderPage {
                        path: folder.clone(),
                        page: Page::Disc(disc.id.clone()),
                    }
                }
            }
            Item::Track { .. } => return None,
        })
    }
}

#[derive(Clone)]
enum TreeRow {
    Node {
        node: Node,
        parent: Option<TreeId>,
        depth: usize,
    },
    Message {
        parent: Option<TreeId>,
        depth: usize,
        text: String,
    },
}

impl TreeRow {
    fn node(&self) -> Option<&Node> {
        match self {
            Self::Node { node, .. } => Some(node),
            _ => None,
        }
    }

    fn parent(&self) -> Option<&TreeId> {
        match self {
            Self::Node { parent, .. } | Self::Message { parent, .. } => parent.as_ref(),
        }
    }
}

#[derive(Clone)]
enum Source {
    Folder { root: Option<RootId>, path: PathBuf },
    Album(i64, Arc<Disc>),
    Loaded(Vec<Node>),
    Offline,
}
impl Node {
    fn source(&self) -> Option<Source> {
        Some(match &self.item {
            Item::Collection {
                id, path, online, ..
            } => {
                if !online {
                    Source::Offline
                } else {
                    Source::Folder {
                        root: Some(*id),
                        path: path.clone(),
                    }
                }
            }
            Item::Folder { root, path } => Source::Folder {
                root: *root,
                path: path.clone(),
            },
            Item::Disc { disc, .. } => match disc.id {
                DiscId::Library(id) => Source::Album(id, disc.clone()),
                _ => Source::Loaded(
                    disc.tracks
                        .iter()
                        .cloned()
                        .map(|t| Self::track(t, Some(disc.clone())))
                        .collect(),
                ),
            },
            Item::Release { release, folder } => Source::Loaded(
                release
                    .discs
                    .iter()
                    .map(|disc| Self::disc(disc.clone(), folder.clone()))
                    .collect(),
            ),
            Item::Track { .. } => return None,
        })
    }
}

#[derive(Default)]
struct Branch {
    nodes: Vec<Node>,
    notice: Option<String>,
    limited: bool,
}

impl Branch {
    fn bounded(mut self) -> Self {
        if self.nodes.len() >= BRANCH_LIMIT {
            self.nodes.truncate(BRANCH_LIMIT);
            self.notice = Some("Showing the first 4,096 items at this level.".into());
        }
        self
    }
}

struct Request(Ticket);
impl Drop for Request {
    fn drop(&mut self) {
        self.0.next();
    }
}

#[derive(Default)]
struct Tree {
    expanded: Vec<TreeId>,
    branches: HashMap<Option<TreeId>, Branch>,
    pending: HashMap<Option<TreeId>, Request>,
    root: Option<Source>,
}

impl Tree {
    fn set_expanded(&mut self, list: Vec<TreeId>) {
        let mut seen = HashSet::new();
        self.expanded = list
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .take(EXPANSION_LIMIT)
            .collect();
    }

    fn toggle(&mut self, id: TreeId) {
        if let Some(index) = self.expanded.iter().position(|key| *key == id) {
            self.expanded.remove(index);
        } else {
            if self.expanded.len() == EXPANSION_LIMIT {
                self.expanded.remove(0);
            }
            self.expanded.push(id);
        }
        self.prune();
    }

    fn prune(&mut self) {
        fn visit(tree: &Tree, key: Option<TreeId>, reachable: &mut HashSet<Option<TreeId>>) {
            if !reachable.insert(key.clone()) {
                return;
            }
            if let Some(branch) = tree.branches.get(&key) {
                for node in &branch.nodes {
                    if tree.expanded.contains(&node.id) {
                        visit(tree, Some(node.id.clone()), reachable);
                    }
                }
            }
        }
        // Retry limited branches after a disclosure frees room.
        self.branches.retain(|_, branch| !branch.limited);
        let mut reachable = HashSet::new();
        visit(self, None, &mut reachable);
        self.branches.retain(|key, _| reachable.contains(key));
        self.pending.retain(|key, _| reachable.contains(key));
    }

    fn insert_branch(&mut self, id: Option<TreeId>, mut branch: Branch) {
        let loaded: usize = self
            .branches
            .values()
            .map(|branch| branch.nodes.len())
            .sum();
        let remaining = LOADED_NODE_LIMIT.saturating_sub(loaded);
        if branch.nodes.len() > remaining {
            branch.nodes.truncate(remaining);
            branch.limited = true;
            branch.notice = Some("Collapse another branch to make room for more items.".into());
        }
        self.branches.insert(id, branch);
    }

    fn next_request(&self) -> Option<(Option<TreeId>, Source)> {
        fn find(
            tree: &Tree,
            id: Option<TreeId>,
            node: Option<&Node>,
        ) -> Option<(Option<TreeId>, Source)> {
            if let Some(branch) = tree.branches.get(&id) {
                for node in &branch.nodes {
                    if tree.expanded.contains(&node.id)
                        && let Some(next) = find(tree, Some(node.id.clone()), Some(node))
                    {
                        return Some(next);
                    }
                }
                None
            } else if tree.pending.contains_key(&id) {
                None
            } else {
                Some((
                    id,
                    match node {
                        Some(node) => node.source()?,
                        None => tree.root.clone()?,
                    },
                ))
            }
        }
        if self.pending.len() >= PARALLEL_BRANCHES {
            return None;
        }
        find(self, None, None)
    }

    fn visible(
        &self,
        sort: Option<(Field, ColumnSort)>,
        filter: &str,
        marks: &dyn Fn(&Track) -> crate::store::Mark,
    ) -> Vec<TreeRow> {
        struct Projection<'a> {
            marks: &'a dyn Fn(&Track) -> crate::store::Mark,
            filter: &'a str,
            terms: Vec<crate::store::Term>,
            sort: Option<(Field, ColumnSort)>,
            rows: Vec<TreeRow>,
        }
        fn visit(
            tree: &Tree,
            parent: Option<TreeId>,
            depth: usize,
            inherited: bool,
            authored: bool,
            view: &mut Projection<'_>,
        ) {
            let Some(branch) = tree.branches.get(&parent) else {
                if inherited || view.filter.is_empty() {
                    view.rows.push(TreeRow::Message {
                        parent,
                        depth,
                        text: "Loading…".into(),
                    });
                }
                return;
            };
            let mut siblings: Vec<_> = branch.nodes.iter().collect();
            if let Some((field, direction)) = view.sort.filter(|_| !authored) {
                siblings.sort_by_cached_key(|node| {
                    let value = node.value(field);
                    let numeric = match &node.item {
                        Item::Track { track, .. } if field == Field::Length => track.duration,
                        Item::Disc { disc, .. } if field == Field::Length => Some(disc.duration()),
                        _ if matches!(field, Field::Number | Field::Count | Field::Year) => {
                            value.parse().ok()
                        }
                        _ => None,
                    };
                    let key = (
                        numeric.map_or(0, |value| value.max(0.).to_bits()),
                        value.to_lowercase(),
                    );
                    if direction == ColumnSort::Descending {
                        (None, Some(std::cmp::Reverse(key)))
                    } else {
                        (Some(key), None)
                    }
                });
            }
            for node in siblings {
                let matched =
                    inherited || view.filter.is_empty() || node.matches(&view.terms, view.marks);
                let start = view.rows.len();
                view.rows.push(TreeRow::Node {
                    node: node.clone(),
                    parent: parent.clone(),
                    depth,
                });
                if tree.expanded.contains(&node.id) {
                    visit(
                        tree,
                        Some(node.id.clone()),
                        depth + 1,
                        matched,
                        matches!(node.item, Item::Disc { .. } | Item::Release { .. }),
                        view,
                    );
                }
                if !matched && view.rows.len() == start + 1 {
                    view.rows.pop();
                }
            }
            if let Some(text) = &branch.notice {
                view.rows.push(TreeRow::Message {
                    parent,
                    depth,
                    text: text.clone(),
                });
            } else if branch.nodes.is_empty() && depth > 0 && (inherited || view.filter.is_empty())
            {
                view.rows.push(TreeRow::Message {
                    parent,
                    depth,
                    text: "No folders or supported recordings".into(),
                });
            }
        }

        let mut view = Projection {
            marks,
            filter,
            terms: crate::store::Term::parse(filter),
            sort,
            rows: Vec::new(),
        };
        visit(self, None, 0, false, false, &mut view);
        view.rows
    }
}

fn physical_branch(path: &Path, alive: &dyn Fn() -> bool) -> anyhow::Result<Branch> {
    let crate::discovery::FolderBranch {
        dirs,
        programs,
        unreadable,
    } = crate::discovery::tree_folder(path, alive)?;
    let mut nodes = dirs
        .into_iter()
        .map(|dir| Node::folder(None, dir))
        .collect::<Vec<_>>();
    for entry in Discs::shelve(programs) {
        match entry {
            Shelf::Release(release) => nodes.push(Node::release(release, path.to_owned())),
            Shelf::Disc(disc) if disc.release.is_some() => {}
            Shelf::Disc(disc) if disc.tracks.iter().any(|t| t.has_disc_context()) => {
                nodes.push(Node::disc(disc, path.to_owned()));
            }
            Shelf::Disc(disc) => nodes.extend(
                disc.tracks
                    .iter()
                    .filter(|t| !unreadable.contains(&t.source))
                    .cloned()
                    .map(|t| Node::track(t, None)),
            ),
            Shelf::Link(..) => {}
        }
    }
    Ok(Branch {
        nodes,
        notice: None,
        limited: false,
    }
    .bounded())
}

fn indexed_branch(
    store: &crate::store::Store,
    root: RootId,
    path: &Path,
) -> anyhow::Result<Branch> {
    let crate::store::query::IndexedBranch {
        dirs,
        programs,
        loose,
    } = store.tree_folder(root, path, BRANCH_LIMIT)?;
    let mut discs = programs
        .into_iter()
        .map(|a| Arc::new(Disc::from_album(a)))
        .collect::<Vec<_>>();
    tree_releases(&mut discs);
    let mut taken = HashSet::new();
    let mut contents = Vec::new();
    for entry in Discs::shelve(discs) {
        match entry {
            Shelf::Release(release) => {
                taken.extend(
                    release
                        .discs
                        .iter()
                        .map(|d| crate::store::path_key(&d.folder)),
                );
                contents.push(Node::release(release, path.to_owned()));
            }
            Shelf::Disc(disc)
                if disc.release.is_none() && crate::places::same_path(&disc.folder, path) =>
            {
                contents.push(Node::disc(disc, path.to_owned()));
            }
            _ => {}
        }
    }
    let mut nodes = dirs
        .into_iter()
        .filter(|dir| !taken.contains(&crate::store::path_key(dir)))
        .map(|dir| Node::folder(Some(root), dir))
        .collect::<Vec<_>>();
    nodes.extend(contents);
    nodes.extend(loose.into_iter().map(|t| Node::track(t, None)));
    Ok(Branch {
        nodes,
        notice: None,
        limited: false,
    }
    .bounded())
}

fn merge_immediate_folders(
    branch: &mut Branch,
    root: RootId,
    path: &Path,
    current: &dyn Fn() -> bool,
) {
    // The catalog has musical anchors. Empty and newly added directories can
    // still be browsed without reading tags or descending into them.
    let Ok(dirs) = crate::library::try_subfolders(path) else {
        return;
    };
    let mut seen = HashSet::new();
    for node in &branch.nodes {
        match &node.item {
            Item::Folder { path, .. } => {
                seen.insert(crate::store::path_key(path));
            }
            Item::Release { release, .. } => {
                seen.extend(
                    release
                        .discs
                        .iter()
                        .map(|disc| crate::store::path_key(&disc.folder)),
                );
            }
            _ => {}
        }
    }
    let mut extra = Vec::new();
    for dir in dirs {
        if !current() {
            return;
        }
        if seen.insert(crate::store::path_key(&dir)) {
            extra.push(Node::folder(Some(root), dir));
        }
        if branch.nodes.len() + extra.len() >= BRANCH_LIMIT {
            break;
        }
    }
    extra.append(&mut branch.nodes);
    branch.nodes = extra;
}

impl Browser {
    pub(crate) fn listing_tree(&self) -> bool {
        self.prefs.mode == ViewMode::Tree && self.has_presentations()
    }

    pub(crate) fn tree_expansion(&self, cx: &App) -> Vec<TreeId> {
        self.tree.read(cx).delegate().tree.expanded.clone()
    }

    pub(crate) fn reset_tree(&mut self, expanded: Vec<TreeId>, cx: &mut App) {
        self.tree.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.tree = Tree::default();
            delegate.tree.set_expanded(expanded);
            delegate.rows.clear();
            delegate.selected = None;
            delegate.restore = None;
            table.clear_selection(cx);
            crate::table::scroll_to_top(table);
            cx.notify();
        });
    }

    pub(crate) fn stop_tree_jobs(&mut self, cx: &mut App) {
        self.tree
            .update(cx, |table, _| table.delegate_mut().tree.pending.clear());
    }

    pub(crate) fn invalidate_tree(&mut self, cx: &mut Context<Self>) {
        self.tree.update(cx, |table, _| {
            let state = &mut table.delegate_mut().tree;
            state.root = None;
            state.branches.clear();
            state.pending.clear();
        });
        if self.listing_tree() {
            self.refresh_tree(cx);
        }
    }

    pub(crate) fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let source = match &self.location {
            Location::Folder(path) => Source::Folder {
                root: None,
                path: path.clone(),
            },
            Location::Library(Some(id)) => {
                let catalog = self.catalog.read(cx);
                match catalog.collection(*id) {
                    Some(c) if c.online => Source::Folder {
                        root: Some(*id),
                        path: c.root.path.clone(),
                    },
                    Some(_) => Source::Offline,
                    None if !catalog.loaded => return,
                    None => Source::Offline,
                }
            }
            Location::Library(None) => {
                let catalog = self.catalog.read(cx);
                if !catalog.loaded {
                    return;
                }
                Source::Loaded(
                    catalog
                        .collections
                        .iter()
                        .map(|c| Node {
                            id: TreeId::Collection(c.root.id),
                            item: Item::Collection {
                                id: c.root.id,
                                path: c.root.path.clone(),
                                name: c.root.name.clone(),
                                online: c.online,
                            },
                        })
                        .collect(),
                )
            }
            _ => return,
        };
        self.tree.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.tree.root.is_none() {
                delegate.tree.root = Some(source);
            }
            delegate.filter.clone_from(&self.query);
            TreeTable::project(table, cx);
        });
        self.pump_tree(cx);
        self.update_tree_playable(cx);
    }

    fn pump_tree(&mut self, cx: &mut Context<Self>) {
        while let Some((id, source)) = self.tree.read(cx).delegate().tree.next_request() {
            let ticket = Ticket::default();
            let generation = ticket.next();
            self.tree.update(cx, |table, _| {
                table
                    .delegate_mut()
                    .tree
                    .pending
                    .insert(id.clone(), Request(ticket.clone()));
            });
            let handle = self.catalog.read(cx).handle();
            let workers = self.tree.read(cx).delegate().workers.clone();
            cx.spawn(async move |browser, cx| {
                let result = match (source, handle) {
                    (Source::Loaded(nodes), _) => Ok(Branch {
                        nodes,
                        notice: None,
                        limited: false,
                    }
                    .bounded()),
                    (Source::Offline, _) => Err(anyhow::anyhow!("This collection is unavailable.")),
                    (
                        Source::Folder {
                            root: Some(root),
                            path,
                        },
                        Some(store),
                    ) => {
                        let indexed = path.clone();
                        let result = store
                            .query(&ticket, generation, move |s| {
                                indexed_branch(s, root, &indexed)
                            })
                            .await;
                        match result {
                            Ok(mut branch) => {
                                let work = ticket.clone();
                                cx.background_spawn(async move {
                                    let _slot = workers.acquire().await;
                                    let current = || work.is_current(generation);
                                    anyhow::ensure!(current(), crate::store::Cancelled);
                                    merge_immediate_folders(&mut branch, root, &path, &current);
                                    Ok(branch.bounded())
                                })
                                .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                    (Source::Album(id, disc), Some(store)) => {
                        store
                            .query(&ticket, generation, move |s| {
                                let scope = Scope::Album(id);
                                let keys = s.disc_keys(scope)?;
                                let rows = s.rows(scope, &keys[..keys.len().min(BRANCH_LIMIT)])?;
                                Ok(Branch {
                                    nodes: rows
                                        .into_iter()
                                        .filter(|row| row.available)
                                        .map(|row| Node::track(row.track, Some(disc.clone())))
                                        .collect(),
                                    notice: None,
                                    limited: false,
                                }
                                .bounded())
                            })
                            .await
                    }
                    (Source::Folder { root: None, path }, _) => {
                        let work = ticket.clone();
                        cx.background_spawn(async move {
                            let _slot = workers.acquire().await;
                            anyhow::ensure!(work.is_current(generation), crate::store::Cancelled);
                            physical_branch(&path, &|| work.is_current(generation))
                        })
                        .await
                    }
                    _ => Err(anyhow::anyhow!("The library is unavailable.")),
                };
                let _ = browser.update(cx, |b, cx| {
                    if !ticket.is_current(generation) || !b.listing_tree() {
                        return;
                    }
                    let branch = match result {
                        Ok(branch) => branch,
                        Err(err) => Branch {
                            nodes: Vec::new(),
                            notice: Some(format!("{err:#}")),
                            limited: false,
                        },
                    };
                    b.tree.update(cx, |table, cx| {
                        let state = &mut table.delegate_mut().tree;
                        state.pending.remove(&id);
                        state.insert_branch(id, branch);
                        TreeTable::project(table, cx);
                    });
                    b.pump_tree(cx);
                    b.update_tree_playable(cx);
                    b.finish_listing(cx);
                    cx.emit(ViewChanged);
                    cx.notify();
                });
                Ok::<_, anyhow::Error>(())
            })
            .detach();
        }
    }

    pub(crate) fn tree_visible_tracks(&self, cx: &App) -> crate::catalog::Targets {
        let rows = &self.tree.read(cx).delegate().rows;
        crate::catalog::Targets::Tracks(
            rows.iter()
                .filter_map(TreeRow::node)
                .filter_map(|node| match &node.item {
                    Item::Track { track, .. } => Some(track.clone()),
                    _ => None,
                })
                .collect(),
        )
    }

    fn tree_selection(&self, cx: &App) -> Option<Node> {
        let table = self.tree.read(cx);
        let rows = &table.delegate().rows;
        table
            .selected_row()
            .and_then(|index| rows.get(index))
            .and_then(TreeRow::node)
            .filter(|node| !node.expandable())
            .or_else(|| {
                rows.iter()
                    .filter_map(TreeRow::node)
                    .find(|node| !node.expandable())
            })
            .cloned()
    }

    pub(super) fn tree_can_start(&self, cx: &App) -> bool {
        self.tree_selection(cx).is_some()
    }

    pub(super) fn play_tree_selection(&self, cx: &mut App) {
        if let Some(node) = self.tree_selection(cx) {
            self.play_tree_node(node, false, cx);
        }
    }

    fn update_tree_playable(&self, cx: &mut Context<Self>) {
        if self.active {
            let can_start = self.tree_can_start(cx);
            let player = self.table.read(cx).delegate().player.clone();
            player.update(cx, |p, cx| {
                if p.can_start != can_start {
                    p.can_start = can_start;
                    cx.notify();
                }
            });
        }
    }

    pub(crate) fn activate_tree(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self
            .tree
            .read(cx)
            .delegate()
            .rows
            .get(index)
            .and_then(TreeRow::node)
            .cloned()
        else {
            return;
        };
        if let Some(destination) = node.destination(self.tree_root.clone()) {
            destination.apply(self, window, cx);
        } else {
            self.play_tree_node(node, false, cx);
        }
    }

    fn loose_tree_tracks(&self, chosen: &TreeId, cx: &App) -> TrackList {
        let rows = &self.tree.read(cx).delegate().rows;
        let Some(row) = rows
            .iter()
            .find(|row| row.node().is_some_and(|node| &node.id == chosen))
        else {
            return TrackList::default();
        };
        rows.iter()
            .filter(|sibling| sibling.parent() == row.parent())
            .filter_map(TreeRow::node)
            .filter_map(|node| match &node.item {
                Item::Track { track, disc: None } => Some(track.clone()),
                _ => None,
            })
            .collect()
    }

    fn play_tree_node(&self, node: Node, single: bool, cx: &mut App) {
        let Item::Track { track, disc } = node.item else {
            return;
        };
        let player = self.table.read(cx).delegate().player.clone();
        let program = disc.filter(|_| !single);
        if let Some(key) = track.library_id {
            if let Some(id) = program.as_ref().and_then(|disc| disc.id.album()) {
                play_library(
                    &self.catalog,
                    &player,
                    ListeningList::Disc(Scope::Album(id)),
                    Some(key),
                    |selection, index| Command::PlayDisc {
                        tracks: selection.tracks,
                        from_chapter: Some(index),
                    },
                    cx,
                );
            } else {
                let visible = if single {
                    Arc::from([key])
                } else {
                    self.loose_tree_tracks(&node.id, cx)
                        .iter()
                        .filter_map(|t| t.library_id)
                        .collect()
                };
                let keys = if visible.is_empty() {
                    Arc::from([key])
                } else {
                    visible
                };
                play_library(
                    &self.catalog,
                    &player,
                    ListeningList::Displayed(Scope::Library(None), keys),
                    Some(key),
                    Command::Play,
                    cx,
                );
            }
        } else if let Some(disc) = program {
            let tracks = disc.tracks.iter().cloned().collect::<TrackList>();
            if let Some(index) = tracks.iter().position(|item| item.same_item(&track)) {
                player.update(cx, |p, cx| {
                    p.command(
                        Command::PlayDisc {
                            tracks,
                            from_chapter: Some(index),
                        },
                        cx,
                    )
                });
            }
        } else {
            let visible = if single {
                TrackList::default()
            } else {
                self.loose_tree_tracks(&node.id, cx)
            };
            let (tracks, index) = match visible.iter().position(|item| item.same_item(&track)) {
                Some(index) => (visible, index),
                None => (TrackList::from([track]), 0),
            };
            player.update(cx, |p, cx| {
                p.command(Command::Play(tracks.into(), index), cx)
            });
        }
    }

    pub(crate) fn open_folder_page(
        &mut self,
        path: PathBuf,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(
            Route::new(Location::Folder(path), Some(page)),
            Arrival::Regular,
            window,
            cx,
        );
    }

    fn toggle_tree(&mut self, id: TreeId, cx: &mut Context<Self>) {
        self.tree.update(cx, |table, cx| {
            table.delegate_mut().tree.toggle(id);
            TreeTable::project(table, cx);
        });
        self.pump_tree(cx);
        self.update_tree_playable(cx);
        cx.emit(ViewChanged);
    }

    pub(crate) fn tree_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.modified()
            || !matches!(key, "left" | "right" | "enter" | "escape")
        {
            return;
        }
        if key == "escape" {
            self.tree.update(cx, |table, cx| {
                table.delegate_mut().clear_selection();
                table.clear_selection(cx);
                cx.notify();
            });
            cx.emit(ViewChanged);
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        let state = self.tree.read(cx);
        let Some(index) = state.selected_row() else {
            return;
        };
        let tree = state.delegate();
        let Some(row) = tree.rows.get(index) else {
            return;
        };
        let selected = row.node();
        let branch = selected.filter(|node| node.expandable());
        let expanded = branch.is_some_and(|node| tree.tree.expanded.contains(&node.id));
        match key {
            "enter" => self.activate_tree(index, window, cx),
            "right"
                if let Some(branch) = branch
                    && !expanded =>
            {
                self.toggle_tree(branch.id.clone(), cx)
            }
            "left"
                if let Some(branch) = branch
                    && expanded =>
            {
                self.toggle_tree(branch.id.clone(), cx)
            }
            "right"
                if let Some(node) = selected
                    && expanded =>
            {
                if tree
                    .rows
                    .get(index + 1)
                    .is_some_and(|child| child.parent() == Some(&node.id) && child.node().is_some())
                {
                    self.tree
                        .update(cx, |t, cx| t.set_selected_row(index + 1, cx));
                }
            }
            "left" => {
                let parent = row.parent().and_then(|parent| tree.position(parent));
                if let Some(index) = parent {
                    self.tree.update(cx, |t, cx| t.set_selected_row(index, cx));
                }
            }
            _ => {}
        }
        window.prevent_default();
        cx.stop_propagation();
    }
}

#[cfg(test)]
#[path = "treeTests.rs"]
mod tests;

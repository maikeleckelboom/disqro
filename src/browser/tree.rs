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
/// Items one request lists, reads and materializes for a branch.
const PAGE: usize = 4096;
/// Pages one branch keeps loaded. Loading another releases the farthest.
const RESIDENT_PAGES: usize = 4;
/// Rows beyond the viewport at which the next batch is requested.
pub(super) const LOOKAHEAD: usize = 512;
const PARALLEL_BRANCHES: usize = 4;
const LOADED_NODE_LIMIT: usize = 32_768;
/// Catalog folders with matching recordings that filtering resolves.
const MATCHED_FOLDERS: usize = 4096;

/// Folders of an indexed tree that hold recordings matching its filter,
/// with every ancestor, as path keys.
#[derive(Default)]
pub(super) struct Descendants {
    pub(super) query: String,
    pub(super) folders: HashSet<String>,
    pub(super) truncated: bool,
}

impl Descendants {
    fn new(query: String, matched: Vec<PathBuf>, truncated: bool) -> Self {
        let mut folders = HashSet::new();
        for folder in matched {
            let mut path = Some(folder.as_path());
            while let Some(current) = path
                && folders.insert(crate::store::path_key(current))
            {
                path = current.parent();
            }
        }
        Self {
            query,
            folders,
            truncated,
        }
    }

    fn contains(&self, node: &Node) -> bool {
        let key = |path: &Path| self.folders.contains(&crate::store::path_key(path));
        match &node.item {
            Item::Collection { path, .. } | Item::Folder { path, .. } => key(path),
            Item::Release { release, .. } => release.discs.iter().any(|d| key(&d.folder)),
            Item::Disc { disc, .. } => key(&disc.folder),
            Item::Track { .. } => false,
        }
    }
}

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
    /// A released item of a large branch. Its page loads again as the
    /// viewport approaches, so the rows keep their place in the list.
    Released { parent: Option<TreeId> },
}

impl TreeRow {
    fn node(&self) -> Option<&Node> {
        match self {
            Self::Node { node, .. } => Some(node),
            _ => None,
        }
    }

    pub(super) fn text(&self) -> Option<String> {
        match self {
            Self::Message { text, .. } => Some(text.clone()),
            _ => None,
        }
    }

    fn parent(&self) -> Option<&TreeId> {
        match self {
            Self::Node { parent, .. }
            | Self::Message { parent, .. }
            | Self::Released { parent, .. } => parent.as_ref(),
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

/// Where a branch's next page starts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cursor {
    /// A folder read from disk.
    Listing(crate::library::ListingCursor),
    /// A collection folder: its subfolders on disk, then its catalog contents.
    Indexed(IndexedCursor),
    /// An index into a list known in full, such as a disc's tracks.
    Offset(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum IndexedCursor {
    Dirs(String),
    Contents(crate::store::query::ContentsCursor),
}

/// One page of a branch. A released page keeps its length and start, so
/// its rows hold their place and it can be read again.
struct Slot {
    start: Option<Cursor>,
    len: usize,
    nodes: Option<Vec<Node>>,
}

/// A batch a branch waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Want {
    Next,
    Page(usize),
}

/// What a request reads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fetch {
    First,
    Next(Cursor),
    Page(usize, Option<Cursor>),
}

impl Fetch {
    fn cursor(&self) -> Option<&Cursor> {
        match self {
            Self::First => None,
            Self::Next(cursor) => Some(cursor),
            Self::Page(_, cursor) => cursor.as_ref(),
        }
    }
}

#[derive(Default)]
struct Branch {
    pages: Vec<Slot>,
    notice: Option<String>,
    limited: bool,
    /// Where the next page starts when more items follow.
    more: Option<Cursor>,
    wanted: Option<Want>,
}

impl Branch {
    fn page(nodes: Vec<Node>, more: Option<Cursor>) -> Self {
        Self {
            pages: vec![Slot {
                start: None,
                len: nodes.len(),
                nodes: Some(nodes),
            }],
            more,
            ..Default::default()
        }
    }

    /// One page of a list known in full.
    fn slice(nodes: &[Node], offset: usize) -> Self {
        let end = nodes.len().min(offset.saturating_add(PAGE));
        Self::page(
            nodes.get(offset..end).unwrap_or_default().to_vec(),
            (end < nodes.len()).then_some(Cursor::Offset(end)),
        )
    }

    /// The loaded items, in order.
    fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.pages
            .iter()
            .filter_map(|page| page.nodes.as_deref())
            .flatten()
    }

    fn resident(&self) -> usize {
        self.pages
            .iter()
            .filter_map(|page| page.nodes.as_ref())
            .map(Vec::len)
            .sum()
    }

    fn complete(&self) -> bool {
        self.more.is_none() && self.pages.iter().all(|page| page.nodes.is_some())
    }

    /// Releases the loaded page farthest from `keep`. Returns whether one was.
    fn release_farthest(&mut self, keep: usize) -> bool {
        let farthest = self
            .pages
            .iter()
            .enumerate()
            .filter(|(index, page)| *index != keep && page.nodes.is_some())
            .max_by_key(|(index, _)| index.abs_diff(keep))
            .map(|(index, _)| index);
        if let Some(index) = farthest {
            self.pages[index].nodes = None;
        }
        farthest.is_some()
    }
}

/// Rows whose batch loads once the viewport comes within `LOOKAHEAD` of them.
#[derive(Clone, Debug)]
pub(super) struct Boundary {
    pub(super) rows: std::ops::Range<usize>,
    pub(super) branch: Option<TreeId>,
    pub(super) want: Want,
}

impl Boundary {
    pub(super) fn near(&self, visible: &std::ops::Range<usize>) -> bool {
        self.rows.start < visible.end.saturating_add(LOOKAHEAD)
            && self.rows.end.saturating_add(LOOKAHEAD) > visible.start
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
                for node in branch.nodes() {
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

    fn loaded(&self) -> usize {
        self.branches.values().map(Branch::resident).sum()
    }

    fn insert_branch(&mut self, id: Option<TreeId>, mut branch: Branch) {
        let remaining = LOADED_NODE_LIMIT.saturating_sub(self.loaded());
        if let Some(page) = branch.pages.first_mut()
            && let Some(nodes) = &mut page.nodes
            && nodes.len() > remaining
        {
            nodes.truncate(remaining);
            page.len = nodes.len();
            branch.limited = true;
            branch.more = None;
            branch.notice = Some("Collapse another branch to make room for more items.".into());
        }
        self.branches.insert(id, branch);
    }

    /// Asks for a batch of a branch. Returns whether it is wanted now.
    fn want(&mut self, id: &Option<TreeId>, want: Want) -> bool {
        let pending = self.pending.contains_key(id);
        let Some(branch) = self.branches.get_mut(id) else {
            return false;
        };
        let valid = match want {
            Want::Next => branch.more.is_some(),
            Want::Page(index) => branch.pages.get(index).is_some_and(|p| p.nodes.is_none()),
        };
        if !valid || pending || branch.wanted.is_some() {
            return false;
        }
        branch.wanted = Some(want);
        true
    }

    /// Takes the batch a request read. A batch for a position the branch no
    /// longer waits at is stale and ignored. While browsing, a branch keeps
    /// `RESIDENT_PAGES` loaded and releases the farthest; a filter keeps
    /// every page it searched, within the loaded total.
    fn receive(
        &mut self,
        id: &Option<TreeId>,
        fetch: &Fetch,
        page: Branch,
        filtering: bool,
    ) -> bool {
        if *fetch == Fetch::First {
            self.insert_branch(id.clone(), page);
            return true;
        }
        let loaded = self.loaded();
        let Some(branch) = self.branches.get_mut(id) else {
            return false;
        };
        branch.wanted = None;
        let others = loaded - branch.resident();
        let nodes: Vec<Node> = page.nodes().cloned().collect();
        let keep = match fetch {
            Fetch::Next(cursor) if branch.more.as_ref() == Some(cursor) => {
                let known: HashSet<_> = branch.nodes().map(|node| node.id.clone()).collect();
                let nodes: Vec<_> = nodes
                    .into_iter()
                    .filter(|n| !known.contains(&n.id))
                    .collect();
                if filtering && loaded + nodes.len() > LOADED_NODE_LIMIT {
                    return true;
                }
                branch.pages.push(Slot {
                    start: Some(cursor.clone()),
                    len: nodes.len(),
                    nodes: Some(nodes),
                });
                branch.more = page.more;
                branch.pages.len() - 1
            }
            Fetch::Page(index, start)
                if branch
                    .pages
                    .get(*index)
                    .is_some_and(|slot| slot.start == *start && slot.nodes.is_none()) =>
            {
                let slot = &mut branch.pages[*index];
                slot.len = nodes.len();
                slot.nodes = Some(nodes);
                *index
            }
            _ => return false,
        };
        branch.notice = page.notice;
        if !filtering {
            while (branch.pages.iter().filter(|p| p.nodes.is_some()).count() > RESIDENT_PAGES
                || others + branch.resident() > LOADED_NODE_LIMIT)
                && branch.release_farthest(keep)
            {}
        }
        true
    }

    fn next_request(&self) -> Option<(Option<TreeId>, Source, Fetch)> {
        fn find(
            tree: &Tree,
            id: Option<TreeId>,
            node: Option<&Node>,
        ) -> Option<(Option<TreeId>, Source, Fetch)> {
            let source = || match node {
                Some(node) => node.source(),
                None => tree.root.clone(),
            };
            if let Some(branch) = tree.branches.get(&id) {
                if !tree.pending.contains_key(&id) {
                    let fetch = match branch.wanted {
                        Some(Want::Next) => branch.more.clone().map(Fetch::Next),
                        Some(Want::Page(index)) => branch
                            .pages
                            .get(index)
                            .map(|page| Fetch::Page(index, page.start.clone())),
                        None => None,
                    };
                    if let Some(fetch) = fetch {
                        return Some((id, source()?, fetch));
                    }
                }
                for node in branch.nodes() {
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
                Some((id, source()?, Fetch::First))
            }
        }
        if self.pending.len() >= PARALLEL_BRANCHES {
            return None;
        }
        find(self, None, None)
    }

    /// The rows to show. A filter keeps matching items and the ancestors of
    /// matches in loaded branches. With `descendants` from the catalog it also
    /// keeps folders whose unloaded contents match.
    #[cfg(test)]
    fn visible(
        &self,
        sort: Option<(Field, ColumnSort)>,
        filter: &str,
        marks: &dyn Fn(&Track) -> crate::store::Mark,
    ) -> Vec<TreeRow> {
        self.visible_with(sort, filter, None, marks).0
    }

    fn visible_with(
        &self,
        sort: Option<(Field, ColumnSort)>,
        filter: &str,
        descendants: Option<&Descendants>,
        marks: &dyn Fn(&Track) -> crate::store::Mark,
    ) -> (Vec<TreeRow>, Vec<Boundary>) {
        struct Projection<'a> {
            boundaries: Vec<Boundary>,
            marks: &'a dyn Fn(&Track) -> crate::store::Mark,
            descendants: Option<&'a Descendants>,
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
            let filtering = !view.filter.is_empty();
            // Only a complete branch can be sorted. A partial one keeps the
            // order it is read in, so later batches extend it in place.
            let sort = view.sort.filter(|_| !authored && branch.complete());
            let mut siblings: Vec<Result<&Node, (usize, usize)>> = Vec::new();
            for (index, page) in branch.pages.iter().enumerate() {
                match &page.nodes {
                    Some(nodes) => siblings.extend(nodes.iter().map(Ok)),
                    None if !filtering => siblings.push(Err((index, page.len))),
                    // A filter reads released pages of open branches again.
                    None => view.boundaries.push(Boundary {
                        rows: 0..usize::MAX,
                        branch: parent.clone(),
                        want: Want::Page(index),
                    }),
                }
            }
            if let Some((field, direction)) = sort {
                let mut siblings_sorted: Vec<&Node> =
                    siblings.iter().filter_map(|s| s.ok()).collect();
                siblings_sorted.sort_by_cached_key(|node| {
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
                siblings = siblings_sorted.into_iter().map(Ok).collect();
            }
            for sibling in siblings {
                let node = match sibling {
                    Ok(node) => node,
                    Err((page, len)) => {
                        view.boundaries.push(Boundary {
                            rows: view.rows.len()..view.rows.len() + len,
                            branch: parent.clone(),
                            want: Want::Page(page),
                        });
                        view.rows.extend((0..len).map(|_| TreeRow::Released {
                            parent: parent.clone(),
                        }));
                        continue;
                    }
                };
                let matched =
                    inherited || view.filter.is_empty() || node.matches(&view.terms, view.marks);
                let holds_matches = !matched && view.descendants.is_some_and(|d| d.contains(node));
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
                if !matched && !holds_matches && view.rows.len() == start + 1 {
                    view.rows.pop();
                }
            }
            if branch.more.is_some() {
                // A filter keeps reading the branch; browsing reads ahead of
                // the viewport.
                let end = view.rows.len();
                view.boundaries.push(Boundary {
                    rows: if filtering {
                        0..usize::MAX
                    } else {
                        end.saturating_sub(1)..end
                    },
                    branch: parent.clone(),
                    want: Want::Next,
                });
            }
            if let Some(text) = &branch.notice {
                view.rows.push(TreeRow::Message {
                    parent,
                    depth,
                    text: text.clone(),
                });
            } else if branch.pages.iter().all(|page| page.len == 0)
                && branch.more.is_none()
                && depth > 0
                && (inherited || view.filter.is_empty())
            {
                view.rows.push(TreeRow::Message {
                    parent,
                    depth,
                    text: "No folders or supported recordings".into(),
                });
            }
        }

        let mut view = Projection {
            boundaries: Vec::new(),
            marks,
            descendants,
            filter,
            terms: crate::store::Term::parse(filter),
            sort,
            rows: Vec::new(),
        };
        visit(self, None, 0, false, false, &mut view);
        (view.rows, view.boundaries)
    }
}

/// One page of a folder read from disk.
fn physical_branch(
    path: &Path,
    after: Option<&crate::library::ListingCursor>,
    alive: &dyn Fn() -> bool,
) -> anyhow::Result<Branch> {
    let crate::discovery::FolderBranch {
        dirs,
        programs,
        unreadable,
        next,
        paged,
        notice,
    } = crate::discovery::tree_folder(path, after, PAGE, alive)?;
    let mut nodes = dirs
        .into_iter()
        .map(|dir| Node::folder(None, dir))
        .collect::<Vec<_>>();
    for entry in Discs::shelve(programs) {
        match entry {
            Shelf::Release(release) if !paged => {
                nodes.push(Node::release(release, path.to_owned()))
            }
            Shelf::Disc(disc) if !paged && disc.release.is_some() => {}
            // A page of a large folder groups only authored programs: an
            // album by tags could continue on another page.
            Shelf::Disc(disc)
                if disc.tracks.iter().any(|t| t.has_disc_context())
                    && (!paged || disc.tracks.iter().all(|t| t.disc.is_some())) =>
            {
                nodes.push(Node::disc(disc, path.to_owned()));
            }
            Shelf::Disc(disc) => nodes.extend(
                disc.tracks
                    .iter()
                    .filter(|t| !unreadable.contains(&t.source))
                    .cloned()
                    .map(|t| Node::track(t, None)),
            ),
            Shelf::Release(release) => nodes.extend(release.discs.iter().flat_map(|disc| {
                disc.tracks
                    .iter()
                    .cloned()
                    .map(|t| Node::track(t, None))
                    .collect::<Vec<_>>()
            })),
            Shelf::Link(..) => {}
        }
    }
    let mut branch = Branch::page(nodes, next.map(Cursor::Listing));
    branch.notice = notice;
    Ok(branch)
}

/// A page of a collection folder's subfolders, read from disk so that empty
/// and newly added folders can be browsed without reading tags.
fn indexed_dirs(
    root: RootId,
    path: &Path,
    after: Option<&str>,
    alive: &dyn Fn() -> bool,
) -> anyhow::Result<(Vec<Node>, Option<Cursor>)> {
    let after = after.map(|name| crate::library::ListingCursor::Dirs(name.to_owned()));
    let listing = crate::library::list_page(path, after.as_ref(), PAGE, false, alive)?;
    Ok((
        listing
            .dirs
            .into_iter()
            .map(|dir| Node::folder(Some(root), dir))
            .collect(),
        listing
            .next_dir
            .map(|name| Cursor::Indexed(IndexedCursor::Dirs(name))),
    ))
}

/// A page of the programs and loose recordings a collection folder holds.
/// Returns the disc folders its releases include, which are not listed as
/// folders.
fn indexed_contents(
    store: &crate::store::Store,
    root: RootId,
    path: &Path,
    at: crate::store::query::ContentsCursor,
) -> anyhow::Result<(Vec<Node>, HashSet<String>, Option<Cursor>)> {
    let crate::store::query::IndexedBranch {
        programs,
        loose,
        next,
    } = store.tree_folder(root, path, at, PAGE)?;
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
    contents.extend(loose.into_iter().map(|t| Node::track(t, None)));
    Ok((
        contents,
        taken,
        next.map(|at| Cursor::Indexed(IndexedCursor::Contents(at))),
    ))
}

/// Subfolders first, then once they are all listed, the folder's own
/// recordings. Folders a release includes are shown inside the release.
fn indexed_page(
    dirs: Vec<Node>,
    contents: Option<(Vec<Node>, HashSet<String>)>,
    more: Option<Cursor>,
) -> Branch {
    let Some((contents, taken)) = contents else {
        return Branch::page(dirs, more);
    };
    let mut nodes: Vec<_> = dirs
        .into_iter()
        .filter(|node| match &node.item {
            Item::Folder { path, .. } => !taken.contains(&crate::store::path_key(path)),
            _ => true,
        })
        .collect();
    nodes.extend(contents);
    Branch::page(nodes, more)
}

#[cfg(test)]
fn indexed_branch(
    store: &crate::store::Store,
    root: RootId,
    path: &Path,
) -> anyhow::Result<Branch> {
    let (dirs, more) = indexed_dirs(root, path, None, &|| true)?;
    if more.is_some() {
        return Ok(indexed_page(dirs, None, more));
    }
    let (contents, taken, more) = indexed_contents(store, root, path, Default::default())?;
    Ok(indexed_page(dirs, Some((contents, taken)), more))
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
        self.refresh_tree_matches(cx);
        self.pump_tree(cx);
        self.update_tree_playable(cx);
    }

    /// In an indexed tree, asks the catalog which folders hold recordings
    /// matching the filter, so folders whose contents are not loaded yet
    /// stay listed. Folders on disk are never scanned for a filter.
    fn refresh_tree_matches(&mut self, cx: &mut Context<Self>) {
        let root = match &self.location {
            Location::Library(root) => *root,
            _ => {
                self.tree
                    .update(cx, |table, _| table.delegate_mut().descendants = None);
                return;
            }
        };
        let query = self.query.clone();
        let table = self.tree.read(cx).delegate();
        if query.trim().is_empty()
            || table
                .descendants
                .as_ref()
                .is_some_and(|matches| matches.query == query)
        {
            return;
        }
        let ticket = table.matching.clone();
        let generation = ticket.next();
        let Some(store) = self.catalog.read(cx).handle() else {
            return;
        };
        cx.spawn(async move |browser, cx| {
            let text = query.clone();
            let result = store
                .query(&ticket, generation, move |s| {
                    s.tree_matches(root, &text, MATCHED_FOLDERS)
                })
                .await;
            let Ok((folders, truncated)) = result else {
                return;
            };
            let _ = browser.update(cx, |b, cx| {
                if !ticket.is_current(generation) {
                    return;
                }
                b.tree.update(cx, |table, cx| {
                    table.delegate_mut().descendants =
                        Some(Arc::new(Descendants::new(query, folders, truncated)));
                    TreeTable::project(table, cx);
                });
                cx.emit(ViewChanged);
            });
        })
        .detach();
    }

    fn pump_tree(&mut self, cx: &mut Context<Self>) {
        while let Some((id, source, fetch)) = self.tree.read(cx).delegate().tree.next_request() {
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
            let cursor = fetch.cursor().cloned();
            cx.spawn(async move |browser, cx| {
                let result = match (source, handle) {
                    (Source::Loaded(nodes), _) => Ok(Branch::slice(
                        &nodes,
                        match cursor {
                            Some(Cursor::Offset(offset)) => offset,
                            _ => 0,
                        },
                    )),
                    (Source::Offline, _) => Err(anyhow::anyhow!("This collection is unavailable.")),
                    (
                        Source::Folder {
                            root: Some(root),
                            path,
                        },
                        Some(store),
                    ) => {
                        let contents_at = match &cursor {
                            Some(Cursor::Indexed(IndexedCursor::Contents(at))) => Some(*at),
                            _ => None,
                        };
                        // Subfolders are read from disk on a worker, never on
                        // the catalog thread.
                        let dirs = match (contents_at, &cursor) {
                            (Some(_), _) => Ok((Vec::new(), None)),
                            (None, cursor) => {
                                let after = match cursor {
                                    Some(Cursor::Indexed(IndexedCursor::Dirs(name))) => {
                                        Some(name.clone())
                                    }
                                    _ => None,
                                };
                                let (work, path) = (ticket.clone(), path.clone());
                                cx.background_spawn(async move {
                                    let _slot = workers.acquire().await;
                                    let current = || work.is_current(generation);
                                    anyhow::ensure!(current(), crate::store::Cancelled);
                                    // An unreadable folder still lists its
                                    // indexed recordings.
                                    Ok(indexed_dirs(root, &path, after.as_deref(), &current)
                                        .unwrap_or_default())
                                })
                                .await
                            }
                        };
                        match dirs {
                            Ok((dirs, Some(more))) => Ok(indexed_page(dirs, None, Some(more))),
                            Ok((dirs, None)) => {
                                store
                                    .query(&ticket, generation, move |s| {
                                        let (contents, taken, more) = indexed_contents(
                                            s,
                                            root,
                                            &path,
                                            contents_at.unwrap_or_default(),
                                        )?;
                                        Ok(indexed_page(dirs, Some((contents, taken)), more))
                                    })
                                    .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                    (Source::Album(id, disc), Some(store)) => {
                        let offset = match cursor {
                            Some(Cursor::Offset(offset)) => offset,
                            _ => 0,
                        };
                        store
                            .query(&ticket, generation, move |s| {
                                let scope = Scope::Album(id);
                                let keys = s.disc_keys(scope)?;
                                let end = keys.len().min(offset.saturating_add(PAGE));
                                let rows =
                                    s.rows(scope, keys.get(offset..end).unwrap_or_default())?;
                                Ok(Branch::page(
                                    rows.into_iter()
                                        .filter(|row| row.available)
                                        .map(|row| Node::track(row.track, Some(disc.clone())))
                                        .collect(),
                                    (end < keys.len()).then_some(Cursor::Offset(end)),
                                ))
                            })
                            .await
                    }
                    (Source::Folder { root: None, path }, _) => {
                        let work = ticket.clone();
                        let after = match cursor {
                            Some(Cursor::Listing(after)) => Some(after),
                            _ => None,
                        };
                        cx.background_spawn(async move {
                            let _slot = workers.acquire().await;
                            anyhow::ensure!(work.is_current(generation), crate::store::Cancelled);
                            physical_branch(&path, after.as_ref(), &|| work.is_current(generation))
                        })
                        .await
                    }
                    _ => Err(anyhow::anyhow!("The library is unavailable.")),
                };
                let _ = browser.update(cx, |b, cx| {
                    if !ticket.is_current(generation) || !b.listing_tree() {
                        return;
                    }
                    b.tree.update(cx, |table, cx| {
                        let filtering = !table.delegate().filter.is_empty();
                        let state = &mut table.delegate_mut().tree;
                        state.pending.remove(&id);
                        match (&fetch, result) {
                            (Fetch::First, result) => state.insert_branch(
                                id,
                                result.unwrap_or_else(|err| Branch {
                                    notice: Some(format!("{err:#}")),
                                    ..Default::default()
                                }),
                            ),
                            (fetch, Ok(page)) => {
                                state.receive(&id, fetch, page, filtering);
                            }
                            (_, Err(err)) => {
                                if let Some(branch) = state.branches.get_mut(&id) {
                                    branch.wanted = None;
                                    branch.notice = Some(format!("{err:#}"));
                                }
                            }
                        }
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

    /// What the filter searches in Tree view, when that differs from the
    /// whole view: open folders on disk, or a capped set of catalog folders.
    pub(crate) fn tree_filter_scope(&self, cx: &App) -> Option<&'static str> {
        if !self.listing_tree() {
            return None;
        }
        match &self.location {
            Location::Folder(_) => Some("Filter open folders · Unopened folders are not searched"),
            Location::Library(_)
                if self
                    .tree
                    .read(cx)
                    .delegate()
                    .descendants
                    .as_ref()
                    .is_some_and(|matches| matches.truncated && matches.query == self.query) =>
            {
                Some("Filter · Showing the first 4,096 folders with matches")
            }
            _ => None,
        }
    }

    /// Requests the batches the viewport approaches. Loading is driven by
    /// the table, never by the listener.
    pub(super) fn load_tree(&mut self, wants: Vec<(Option<TreeId>, Want)>, cx: &mut Context<Self>) {
        let mut any = false;
        self.tree.update(cx, |table, _| {
            let tree = &mut table.delegate_mut().tree;
            for (branch, want) in wants {
                any |= tree.want(&branch, want);
            }
        });
        if any {
            self.pump_tree(cx);
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
        let row = self.tree.read(cx).delegate().rows.get(index).cloned();
        let Some(node) = row.as_ref().and_then(TreeRow::node).cloned() else {
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

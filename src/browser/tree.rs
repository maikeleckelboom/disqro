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

mod reveal;
mod table;
pub(super) use reveal::recording_identity;
pub(crate) use table::TreeTable;

const EXPANSION_LIMIT: usize = 128;
/// Items collapsed inside expanded subtrees that the tree remembers.
const CLOSED_LIMIT: usize = 4096;
/// Items one request lists, reads and materializes for a branch.
const PAGE: usize = 4096;
/// Pages one branch keeps loaded while browsing. Loading another releases
/// the farthest.
const RESIDENT_PAGES: usize = 4;
/// Rows beyond the viewport at which the next batch is requested.
pub(super) const LOOKAHEAD: usize = 512;
const PARALLEL_BRANCHES: usize = 4;
/// Items held across every branch: loaded pages, the filter matches of
/// released pages and the open items that keep their place. Pages the
/// viewport is not near are released first, so the total bounds memory,
/// never what can be reached.
const LOADED_NODE_LIMIT: usize = 32_768;
/// Items a branch may hold to be sorted as a whole. A larger branch keeps
/// the order it is read in, and says so.
const SORTED_BRANCH_LIMIT: usize = 24_576;
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

/// What a filter keeps of a branch.
struct Lens<'a> {
    query: &'a str,
    terms: Vec<crate::store::Term>,
    marks: &'a dyn Fn(&Track) -> crate::store::Mark,
    descendants: Option<&'a Descendants>,
}

impl<'a> Lens<'a> {
    /// A lens for `query`, or none when nothing is filtered.
    fn new(
        query: &'a str,
        descendants: Option<&'a Descendants>,
        marks: &'a dyn Fn(&Track) -> crate::store::Mark,
    ) -> Option<Self> {
        (!query.trim().is_empty()).then(|| Self {
            query,
            terms: crate::store::Term::parse(query),
            marks,
            descendants: descendants.filter(|d| d.query == query),
        })
    }

    fn matches(&self, node: &Node) -> bool {
        node.matches(&self.terms, self.marks)
    }

    /// Whether the catalog knows of matches inside an item not yet read.
    fn holds(&self, node: &Node) -> bool {
        self.descendants.is_some_and(|d| d.contains(node))
    }

    /// Whether a filtered view can show `node`, or reach a match through it.
    fn keeps(&self, node: &Node, open: bool) -> bool {
        open || self.matches(node) || self.holds(node)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
        /// Whether the item is expanded.
        open: bool,
    },
    Message {
        parent: Option<TreeId>,
        depth: usize,
        text: String,
    },
    /// Consecutive items of a released page, held as one entry however many
    /// rows they span. The page loads again as the viewport approaches, so
    /// the items keep their place in the list.
    Released {
        parent: Option<TreeId>,
        count: usize,
    },
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

    /// Whether the entry is an expanded item.
    fn open(&self) -> bool {
        matches!(self, Self::Node { open: true, .. })
    }

    /// The rows this entry spans.
    fn span(&self) -> usize {
        match self {
            Self::Released { count, .. } => *count,
            _ => 1,
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
    /// The open items of a released page, by position, so their branches
    /// keep their rows and stay reachable.
    anchors: Vec<(usize, Node)>,
    /// What a filter keeps of a released page. A filter reads each page
    /// once and needs only these to stay loaded.
    hits: Option<Hits>,
    /// When the viewport was last near the page.
    used: u64,
}

struct Hits {
    query: String,
    nodes: Vec<(usize, Node)>,
}

impl Slot {
    fn new(start: Option<Cursor>, nodes: Vec<Node>) -> Self {
        Self {
            start,
            len: nodes.len(),
            nodes: Some(nodes),
            anchors: Vec::new(),
            hits: None,
            used: 0,
        }
    }

    /// Items the page holds in memory.
    fn resident(&self) -> usize {
        self.nodes.as_ref().map_or(0, Vec::len)
            + self.anchors.len()
            + self.hits.as_ref().map_or(0, |hits| hits.nodes.len())
    }

    fn hits(&self, query: &str) -> Option<&[(usize, Node)]> {
        self.hits
            .as_ref()
            .filter(|hits| hits.query == query)
            .map(|hits| hits.nodes.as_slice())
    }

    /// Fills the page with its items again.
    fn load(&mut self, nodes: Vec<Node>) {
        self.len = nodes.len();
        self.nodes = Some(nodes);
        self.anchors.clear();
        self.hits = None;
    }

    /// Drops the page's items. Open items stay as anchors and, under a
    /// filter, so do the items it keeps.
    fn release(&mut self, open: &dyn Fn(&TreeId) -> bool, lens: Option<&Lens>) {
        let Some(nodes) = self.nodes.take() else {
            return;
        };
        self.anchors = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| open(&node.id))
            .map(|(index, node)| (index, node.clone()))
            .collect();
        self.hits = lens.map(|lens| Hits {
            query: lens.query.to_owned(),
            nodes: nodes
                .into_iter()
                .enumerate()
                .filter(|(_, node)| !open(&node.id) && lens.keeps(node, false))
                .collect(),
        });
    }
}

/// A batch a branch waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Want {
    /// The first page of an open branch inside an expanded subtree.
    First,
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
    /// Where the next page starts when more items follow.
    more: Option<Cursor>,
    /// The batch the branch waits for, and whether a filter searches it
    /// rather than the viewport showing it.
    wanted: Option<(Want, bool)>,
    /// Too large to sort as a whole, so shown in the order it is read.
    unsorted: bool,
    /// Its items are open unless collapsed, inside an expanded subtree.
    deep: bool,
}

impl Branch {
    fn page(nodes: Vec<Node>, more: Option<Cursor>) -> Self {
        Self {
            pages: vec![Slot::new(None, nodes)],
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

    /// Items that can be open: every loaded item and the anchors of
    /// released pages.
    fn reachable(&self) -> impl Iterator<Item = &Node> {
        self.pages.iter().flat_map(|page| {
            page.nodes
                .iter()
                .flatten()
                .chain(page.anchors.iter().map(|(_, node)| node))
        })
    }

    fn resident(&self) -> usize {
        self.pages.iter().map(Slot::resident).sum()
    }

    /// Items known so far, loaded or released.
    fn len(&self) -> usize {
        self.pages.iter().map(|page| page.len).sum()
    }

    fn loaded_pages(&self) -> usize {
        self.pages
            .iter()
            .filter(|page| page.nodes.is_some())
            .count()
    }

    fn complete(&self) -> bool {
        self.more.is_none() && self.pages.iter().all(|page| page.nodes.is_some())
    }

    /// The least recently used page. Break ties by distance from `keep`.
    fn least_recent(&self, keep: usize) -> Option<usize> {
        self.pages
            .iter()
            .enumerate()
            .filter(|(index, page)| *index != keep && page.nodes.is_some())
            .min_by_key(|(index, page)| (page.used, std::cmp::Reverse(index.abs_diff(keep))))
            .map(|(index, _)| index)
    }
}

/// Rows whose batch loads once the viewport comes within `LOOKAHEAD` of them.
#[derive(Clone, Debug)]
pub(super) struct Boundary {
    pub(super) rows: std::ops::Range<usize>,
    pub(super) branch: Option<TreeId>,
    pub(super) want: Want,
    /// A filter reads this batch wherever the viewport is.
    pub(super) search: bool,
}

impl Boundary {
    pub(super) fn near(&self, visible: &std::ops::Range<usize>) -> bool {
        self.rows.start < visible.end.saturating_add(LOOKAHEAD)
            && self.rows.end.saturating_add(LOOKAHEAD) > visible.start
    }
}

/// The rows a page's items occupy, so the pages the viewport approaches
/// stay loaded.
#[derive(Clone, Debug)]
struct Span {
    branch: Option<TreeId>,
    page: usize,
    rows: std::ops::Range<usize>,
}

struct Request(Ticket);
impl Drop for Request {
    fn drop(&mut self) {
        self.0.next();
    }
}

/// A change to what is expanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExpandAction {
    /// One item, leaving what is open inside it as it was.
    Toggle,
    /// An item and everything inside it.
    ExpandAll,
    /// An item, forgetting everything open inside it.
    CollapseAll,
}

/// What is expanded in a tree. Expand All marks a subtree rather than
/// listing its items, so items nobody has read yet open as they load.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Expansion {
    /// Items opened one at a time, oldest first.
    pub(crate) open: Vec<TreeId>,
    /// Subtrees whose items are all open; `None` is the whole tree.
    pub(crate) deep: Vec<Option<TreeId>>,
    /// Items collapsed inside an expanded subtree.
    pub(crate) closed: Vec<TreeId>,
    /// The parent of each item above, so a subtree can forget what is open
    /// inside it before it is read.
    pub(crate) links: Vec<(TreeId, Option<TreeId>)>,
}

#[derive(Default)]
struct Tree {
    expanded: Vec<TreeId>,
    deep: HashSet<Option<TreeId>>,
    closed: HashSet<TreeId>,
    /// The parent of every open item and loaded branch.
    links: HashMap<TreeId, Option<TreeId>>,
    /// Open branches inside expanded subtrees that the viewport has asked
    /// to read. The rest wait until it comes near.
    first: HashSet<TreeId>,
    branches: HashMap<Option<TreeId>, Branch>,
    pending: HashMap<Option<TreeId>, Request>,
    root: Option<Source>,
    sort: Option<(Field, ColumnSort)>,
    /// The filter whose matches released pages keep.
    query: String,
    /// The filter's matches fill the loaded total, so it reads no further.
    saturated: bool,
    /// Advances whenever the viewport moves, to order pages by use.
    clock: u64,
    /// Exact rows resolved by identity beyond the pages currently held.
    /// Normal paging merges them in once it reaches their position.
    direct: HashMap<Option<TreeId>, Node>,
}

impl Tree {
    fn set_expansion(&mut self, expansion: Expansion) {
        let mut seen = HashSet::new();
        self.expanded = expansion
            .open
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .take(EXPANSION_LIMIT)
            .collect();
        self.deep = expansion.deep.into_iter().take(EXPANSION_LIMIT).collect();
        self.closed = expansion.closed.into_iter().take(CLOSED_LIMIT).collect();
        self.links = expansion.links.into_iter().collect();
        self.first.clear();
    }

    fn expansion(&self) -> Expansion {
        let mut deep: Vec<_> = self.deep.iter().cloned().collect();
        deep.sort();
        let mut closed: Vec<_> = self.closed.iter().cloned().collect();
        closed.sort();
        let links = self
            .expanded
            .iter()
            .chain(deep.iter().flatten())
            .chain(&closed)
            .filter_map(|id| Some((id.clone(), self.links.get(id)?.clone())))
            .collect();
        Expansion {
            open: self.expanded.clone(),
            deep,
            closed,
            links,
        }
    }

    /// Whether an item inside a subtree that is expanded throughout (`deep`)
    /// opens unless collapsed.
    fn opens_by_default(&self, id: &TreeId, deep: bool) -> bool {
        deep || self.deep.contains(&Some(id.clone()))
    }

    /// Whether item `id` of a branch is open. `deep` tells whether that
    /// branch lies inside an expanded subtree.
    fn is_open(&self, id: &TreeId, deep: bool) -> bool {
        !self.closed.contains(id) && (self.expanded.contains(id) || self.opens_by_default(id, deep))
    }

    /// Whether the items of branch `id` open unless collapsed.
    fn branch_deep(&self, id: &Option<TreeId>) -> bool {
        self.deep.contains(id) || self.branches.get(id).is_some_and(|branch| branch.deep)
    }

    /// Expands or collapses one item of branch `parent`, leaving what is
    /// open inside it as it was.
    fn toggle(&mut self, id: TreeId, parent: &Option<TreeId>) {
        let deep = self.branch_deep(parent);
        self.links.insert(id.clone(), parent.clone());
        if self.is_open(&id, deep) {
            self.expanded.retain(|key| *key != id);
            if self.opens_by_default(&id, deep) && self.closed.len() < CLOSED_LIMIT {
                self.closed.insert(id);
            }
        } else {
            self.closed.remove(&id);
            if !self.opens_by_default(&id, deep) {
                if self.expanded.len() == EXPANSION_LIMIT {
                    self.expanded.remove(0);
                }
                self.expanded.push(id);
            }
        }
        self.reconcile();
    }

    /// Whether `id` lies inside item `ancestor`, by the parents the tree has
    /// seen, or for folders by their paths.
    fn descends(&self, id: &TreeId, ancestor: &TreeId) -> bool {
        let mut current = id;
        for _ in 0..256 {
            match self.links.get(current) {
                Some(Some(parent)) if parent == ancestor => return true,
                Some(Some(parent)) => current = parent,
                _ => break,
            }
        }
        match (id, ancestor) {
            (TreeId::Folder(_, path), TreeId::Folder(_, above)) => path
                .strip_prefix(above.as_str())
                .is_some_and(|rest| rest.starts_with(['\\', '/'])),
            _ => false,
        }
    }

    /// Expands `id` and everything inside it, or the whole tree for `None`.
    /// Nothing is read until the viewport comes near it.
    fn expand_all(&mut self, id: Option<TreeId>, parent: &Option<TreeId>) {
        match &id {
            None => {
                self.closed.clear();
            }
            Some(item) => {
                self.links.insert(item.clone(), parent.clone());
                self.closed.remove(item);
                let inside: Vec<_> = self
                    .closed
                    .iter()
                    .filter(|key| self.descends(key, item))
                    .cloned()
                    .collect();
                for key in inside {
                    self.closed.remove(&key);
                }
            }
        }
        if self.deep.len() < EXPANSION_LIMIT || id.is_none() {
            if id.is_none() {
                self.deep.clear();
            }
            self.deep.insert(id);
        }
        self.reconcile();
    }

    /// Collapses `id` and forgets everything open inside it, read or not,
    /// or collapses the whole tree for `None`.
    fn collapse_all(&mut self, id: Option<TreeId>, parent: &Option<TreeId>) {
        let Some(item) = id else {
            self.expanded.clear();
            self.deep.clear();
            self.closed.clear();
            self.first.clear();
            self.reconcile();
            return;
        };
        let inside = |tree: &Tree, key: &TreeId| tree.descends(key, &item);
        let expanded: Vec<_> = self
            .expanded
            .iter()
            .filter(|key| !inside(self, key) && **key != item)
            .cloned()
            .collect();
        let deep: HashSet<_> = self
            .deep
            .iter()
            .filter(|key| {
                key.as_ref()
                    .is_none_or(|key| !inside(self, key) && *key != item)
            })
            .cloned()
            .collect();
        let closed: HashSet<_> = self
            .closed
            .iter()
            .filter(|key| !inside(self, key))
            .cloned()
            .collect();
        (self.expanded, self.deep, self.closed) = (expanded, deep, closed);
        let first: HashSet<_> = self
            .first
            .iter()
            .filter(|key| !inside(self, key))
            .cloned()
            .collect();
        self.first = first;
        self.links.insert(item.clone(), parent.clone());
        if self.opens_by_default(&item, self.branch_deep(parent)) {
            self.closed.insert(item);
        }
        self.reconcile();
    }

    /// Whether anything inside `id` (the whole tree for `None`) is open.
    fn has_open_inside(&self, id: &Option<TreeId>) -> bool {
        match id {
            None => !self.expanded.is_empty() || !self.deep.is_empty(),
            Some(item) => {
                self.deep.contains(id)
                    || self.expanded.iter().any(|key| self.descends(key, item))
                    || self
                        .deep
                        .iter()
                        .flatten()
                        .any(|key| self.descends(key, item))
            }
        }
    }

    /// Whether `id` (the whole tree for `None`) is expanded throughout.
    fn expanded_throughout(&self, id: &Option<TreeId>) -> bool {
        let deep = self.deep.contains(&None)
            || self.deep.contains(id)
            || id.as_ref().is_some_and(|item| {
                self.links
                    .get(item)
                    .is_some_and(|parent| self.branch_deep(parent))
            });
        deep && match id {
            None => self.closed.is_empty(),
            Some(item) => {
                !self.closed.contains(item)
                    && !self.closed.iter().any(|key| self.descends(key, item))
            }
        }
    }

    /// Drops the branches no open item reaches, which cancels their requests,
    /// and records which branches lie inside expanded subtrees.
    fn reconcile(&mut self) {
        fn visit(
            tree: &Tree,
            key: Option<TreeId>,
            deep: bool,
            reachable: &mut HashMap<Option<TreeId>, bool>,
            links: &mut Vec<(TreeId, Option<TreeId>)>,
        ) {
            if reachable.contains_key(&key) {
                return;
            }
            reachable.insert(key.clone(), deep);
            for node in tree.children(&key) {
                if node.expandable() && tree.is_open(&node.id, deep) {
                    links.push((node.id.clone(), key.clone()));
                    let inner = tree.opens_by_default(&node.id, deep);
                    visit(tree, Some(node.id.clone()), inner, reachable, links);
                }
            }
        }
        let mut reachable = HashMap::new();
        let mut links = Vec::new();
        visit(
            self,
            None,
            self.deep.contains(&None),
            &mut reachable,
            &mut links,
        );
        self.branches.retain(|key, _| reachable.contains_key(key));
        self.direct.retain(|key, _| reachable.contains_key(key));
        for (key, branch) in &mut self.branches {
            branch.deep = reachable[key];
        }
        self.pending.retain(|key, _| reachable.contains_key(key));
        let awaiting: HashSet<TreeId> = links.iter().map(|(id, _)| id.clone()).collect();
        self.links.extend(links);
        let kept: HashSet<&TreeId> = self
            .branches
            .keys()
            .flatten()
            .chain(&awaiting)
            .chain(&self.expanded)
            .chain(self.deep.iter().flatten())
            .chain(&self.closed)
            .collect();
        let keep: HashSet<TreeId> = kept.into_iter().cloned().collect();
        self.links.retain(|key, _| keep.contains(key));
        let open: HashSet<&TreeId> = self.links.keys().collect();
        let first: HashSet<TreeId> = self
            .first
            .iter()
            .filter(|key| open.contains(key) && !self.branches.contains_key(&Some((*key).clone())))
            .cloned()
            .collect();
        self.first = first;
    }

    fn loaded(&self) -> usize {
        self.branches.values().map(Branch::resident).sum::<usize>() + self.direct.len()
    }

    fn children(&self, parent: &Option<TreeId>) -> impl Iterator<Item = &Node> {
        let branch = self.branches.get(parent);
        let direct = self.direct.get(parent).filter(|direct| {
            branch.is_none_or(|branch| !branch.reachable().any(|node| node.id == direct.id))
        });
        branch.into_iter().flat_map(Branch::reachable).chain(direct)
    }

    /// Whether the active sort orders the items of branch `id`. Discs and
    /// releases keep their authored order.
    fn sorts(&self, id: &Option<TreeId>) -> bool {
        self.sort.is_some() && !matches!(id, Some(TreeId::Disc(_) | TreeId::Release(_)))
    }

    /// Keeps released pages' matches only for `query`.
    fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }
        query.clone_into(&mut self.query);
        self.saturated = false;
        for page in self.branches.values_mut().flat_map(|b| b.pages.iter_mut()) {
            if page.hits.as_ref().is_some_and(|hits| hits.query != query) {
                page.hits = None;
            }
        }
    }

    fn set_sort(&mut self, sort: Option<(Field, ColumnSort)>) {
        if self.sort != sort {
            self.sort = sort;
            for branch in self.branches.values_mut() {
                branch.unsorted = false;
            }
        }
    }

    /// Marks the pages the viewport is near as in use.
    fn touch(&mut self, spans: &[Span], near: std::ops::Range<usize>) {
        self.clock += 1;
        let clock = self.clock;
        for span in spans {
            if span.rows.start < near.end
                && span.rows.end.max(span.rows.start + 1) > near.start
                && let Some(page) = self
                    .branches
                    .get_mut(&span.branch)
                    .and_then(|branch| branch.pages.get_mut(span.page))
            {
                page.used = clock;
            }
        }
    }

    /// Takes the first page of a branch. Every open branch reads its first
    /// page, so it counts as unused until the viewport comes near it.
    fn insert_branch(&mut self, id: Option<TreeId>, branch: Branch, lens: Option<&Lens>) {
        if let Some(key) = &id {
            self.first.remove(key);
        }
        self.branches.insert(id.clone(), branch);
        self.reconcile();
        self.settle(&id, 0, false, lens);
    }

    /// Asks for the first pages of the open branches inside expanded
    /// subtrees that the viewport is near, and no others: one it has moved
    /// away from before its turn waits again. Returns whether any is new.
    fn want_first(&mut self, ids: HashSet<TreeId>) -> bool {
        let wanted: HashSet<TreeId> = ids
            .into_iter()
            .filter(|key| {
                let id = Some(key.clone());
                !self.branches.contains_key(&id) && !self.pending.contains_key(&id)
            })
            .collect();
        let new = wanted.iter().any(|key| !self.first.contains(key));
        self.first = wanted;
        new
    }

    /// Asks for a batch of a branch. Returns whether it is wanted now.
    fn want(&mut self, id: &Option<TreeId>, want: Want, search: bool) -> bool {
        let pending = self.pending.contains_key(id);
        let query = self.query.clone();
        let Some(branch) = self.branches.get_mut(id) else {
            return false;
        };
        let valid = match want {
            Want::First => false,
            Want::Next => branch.more.is_some(),
            Want::Page(index) => branch
                .pages
                .get(index)
                .is_some_and(|p| p.nodes.is_none() && !(search && p.hits(&query).is_some())),
        };
        if !valid || pending || branch.wanted.is_some() {
            return false;
        }
        branch.wanted = Some((want, search));
        true
    }

    /// Takes the batch a request read. A batch for a position the branch no
    /// longer waits at is stale and ignored.
    fn receive(
        &mut self,
        id: &Option<TreeId>,
        fetch: &Fetch,
        page: Branch,
        lens: Option<&Lens>,
    ) -> bool {
        if *fetch == Fetch::First {
            self.insert_branch(id.clone(), page, lens);
            return true;
        }
        let clock = self.clock;
        let Some(branch) = self.branches.get_mut(id) else {
            return false;
        };
        let search = branch.wanted.take().is_some_and(|(_, search)| search);
        let mut nodes: Vec<Node> = page.nodes().cloned().collect();
        let keep = match fetch {
            Fetch::Next(cursor) if branch.more.as_ref() == Some(cursor) => {
                let known: HashSet<_> = branch.reachable().map(|node| &node.id).collect();
                nodes.retain(|n| !known.contains(&n.id));
                branch.pages.push(Slot::new(Some(cursor.clone()), nodes));
                branch.more = page.more;
                branch.pages.len() - 1
            }
            Fetch::Page(index, start)
                if branch
                    .pages
                    .get(*index)
                    .is_some_and(|slot| slot.start == *start && slot.nodes.is_none()) =>
            {
                // A page read again holds what it held unless the folder
                // changed; an item another page lists stays there.
                let others: HashSet<_> = branch
                    .pages
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| at != index)
                    .flat_map(|(_, page)| page.nodes.iter().flatten())
                    .map(|node| node.id.clone())
                    .collect();
                nodes.retain(|n| !others.contains(&n.id));
                branch.pages[*index].load(nodes);
                *index
            }
            _ => return false,
        };
        branch.pages[keep].used = if search { 0 } else { clock };
        branch.notice = page.notice;
        self.settle(id, keep, search, lens);
        true
    }

    /// Keeps the loaded total within its bound after page `keep` of branch
    /// `id` was read. While browsing, a branch keeps `RESIDENT_PAGES`
    /// unless it is sorted; then pages the viewport is not near are
    /// released, least recently used first. The pages the viewport is near
    /// are never released, so loading cannot alternate between them. A page
    /// a filter searched keeps only its matches when there is no room, and
    /// once even those fill the total, the filter reads no further.
    fn settle(&mut self, id: &Option<TreeId>, keep: usize, search: bool, lens: Option<&Lens>) {
        let sorts = self.sorts(id);
        let deep = self.branch_deep(id);
        let tree = &*self;
        let open = |node: &Node| tree.retains_anchor(node, deep);
        let anchors = |branch: &Branch| -> HashSet<TreeId> {
            branch
                .nodes()
                .filter(|node| open(node))
                .map(|n| n.id.clone())
                .collect()
        };
        let opened = self.branches.get(id).map(anchors).unwrap_or_default();
        let open = |key: &TreeId| opened.contains(key);
        if let Some(branch) = self.branches.get_mut(id) {
            if sorts && !branch.unsorted && branch.len() > SORTED_BRANCH_LIMIT {
                branch.unsorted = true;
            }
            if !sorts || branch.unsorted {
                while branch.loaded_pages() > RESIDENT_PAGES
                    && let Some(index) = branch.least_recent(keep)
                {
                    branch.pages[index].release(&open, lens);
                }
            }
        }
        while self.loaded() > LOADED_NODE_LIMIT {
            let clock = self.clock;
            let least = self
                .branches
                .iter()
                .flat_map(|(key, branch)| {
                    branch
                        .pages
                        .iter()
                        .enumerate()
                        .map(move |(index, page)| (key, index, page))
                })
                .filter(|(key, index, page)| {
                    page.nodes.is_some() && page.used < clock && !(*key == id && *index == keep)
                })
                .min_by_key(|(_, _, page)| page.used)
                .map(|(key, index, _)| (key.clone(), index));
            let owner = least.as_ref().map_or(id, |(key, _)| key);
            let deep = self.branch_deep(owner);
            let opened: HashSet<TreeId> = self
                .branches
                .get(owner)
                .map(|branch| {
                    branch
                        .nodes()
                        .filter(|n| self.retains_anchor(n, deep))
                        .map(|n| n.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            let open = |key: &TreeId| opened.contains(key);
            let page = match least {
                Some((key, index)) => self.branches.get_mut(&key).map(|b| &mut b.pages[index]),
                None if search && lens.is_some() => self
                    .branches
                    .get_mut(id)
                    .and_then(|b| b.pages.get_mut(keep))
                    .filter(|page| page.nodes.is_some()),
                None => None,
            };
            match page {
                Some(page) => page.release(&open, lens),
                None => {
                    if lens.is_some() {
                        self.saturated = true;
                        let excess = self.loaded().saturating_sub(LOADED_NODE_LIMIT);
                        if let Some(hits) = self
                            .branches
                            .get_mut(id)
                            .and_then(|branch| branch.pages.get_mut(keep))
                            .and_then(|page| page.hits.as_mut())
                        {
                            hits.nodes.truncate(hits.nodes.len().saturating_sub(excess));
                        }
                    }
                    break;
                }
            }
        }
    }

    fn retains_anchor(&self, node: &Node, deep: bool) -> bool {
        let id = &node.id;
        node.expandable()
            && self.is_open(id, deep)
            && (self.reads_first(id)
                || self.branches.contains_key(&Some(id.clone()))
                || self.pending.contains_key(&Some(id.clone())))
    }

    /// Whether the first page of open item `id` is read without waiting for
    /// the viewport: it was opened by itself, Expand All was chosen on it,
    /// or the viewport asked for it.
    fn reads_first(&self, id: &TreeId) -> bool {
        self.reads_eagerly(id) || self.first.contains(id)
    }

    /// Whether open item `id` was opened by itself or with Expand All, so
    /// its first page is read wherever the viewport is.
    fn reads_eagerly(&self, id: &TreeId) -> bool {
        self.expanded.contains(id) || self.deep.contains(&Some(id.clone()))
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
                        Some((Want::First, _)) | None => None,
                        Some((Want::Next, _)) => branch.more.clone().map(Fetch::Next),
                        Some((Want::Page(index), _)) => branch
                            .pages
                            .get(index)
                            .map(|page| Fetch::Page(index, page.start.clone())),
                    };
                    if let Some(fetch) = fetch {
                        return Some((id, source()?, fetch));
                    }
                }
                for node in tree.children(&id) {
                    if tree.is_open(&node.id, branch.deep)
                        && let Some(next) = find(tree, Some(node.id.clone()), Some(node))
                    {
                        return Some(next);
                    }
                }
                None
            } else if tree.pending.contains_key(&id) {
                None
            } else if id.as_ref().is_none_or(|key| tree.reads_first(key)) {
                Some((id, source()?, Fetch::First))
            } else {
                None
            }
        }
        if self.pending.len() >= PARALLEL_BRANCHES {
            return None;
        }
        find(self, None, None)
    }

    /// The rows to show. A filter keeps matching items and the ancestors of
    /// matches; with `descendants` from the catalog it also keeps folders
    /// whose unloaded contents match.
    #[cfg(test)]
    fn visible(
        &mut self,
        sort: Option<(Field, ColumnSort)>,
        filter: &str,
        marks: &dyn Fn(&Track) -> crate::store::Mark,
    ) -> Vec<TreeRow> {
        self.set_sort(sort);
        self.set_query(filter);
        self.project(Lens::new(filter, None, marks).as_ref()).rows
    }

    fn project<'a>(&'a self, lens: Option<&'a Lens<'a>>) -> Projection<'a> {
        let mut view = Projection {
            tree: self,
            lens,
            rows: Vec::new(),
            len: 0,
            boundaries: Vec::new(),
            spans: Vec::new(),
        };
        view.visit(None, 0, false);
        if lens.is_some() && self.saturated {
            view.push(TreeRow::Message {
                parent: None,
                depth: 0,
                text: "More matches than can be listed · Refine the filter".into(),
            });
        }
        view
    }
}

/// The rows of a tree, with the batches they wait for.
struct Projection<'a> {
    tree: &'a Tree,
    lens: Option<&'a Lens<'a>>,
    rows: Vec<TreeRow>,
    /// The rows `rows` span.
    len: usize,
    boundaries: Vec<Boundary>,
    spans: Vec<Span>,
}

impl Projection<'_> {
    fn push(&mut self, row: TreeRow) {
        self.len += row.span();
        self.rows.push(row);
    }

    fn boundary(
        &mut self,
        rows: std::ops::Range<usize>,
        branch: &Option<TreeId>,
        want: Want,
        search: bool,
    ) {
        self.boundaries.push(Boundary {
            rows,
            branch: branch.clone(),
            want,
            search,
        });
    }

    fn visit(&mut self, parent: Option<TreeId>, depth: usize, inherited: bool) {
        let tree = self.tree;
        let Some(branch) = tree.branches.get(&parent) else {
            if let Some(direct) = tree.direct.get(&parent) {
                let matched = inherited || self.lens.is_none_or(|lens| lens.matches(direct));
                self.node(direct, &parent, depth, tree.branch_deep(&parent), matched);
            }
            if inherited || self.lens.is_none() {
                let row = self.len;
                if let Some(key) = &parent
                    && !tree.reads_eagerly(key)
                {
                    self.boundary(row..row + 1, &parent, Want::First, false);
                }
                self.push(TreeRow::Message {
                    parent,
                    depth,
                    text: "Loading…".into(),
                });
            }
            return;
        };
        let sorts = tree.sorts(&parent);
        let sorted = sorts && !branch.unsorted;
        if self.lens.is_some() && !inherited {
            self.search(branch, &parent, depth, sorted);
        } else {
            self.browse(branch, &parent, depth, sorted);
        }
        if let Some(direct) = tree.direct.get(&parent) {
            let already_listed = !(sorted && self.lens.is_none() && !branch.complete())
                && branch.reachable().any(|node| node.id == direct.id);
            if !already_listed {
                let matched = inherited || self.lens.is_none_or(|lens| lens.matches(direct));
                self.node(direct, &parent, depth, branch.deep, matched);
            }
        }
        let text = if let Some(text) = &branch.notice {
            Some(text.clone())
        } else if sorts && branch.unsorted {
            Some("Too many items to sort · Shown in folder order".into())
        } else if !tree.direct.contains_key(&parent)
            && branch.pages.iter().all(|page| page.len == 0)
            && branch.more.is_none()
            && depth > 0
            && (inherited || self.lens.is_none())
        {
            Some("No folders or supported recordings".into())
        } else {
            None
        };
        if let Some(text) = text {
            self.push(TreeRow::Message {
                parent,
                depth,
                text,
            });
        }
    }

    /// Every item of a branch, in place, with released pages as runs of
    /// rows the viewport loads again.
    fn browse(&mut self, branch: &Branch, parent: &Option<TreeId>, depth: usize, sorted: bool) {
        if sorted {
            let start = self.len;
            if branch.complete() {
                let mut nodes: Vec<&Node> = branch.nodes().collect();
                sort_nodes(&mut nodes, self.tree.sort);
                for node in nodes {
                    self.node(node, parent, depth, branch.deep, true);
                }
            } else {
                // A sorted branch is shown once all of it is read, never in
                // part.
                let count = branch.len();
                if count > 0 {
                    self.push(TreeRow::Released {
                        parent: parent.clone(),
                        count,
                    });
                }
                self.push(TreeRow::Message {
                    parent: parent.clone(),
                    depth,
                    text: "Sorting…".into(),
                });
                let missing = branch
                    .pages
                    .iter()
                    .position(|page| page.nodes.is_none())
                    .map(Want::Page)
                    .or(branch.more.is_some().then_some(Want::Next));
                if let Some(want) = missing {
                    self.boundary(start..self.len, parent, want, false);
                }
            }
            for page in 0..branch.pages.len() {
                self.spans.push(Span {
                    branch: parent.clone(),
                    page,
                    rows: start..self.len,
                });
            }
            return;
        }
        for (index, page) in branch.pages.iter().enumerate() {
            let start = self.len;
            match &page.nodes {
                Some(nodes) => {
                    for node in nodes {
                        self.node(node, parent, depth, branch.deep, true);
                    }
                }
                None => {
                    let mut at = 0;
                    for (offset, node) in &page.anchors {
                        if *offset > at {
                            self.push(TreeRow::Released {
                                parent: parent.clone(),
                                count: offset - at,
                            });
                        }
                        self.node(node, parent, depth, branch.deep, true);
                        at = offset + 1;
                    }
                    if page.len > at {
                        self.push(TreeRow::Released {
                            parent: parent.clone(),
                            count: page.len - at,
                        });
                    }
                    self.boundary(start..self.len, parent, Want::Page(index), false);
                }
            }
            self.spans.push(Span {
                branch: parent.clone(),
                page: index,
                rows: start..self.len,
            });
        }
        if branch.more.is_some() {
            let end = self.len;
            self.boundary(end.saturating_sub(1)..end, parent, Want::Next, false);
        }
    }

    /// The items of a branch a filter keeps. The filter reads every page
    /// of the branch wherever the viewport is, keeping only what it needs.
    fn search(&mut self, branch: &Branch, parent: &Option<TreeId>, depth: usize, sorted: bool) {
        let Some(lens) = self.lens else {
            return;
        };
        let saturated = self.tree.saturated;
        let mut found: Vec<(usize, &Node)> = Vec::new();
        let mut complete = branch.more.is_none();
        for (index, page) in branch.pages.iter().enumerate() {
            match (&page.nodes, page.hits(lens.query)) {
                (Some(nodes), _) => found.extend(nodes.iter().map(|node| (index, node))),
                (None, Some(hits)) => {
                    let mut retained: Vec<_> = page.anchors.iter().chain(hits).collect();
                    retained.sort_unstable_by_key(|(offset, _)| *offset);
                    found.extend(retained.into_iter().map(|(_, node)| (index, node)));
                }
                (None, None) => {
                    complete = false;
                    if !saturated {
                        self.boundary(0..usize::MAX, parent, Want::Page(index), true);
                    }
                }
            }
        }
        if branch.more.is_some() && !saturated {
            self.boundary(0..usize::MAX, parent, Want::Next, true);
        }
        let start = self.len;
        let mut spans: Vec<Option<std::ops::Range<usize>>> = vec![None; branch.pages.len()];
        if sorted {
            let mut nodes: Vec<&Node> = found.iter().map(|(_, node)| *node).collect();
            sort_nodes(&mut nodes, self.tree.sort);
            for node in nodes {
                self.node(node, parent, depth, branch.deep, lens.matches(node));
            }
            for (index, _) in &found {
                spans[*index] = Some(start..self.len);
            }
        } else {
            for (index, node) in found {
                let at = self.len;
                self.node(node, parent, depth, branch.deep, lens.matches(node));
                let span = spans[index].get_or_insert(at..at);
                span.end = self.len;
            }
        }
        for (page, rows) in spans.into_iter().enumerate() {
            if let Some(rows) = rows {
                self.spans.push(Span {
                    branch: parent.clone(),
                    page,
                    rows,
                });
            }
        }
        if sorted && !complete && !saturated {
            self.push(TreeRow::Message {
                parent: parent.clone(),
                depth,
                text: "Searching…".into(),
            });
        }
    }

    /// One item and, when it is open, its branch. A filter drops an item
    /// that neither matches nor leads to a match.
    fn node(
        &mut self,
        node: &Node,
        parent: &Option<TreeId>,
        depth: usize,
        deep: bool,
        matched: bool,
    ) {
        let holds = !matched && self.lens.is_some_and(|lens| lens.holds(node));
        let (row, len) = (self.rows.len(), self.len);
        let open = node.expandable() && self.tree.is_open(&node.id, deep);
        self.push(TreeRow::Node {
            node: node.clone(),
            parent: parent.clone(),
            depth,
            open,
        });
        if open {
            self.visit(Some(node.id.clone()), depth + 1, matched);
        }
        if !matched && !holds && self.rows.len() == row + 1 {
            self.rows.pop();
            self.len = len;
        }
    }
}

/// Orders sibling items by `sort`, numbers by value and text without case.
fn sort_nodes(nodes: &mut [&Node], sort: Option<(Field, ColumnSort)>) {
    let Some((field, direction)) = sort else {
        return;
    };
    nodes.sort_by_cached_key(|node| {
        let value = node.value(field);
        let numeric = match &node.item {
            Item::Track { track, .. } if field == Field::Length => track.duration,
            Item::Disc { disc, .. } if field == Field::Length => Some(disc.duration()),
            _ if matches!(field, Field::Number | Field::Count | Field::Year) => value.parse().ok(),
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

/// One page of a folder read from disk.
fn physical_branch(
    path: &Path,
    after: Option<&crate::library::ListingCursor>,
    alive: &dyn Fn() -> bool,
) -> anyhow::Result<Branch> {
    physical_page(path, after, PAGE, alive)
}

fn physical_page(
    path: &Path,
    after: Option<&crate::library::ListingCursor>,
    limit: usize,
    alive: &dyn Fn() -> bool,
) -> anyhow::Result<Branch> {
    let crate::discovery::FolderBranch {
        dirs,
        programs,
        unreadable,
        next,
        paged,
        notice,
    } = crate::discovery::tree_folder(path, after, limit, alive)?;
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
            Shelf::Link(..) | Shelf::Artist(_) => {}
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
        self.presentation() == Some(ViewMode::Tree)
    }

    pub(crate) fn tree_expansion(&self, cx: &App) -> Expansion {
        self.tree.read(cx).delegate().tree.expansion()
    }

    pub(crate) fn reset_tree(&mut self, expansion: Expansion, cx: &mut App) {
        self.tree.update(cx, |table, cx| {
            table.delegate_mut().reset(expansion);
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
                                    anyhow::ensure!(current(), crate::store::Canceled);
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
                                let (rows, next) = s.disc_page(id, offset, PAGE)?;
                                Ok(Branch::page(
                                    rows.into_iter()
                                        .filter(|row| row.available)
                                        .map(|row| Node::track(row.track, Some(disc.clone())))
                                        .collect(),
                                    next.map(Cursor::Offset),
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
                            anyhow::ensure!(work.is_current(generation), crate::store::Canceled);
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
                        table.delegate_mut().take(id, &fetch, result, cx);
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
    pub(super) fn load_tree(
        &mut self,
        wants: Vec<(Option<TreeId>, Want, bool)>,
        cx: &mut Context<Self>,
    ) {
        let mut any = false;
        self.tree.update(cx, |table, _| {
            let tree = &mut table.delegate_mut().tree;
            let first = wants
                .iter()
                .filter(|(_, want, _)| *want == Want::First)
                .filter_map(|(branch, _, _)| branch.clone())
                .collect();
            any |= tree.want_first(first);
            for (branch, want, search) in wants {
                any |= tree.want(&branch, want, search);
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
            .and_then(|index| table.delegate().row(index))
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
        let row = self.tree.read(cx).delegate().row(index).cloned();
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

    /// Expands or collapses item `id` of branch `parent` alone.
    fn toggle_tree(&mut self, id: TreeId, parent: Option<TreeId>, cx: &mut Context<Self>) {
        self.disclose_tree(ExpandAction::Toggle, Some(id), parent, cx);
    }

    /// Changes what is expanded, at item `id` of branch `parent` or across
    /// the whole tree for `None`. Selection, playback and the location stay
    /// as they are; when a collapse hides the selected item, the collapsed
    /// item is selected instead.
    pub(super) fn disclose_tree(
        &mut self,
        action: ExpandAction,
        id: Option<TreeId>,
        parent: Option<TreeId>,
        cx: &mut Context<Self>,
    ) {
        self.tree.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let tree = &mut delegate.tree;
            match (action, id.clone()) {
                (ExpandAction::Toggle, Some(item)) => tree.toggle(item, &parent),
                (ExpandAction::Toggle, None) => return,
                (ExpandAction::ExpandAll, id) => tree.expand_all(id, &parent),
                (ExpandAction::CollapseAll, id) => tree.collapse_all(id, &parent),
            }
            TreeTable::project(table, cx);
            let d = table.delegate();
            if action != ExpandAction::ExpandAll
                && d.selected
                    .as_ref()
                    .is_some_and(|key| d.position(key).is_none())
                && let Some(row) = id.as_ref().and_then(|key| d.position(key))
            {
                table.delegate_mut().on_selection(row);
                table.set_selected_row(row, cx);
            }
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
        let Some(row) = tree.row(index) else {
            return;
        };
        let selected = row.node();
        let branch = selected.filter(|node| node.expandable());
        let expanded = row.open();
        let parent = row.parent().cloned();
        match key {
            "enter" => self.activate_tree(index, window, cx),
            "right"
                if let Some(branch) = branch
                    && !expanded =>
            {
                self.toggle_tree(branch.id.clone(), parent, cx)
            }
            "left"
                if let Some(branch) = branch
                    && expanded =>
            {
                self.toggle_tree(branch.id.clone(), parent, cx)
            }
            "right"
                if let Some(node) = selected
                    && expanded =>
            {
                if tree
                    .row(index + 1)
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
mod tests;

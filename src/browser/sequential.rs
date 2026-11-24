//! Ordered destinations projected from a coherent, unfiltered collection.
use super::*;
use crate::{release::Ordered, store::AlbumStructure};
use std::{collections::HashMap, path::Path};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SequenceKind {
    Disc,
    Release,
}

impl SequenceKind {
    pub(crate) fn action(self, next: bool) -> &'static str {
        match (self, next) {
            (Self::Disc, false) => "Previous CD",
            (Self::Disc, true) => "Next CD",
            (Self::Release, false) => "Previous Release",
            (Self::Release, true) => "Next Release",
        }
    }
}

struct DestinationNode {
    id: DiscId,
    title: String,
    membership: Option<Arc<Membership>>,
    stop: bool,
}

#[derive(Default)]
struct Order {
    ranks: HashMap<usize, usize>,
    stops: Vec<(usize, usize)>,
}

impl Order {
    fn push(&mut self, index: usize, stop: bool) {
        let rank = self.ranks.len();
        self.ranks.insert(index, rank);
        if stop {
            self.stops.push((rank, index));
        }
    }

    fn adjacent(&self, index: usize, next: bool) -> Option<usize> {
        let rank = *self.ranks.get(&index)?;
        let at = self
            .stops
            .partition_point(|&(r, _)| r < rank || (next && r == rank));
        self.stops
            .get(if next { at } else { at.checked_sub(1)? })
            .map(|&(_, i)| i)
    }
}

/// Both axes use the same projection. A standalone CD is also a release.
/// A loose group is an anchor if deliberately opened and is skipped when
/// stepping from another destination. The projection holds no track data.
#[derive(Default)]
pub(super) struct Sequence {
    nodes: Vec<DestinationNode>,
    positions: HashMap<DiscId, usize>,
    discs: Order,
    releases: Order,
}

impl Sequence {
    fn new(nodes: Vec<DestinationNode>) -> Self {
        let positions = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();
        let mut discs = Order::default();
        let mut releases = Order::default();
        for item in crate::release::ordered(
            nodes
                .iter()
                .map(|n| n.membership.as_ref().map(|m| (&m.key, m.number))),
        ) {
            match item {
                Ordered::Disc(index) => {
                    discs.push(index, nodes[index].stop);
                    releases.push(index, nodes[index].stop);
                }
                Ordered::Release(indices) => {
                    releases.push(indices[0], indices.iter().any(|&i| nodes[i].stop));
                    for index in indices {
                        discs.push(index, nodes[index].stop);
                    }
                }
            }
        }
        Self {
            nodes,
            positions,
            discs,
            releases,
        }
    }

    pub(super) fn folder(
        discs: &[Arc<Disc>],
        unreadable: &std::collections::HashSet<PathBuf>,
    ) -> Self {
        Self::new(
            discs
                .iter()
                .map(|disc| DestinationNode {
                    id: disc.id.clone(),
                    title: disc.title.clone(),
                    membership: disc.release.clone(),
                    stop: disc.tracks.iter().any(|t| {
                        !unreadable.contains(&t.source)
                            && (disc.release.is_some() || t.has_disc_context())
                    }),
                })
                .collect(),
        )
    }

    fn indexed(albums: Vec<AlbumStructure>) -> Self {
        Self::new(
            albums
                .into_iter()
                .map(|album| DestinationNode {
                    id: DiscId::Library(album.id),
                    title: album.title,
                    stop: album.is_disc || album.release.is_some(),
                    membership: album.release.map(|m| {
                        Arc::new(Membership {
                            key: DiscId::Library(m.first),
                            title: m.title,
                            number: m.number,
                            subtitle: m.subtitle,
                        })
                    }),
                })
                .collect(),
        )
    }

    fn node(&self, id: &DiscId) -> Option<&DestinationNode> {
        self.nodes.get(*self.positions.get(id)?)
    }

    fn physical_node(&self, id: &DiscId) -> Option<&DestinationNode> {
        self.node(id)
            .or_else(|| self.nodes.iter().find(|n| n.id.same(id)))
    }

    fn adjacent(&self, id: &DiscId, kind: SequenceKind, next: bool) -> Option<&DestinationNode> {
        let (id, order) = match kind {
            SequenceKind::Disc => (id, &self.discs),
            SequenceKind::Release => (
                self.node(id)?.membership.as_ref().map_or(id, |m| &m.key),
                &self.releases,
            ),
        };
        self.nodes
            .get(order.adjacent(*self.positions.get(id)?, next)?)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct FolderScope {
    path: PathBuf,
    release_only: bool,
}

impl FolderScope {
    fn for_disc<'a>(
        folder: &Path,
        prefs: &'a ViewPrefs,
        roots: impl Iterator<Item = &'a Path>,
    ) -> Option<Self> {
        let rule = prefs.subfolder_rule(folder);
        if rule.is_some_and(|rule| !rule.include) {
            return None;
        }
        let known = rule
            .map(|rule| rule.path.as_path())
            .into_iter()
            .chain(roots.filter(|root| library::within(folder, root)))
            .max_by_key(|root| root.as_os_str().len());
        if let Some(root) = known {
            return (!crate::places::same_path(folder, root)).then(|| Self {
                path: root.to_owned(),
                release_only: false,
            });
        }
        // A numbered directory is only a candidate. The ordinary release
        // grouping must confirm its siblings before they become destinations.
        crate::release::disc_folder(folder.file_name()?.to_str()?)?;
        Some(Self {
            path: folder.parent()?.to_owned(),
            release_only: true,
        })
    }

    fn has_release_candidates(parent: &Path) -> bool {
        let mut first_two = [false; 2];
        for folder in library::subfolders(parent) {
            let disc = folder
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(crate::release::disc_folder);
            if let Some((number @ 1..=2, _)) = disc {
                first_two[number as usize - 1] = true;
            }
        }
        first_two.into_iter().all(|found| found)
    }
}

#[derive(Default)]
pub(super) struct FolderTraversal {
    scope: Option<FolderScope>,
    sequence: Option<Sequence>,
    task: Option<Task<()>>,
}

impl FolderTraversal {
    fn current(&self, id: &DiscId) -> Option<(&Sequence, &DiscId, &Path)> {
        let scope = self.scope.as_ref()?;
        let sequence = self.sequence.as_ref()?;
        let node = sequence.physical_node(id)?;
        if scope.release_only && node.membership.is_none() {
            return None;
        }
        Some((sequence, &node.id, &scope.path))
    }
}

#[derive(Default)]
pub(super) struct IndexedSequence {
    data: Option<(RootId, Sequence)>,
    request: Ticket,
    pending: Option<(Option<RootId>, crate::store::AlbumId)>,
}

impl IndexedSequence {
    pub(super) fn cancel(&mut self) {
        if self.pending.take().is_some() {
            self.request.next();
        }
    }
}

impl Browser {
    pub(super) fn physical_sequence_disc(&self) -> Option<&DiscId> {
        match &self.page {
            Some(Page::Disc(id)) => Some(id),
            None if !self.listing_shelf() && self.programs.len() == 1 => {
                let id = &self.programs[0].id;
                self.folder_sequence
                    .node(id)
                    .filter(|n| {
                        n.stop
                            && id
                                .folder()
                                .is_some_and(|folder| crate::places::same_path(folder, &self.path))
                    })
                    .map(|n| &n.id)
            }
            _ => None,
        }
    }

    fn physical_scope(&self, folder: &Path, cx: &App) -> Option<FolderScope> {
        FolderScope::for_disc(
            folder,
            &self.prefs,
            self.catalog
                .read(cx)
                .collections
                .iter()
                .filter(|c| c.online)
                .map(|c| c.root.path.as_path()),
        )
    }

    pub(super) fn reveal_collection_scope(&self, folder: &Path, cx: &App) -> Option<PathBuf> {
        self.physical_scope(folder, cx)
            .filter(|scope| !scope.release_only)
            .map(|scope| scope.path)
    }

    pub(super) fn refresh_folder_traversal(&mut self, cx: &mut Context<Self>) {
        let scope = if self.location.is_folder() && self.folder_ready && !self.busy {
            self.physical_sequence_disc()
                .and_then(DiscId::folder)
                .filter(|folder| crate::places::same_path(folder, &self.path))
                .and_then(|folder| self.physical_scope(folder, cx))
        } else {
            None
        };
        if self.folder_traversal.scope == scope {
            return;
        }
        self.folder_traversal = FolderTraversal::default();
        let Some(scope) = scope else { return };
        self.folder_traversal.scope = Some(scope.clone());
        let scanner = self.scanner.clone();
        self.folder_traversal.task = Some(cx.spawn(async move |this, cx| {
            if scope.release_only {
                let parent = scope.path.clone();
                if !cx
                    .background_spawn(async move { FolderScope::has_release_candidates(&parent) })
                    .await
                {
                    return;
                }
            }
            let batches = scanner.open(scope.path.clone(), true);
            let result = cx
                .background_spawn(async move {
                    let mut tracks = Vec::new();
                    let mut unreadable = std::collections::HashSet::new();
                    let mut complete = true;
                    while let Ok(batch) = batches.recv().await {
                        complete &= batch.incomplete.is_empty();
                        unreadable.extend(batch.unreadable);
                        for (i, track) in batch.rows {
                            if i == tracks.len() {
                                tracks.push(track);
                            } else {
                                tracks[i] = track;
                            }
                        }
                    }
                    if !complete {
                        return None;
                    }
                    let discs = Discs::group(tracks.iter().flatten(), &HashMap::new(), &scope.path);
                    Some(Sequence::folder(&discs, &unreadable))
                })
                .await;
            let _ = this.update(cx, |b, cx| {
                b.folder_traversal.sequence = result;
                cx.emit(ViewChanged);
                cx.notify();
            });
        }));
    }

    pub(crate) fn sequence_kind(&self) -> Option<SequenceKind> {
        match (&self.location, &self.page) {
            (Location::Album { .. }, _) => Some(SequenceKind::Disc),
            (Location::Release { .. }, _) | (Location::Folder(_), Some(Page::Release(_))) => {
                Some(SequenceKind::Release)
            }
            (Location::Folder(_), _) => self.physical_sequence_disc().map(|_| SequenceKind::Disc),
            _ => None,
        }
    }

    fn indexed_destination(&self) -> Option<(Option<RootId>, crate::store::AlbumId)> {
        match self.location {
            Location::Album { root, id, .. } | Location::Release { root, id, .. } => {
                Some((root, id))
            }
            _ => None,
        }
    }

    pub(super) fn refresh_indexed_sequence(&mut self, changed: RootId, cx: &mut Context<Self>) {
        if self
            .indexed_sequence
            .data
            .as_ref()
            .is_some_and(|(root, _)| *root == changed)
        {
            self.indexed_sequence.data = None;
        }
        if self
            .indexed_destination()
            .is_some_and(|(root, _)| root.is_none_or(|root| root == changed))
        {
            self.indexed_sequence.cancel();
            self.membership = None;
            self.load_indexed_sequence(cx);
            cx.emit(ViewChanged);
            cx.notify();
        }
    }

    pub(super) fn load_indexed_sequence(&mut self, cx: &mut Context<Self>) {
        let Some((root, id)) = self.indexed_destination() else {
            self.indexed_sequence.cancel();
            return;
        };
        let catalog = self.catalog.read(cx);
        if catalog.loaded && root.is_some_and(|id| catalog.collection(id).is_none_or(|c| !c.online))
        {
            self.indexed_sequence.cancel();
            self.indexed_sequence.data = None;
            self.membership_pending = false;
            return;
        }
        if let Some((actual_root, sequence)) = &self.indexed_sequence.data
            && (root == Some(*actual_root)
                || (root.is_none() && sequence.node(&DiscId::Library(id)).is_some()))
        {
            self.membership = if matches!(self.location, Location::Album { .. }) {
                sequence
                    .node(&DiscId::Library(id))
                    .and_then(|n| n.membership.clone())
            } else {
                None
            };
            self.membership_pending = false;
            self.indexed_sequence.cancel();
            return;
        }
        let Some(store) = catalog.handle() else {
            return;
        };
        self.membership_pending = matches!(self.location, Location::Album { .. });
        if self
            .indexed_sequence
            .pending
            .is_some_and(|pending| (root.is_some() && pending.0 == root) || pending == (root, id))
        {
            return;
        }
        self.indexed_sequence.pending = Some((root, id));
        let revision = self.indexed_sequence.request.next();
        let query = store.query(&self.indexed_sequence.request, revision, move |s| {
            s.navigation_albums(root, id)
                .map(|result| result.map(|(root, nodes)| (root, Sequence::indexed(nodes))))
        });
        cx.spawn(async move |this, cx| {
            let result = query.await;
            let _ = this.update(cx, |browser, cx| {
                if !browser.indexed_sequence.request.is_current(revision) {
                    return;
                }
                browser.indexed_sequence.pending = None;
                browser.membership_pending = false;
                if let Ok(Some((root, sequence))) = result {
                    match &mut browser.location {
                        Location::Album { root: current, .. }
                        | Location::Release { root: current, .. } => {
                            current.get_or_insert(root);
                        }
                        _ => return,
                    }
                    browser.indexed_sequence.data = Some((root, sequence));
                    browser.load_indexed_sequence(cx);
                }
                cx.emit(ViewChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn sequential_target(&self, kind: SequenceKind, next: bool) -> Option<(Route, String)> {
        if self.sequence_kind() == Some(SequenceKind::Release) && kind == SequenceKind::Disc {
            return None;
        }
        let (sequence, current, root, folder) = match (&self.location, &self.page) {
            (Location::Folder(path), page) if !self.busy => {
                let id = match page {
                    Some(Page::Release(id)) => id,
                    _ => self.physical_sequence_disc()?,
                };
                let (sequence, id, folder) = self.folder_traversal.current(id).or_else(|| {
                    Some((
                        &self.folder_sequence,
                        &self.folder_sequence.physical_node(id)?.id,
                        path.as_path(),
                    ))
                })?;
                (sequence, id.clone(), None, Some(folder))
            }
            (Location::Album { id, root, .. } | Location::Release { id, root, .. }, _) => {
                let (actual_root, sequence) = self.indexed_sequence.data.as_ref()?;
                if root.is_some_and(|root| root != *actual_root) {
                    return None;
                }
                (sequence, DiscId::Library(*id), Some(*actual_root), None)
            }
            _ => return None,
        };
        let node = sequence.adjacent(&current, kind, next)?;
        let release = (kind == SequenceKind::Release)
            .then_some(node.membership.as_deref())
            .flatten();
        let title = release.map_or(&node.title, |m| &m.title).clone();
        let route = match node.id {
            DiscId::Library(id) if release.is_some() => Route::new(
                Location::Release {
                    root,
                    id,
                    title: title.clone(),
                },
                None,
            ),
            DiscId::Library(id) => Route::new(
                Location::Album {
                    root,
                    id,
                    title: title.clone(),
                },
                None,
            ),
            _ => Route::new(
                Location::Folder(folder?.to_owned()),
                Some(if release.is_some() {
                    Page::Release(node.id.clone())
                } else {
                    Page::Disc(node.id.clone())
                }),
            ),
        };
        let label = match (kind, &node.membership) {
            (SequenceKind::Disc, Some(m)) if m.subtitle.is_empty() => {
                format!("{} › CD {}", m.title, m.number)
            }
            (SequenceKind::Disc, Some(m)) => {
                format!("{} › CD {} · {}", m.title, m.number, m.subtitle)
            }
            _ => title,
        };
        Some((route, label))
    }

    pub(crate) fn sequence_label(&self, kind: SequenceKind, next: bool) -> Option<String> {
        self.sequential_target(kind, next).map(|(_, label)| label)
    }

    pub(crate) fn step_sequence(
        &mut self,
        kind: Option<SequenceKind>,
        next: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((route, _)) = kind
            .or_else(|| self.sequence_kind())
            .and_then(|kind| self.sequential_target(kind, next))
        {
            let arrival = match &route.location {
                Location::Folder(folder) => Arrival::Sequence(
                    self.subfolders || !crate::places::same_path(folder, &self.path),
                ),
                _ => Arrival::Regular,
            };
            self.move_to(route, arrival, window, cx);
        } else {
            self.begin_navigation(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DestinationNode, DiscId, FolderScope, Membership, Sequence, SequenceKind, ViewPrefs,
    };
    use std::{path::Path, sync::Arc};

    fn disc(id: i64, first: Option<(i64, u32)>) -> DestinationNode {
        DestinationNode {
            id: DiscId::Library(id),
            title: format!("Album {id}"),
            membership: first.map(|(first, number)| {
                Arc::new(Membership {
                    key: DiscId::Library(first),
                    title: format!("Release {first}"),
                    number,
                    subtitle: String::new(),
                })
            }),
            stop: true,
        }
    }

    fn neighbor(sequence: &Sequence, id: i64, kind: SequenceKind, next: bool) -> Option<i64> {
        sequence
            .adjacent(&DiscId::Library(id), kind, next)?
            .id
            .album()
    }

    #[test]
    fn cds_follow_authored_order_across_releases_in_both_directions() {
        let sequence = Sequence::new(vec![
            disc(10, Some((1, 10))),
            disc(1, Some((1, 1))),
            disc(20, None),
            disc(3, Some((1, 3))),
            disc(32, Some((31, 2))),
            disc(31, Some((31, 1))),
            disc(2, Some((1, 2))),
        ]);
        let cds = [1, 2, 3, 10, 20, 31, 32];
        for pair in cds.windows(2) {
            assert_eq!(
                neighbor(&sequence, pair[0], SequenceKind::Disc, true),
                Some(pair[1])
            );
            assert_eq!(
                neighbor(&sequence, pair[1], SequenceKind::Disc, false),
                Some(pair[0])
            );
        }
        assert_eq!(neighbor(&sequence, 1, SequenceKind::Disc, false), None);
        assert_eq!(neighbor(&sequence, 32, SequenceKind::Disc, true), None);
        assert_eq!(neighbor(&sequence, 30, SequenceKind::Disc, true), None);
    }

    #[test]
    fn release_steps_use_the_same_anchors_and_include_standalone_albums() {
        let sequence = Sequence::new(vec![
            disc(14, Some((11, 4))),
            disc(11, Some((11, 1))),
            disc(23, None),
            disc(32, Some((31, 2))),
            disc(31, Some((31, 1))),
        ]);
        assert_eq!(
            neighbor(&sequence, 14, SequenceKind::Release, true),
            Some(23)
        );
        assert_eq!(
            neighbor(&sequence, 23, SequenceKind::Release, true),
            Some(31)
        );
        assert_eq!(
            neighbor(&sequence, 31, SequenceKind::Release, false),
            Some(23)
        );
        assert_eq!(
            neighbor(&sequence, 23, SequenceKind::Release, false),
            Some(11)
        );
        assert_eq!(neighbor(&sequence, 32, SequenceKind::Release, true), None);
        assert_eq!(neighbor(&sequence, 11, SequenceKind::Release, false), None);
    }

    #[test]
    fn a_rebuilt_index_uses_the_new_shelf_order_with_the_same_identities() {
        let first = Sequence::new((1..=1011).map(|id| disc(id, None)).collect());
        let reordered = Sequence::new((1..=1011).rev().map(|id| disc(id, None)).collect());
        assert_eq!(neighbor(&first, 777, SequenceKind::Disc, true), Some(778));
        assert_eq!(
            neighbor(&reordered, 777, SequenceKind::Disc, true),
            Some(776)
        );
        assert_eq!(
            neighbor(&Sequence::default(), 1, SequenceKind::Release, true),
            None
        );
    }

    #[test]
    fn deliberately_opened_loose_groups_anchor_both_axes_without_becoming_stops() {
        let mut loose = disc(2, None);
        loose.stop = false;
        let sequence = Sequence::new(vec![disc(1, None), loose, disc(3, None)]);
        for kind in [SequenceKind::Disc, SequenceKind::Release] {
            assert_eq!(neighbor(&sequence, 2, kind, false), Some(1));
            assert_eq!(neighbor(&sequence, 2, kind, true), Some(3));
            assert_eq!(neighbor(&sequence, 1, kind, true), Some(3));
            assert_eq!(neighbor(&sequence, 3, kind, false), Some(1));
        }
    }

    #[test]
    fn physical_scope_uses_the_nearest_collection_and_respects_explicit_boundaries() {
        let root = Path::new(r"C:\Music");
        let smaller = Path::new(r"C:\Music\Selected");
        let parent = smaller.join("Release");
        let cd = parent.join("CD 2");
        let mut prefs = ViewPrefs::default();
        let scope = FolderScope::for_disc(&cd, &prefs, [root].into_iter()).unwrap();
        assert_eq!(scope.path, root);
        assert!(!scope.release_only);
        prefs.set_subfolders(smaller, true);
        let scope = FolderScope::for_disc(&cd, &prefs, [root].into_iter()).unwrap();
        assert_eq!(scope.path, smaller);
        assert!(!scope.release_only);
        prefs.set_subfolders(smaller, false);
        assert!(FolderScope::for_disc(&cd, &prefs, [root].into_iter()).is_none());

        let prefs = ViewPrefs::default();
        let unrelated = Path::new(r"C:\Music Archive");
        let scope = FolderScope::for_disc(&cd, &prefs, [unrelated].into_iter()).unwrap();
        assert_eq!(scope.path, parent);
        assert!(scope.release_only);
        assert!(FolderScope::for_disc(&parent, &prefs, [unrelated].into_iter()).is_none());
    }

    #[test]
    fn an_inferred_release_requires_a_matching_membership_in_the_projection() {
        use super::FolderTraversal;
        let mut node = disc(1, None);
        node.id = DiscId::Cue(r"C:\Music\CD 1\Disc.cue".into());
        let id = DiscId::Cue("c:/music/cd 1/disc.cue".into());
        let mut traversal = FolderTraversal {
            scope: Some(FolderScope {
                path: r"C:\Music".into(),
                release_only: true,
            }),
            sequence: Some(Sequence::new(vec![node])),
            ..FolderTraversal::default()
        };
        assert!(traversal.current(&id).is_none());
        traversal.scope.as_mut().unwrap().release_only = false;
        assert!(traversal.current(&id).is_some());
    }
}

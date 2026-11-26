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

pub(super) fn program_label(
    title: &str,
    member: Option<&Membership>,
    kind: SequenceKind,
    compact: bool,
) -> String {
    let Some(member) = member else {
        return title.to_owned();
    };
    if kind == SequenceKind::Release {
        return member.title.clone();
    }
    let mut disc = format!("CD {}", member.number);
    if !member.subtitle.is_empty() {
        disc.push_str(" · ");
        disc.push_str(&member.subtitle);
    }
    if compact {
        disc
    } else {
        format!("{} › {disc}", member.title)
    }
}

struct DestinationNode {
    id: DiscId,
    title: String,
    membership: Option<Arc<Membership>>,
    stop: bool,
}

impl DestinationNode {
    fn label(&self, kind: SequenceKind, compact: bool) -> String {
        program_label(&self.title, self.membership.as_deref(), kind, compact)
    }

    fn route(
        &self,
        kind: SequenceKind,
        root: Option<RootId>,
        folder: Option<&Path>,
    ) -> Option<Route> {
        let release = kind == SequenceKind::Release && self.membership.is_some();
        let location = match self.id {
            DiscId::Library(id) if release => Location::Release {
                root,
                id,
                title: self.label(kind, false),
            },
            DiscId::Library(id) => Location::Album {
                root,
                id,
                title: self.title.clone(),
            },
            _ => {
                return Some(Route::new(
                    Location::Folder(folder?.to_owned()),
                    Some(if release {
                        Page::Release(self.id.clone())
                    } else {
                        Page::Disc(self.id.clone())
                    }),
                ));
            }
        };
        Some(Route::new(location, None))
    }
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

    fn related(
        &self,
        selected: Route,
        id: &DiscId,
        kind: SequenceKind,
        root: Option<RootId>,
        folder: Option<&Path>,
    ) -> Option<Vec<BreadcrumbChoice>> {
        let current = self.physical_node(id)?;
        let current_index = self.positions[&current.id];
        let member = current.membership.as_deref();
        // A release whose grouping disappeared after a rescan is no longer
        // a destination. A standalone CD uses the release order instead.
        if kind == SequenceKind::Release && member.is_none() {
            return None;
        }
        let group = (kind == SequenceKind::Disc).then_some(member).flatten();
        let (axis, order) = if group.is_some() {
            (SequenceKind::Disc, &self.discs)
        } else {
            (SequenceKind::Release, &self.releases)
        };
        let rank = *order.ranks.get(&current_index)?;
        let first = order.stops.partition_point(|&(r, _)| r < rank);
        let current_missing = order.stops.get(first).is_none_or(|&(r, _)| r != rank);
        let indices = order.stops[..first]
            .iter()
            .map(|&(_, index)| index)
            .chain(current_missing.then_some(current_index))
            .chain(order.stops[first..].iter().map(|&(_, index)| index));
        Some(
            indices
                .filter_map(|index| {
                    let node = &self.nodes[index];
                    if let Some(group) = group
                        && node
                            .membership
                            .as_ref()
                            .is_none_or(|m| !m.key.same(&group.key))
                    {
                        return None;
                    }
                    let current = index == current_index;
                    let route = if current {
                        selected.clone()
                    } else {
                        node.route(axis, root, folder)?
                    };
                    Some(BreadcrumbChoice::new(
                        route,
                        node.label(axis, true),
                        current,
                    ))
                })
                .collect(),
        )
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
    sequence: Option<Arc<Sequence>>,
    task: Option<Task<()>>,
}

impl FolderTraversal {
    fn current(&self, id: &DiscId) -> Option<(&Arc<Sequence>, &DiscId, &Path)> {
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
    data: Option<(RootId, Arc<Sequence>)>,
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
    fn indexed_projection(&self, route: &Route) -> Option<(RootId, &Arc<Sequence>)> {
        let root = match route.location {
            Location::Album { root, .. } | Location::Release { root, .. } => root,
            _ => return None,
        };
        let (actual, sequence) = self.indexed_sequence.data.as_ref()?;
        root.is_none_or(|root| root == *actual)
            .then_some((*actual, sequence))
    }

    pub(super) fn program_title(&self, route: &Route, compact: bool) -> Option<String> {
        let (id, kind) = route.program()?;
        match &route.location {
            Location::Folder(path) if crate::places::same_path(path, &self.path) => {
                if let Some(node) = self.folder_sequence.physical_node(&id) {
                    return Some(node.label(kind, compact));
                }
                // The program can already be visible during progressive discovery.
                let disc = self.programs.iter().find(|d| d.id.same(&id))?;
                Some(program_label(
                    &disc.title,
                    disc.release.as_deref(),
                    kind,
                    compact,
                ))
            }
            Location::Album { title, .. } | Location::Release { title, .. } => {
                if let Some(node) = self
                    .indexed_projection(route)
                    .and_then(|(_, s)| s.node(&id))
                {
                    return Some(node.label(kind, compact));
                }
                let member = (self.location == route.location)
                    .then_some(self.membership.as_deref())
                    .flatten();
                Some(program_label(title, member, kind, compact))
            }
            _ => None,
        }
    }

    pub(super) fn program_choices(&self, route: &Route) -> BreadcrumbChoices {
        let Some((id, kind)) = route.program() else {
            return BreadcrumbChoices::Unavailable;
        };
        let (sequence, root, folder) = match &route.location {
            Location::Folder(_) if self.busy => return BreadcrumbChoices::Waiting,
            Location::Folder(path) if crate::places::same_path(path, &self.path) => {
                let local = self.folder_sequence.physical_node(&id);
                // Preserve the projection that already defines this CD's
                // release. Release-level siblings may use the wider traversal.
                let traversal = self
                    .folder_traversal
                    .current(&id)
                    .filter(|(sequence, key, _)| {
                        if kind == SequenceKind::Release {
                            sequence
                                .node(key)
                                .and_then(|n| n.membership.as_ref())
                                .is_some_and(|m| m.key.same(key))
                        } else {
                            local.is_none_or(|node| node.membership.is_none())
                        }
                    });
                let (sequence, folder) = traversal
                    .map(|(sequence, _, folder)| (sequence, folder))
                    .unwrap_or((&self.folder_sequence, path.as_path()));
                (sequence.clone(), None, Some(folder.to_owned()))
            }
            Location::Album { .. } | Location::Release { .. } => {
                let Some((root, sequence)) = self.indexed_projection(route) else {
                    return if self.indexed_sequence.pending.is_some() {
                        BreadcrumbChoices::Waiting
                    } else {
                        BreadcrumbChoices::Unavailable
                    };
                };
                (sequence.clone(), Some(root), None)
            }
            _ => return BreadcrumbChoices::Unavailable,
        };
        let route = route.clone();
        BreadcrumbChoices::Enumerate(Box::new(move || {
            sequence.related(route, &id, kind, root, folder.as_deref())
        }))
    }

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

    pub(super) fn refresh_folder_traversal(&mut self, cx: &mut Context<Self>) {
        let scope =
            if !self.listing_tree() && self.location.is_folder() && self.folder_ready && !self.busy
            {
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
                    Some(Arc::new(Sequence::folder(&discs, &unreadable)))
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
            s.navigation_albums(root, id).map(|result| {
                result.map(|(root, nodes)| (root, Arc::new(Sequence::indexed(nodes))))
            })
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
        let mut route = node.route(kind, root, folder)?;
        route.shelf.clone_from(&self.shelf_origin);
        Some((route, node.label(kind, false)))
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
            let arrival = self.sequence_arrival(&route);
            self.move_to(route, arrival, window, cx);
        } else {
            self.begin_navigation(cx);
        }
    }

    pub(super) fn sequence_arrival(&self, route: &Route) -> Arrival {
        match (&route.location, &route.page) {
            (Location::Folder(folder), Some(_)) => {
                Arrival::Sequence(self.subfolders || !crate::places::same_path(folder, &self.path))
            }
            _ => Arrival::Regular,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DestinationNode, DiscId, FolderScope, Location, Membership, Route, Sequence, SequenceKind,
        ViewPrefs,
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
    fn breadcrumb_menus_preserve_authored_membership_and_release_routes() {
        let mut loose = disc(40, None);
        loose.stop = false;
        let sequence = Sequence::new(vec![
            disc(3, Some((1, 3))),
            disc(1, Some((1, 1))),
            loose,
            disc(8, None),
            disc(2, Some((1, 2))),
            disc(9, Some((7, 2))),
            disc(7, Some((7, 1))),
        ]);
        let root = Some(5);
        let cd = Route::new(
            Location::Album {
                root,
                id: 2,
                title: "An old title".into(),
            },
            None,
        );
        let siblings = sequence
            .related(
                cd.clone(),
                &DiscId::Library(2),
                SequenceKind::Disc,
                root,
                None,
            )
            .unwrap();
        assert_eq!(
            siblings
                .iter()
                .map(|c| c.label.as_ref())
                .collect::<Vec<_>>(),
            ["CD 1", "CD 2", "CD 3"]
        );
        assert_eq!(siblings.iter().filter(|c| c.current).count(), 1);
        assert!(siblings[1].current && siblings[1].route.same(&cd));
        assert!(matches!(
            siblings[0].route.location,
            Location::Album {
                root: Some(5),
                id: 1,
                ..
            }
        ));

        let release = Route::new(
            Location::Release {
                root,
                id: 1,
                title: "Release 1".into(),
            },
            None,
        );
        let alternatives = sequence
            .related(
                release,
                &DiscId::Library(1),
                SequenceKind::Release,
                root,
                None,
            )
            .unwrap();
        assert_eq!(
            alternatives
                .iter()
                .map(|c| c.label.as_ref())
                .collect::<Vec<_>>(),
            ["Release 1", "Album 8", "Release 7"]
        );
        assert!(matches!(
            alternatives[1].route.location,
            Location::Album { id: 8, .. }
        ));
        assert!(matches!(
            alternatives[2].route.location,
            Location::Release { id: 7, .. }
        ));
        let removed = Route::new(
            Location::Release {
                root,
                id: 40,
                title: "Removed release".into(),
            },
            None,
        );
        assert!(
            sequence
                .related(
                    removed,
                    &DiscId::Library(40),
                    SequenceKind::Release,
                    root,
                    None
                )
                .is_none()
        );
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
            sequence: Some(Arc::new(Sequence::new(vec![node]))),
            ..FolderTraversal::default()
        };
        assert!(traversal.current(&id).is_none());
        traversal.scope.as_mut().unwrap().release_only = false;
        assert!(traversal.current(&id).is_some());
    }
}

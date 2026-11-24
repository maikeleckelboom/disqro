//! Ordered destinations projected from a coherent, unfiltered collection.
use super::*;
use crate::{release::Ordered, store::AlbumStructure};
use std::collections::HashMap;

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
}

#[derive(Default)]
struct Order {
    indices: Vec<usize>,
    positions: HashMap<usize, usize>,
}

impl Order {
    fn push(&mut self, index: usize) {
        self.positions.insert(index, self.indices.len());
        self.indices.push(index);
    }

    fn adjacent(&self, index: usize, next: bool) -> Option<usize> {
        let at = *self.positions.get(&index)?;
        let position = if next { at + 1 } else { at.checked_sub(1)? };
        self.indices.get(position).copied()
    }
}

/// Both axes use the same projection. A standalone CD is also a release.
/// Structural nodes affect position through the source shelf order and never
/// become stops. This index owns titles and identities, with no track data.
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
                    discs.push(index);
                    releases.push(index);
                }
                Ordered::Release(indices) => {
                    releases.push(indices[0]);
                    for index in indices {
                        discs.push(index);
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

    pub(super) fn folder(discs: &[Arc<Disc>]) -> Self {
        Self::new(
            discs
                .iter()
                .filter(|disc| {
                    disc.release.is_some()
                        || disc.tracks.first().is_some_and(|t| t.has_disc_context())
                })
                .map(|disc| DestinationNode {
                    id: disc.id.clone(),
                    title: disc.title.clone(),
                    membership: disc.release.clone(),
                })
                .collect(),
        )
    }

    fn indexed(albums: Vec<AlbumStructure>) -> Self {
        Self::new(
            albums
                .into_iter()
                .filter(|album| album.is_disc || album.release.is_some())
                .map(|album| DestinationNode {
                    id: DiscId::Library(album.id),
                    title: album.title,
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
    pub(crate) fn sequence_kind(&self) -> Option<SequenceKind> {
        match (&self.location, &self.page) {
            (Location::Album { .. }, _) | (Location::Folder(_), Some(Page::Disc(_))) => {
                Some(SequenceKind::Disc)
            }
            (Location::Release { .. }, _) | (Location::Folder(_), Some(Page::Release(_))) => {
                Some(SequenceKind::Release)
            }
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
        let (sequence, current, root) = match (&self.location, &self.page) {
            (Location::Folder(_), Some(Page::Disc(id) | Page::Release(id))) if !self.busy => {
                (&self.folder_sequence, id.clone(), None)
            }
            (Location::Album { id, root, .. } | Location::Release { id, root, .. }, _) => {
                let (actual_root, sequence) = self.indexed_sequence.data.as_ref()?;
                if root.is_some_and(|root| root != *actual_root) {
                    return None;
                }
                (sequence, DiscId::Library(*id), Some(*actual_root))
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
                self.location.clone(),
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
        let Some(kind) = kind.or_else(|| self.sequence_kind()) else {
            return;
        };
        if let Some((route, _)) = self.sequential_target(kind, next) {
            self.move_to(route, Arrival::Regular, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DestinationNode, DiscId, Membership, Sequence, SequenceKind};
    use std::sync::Arc;

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
}

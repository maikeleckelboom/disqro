use super::*;
use crate::store::{Store, TrackId};
use gpui_kit::component::WindowExt;

const REVEAL_DEPTH: usize = 96;

fn real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return false;
            }
        }
        metadata.is_dir()
    })
}

fn folder_lineage(base: &Path, folder: &Path, root: Option<RootId>) -> Option<Vec<Node>> {
    if !crate::library::within(folder, base) {
        return None;
    }
    let mut lineage = Vec::new();
    for ancestor in folder.ancestors() {
        if crate::places::same_path(base, ancestor) {
            lineage.reverse();
            return Some(lineage);
        }
        if lineage.len() == REVEAL_DEPTH || !real_directory(ancestor) {
            return None;
        }
        lineage.push(Node::folder(root, ancestor.to_owned()));
    }
    None
}

fn indexed_path(store: &Store, root: Option<RootId>, id: TrackId) -> anyhow::Result<Vec<Node>> {
    let Some(row) = store
        .rows(Scope::Library(None), &[id])?
        .pop()
        .filter(|row| row.available)
    else {
        return Ok(Vec::new());
    };
    let Some(reg) = store.registered_disc(&row.track)? else {
        return Ok(Vec::new());
    };
    if root.is_some_and(|root| root != reg.root) {
        return Ok(Vec::new());
    }
    let (path, name) = store.root_location(reg.root)?;
    let Some(mut album) = store.tree_album(reg.album)? else {
        return Ok(Vec::new());
    };
    album.folder = path.join(&album.folder);
    let home = album.folder.clone();
    let id = DiscId::Library(reg.album);
    let mut lineage = Vec::new();
    if root.is_none() {
        lineage.push(Node {
            has_matching_descendant: false,
            id: TreeId::Collection(reg.root),
            item: Item::Collection {
                id: reg.root,
                online: true,
                path: path.clone(),
                name,
            },
        });
    }
    let mut container = home.as_path();
    let mut program = None;
    // A numbered CD directory can be replaced by its authored release in
    // the parent's branch. Use the ordinary branch projection to confirm it.
    if reg.release.is_some() {
        let mut candidates = vec![home.as_path()];
        if let Some(parent) = home.parent().filter(|p| crate::library::within(p, &path)) {
            candidates.push(parent);
        }
        'candidates: for folder in candidates {
            let mut cursor = Default::default();
            loop {
                let (nodes, _, more) = indexed_contents(store, reg.root, folder, cursor)?;
                for node in nodes {
                    let Item::Release { release, .. } = &node.item else {
                        continue;
                    };
                    let Some(disc) = release.discs.iter().find(|disc| disc.id == id) else {
                        continue;
                    };
                    program = Some(disc.clone());
                    container = folder;
                    let Some(folders) = folder_lineage(&path, container, Some(reg.root)) else {
                        return Ok(Vec::new());
                    };
                    lineage.extend(folders);
                    lineage.push(node);
                    break 'candidates;
                }
                let Some(Cursor::Indexed(IndexedCursor::Contents(next))) = more else {
                    break;
                };
                cursor = next;
            }
        }
    }
    if program.is_none() {
        let Some(folders) = folder_lineage(&path, container, Some(reg.root)) else {
            return Ok(Vec::new());
        };
        lineage.extend(folders);
        if album.authored
            || crate::release::disc_folder(&crate::library::folder_name(&home)).is_some()
        {
            album.release = None;
            program = Some(Arc::new(Disc::from_album(album)));
        }
    }
    if let Some(disc) = &program {
        lineage.push(Node::disc(disc.clone(), container.to_owned()));
    }
    lineage.push(Node::track(row.track, program));
    Ok(lineage)
}

fn physical_path(
    base: &Path,
    target: &TreeId,
    extent: Option<&crate::library::DiscTrack>,
    group_limit: usize,
    alive: &dyn Fn() -> bool,
) -> anyhow::Result<Vec<Node>> {
    let TreeId::File { source, chapter } = target else {
        return Ok(Vec::new());
    };
    let anchor = Path::new(chapter.as_ref().map_or(source, |(sheet, ..)| sheet));
    let Some(home) = anchor.parent() else {
        return Ok(Vec::new());
    };
    let Some(mut lineage) = folder_lineage(base, home, None) else {
        return Ok(Vec::new());
    };
    let Some(name) = anchor.file_name() else {
        return Ok(Vec::new());
    };
    let cursor = crate::library::ListingCursor::AtFile(name.to_string_lossy().into_owned());
    let page = physical_page(home, Some(&cursor), 32, group_limit, alive)?;
    let matches = |t: &Track| {
        TreeId::track(t) == *target
            && extent.is_none_or(|d| t.disc.as_ref().is_some_and(|chapter| chapter.end == d.end))
    };
    for node in page.nodes() {
        match &node.item {
            Item::Disc { disc, .. } => {
                if let Some(track) = disc.tracks.iter().find(|track| matches(track)) {
                    lineage.push(node.clone());
                    lineage.push(Node::track(track.clone(), Some(disc.clone())));
                    return Ok(lineage);
                }
            }
            Item::Release { release, .. } => {
                for disc in &release.discs {
                    if let Some(track) = disc.tracks.iter().find(|track| matches(track)) {
                        lineage.push(node.clone());
                        lineage.push(Node::disc(disc.clone(), home.to_owned()));
                        lineage.push(Node::track(track.clone(), Some(disc.clone())));
                        return Ok(lineage);
                    }
                }
            }
            Item::Track { track, .. } if matches(track) => {
                lineage.push(node.clone());
                return Ok(lineage);
            }
            _ => {}
        }
    }
    Ok(Vec::new())
}

pub(in crate::browser) fn recording_identity(key: &RowKey) -> Option<TreeId> {
    match key {
        RowKey::Key(id) => Some(TreeId::IndexedTrack(*id)),
        RowKey::Track(source, disc) => Some(TreeId::File {
            source: crate::store::path_key(source),
            chapter: disc
                .as_ref()
                .map(|d| (crate::store::path_key(&d.sheet), d.number, d.start)),
        }),
        RowKey::Tree(id @ (TreeId::IndexedTrack(_) | TreeId::File { .. })) => Some(id.clone()),
        _ => None,
    }
}

/// The normal page containing one lineage member and cursors for the preceding
/// interval. No row can be displayed until its sibling position is known.
struct Located {
    id: TreeId,
    branch: Branch,
}

impl reader::Reader {
    async fn locate(
        &self,
        source: Source,
        target: TreeId,
        sort: Option<(Field, ColumnSort)>,
        cx: &AsyncApp,
    ) -> anyhow::Result<Located> {
        let mut page = self.read(source.clone(), None, sort, cx).await?;
        let mut prefix = Vec::new();
        let mut cursor = None;
        if let Some(index) = page.snapshot.clone() {
            let id = target.clone();
            let descendants = self.descendants.clone();
            let (rank, found) = cx
                .background_spawn(async move {
                    let rank = index
                        .position(&id)?
                        .ok_or_else(|| anyhow::anyhow!("Recording moved"))?;
                    let page = index.page(rank / PAGE * PAGE)?;
                    let page = match descendants {
                        Some(index) => index.annotate(page)?,
                        None => page,
                    };
                    anyhow::Ok((rank, page))
                })
                .await?;
            for start in (0..rank / PAGE * PAGE).step_by(PAGE) {
                prefix.push(Slot::unloaded(
                    (start > 0).then_some(Cursor::Offset(start)),
                    PAGE,
                ));
            }
            let start = rank / PAGE * PAGE;
            cursor = (start > 0).then_some(Cursor::Offset(start));
            page = found;
        } else {
            loop {
                let id = target.clone();
                let (next, found) = cx
                    .background_spawn(async move {
                        let found = page.nodes().any(|n| n.id == id);
                        (page, found)
                    })
                    .await;
                page = next;
                if found {
                    break;
                }
                let next = page
                    .more
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Recording moved"))?;
                prefix.push(Slot::unloaded(cursor, page.pages[0].len));
                cursor = Some(next.clone());
                page = self.read(source.clone(), Some(next), None, cx).await?;
            }
        }
        page.pages[0].start = cursor;
        prefix.append(&mut page.pages);
        page.pages = prefix;
        Ok(Located {
            id: target,
            branch: page,
        })
    }
}

impl Slot {
    fn unloaded(start: Option<Cursor>, len: usize) -> Self {
        Self {
            start,
            len,
            slice: None,
            nodes: None,
            children: Vec::new(),
            rows: len,
            matched_rows: None,
            used: 0,
        }
    }
}

impl Tree {
    fn open_lineage(&mut self, path: Vec<Located>, lens: Option<&Lens>) {
        let mut parent = None;
        let mut deep = self.deep.contains(&None);
        let budget = self.node_limit() / 2;
        let path_nodes: usize = path.iter().map(|p| p.branch.resident()).sum();
        let width = if path_nodes > budget {
            (budget / path.len()).max(1)
        } else {
            usize::MAX
        };
        let mut touched = Vec::new();
        self.selected = path.last().map(|member| member.id.clone());
        for Located { id, mut branch } in path {
            self.pending.remove(&parent);
            let mut target = branch.pages.pop().expect("located page");
            let mut pages = Vec::new();
            if let Some(resident) = self.branches.remove(&parent) {
                // Reuse ordinary or subdivided source batches before the target.
                let mut old = resident.pages.into_iter().peekable();
                for prefix in branch.pages {
                    if old.peek().is_some_and(|p| p.start == prefix.start) {
                        let start = pages.len();
                        while old.peek().is_some_and(|p| p.start == prefix.start) {
                            pages.push(old.next().unwrap());
                        }
                        if pages[start..].iter().map(|slot| slot.len).sum::<usize>() != prefix.len {
                            pages.truncate(start);
                            pages.push(prefix);
                        }
                    } else {
                        pages.push(prefix);
                    }
                }
                while old.peek().is_some_and(|p| p.start == target.start) {
                    target.children.append(&mut old.next().unwrap().children);
                }
                branch.pages = pages;
                pages = if old.peek().is_some_and(|p| p.start == branch.more) {
                    branch.more = resident.more;
                    old.collect()
                } else {
                    Vec::new()
                };
            }
            let target_page = branch.pages.len();
            target.used = self.clock;
            branch.pages.push(target);
            branch.pages.append(&mut pages);
            branch.deep = deep;
            self.branches.insert(parent.clone(), branch);
            self.restore_children(&parent, target_page);
            // Leave room for an ordinary incoming batch during subsequent prefetch.
            let index = self.bound_lineage_page(&parent, target_page, &id, width, lens);
            touched.push((parent.clone(), index));
            if !matches!(id, TreeId::IndexedTrack(_) | TreeId::File { .. }) {
                self.links.insert(id.clone(), parent.clone());
                self.closed.remove(&id);
                if !self.is_open(&id, deep) {
                    if self.expanded.len() == EXPANSION_LIMIT {
                        self.expanded.remove(0);
                    }
                    self.expanded.push(id.clone());
                }
                deep = self.opens_by_default(&id, deep);
            }
            parent = Some(id);
        }
        self.reconcile();
        for (owner, index) in touched {
            self.settle(&owner, index, lens);
        }
    }

    /// Keep a contiguous window around the lineage member. Both sides use
    /// the same source cursor and reload their own slice of that batch.
    fn bound_lineage_page(
        &mut self,
        owner: &Option<TreeId>,
        index: usize,
        id: &TreeId,
        width: usize,
        lens: Option<&Lens>,
    ) -> usize {
        let pages = &mut self.branches.get_mut(owner).unwrap().pages;
        let slot = &mut pages[index];
        if slot.len <= width {
            return index;
        }
        let mut nodes = slot.nodes.take().unwrap();
        let at = nodes
            .iter()
            .position(|node| &node.id == id)
            .expect("located member");
        let begin = at.saturating_sub(width / 2).min(nodes.len() - width);
        let end = begin + width;
        let after = nodes.split_off(end);
        let middle = nodes.split_off(begin);
        let start = slot.start.clone();
        let used = slot.used;
        let mut segments = Vec::new();
        for (offset, nodes) in [(0, nodes), (begin, middle), (end, after)] {
            if nodes.is_empty() {
                continue;
            }
            let mut part = Slot::new(start.clone(), nodes);
            part.slice = Some(offset..offset + part.len);
            part.used = used;
            segments.push(part);
        }
        let count = segments.len();
        pages.splice(index..=index, segments);
        let keep = index + usize::from(begin > 0);
        for i in index..index + count {
            if i != keep {
                self.unload_page(owner, i, lens);
            }
        }
        keep
    }
}

impl Browser {
    pub(in crate::browser) fn finish_tree_reveal(
        &mut self,
        target: TreeId,
        key: RowKey,
        viewport: Viewport,
        center: bool,
        cx: &mut Context<Self>,
    ) {
        if self.tree.read(cx).delegate().position(&target).is_some() {
            self.pending = None;
            self.tree.update(cx, |table, cx| {
                TreeTable::land(table, target, viewport, center, cx);
            });
            return;
        }
        let revision = self.intent.next();
        let ticket = Ticket::default();
        let generation = ticket.next();
        let location = self.location.clone();
        let store = self.catalog.read(cx).handle();
        let workers = self.tree.read(cx).delegate().workers.clone();
        let tree = &self.tree.read(cx).delegate().tree;
        let Some(root) = tree.root.clone() else {
            return;
        };
        let order = tree.sort;
        let indexes: HashMap<_, _> = tree
            .branches
            .iter()
            .filter_map(|(id, b)| Some((id.clone(), b.snapshot.clone()?)))
            .collect();
        self.tree.update(cx, |table, _| {
            table.delegate_mut().tree.reveal = Some(Request(ticket.clone()))
        });
        let descendants = self
            .tree
            .read(cx)
            .delegate()
            .tree
            .descendants
            .clone()
            .filter(|d| d.query == self.query);
        let window = self.window;
        cx.spawn(async move |browser, cx| {
            let reader = reader::Reader {
                descendants,
                handle: store.clone(),
                workers: workers.clone(),
                ticket: ticket.clone(),
                generation,
            };
            let result = match (location, target.clone(), store) {
                (Location::Library(root), TreeId::IndexedTrack(id), Some(store)) => {
                    store
                        .query(&ticket, generation, move |s| indexed_path(s, root, id))
                        .await
                }
                (Location::Folder(base), id @ TreeId::File { .. }, _) => {
                    let ticket = ticket.clone();
                    cx.background_spawn(async move {
                        let _worker = workers.acquire().await;
                        let alive = || ticket.is_current(generation);
                        if !alive() {
                            return Err(crate::store::Canceled.into());
                        }
                        let extent = match &key {
                            RowKey::Track(_, extent) => extent.as_ref(),
                            _ => None,
                        };
                        physical_path(&base, &id, extent, crate::library::GROUPED_FILES, &alive)
                    })
                    .await
                }
                _ => Ok(Vec::new()),
            };
            let result = async {
                let nodes = result?;
                let mut path = Vec::new();
                let mut source = Some(root);
                let mut sort = order;
                let mut parent = None;
                for node in nodes {
                    let current = source
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("Invalid recording hierarchy"))?;
                    source = node.source();
                    let next_sort = if matches!(node.item, Item::Disc { .. } | Item::Release { .. })
                    {
                        None
                    } else {
                        order
                    };
                    let (current, current_sort) = match indexes.get(&parent) {
                        Some(index) => (Source::Snapshot(index.clone()), None),
                        None => (current, sort),
                    };
                    parent = Some(node.id.clone());
                    path.push(reader.locate(current, node.id, current_sort, cx).await?);
                    sort = next_sort;
                }
                anyhow::Ok(path)
            }
            .await;
            if !ticket.is_current(generation) {
                return;
            }
            let _ = cx.update(|cx| {
                window.update(cx, |_, w, cx| {
                    let _ = browser.update(cx, |b, cx| {
                        if !ticket.is_current(generation)
                            || !b.navigation_is_current(revision)
                            || !b.listing_tree()
                            || b.tree.read(cx).delegate().tree.sort != order
                        {
                            return;
                        }
                        b.cancel_tree_reveal(cx);
                        b.pending = None;
                        if let Ok(nodes) = result
                            && !nodes.is_empty()
                        {
                            b.tree.update(cx, |table, cx| {
                                let d = table.delegate_mut();
                                let index = d.tree.descendants.clone();
                                let marks = |track: &Track| b.catalog.read(cx).mark_of(track);
                                let lens = Lens::new(&d.filter, index.as_deref(), &marks);
                                d.tree.open_lineage(nodes, lens.as_ref());
                                TreeTable::project(table, cx);
                            });
                            if center
                                && !b.query.is_empty()
                                && b.tree.read(cx).delegate().position(&target).is_none()
                            {
                                b.clear_search(w, cx);
                                b.refresh_tree(cx);
                            }
                            b.tree.update(cx, |table, cx| {
                                TreeTable::land(table, target, viewport, center, cx);
                            });
                            b.pump_tree(cx);
                            b.update_tree_playable(cx);
                        } else if center {
                            w.push_notification("This recording is unavailable in the tree.", cx);
                        }
                        cx.emit(ViewChanged);
                        cx.notify();
                    });
                })
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn a_physical_reveal_reaches_a_cue_after_the_first_page_by_identity() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("Deep Set");
        std::fs::create_dir(&home).unwrap();
        for i in 0..3 {
            std::fs::write(home.join(format!("File {i:05}.wav")), []).unwrap();
        }
        let sheet = home.join("Last CUE.cue");
        std::fs::write(home.join("Last CUE.wav"), []).unwrap();
        std::fs::write(home.join("Other.wav"), []).unwrap();
        std::fs::write(&sheet, "FILE \"Last CUE.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE Other.wav WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00").unwrap();
        let targets = crate::cue::read(&sheet).unwrap();
        let track = &targets[1];
        let id = TreeId::track(track);
        let path = physical_path(dir.path(), &id, track.disc.as_ref(), 2, &|| true).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path.last().unwrap().id, id);
        assert!(path[0].expandable());
        assert!(matches!(path[1].item, Item::Disc { .. }));
    }

    #[::core::prelude::v1::test]
    fn indexed_reveal_resolves_a_multifile_chapter_under_the_containing_release() {
        let dir = tempfile::tempdir().unwrap();
        for disc in 1..=2 {
            let path = dir.path().join(format!("Set/CD {disc}"));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("a.wav"), b"source").unwrap();
            std::fs::write(path.join("b.wav"), b"another source").unwrap();
            std::fs::write(path.join("image.cue"), format!("TITLE \"Set (CD{disc})\"\nFILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE b.wav WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00")).unwrap();
        }
        let mut store = Store::memory();
        let root = store.add_root(dir.path()).unwrap();
        crate::store::index_root(&mut store, root, None, &|| true, &mut |_| {}).unwrap();
        let scope = Scope::Library(Some(root));
        let rows = store.rows(scope, &store.disc_keys(scope).unwrap()).unwrap();
        let target = rows
            .iter()
            .find(|row| row.track.source.ends_with("CD 2/b.wav"))
            .unwrap();
        let path = indexed_path(&store, Some(root), target.id).unwrap();
        assert!(matches!(&path[0].item, Item::Folder { path, .. } if path.ends_with("Set")));
        assert!(matches!(path[1].item, Item::Release { .. }));
        assert!(matches!(path[2].item, Item::Disc { .. }));
        assert_eq!(path[3].id, TreeId::IndexedTrack(target.id));
    }
    #[::core::prelude::v1::test]
    fn deep_selection_survives_ancestor_pressure_and_sibling_reloads() {
        let levels: Vec<Vec<Node>> = (0..3)
            .map(|depth| {
                (0..8)
                    .map(|i| {
                        let path = PathBuf::from(format!("fixture/level-{depth}/item-{i}"));
                        if depth == 2 {
                            Node::track(Arc::new(Track::file(path)), None)
                        } else {
                            Node::folder(None, path)
                        }
                    })
                    .collect()
            })
            .collect();
        let target = &levels[2][4].id;
        let mut tree = Tree {
            node_limit: Some(20),
            ..Default::default()
        };
        let locate = || {
            levels
                .iter()
                .map(|nodes| Located {
                    id: nodes[4].id.clone(),
                    branch: Branch::page(nodes.clone(), None),
                })
                .collect()
        };
        let verify = |tree: &Tree| {
            let projection = tree.project(None);
            let mut rank = 0;
            let found = projection.rows.iter().find(|row| {
                if row.node().is_some_and(|n| &n.id == target) {
                    true
                } else {
                    rank += row.span();
                    false
                }
            });
            assert!(found.is_some());
            assert_eq!(rank, 14);
            assert_eq!(projection.len, 24);
            assert_eq!(tree.selected.as_ref(), Some(target));
            assert!(tree.loaded() <= tree.node_limit());
        };
        tree.open_lineage(locate(), None);
        verify(&tree);
        // Force overlap between late sibling reads in different ancestors.
        let mut parent = None;
        for nodes in &levels {
            for page in [2, 0] {
                assert!(tree.receive(
                    &parent,
                    &Fetch::Page(page, None),
                    Branch::page(nodes.clone(), None),
                    None
                ));
                verify(&tree);
            }
            parent = Some(nodes[4].id.clone());
        }
        tree.open_lineage(locate(), None);
        verify(&tree);
    }

    #[::core::prelude::v1::test]
    fn revealed_recording_keeps_its_rank_and_siblings_during_prefetch_and_backward_paging() {
        const BATCH: usize = 4;
        let source = |offset: usize| {
            (offset..offset + BATCH)
                .map(|i| {
                    Node::track(
                        Arc::new(Track::file(format!("fixture/{i:06}.wav").into())),
                        None,
                    )
                })
                .collect::<Vec<_>>()
        };
        let offset = 6 * BATCH;
        let nodes = source(offset);
        let target = nodes[BATCH - 2].id.clone();
        let mut branch = Branch::page(nodes, Some(Cursor::Offset(offset + BATCH)));
        branch.pages[0].start = Some(Cursor::Offset(offset));
        let mut pages = (0..offset)
            .step_by(BATCH)
            .map(|at| Slot::unloaded((at > 0).then_some(Cursor::Offset(at)), BATCH))
            .collect::<Vec<_>>();
        pages.append(&mut branch.pages);
        branch.pages = pages;
        let mut tree = Tree::default();
        tree.open_lineage(
            vec![Located {
                id: target.clone(),
                branch,
            }],
            None,
        );
        let verify = |tree: &Tree| {
            let projection = tree.project(None);
            let mut rank = 0;
            for row in projection.rows {
                if row.node().is_some_and(|n| n.id == target) {
                    assert_eq!(rank, offset + BATCH - 2);
                    return;
                }
                rank += row.span();
            }
            panic!("target vanished");
        };
        verify(&tree);
        tree.receive(
            &None,
            &Fetch::Next(Cursor::Offset(offset + BATCH)),
            Branch::page(source(offset + BATCH), None),
            None,
        );
        verify(&tree);
        tree.receive(
            &None,
            &Fetch::Page(0, None),
            Branch::page(source(0), Some(Cursor::Offset(BATCH))),
            None,
        );
        verify(&tree);
        let sibling = &tree.branches[&None].pages[6].nodes.as_ref().unwrap()[BATCH - 1];
        assert_eq!(
            sibling.value(Field::Title),
            format!("{:06}", offset + BATCH - 1)
        );
    }
}

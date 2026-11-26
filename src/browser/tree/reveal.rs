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
        for folder in candidates {
            let (nodes, _, _) = indexed_contents(store, reg.root, folder, Default::default())?;
            if let Some(node) = nodes.into_iter().find(|node| {
                matches!(&node.item, Item::Release { release, .. } if release.discs.iter().any(|disc| disc.id == id))
            }) {
                let Item::Release { release, .. } = &node.item else { unreachable!() };
                program = release.discs.iter().find(|disc| disc.id == id).cloned();
                container = folder;
                let Some(folders) = folder_lineage(&path, container, Some(reg.root)) else {
                    return Ok(Vec::new());
                };
                lineage.extend(folders);
                lineage.push(node);
                break;
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
    let page = physical_page(home, Some(&cursor), 32, alive)?;
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
            let (rank, found) = cx
                .background_spawn(async move {
                    let rank = index
                        .position(&id)?
                        .ok_or_else(|| anyhow::anyhow!("Recording moved"))?;
                    anyhow::Ok((rank, index.page(rank / PAGE * PAGE)?))
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
                page = self.raw(source.clone(), Some(next), cx).await?;
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
            nodes: None,
            rows: len,
            matched_rows: None,
            used: 0,
        }
    }
}

impl Tree {
    fn open_lineage(&mut self, path: Vec<Located>) {
        let mut parent = None;
        let mut deep = self.deep.contains(&None);
        for Located { id, mut branch } in path {
            self.pending.remove(&parent);
            let target_page = branch.pages.len() - 1;
            if let Some(mut resident) = self.branches.remove(&parent) {
                // Preserve already loaded siblings and subtrees of the same generation.
                for (old, incoming) in resident
                    .pages
                    .iter_mut()
                    .zip(&mut branch.pages[..target_page])
                {
                    if old.start == incoming.start && old.len == incoming.len {
                        std::mem::swap(old, incoming);
                    }
                }
                if resident
                    .pages
                    .get(target_page + 1)
                    .is_some_and(|next| next.start == branch.more)
                {
                    branch.pages.extend(resident.pages.drain(target_page + 1..));
                    branch.more = resident.more;
                }
            }
            branch.pages[target_page].used = self.clock + 1;
            branch.deep = deep;
            self.branches.insert(parent.clone(), branch);
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
        // Make the loaded page of each ancestor recent before applying the
        // normal eviction budget, just as if the viewport had reached it.
        let touched: Vec<_> = self
            .branches
            .iter()
            .filter_map(|(id, branch)| {
                branch
                    .pages
                    .iter()
                    .position(|p| p.used > self.clock)
                    .map(|i| (id.clone(), i))
            })
            .collect();
        for (id, i) in touched {
            self.settle(&id, i, false, None);
        }
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
        let window = self.window;
        cx.spawn(async move |browser, cx| {
            let reader = reader::Reader {
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
                        physical_path(&base, &id, extent, &alive)
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
                        if !b.navigation_is_current(revision)
                            || !b.listing_tree()
                            || b.tree.read(cx).delegate().tree.sort != order
                        {
                            return;
                        }
                        if let Ok(nodes) = result
                            && !nodes.is_empty()
                        {
                            b.tree.update(cx, |table, cx| {
                                table.delegate_mut().tree.open_lineage(nodes);
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
        for i in 0..=crate::library::GROUPED_FILES {
            std::fs::write(home.join(format!("File {i:05}.wav")), []).unwrap();
        }
        let sheet = home.join("Last CUE.cue");
        std::fs::write(home.join("Last CUE.wav"), []).unwrap();
        std::fs::write(home.join("Other.wav"), []).unwrap();
        std::fs::write(&sheet, "FILE \"Last CUE.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE Other.wav WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00").unwrap();
        let targets = crate::cue::read(&sheet).unwrap();
        let track = &targets[1];
        let id = TreeId::track(track);
        let path = physical_path(dir.path(), &id, track.disc.as_ref(), &|| true).unwrap();
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
    fn revealed_recording_keeps_its_rank_and_siblings_during_prefetch_and_backward_paging() {
        let source = |offset: usize| {
            (offset..offset + PAGE)
                .map(|i| {
                    Node::track(
                        Arc::new(Track::file(format!("fixture/{i:06}.wav").into())),
                        None,
                    )
                })
                .collect::<Vec<_>>()
        };
        let offset = 6 * PAGE;
        let nodes = source(offset);
        let target = nodes[PAGE - 2].id.clone();
        let mut branch = Branch::page(nodes, Some(Cursor::Offset(offset + PAGE)));
        branch.pages[0].start = Some(Cursor::Offset(offset));
        let mut pages = (0..offset)
            .step_by(PAGE)
            .map(|at| Slot::unloaded((at > 0).then_some(Cursor::Offset(at)), PAGE))
            .collect::<Vec<_>>();
        pages.append(&mut branch.pages);
        branch.pages = pages;
        let mut tree = Tree::default();
        tree.open_lineage(vec![Located {
            id: target.clone(),
            branch,
        }]);
        let verify = |tree: &Tree| {
            let projection = tree.project(None);
            let mut rank = 0;
            for row in projection.rows {
                if row.node().is_some_and(|n| n.id == target) {
                    assert_eq!(rank, offset + PAGE - 2);
                    return;
                }
                rank += row.span();
            }
            panic!("target vanished");
        };
        verify(&tree);
        tree.receive(
            &None,
            &Fetch::Next(Cursor::Offset(offset + PAGE)),
            Branch::page(source(offset + PAGE), None),
            None,
        );
        verify(&tree);
        tree.receive(
            &None,
            &Fetch::Page(0, None),
            Branch::page(source(0), Some(Cursor::Offset(PAGE))),
            None,
        );
        verify(&tree);
        let sibling = &tree.branches[&None].pages[6].nodes.as_ref().unwrap()[PAGE - 1];
        assert_eq!(
            sibling.value(Field::Title),
            format!("{:06}", offset + PAGE - 1)
        );
    }
}

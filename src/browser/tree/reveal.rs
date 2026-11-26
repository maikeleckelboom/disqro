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

impl Tree {
    fn open_lineage(&mut self, nodes: Vec<Node>) {
        self.direct.clear();
        let mut deep = self.deep.contains(&None);
        let mut parent = None;
        for node in nodes {
            let id = node.id.clone();
            if node.expandable() {
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
            self.direct.insert(parent, node);
            parent = Some(id);
        }
        self.reconcile();
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
        let ticket = self.intent.clone();
        let revision = ticket.next();
        let location = self.location.clone();
        let store = self.catalog.read(cx).handle();
        let workers = self.tree.read(cx).delegate().workers.clone();
        let window = self.window;
        cx.spawn(async move |browser, cx| {
            let result = match (location, target.clone(), store) {
                (Location::Library(root), TreeId::IndexedTrack(id), Some(store)) => {
                    store
                        .query(&ticket, revision, move |s| indexed_path(s, root, id))
                        .await
                }
                (Location::Folder(base), id @ TreeId::File { .. }, _) => {
                    let ticket = ticket.clone();
                    cx.background_spawn(async move {
                        let _worker = workers.acquire().await;
                        let alive = || ticket.is_current(revision);
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
            if !ticket.is_current(revision) {
                return;
            }
            let _ = cx.update(|cx| {
                window.update(cx, |_, w, cx| {
                    let _ = browser.update(cx, |b, cx| {
                        if !b.navigation_is_current(revision) || !b.listing_tree() {
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
        let mut tree = Tree {
            root: Some(Source::Folder {
                root: None,
                path: dir.path().into(),
            }),
            ..Default::default()
        };
        tree.open_lineage(path);
        assert_eq!(
            tree.project(None)
                .rows
                .iter()
                .filter_map(TreeRow::node)
                .filter(|node| node.id == id)
                .count(),
            1
        );
        assert!(tree.next_request().is_some());
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
}

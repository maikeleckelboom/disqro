use super::{
    Boundary, Branch, Cursor, Descendants, EXPANSION_LIMIT, Fetch, Item, LOADED_NODE_LIMIT, Lens,
    Node, PAGE, RESIDENT_PAGES, Request, Source, Tree, TreeId, TreeRow, Want, indexed_branch,
    physical_branch,
};
use crate::library::Track;
use crate::{
    store::{Mark, Scope, Store, Ticket, index_root},
    table::Field,
};
use gpui_kit::component::table::ColumnSort;
use std::{
    collections::HashMap,
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

fn cue(folder: &Path, name: &str, chapters: u8, multiline: bool) {
    fs::create_dir_all(folder).unwrap();
    let mut sheet = format!("TITLE \"{name}\"\n");
    for chapter in 1..=chapters {
        if chapter == 1 || multiline {
            let source = format!("Source {chapter:02}.wav");
            write_wav(&folder.join(&source), if multiline { 2 } else { 45 });
            sheet.push_str(&format!("FILE \"{source}\" WAVE\n"));
        }
        let seconds = if multiline { 0 } else { (chapter - 1) * 2 };
        sheet.push_str(&format!("TRACK {chapter:02} AUDIO\nTITLE \"Chapter {chapter:02}\"\nINDEX 01 00:{seconds:02}:00\n"));
    }
    fs::write(folder.join(format!("{name}.cue")), sheet).unwrap();
}

fn write_wav(path: &Path, seconds: u32) {
    let samples = seconds * 8000 * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(samples + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&8000_u32.to_le_bytes());
    wav.extend_from_slice(&16000_u32.to_le_bytes());
    wav.extend_from_slice(b"\x02\0\x10\0data");
    wav.extend_from_slice(&samples.to_le_bytes());
    wav.resize(wav.len() + samples as usize, 0);
    fs::write(path, wav).unwrap();
}

#[test]
fn physical_tree_loads_only_expanded_branches_and_keeps_all_authored_chapters() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let deep = root.join("Nested").join("Image Album");
    cue(&deep, "Image Album", 20, false);
    cue(&root.join("Multipart"), "Multipart", 3, true);
    write_wav(&root.join("Loose.wav"), 1);
    let start = physical_branch(root, None, &|| true).unwrap();
    assert_eq!(start.nodes().count(), 3);
    assert!(start.nodes().all(|n| !matches!(n.item, Item::Disc { .. })));
    let Node { id, .. } = start
        .nodes()
        .find(|n| n.value(Field::Title) == "Nested")
        .unwrap();
    let parent_id = id.clone();
    let mut tree = Tree::default();
    tree.branches.insert(None, start);
    assert!(tree.next_request().is_none());
    tree.toggle(parent_id.clone(), &None);
    assert_eq!(tree.next_request().unwrap().0, Some(parent_id.clone()));
    let nested = physical_branch(&root.join("Nested"), None, &|| true).unwrap();
    let folder = nested.nodes().next().unwrap().id.clone();
    tree.branches.insert(Some(parent_id.clone()), nested);
    tree.toggle(folder.clone(), &Some(parent_id.clone()));
    let branch = physical_branch(&deep, None, &|| true).unwrap();
    assert_eq!(branch.nodes().count(), 1);
    let disc = branch.nodes().next().unwrap().clone();
    let Some(Source::Loaded(children)) = disc.source() else {
        panic!("Expected an authored CUE");
    };
    assert_eq!(children.len(), 20);
    let key = children[19].id.clone();
    tree.branches.insert(Some(folder.clone()), branch);
    tree.toggle(disc.id.clone(), &Some(folder.clone()));
    tree.branches
        .insert(Some(disc.id.clone()), Branch::page(children.to_vec(), None));
    let descending = tree.visible(None, "", &|_| Mark::default());
    let chapters = descending
        .iter()
        .filter_map(TreeRow::node)
        .filter(|n| matches!(n.item, Item::Track { disc: Some(_), .. }))
        .collect::<Vec<_>>();
    assert_eq!(chapters[0].value(Field::Title), "Chapter 01");
    assert_eq!(chapters[19].id, key);
    let matches = tree.visible(None, "chapter 20", &|_| Mark::default());
    let labels: Vec<_> = matches
        .iter()
        .filter_map(TreeRow::node)
        .map(|n| n.value(Field::Title))
        .collect();
    assert_eq!(
        labels,
        ["Nested", "Image Album", "Image Album", "Chapter 20"]
    );
    let ticket = Ticket::default();
    let generation = ticket.next();
    tree.pending.insert(Some(disc.id), Request(ticket.clone()));
    tree.toggle(parent_id, &None);
    assert!(!ticket.is_current(generation));
    assert_eq!(tree.branches.len(), 1);
    let multi = physical_branch(&root.join("Multipart"), None, &|| true).unwrap();
    let Some(Source::Loaded(children)) = multi.nodes().next().unwrap().source() else {
        panic!("Expected a multi-file CUE");
    };
    assert_eq!(children.len(), 3);
}

#[test]
fn indexed_children_retain_names_and_build_only_verified_releases() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Music");
    let release = root.join("Live Set");
    cue(&release.join("CD 1"), "Live Set (CD1)", 2, false);
    cue(&release.join("CD 2"), "Live Set (CD2)", 2, true);
    fs::create_dir_all(root.join("Empty Folder")).unwrap();
    write_wav(&root.join("Loose.wav"), 1);
    let mut store = Store::open(&temp.path().join("library.sqlite3")).unwrap();
    let id = store.add_root(&root).unwrap();
    index_root(&mut store, id, None, &|| true, &mut |_| {}).unwrap();
    let top = indexed_branch(&store, id, &root).unwrap();
    assert_eq!(
        top.nodes()
            .map(|n| n.value(Field::Title))
            .collect::<Vec<_>>(),
        ["Empty Folder", "Live Set", "Loose"]
    );
    let branch = indexed_branch(&store, id, &release).unwrap();
    assert_eq!(branch.nodes().count(), 1);
    let Some(Source::Loaded(discs)) = branch.nodes().next().unwrap().source() else {
        panic!("Expected a verified release");
    };
    assert_eq!(
        discs
            .iter()
            .map(|n| n.value(Field::Title))
            .collect::<Vec<_>>(),
        ["CD 1", "CD 2"]
    );
    let Some(Source::Album(disc, _)) = discs[1].source() else {
        panic!("Expected an indexed disc");
    };
    let keys = store.disc_keys(Scope::Album(disc)).unwrap();
    assert_eq!(keys.len(), 2);
    let again = indexed_branch(&store, id, &release).unwrap();
    assert_eq!(
        branch.nodes().next().unwrap().id,
        again.nodes().next().unwrap().id
    );
}

#[test]
fn expansion_bookmarks_are_bounded_and_never_reuse_a_pending_generation() {
    let mut tree = Tree::default();
    let ids = (0..300).map(TreeId::Collection).collect::<Vec<_>>();
    let saved = serde_json::to_vec(&ids).unwrap();
    tree.set_expansion(super::Expansion {
        open: serde_json::from_slice(&saved).unwrap(),
        ..Default::default()
    });
    assert_eq!(tree.expanded.len(), EXPANSION_LIMIT);
    assert!(tree.expanded.contains(&TreeId::Collection(0)));
    tree.toggle(TreeId::Collection(500), &None);
    assert!(!tree.expanded.contains(&TreeId::Collection(0)));
    assert_eq!(tree.expanded.len(), EXPANSION_LIMIT);
}

fn folders(count: usize) -> Vec<Node> {
    (0..count)
        .map(|i| Node::folder(None, PathBuf::from(format!("C:\\Music\\Folder {i:05}"))))
        .collect()
}

fn titles(rows: &[TreeRow]) -> Vec<String> {
    rows.iter()
        .filter_map(TreeRow::node)
        .map(|n| n.value(Field::Title))
        .collect()
}

/// Drives a tree as the table does: projects it, keeps the pages near a
/// viewport in use, requests the batches the viewport approaches and serves
/// every request from `children`, while measuring what stays loaded.
const BATCH: usize = 8;
const AHEAD: usize = 2;
const WINDOW: usize = 3;
const BUDGET: usize = 8 * BATCH;

struct Harness {
    tree: Tree,
    children: HashMap<Option<TreeId>, Vec<Node>>,
    shown: Range<usize>,
    page_size: usize,
    filter: String,
    sort: Option<(Field, ColumnSort)>,
    rows: Vec<TreeRow>,
    len: usize,
    boundaries: Vec<Boundary>,
    /// The most items ever loaded at once.
    peak: usize,
    /// Batches read, by branch.
    reads: HashMap<Option<TreeId>, usize>,
}

impl Harness {
    fn new(root: Vec<Node>) -> Self {
        let mut children = HashMap::new();
        children.insert(None, root.clone());
        Self {
            tree: Tree {
                root: Some(Source::Loaded(root.into())),
                node_limit: Some(BUDGET),
                ..Default::default()
            },
            children,
            shown: 0..WINDOW,
            page_size: BATCH,
            filter: String::new(),
            sort: None,
            rows: Vec::new(),
            len: 0,
            boundaries: Vec::new(),
            peak: 0,
            reads: HashMap::new(),
        }
    }

    fn project(&mut self) {
        self.tree.set_sort(self.sort);
        self.tree.set_query(&self.filter);
        let marks = |_: &Track| Mark::default();
        let lens = Lens::new(&self.filter, None, &marks);
        let view = self.tree.project(lens.as_ref());
        let (rows, len, boundaries, spans) = (view.rows, view.len, view.boundaries, view.spans);
        assert_eq!(rows.iter().map(TreeRow::span).sum::<usize>(), len);
        self.rows = rows;
        self.len = len;
        self.boundaries = boundaries;
        let near = self.shown.start.saturating_sub(AHEAD)..self.shown.end.saturating_add(AHEAD);
        self.tree.touch(&spans, near);
    }

    /// Serves requests until the tree waits for nothing near the viewport.
    fn settle(&mut self) {
        for _ in 0..10_000 {
            self.project();
            let wants: Vec<_> = self
                .boundaries
                .iter()
                .filter(|b| b.near(&self.shown, AHEAD))
                .map(|b| (b.branch.clone(), b.want, b.search))
                .collect();
            let first = wants
                .iter()
                .filter(|(_, want, _)| *want == Want::First)
                .filter_map(|(branch, _, _)| branch.clone())
                .collect();
            self.tree.want_first(first);
            for (branch, want, search) in wants {
                self.tree.want(&branch, want, search);
            }
            let Some((id, _, fetch)) = self.tree.next_request() else {
                return;
            };
            let all = self.children.get(&id).cloned().unwrap_or_default();
            let offset = match fetch.cursor() {
                Some(Cursor::Offset(offset)) => *offset,
                _ => 0,
            };
            *self.reads.entry(id.clone()).or_default() += 1;
            let marks = |_: &Track| Mark::default();
            let lens = Lens::new(&self.filter, None, &marks);
            let page = Branch::slice(&all, offset, self.page_size);
            assert!(page.nodes().count() <= self.page_size);
            self.tree.receive(&id, &fetch, page, lens.as_ref());
            self.peak = self.peak.max(self.tree.loaded());
        }
        panic!("the tree kept requesting batches");
    }

    fn scroll(&mut self, top: usize) {
        self.shown = top..top + WINDOW;
        self.settle();
    }

    /// The title at each row, with released rows as `None`.
    fn logical(&self) -> Vec<Option<String>> {
        let mut rows = Vec::with_capacity(self.len);
        for row in &self.rows {
            match row {
                TreeRow::Node { node, .. } => rows.push(Some(node.value(Field::Title))),
                TreeRow::Message { text, .. } => rows.push(Some(format!("[{text}]"))),
                TreeRow::Released { count, .. } => rows.extend((0..*count).map(|_| None)),
            }
        }
        rows
    }

    fn row_of(&self, title: &str) -> Option<usize> {
        self.logical()
            .iter()
            .position(|row| row.as_deref() == Some(title))
    }
}

fn named(path: &str, count: usize) -> Vec<Node> {
    (0..count)
        .map(|i| Node::folder(None, PathBuf::from(format!("C:\\{path}\\Item {i:05}"))))
        .collect()
}

#[test]
fn batches_load_as_the_viewport_approaches_and_released_rows_keep_their_place() {
    let all = folders(RESIDENT_PAGES * BATCH + BATCH + 7);
    let mut view = Harness::new(all.clone());
    view.settle();
    assert_eq!(view.len, BATCH);
    // Scrolling to the end requests each next batch in turn.
    while view.shown.end < view.len {
        view.scroll(view.len - WINDOW);
    }
    // Every item keeps its row; released ones are runs, not one entry each.
    assert_eq!(view.len, all.len());
    assert!(view.rows.len() <= RESIDENT_PAGES * BATCH + 8);
    assert!(view.tree.branches[&None].loaded_pages() <= RESIDENT_PAGES);
    assert!(matches!(
        view.rows[0],
        TreeRow::Released { count: BATCH, .. }
    ));
    assert_eq!(
        view.logical().last().unwrap().as_deref(),
        Some(all.last().unwrap().value(Field::Title).as_str())
    );
    // Returning to the top reads the first page again, in place.
    view.scroll(0);
    assert_eq!(view.len, all.len());
    let logical = view.logical();
    assert_eq!(logical[0].as_deref(), Some("Folder 00000"));
    assert_eq!(logical[1].as_deref(), Some("Folder 00001"));
    assert!(view.tree.branches[&None].loaded_pages() <= RESIDENT_PAGES);
    // One read per batch, plus the first page once more.
    assert_eq!(view.reads[&None], 6 + 1);
}

#[test]
fn a_collection_beyond_the_loaded_total_is_reachable_within_it() {
    // Eight branches cross the global budget before the last page.
    let collections: Vec<Node> = (0..8)
        .map(|i| Node::folder(None, PathBuf::from(format!("C:\\Collection {i}"))))
        .collect();
    let mut view = Harness::new(collections.clone());
    for node in &collections {
        let name = node.value(Field::Title);
        view.children
            .insert(Some(node.id.clone()), named(&name, 12));
        view.tree.toggle(node.id.clone(), &None);
    }
    view.settle();
    let mut top = 0;
    while top + WINDOW < view.len {
        view.scroll(top);
        top += WINDOW;
    }
    view.scroll(view.len - WINDOW);
    assert_eq!(view.len, 8 + 8 * 12);
    // Every row is reached while the loaded total holds, apart from the
    // pages the viewport itself is near.
    assert!(view.peak <= BUDGET + BATCH, "peak {}", view.peak);
    assert!(view.tree.loaded() <= BUDGET);
    // One entry per loaded item plus one per released run.
    assert!(view.rows.len() <= view.tree.loaded() + 64);
    let last = view.logical();
    assert_eq!(last.last().unwrap().as_deref(), Some("Item 00011"));
    eprintln!(
        "paged collection · peak loaded {} · entries {}",
        view.peak,
        view.rows.len()
    );
}

#[test]
fn a_filter_finds_matches_beyond_the_loaded_total_and_stops_reading() {
    let all = folders(BUDGET + 3 * BATCH + 3);
    let mut view = Harness::new(all.clone());
    view.settle();
    view.filter = "folder 00089".into();
    view.settle();
    assert_eq!(titles(&view.rows), ["Folder 00089"]);
    // Each batch is read once; the filter keeps only its matches.
    let batches = all.len().div_ceil(BATCH);
    assert_eq!(view.reads[&None], batches);
    assert!(view.peak <= BUDGET + BATCH, "peak {}", view.peak);
    view.filter = "folder".into();
    view.settle();
    assert_eq!(view.len, all.len());
    view.scroll(all.len() - WINDOW);
    assert_eq!(
        view.logical().last().unwrap().as_deref(),
        Some("Folder 00090")
    );
    assert!(view.tree.loaded() <= BUDGET);
    // Clearing the filter browses again from the loaded pages.
    view.filter.clear();
    view.scroll(0);
    assert_eq!(view.len, all.len());
    assert_eq!(view.logical()[0].as_deref(), Some("Folder 00000"));
    eprintln!("filter peak loaded {}", view.peak);
}

#[test]
fn large_branch_order_and_identity_seek_share_the_same_disk_index() {
    // The background index owns ordering, including ties across source pages.
    let writer = super::snapshot::Writer::new().unwrap();
    let sort = (Field::Title, ColumnSort::Descending);
    let all = folders(2 * BATCH + 3);
    for page in all.chunks(BATCH) {
        writer
            .append_nodes(Branch::page(page.to_vec(), None), sort.0)
            .unwrap();
    }
    let ticket = Ticket::default();
    let generation = ticket.next();
    let index = writer
        .finish(sort.1 == ColumnSort::Descending, ticket, generation)
        .unwrap();
    let mut previous = String::new();
    for offset in (0..all.len()).step_by(BATCH) {
        let page = index.page_with_limit(offset, BATCH).unwrap();
        assert!(page.nodes().count() <= BATCH);
        for node in page.nodes() {
            let title = node.value(Field::Title);
            assert!(previous.is_empty() || previous > title);
            previous = title;
        }
    }
    let rank = index.position(&all[BATCH].id).unwrap().unwrap();
    assert_eq!(rank, all.len() - BATCH - 1);
    assert_eq!(
        index
            .page_with_limit(rank / BATCH * BATCH, BATCH)
            .unwrap()
            .nodes()
            .nth(rank % BATCH)
            .unwrap()
            .id,
        all[BATCH].id
    );
}

#[test]
fn open_items_of_released_pages_keep_their_rows_and_selection() {
    let all = folders(18);
    let open = all[5].id.clone();
    let mut view = Harness::new(all.clone());
    view.page_size = all.len();
    view.children.insert(Some(open.clone()), named("Inner", 10));
    view.tree.toggle(open.clone(), &None);
    view.settle();
    let owner = Some(open.clone());
    let children = view.children[&owner].clone();
    view.tree
        .insert_branch(owner.clone(), Branch::slice(&children, 0, 4), None);
    view.tree.receive(
        &owner,
        &Fetch::Next(Cursor::Offset(4)),
        Branch::slice(&children, 4, 6),
        None,
    );
    view.tree.selected = Some(all[15].id.clone());
    view.project();
    let before = view.len;
    let inner = view.row_of("Item 00009").unwrap();
    assert_eq!(inner, 6 + 9);
    // The first page is released, as when the viewport moves far away.
    view.tree.release_page(&None, 0, None);
    view.project();
    assert_eq!(view.len, before);
    assert!(view.tree.expanded.contains(&open));
    assert!(!view.tree.branches.contains_key(&Some(open.clone())));
    assert!(view.logical()[0].is_none());
    // Deliver only the parent. The selected sibling must keep its rank
    // before any child payload is available.
    let sibling = "Folder 00015";
    let rank = 15 + 10;
    assert!(
        view.tree
            .receive(&None, &Fetch::Page(0, None), Branch::page(all, None), None)
    );
    view.project();
    assert_eq!(view.len, before);
    assert_eq!(view.row_of(sibling), Some(rank));
    assert!(view.tree.receive(
        &owner,
        &Fetch::Page(1, Some(Cursor::Offset(4))),
        Branch::slice(&children, 4, 6),
        None
    ));
    view.project();
    assert_eq!(view.row_of("Item 00009"), Some(inner));
    assert_eq!(view.row_of(sibling), Some(rank));
    assert_eq!(view.len, before);
    view.scroll(6);
    assert_eq!(view.row_of("Folder 00005"), Some(5));
    assert_eq!(view.row_of("Item 00009"), Some(inner));
    // Collapsing it there still works, and reopening reads it again.
    view.tree.toggle(open.clone(), &None);
    view.project();
    assert_eq!(view.len, before - 10);
    assert!(!view.tree.branches.contains_key(&Some(open.clone())));
    view.tree.toggle(open, &None);
    view.scroll(0);
    assert_eq!(view.len, before);
    assert_eq!(view.row_of("Item 00009"), Some(inner));
}

#[test]
fn collapsing_cancels_pending_batches_and_ignores_their_results() {
    let collections = folders(2);
    let parent = collections[0].id.clone();
    let mut view = Harness::new(collections);
    view.children
        .insert(Some(parent.clone()), named("Deep", 3 * BATCH));
    view.tree.toggle(parent.clone(), &None);
    view.settle();
    // A next batch is waiting when the branch is collapsed.
    let id = Some(parent.clone());
    assert!(view.tree.want(&id, Want::Next, false));
    let ticket = Ticket::default();
    let generation = ticket.next();
    view.tree
        .pending
        .insert(id.clone(), Request(ticket.clone()));
    view.tree.toggle(parent.clone(), &None);
    assert!(!ticket.is_current(generation));
    assert!(!view.tree.branches.contains_key(&id));
    // Its result arrives after the collapse and changes nothing.
    let stale = Branch::slice(&named("Deep", 3 * BATCH), BATCH, BATCH);
    assert!(
        !view
            .tree
            .receive(&id, &Fetch::Next(Cursor::Offset(BATCH)), stale, None)
    );
    // A batch for a position the branch has moved past is ignored too.
    view.tree.toggle(parent, &None);
    view.scroll(0);
    let all = named("Deep", 3 * BATCH);
    assert!(!view.tree.receive(
        &id,
        &Fetch::Next(Cursor::Offset(BATCH + 1)),
        Branch::slice(&all, 1, BATCH),
        None
    ));
    assert!(!view.tree.receive(
        &id,
        &Fetch::Page(7, None),
        Branch::slice(&all, 1, BATCH),
        None
    ));
}

#[test]
fn loading_releases_the_least_recently_used_pages_across_branches() {
    let mut tree = Tree {
        node_limit: Some(BUDGET),
        ..Default::default()
    };
    let big = folders(BUDGET);
    let id = Some(TreeId::Collection(1));
    tree.branches
        .insert(id.clone(), Branch::slice(&big, 0, BATCH));
    let mut filler = folders(BUDGET - BATCH);
    filler.iter_mut().enumerate().for_each(|(i, node)| {
        node.id = TreeId::Collection(1000 + i as i64);
    });
    tree.clock = 5;
    tree.branches.insert(None, Branch::page(filler, None));
    tree.branches.get_mut(&None).unwrap().pages[0].used = 1;
    tree.branches.get_mut(&id).unwrap().pages[0].used = 5;
    assert!(tree.want(&id, Want::Next, false));
    // Already waiting: a second request is not queued.
    assert!(!tree.want(&id, Want::Next, false));
    let fetch = Fetch::Next(Cursor::Offset(BATCH));
    assert!(tree.receive(&id, &fetch, Branch::slice(&big, BATCH, BATCH), None));
    // The other branch's page the viewport left is released, not this one.
    assert!(tree.loaded() <= BUDGET);
    assert!(tree.branches[&None].pages[0].nodes.is_none());
    assert!(!tree.branches.contains_key(&id));
}

#[test]
fn folder_listing_pages_keep_their_order_and_bounded_reads() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for i in 0..13 {
        fs::create_dir(root.join(format!("Folder {i}"))).unwrap();
    }
    fs::create_dir(root.join(".hidden")).unwrap();
    for i in 0..3 {
        write_wav(&root.join(format!("Track {i}.wav")), 0);
    }
    let mut after = None;
    let mut dirs = Vec::new();
    let mut pages = 0;
    let files = loop {
        let page = crate::library::list_page(root, after.as_ref(), 3, Some(4), || true).unwrap();
        pages += 1;
        assert!(page.dirs.len() <= 3);
        dirs.extend(page.dirs);
        match (page.next_dir, page.files) {
            (Some(next), None) => after = Some(crate::library::ListingCursor::Dirs(next)),
            (None, Some(files)) => break files,
            _ => panic!("A page lists subfolders or, once they end, files"),
        }
    };
    assert_eq!(pages, 5);
    // Explorer order: `Folder 2` precedes `Folder 10`.
    let names: Vec<_> = dirs
        .iter()
        .map(|d| crate::library::folder_name(d))
        .collect();
    assert_eq!(names.len(), 13);
    assert_eq!(names[..3], ["Folder 0", "Folder 1", "Folder 2"]);
    assert_eq!(names[12], "Folder 12");
    let crate::library::ListedFiles::All { audio, sheets } = files else {
        panic!("A small folder groups all of its files");
    };
    assert_eq!((audio.len(), sheets.len()), (3, 0));
    let canceled = crate::library::list_page(root, None, 3, Some(4), || false);
    assert!(canceled.is_err());
}

#[test]
fn indexed_contents_continue_inside_a_large_loose_album() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Music");
    fs::create_dir_all(&root).unwrap();
    for i in 0..7 {
        write_wav(&root.join(format!("Loose {i}.wav")), 0);
    }
    let mut store = Store::open(&temp.path().join("library.sqlite3")).unwrap();
    let id = store.add_root(&root).unwrap();
    index_root(&mut store, id, None, &|| true, &mut |_| {}).unwrap();
    let mut at = crate::store::query::ContentsCursor::default();
    let mut titles = Vec::new();
    loop {
        let page = store.tree_folder(id, &root, at, 3).unwrap();
        assert!(page.programs.len() + page.loose.len() <= 3);
        titles.extend(page.loose.iter().map(|t| t.title.clone()));
        match page.next {
            Some(next) => at = next,
            None => break,
        }
    }
    assert_eq!(titles.len(), 7);
    let unique: std::collections::HashSet<_> = titles.iter().collect();
    assert_eq!(unique.len(), 7);
}

/// Drives the real table selection and deferred scroll before, between, and
/// after the two publications on the private Windows desktop.
pub(in crate::browser) fn finish_filtered_arrival(
    browser: &mut super::Browser,
    cx: &mut gpui_kit::Context<super::Browser>,
) {
    use super::{Location, TreeTable, ViewMode};
    let mut track = Track::file("fixture/Needle.wav".into());
    track.library_id = Some(7);
    let target = Node::track(std::sync::Arc::new(track), None);
    let page = || Branch::page(vec![target.clone()], None);
    browser.location = Location::Library(None);
    browser.prefs.mode = ViewMode::Tree;
    browser.query = "needle".into();
    browser.tree.update(cx, |table, cx| {
        let d = table.delegate_mut();
        d.filter.clone_from(&browser.query);
        d.tree.insert_branch(None, page(), None);
        TreeTable::project(table, cx);
        assert_eq!(table.delegate().position(&target.id), Some(0));
        table.delegate_mut().tree.begin_matching(Ticket::default());
    });
    browser.finish_listing(cx);
    assert!(browser.pending.is_some());
    let index = Descendants::build(&Store::memory(), None, browser.query.clone()).unwrap();
    browser.tree.update(cx, |table, cx| {
        table.delegate_mut().tree.publish_matches(index);
        TreeTable::project(table, cx);
    });
    browser.finish_listing(cx);
    assert!(browser.pending.is_some());
    // A catalog rescan may invalidate the reader while it locates the
    // recording. The replacement root must still receive the same arrival.
    browser.tree.update(cx, |table, cx| {
        let d = table.delegate_mut();
        d.tree.root = Some(Source::Loaded(vec![target.clone()].into()));
        d.take(None, &Fetch::First, Ok(Branch::page(Vec::new(), None)), cx);
        TreeTable::project(table, cx);
    });
    browser.finish_listing(cx);
    assert!(browser.tree.read(cx).delegate().tree.reveal.is_some());
    assert!(browser.pending.is_some());
    browser.invalidate_tree(cx);
    assert!(browser.tree.read(cx).delegate().tree.reveal.is_none());
    browser.tree.update(cx, |table, cx| {
        table
            .delegate_mut()
            .take(None, &Fetch::First, Ok(page()), cx);
        TreeTable::project(table, cx);
    });
    browser.finish_listing(cx);
    assert!(browser.pending.is_none());
    let table = browser.tree.read(cx);
    assert_eq!(table.delegate().tree.selected.as_ref(), Some(&target.id));
    assert_eq!(table.selected_row(), Some(0));
    assert_eq!(
        table
            .vertical_scroll_handle
            .0
            .borrow()
            .deferred_scroll_to_item
            .unwrap()
            .item_index,
        0
    );
}

#[test]
fn indexed_filters_keep_unloaded_folders_whose_contents_match() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Music");
    fs::create_dir_all(root.join("Artists").join("Deep")).unwrap();
    fs::create_dir_all(root.join("Other")).unwrap();
    write_wav(&root.join("Artists").join("Deep").join("Needle.wav"), 0);
    write_wav(&root.join("Other").join("Haystack.wav"), 0);
    let mut store = Store::open(&temp.path().join("library.sqlite3")).unwrap();
    let id = store.add_root(&root).unwrap();
    index_root(&mut store, id, None, &|| true, &mut |_| {}).unwrap();
    let mut tree = Tree::default();
    tree.branches
        .insert(None, indexed_branch(&store, id, &root).unwrap());
    // Loaded rows alone cannot see into the collapsed folders.
    assert!(titles(&tree.visible(None, "needle", &|_| Mark::default())).is_empty());
    // The root can be ready before the descendant worker. A recording
    // arrival must remain pending through publication and root replacement.
    assert!(tree.ready_for_arrival());
    let pending = Ticket::default();
    let generation = pending.next();
    tree.pending.insert(None, Request(pending.clone()));
    tree.begin_matching(Ticket::default());
    assert!(!pending.is_current(generation));
    assert!(!tree.ready_for_arrival());
    assert!(tree.next_request().is_none());
    let matches = Descendants::build(&store, Some(id), "needle".into()).unwrap();
    let page = matches
        .annotate(indexed_branch(&store, id, &root).unwrap())
        .unwrap();
    tree.publish_matches(matches);
    assert!(!tree.ready_for_arrival());
    tree.receive(&None, &Fetch::First, page, None);
    assert!(tree.ready_for_arrival());
    let matches = tree.descendants.as_deref().unwrap();
    let marks = |_: &Track| Mark::default();
    let lens = Lens::new("needle", Some(matches), &marks);
    assert_eq!(titles(&tree.project(lens.as_ref()).rows), ["Artists"]);
}

#[test]
#[ignore = "checks descendant membership beyond the historical 4,096-folder cutoff"]
fn indexed_filter_resolves_collapsed_ancestors_beyond_four_thousand_folders() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("library.sqlite3");
    let mut store = Store::open(&path).unwrap();
    let root = store.add_root(fixture.path()).unwrap();
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<5000)
        INSERT INTO files(id,path,path_key,name,kind,root_id,size,modified)
        SELECT i,printf('folder-%d/song.wav',i),printf('folder-%d/song.wav',i),'song.wav',0,?,2,1 FROM n", [root]).unwrap();
    db.execute_batch("INSERT INTO albums(id,key,root_id,title,folder,sort_key,is_release)
        SELECT id,path,root_id,name,printf('folder-%d/disc',id),path,1 FROM files;
        INSERT INTO tracks(anchor,part,source,root_id,album_id,title,artist,album,format,title_key,artist_key,album_key,search,added_at)
        SELECT id,0,id,root_id,id,name,'','','WAV',name,'','',name,1 FROM files;").unwrap();
    let index = Descendants::build(&store, Some(root), "format:wav".into()).unwrap();
    for i in [5000, 4097, 1, 5001] {
        let node = Node::folder(Some(root), format!("folder-{i}").into());
        let batch = index.annotate(Branch::page(vec![node], None)).unwrap();
        assert_eq!(
            batch.nodes().next().unwrap().has_matching_descendant,
            i <= 5000
        );
    }
}

#[test]
#[ignore = "creates an isolated profile for silent native browser qualification"]
fn seed_music_browser_qualification() {
    let profile = PathBuf::from(std::env::var_os("DISQRO_BROWSER_FIXTURE").expect("fixture path"));
    assert!(profile.is_absolute() && !profile.join("library.sqlite3").exists());
    let library = profile.join("Reference Library");
    let release = library.join("Live Set");
    let image_cd = release.join("CD 1");
    cue(&image_cd, "Live Set (CD1)", 20, false);
    if let Some(image) = std::env::var_os("DISQRO_BROWSER_FLAC_IMAGE") {
        fs::copy(image, image_cd.join("Source 01.flac")).unwrap();
        fs::remove_file(image_cd.join("Source 01.wav")).unwrap();
        let sheet = image_cd.join("Live Set (CD1).cue");
        let text = fs::read_to_string(&sheet).unwrap();
        fs::write(&sheet, text.replace("Source 01.wav", "Source 01.flac")).unwrap();
        let branch = physical_branch(&image_cd, None, &|| true).unwrap();
        assert_eq!(branch.nodes().count(), 1);
        let Some(Source::Loaded(chapters)) = branch.nodes().next().unwrap().source() else {
            panic!("Expected an image-backed CD");
        };
        assert_eq!(chapters.len(), 20);
        assert!(chapters.iter().all(|n| n.value(Field::Format) == "FLAC"));
    }
    cue(&release.join("CD 2"), "Live Set (CD2)", 3, true);
    for number in [1, 2] {
        let sheet = release.join(format!("CD {number}/Live Set (CD{number}).cue"));
        let text = fs::read_to_string(&sheet).unwrap();
        let (title, body) = text.split_once('\n').unwrap();
        fs::write(
            sheet,
            format!(
                "{title}\nPERFORMER \"Various Artists\"\n{}",
                body.replace("INDEX 01", "PERFORMER \"The Reference Ensemble\"\nINDEX 01")
            ),
        )
        .unwrap();
    }
    let archive = library.join("Archive");
    fs::create_dir_all(&archive).unwrap();
    write_wav(&archive.join("Loose Recording.wav"), 1);
    write_wav(&library.join("Opening Notes.wav"), 1);
    let imported = profile.join("Imported");
    fs::create_dir_all(&imported).unwrap();
    write_wav(&imported.join("Imported Recording.wav"), 1);
    let mut store = Store::open(&profile.join("library.sqlite3")).unwrap();
    let id = store.add_root(&library).unwrap();
    let imported_id = store.add_root(&imported).unwrap();
    for root in [id, imported_id] {
        index_root(&mut store, root, None, &|| true, &mut |_| {}).unwrap();
    }
    let keys = store.disc_keys(Scope::Library(Some(id))).unwrap();
    store.set_liked(&keys[..2], true).unwrap();
    store.set_rating(&keys[..1], 4).unwrap();
    store
        .create_playlist("Evening Session", &keys[..3])
        .unwrap();
    fs::write(profile.join("reference-root.txt"), id.to_string()).unwrap();
}

#[test]
fn every_sheet_of_a_paged_folder_owns_its_recordings() {
    // Sheets and their sources land on different pages.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let sheets = 3;
    for i in 0..sheets {
        let source = format!("Z Source {i:03}.wav");
        write_wav(&root.join(&source), 0);
        fs::write(
            root.join(format!("A Program {i:03}.cue")),
            format!(
                "TITLE \"Program {i:03}\"\nFILE \"{source}\" WAVE\nTRACK 01 AUDIO\nTITLE \"Opening {i:03}\"\nINDEX 01 00:00:00\n"
            ),
        )
        .unwrap();
    }
    for i in 0..2 {
        write_wav(&root.join(format!("M Take {i:04}.wav")), 1);
    }
    let mut after = None;
    let mut programs = Vec::new();
    let mut loose = Vec::new();
    loop {
        let page = super::physical_page(root, after.as_ref(), 2, 3, &|| true).unwrap();
        for node in page.nodes() {
            match &node.item {
                Item::Disc { disc, .. } => programs.push(disc.title.clone()),
                Item::Track { track, disc: None } => {
                    loose.push(crate::library::name(&track.source))
                }
                _ => {}
            }
        }
        match page.more {
            Some(Cursor::Listing(next)) => after = Some(next),
            None => break,
            Some(other) => panic!("unexpected cursor {other:?}"),
        }
    }
    programs.sort();
    assert_eq!(loose, ["M Take 0000", "M Take 0001"]);
    assert_eq!(programs.len(), sheets);
    assert!(
        loose.iter().all(|name| !name.starts_with("Z Source")),
        "a recording a sheet plays was listed on its own"
    );
    let claimed = crate::library::claimed_sources(
        root,
        &[root.join("Z Source 002.wav"), root.join("M Take 0001.wav")],
        None,
        || true,
    )
    .unwrap();
    assert_eq!(claimed.len(), 1);
}

/// A folder tree: `root` holds `A` and `B`; `A` holds 10 folders that
/// each hold three more.
fn nested() -> (Harness, TreeId, TreeId) {
    let a = Node::folder(None, PathBuf::from("C:\\Music\\A"));
    let b = Node::folder(None, PathBuf::from("C:\\Music\\B"));
    let (a_id, b_id) = (a.id.clone(), b.id.clone());
    let mut view = Harness::new(vec![a, b]);
    view.page_size = INNER;
    view.tree.node_limit = None;
    let inner: Vec<Node> = (0..INNER)
        .map(|i| Node::folder(None, PathBuf::from(format!("C:\\Music\\A\\Inner {i:03}"))))
        .collect();
    for node in &inner {
        let path = format!("C:\\Music\\A\\{}", node.value(Field::Title));
        let leaves = (0..3)
            .map(|j| Node::folder(None, PathBuf::from(format!("{path}\\Leaf {j}"))))
            .collect();
        view.children.insert(Some(node.id.clone()), leaves);
    }
    view.children.insert(Some(a_id.clone()), inner);
    view.children
        .insert(Some(b_id.clone()), named("Music\\B", 2));
    (view, a_id, b_id)
}

const INNER: usize = 10;

fn count(view: &Harness, prefix: &str) -> usize {
    view.logical()
        .iter()
        .flatten()
        .filter(|title| title.starts_with(prefix))
        .count()
}

fn scroll_through(view: &mut Harness) {
    let mut top = 0;
    while top + WINDOW < view.len {
        view.scroll(top);
        top += WINDOW;
    }
    // Rows read near the end push the end further down; follow it.
    loop {
        let len = view.len;
        view.scroll(len.saturating_sub(WINDOW));
        if view.len == len {
            break;
        }
    }
}

fn inner(i: usize) -> TreeId {
    Node::folder(None, PathBuf::from(format!("C:\\Music\\A\\Inner {i:03}"))).id
}

#[test]
fn expand_all_opens_a_whole_subtree_as_the_viewport_reaches_it() {
    let (mut view, a, b) = nested();
    view.settle();
    view.tree.toggle(b.clone(), &None);
    view.tree.expand_all(Some(a.clone()), &None);
    view.settle();
    // The subtree itself is read at once; what lies inside it is read only
    // as the viewport comes near, never all at once.
    let first_reads = view.reads.len();
    assert!(first_reads < INNER, "read {first_reads} branches at once");
    scroll_through(&mut view);
    // One recursive marker opens each folder.
    assert_eq!(count(&view, "Leaf"), INNER * 3);
    assert_eq!(view.reads.len(), 3 + INNER + INNER * 3);
    assert!(view.tree.expanded.len() <= 1);
    assert!(view.tree.expanded_throughout(&Some(a.clone())));
    // Unrelated branches stay as they were.
    assert!(view.row_of("Item 00001").is_some());

    // Collapsing one folder inside keeps the rest open; Expand All again
    // reopens it.
    let before = view.len;
    view.tree.toggle(inner(7), &Some(a.clone()));
    view.project();
    // Its three folders and their three empty-folder notes leave.
    assert_eq!(view.len, before - 6);
    assert_eq!(count(&view, "Leaf"), (INNER - 1) * 3);
    assert!(!view.tree.expanded_throughout(&Some(a.clone())));
    view.tree.expand_all(Some(a.clone()), &None);
    view.scroll(0);
    assert!(view.tree.closed.is_empty());
    assert!(view.tree.expanded_throughout(&Some(a)));
}

#[test]
fn collapse_all_forgets_what_is_open_inside_even_when_unloaded() {
    let (mut view, a, b) = nested();
    view.settle();
    view.tree.toggle(b.clone(), &None);
    view.tree.toggle(a.clone(), &None);
    view.settle();
    // Two folders inside A open one at a time, one inside the other.
    view.tree.toggle(inner(3), &Some(a.clone()));
    view.settle();
    let leaf = view.children[&Some(inner(3))][1].id.clone();
    view.tree.toggle(leaf.clone(), &Some(inner(3)));
    view.tree.toggle(inner(4), &Some(a.clone()));
    view.settle();
    // A deeper folder of a branch never read, from a saved workspace.
    let saved = Node::folder(None, PathBuf::from("C:\\Music\\A\\Inner 250\\Leaf 2")).id;
    view.tree.expanded.push(saved.clone());
    view.tree.collapse_all(Some(a.clone()), &None);
    view.project();
    for gone in [&inner(3), &inner(4), &leaf, &saved, &a] {
        assert!(!view.tree.expanded.contains(gone), "{gone:?} stayed open");
    }
    assert!(view.tree.expanded.contains(&b));
    // Reopening A shows its folders collapsed.
    view.tree.toggle(a.clone(), &None);
    view.settle();
    assert_eq!(view.len, 2 + INNER + 2);
    // Collapse All across the whole tree, then Expand All for all of it.
    view.tree.collapse_all(None, &None);
    view.project();
    assert_eq!(view.len, 2);
    view.tree.expand_all(None, &None);
    scroll_through(&mut view);
    assert!(view.row_of("Inner 000").is_some());
    let tail: Vec<_> = view.logical().into_iter().rev().take(6).collect();
    assert!(view.row_of("Item 00001").is_some(), "{tail:?}");
    assert!(view.tree.expanded_throughout(&None));
}

#[test]
fn expansion_survives_a_restart_and_stale_results_never_reopen() {
    let (mut view, a, _) = nested();
    view.settle();
    view.tree.expand_all(Some(a.clone()), &None);
    view.settle();
    view.tree.toggle(inner(0), &Some(a.clone()));
    let saved = view.tree.expansion();
    assert_eq!(saved.deep, vec![Some(a.clone())]);
    assert_eq!(saved.closed, vec![inner(0)]);
    assert!(saved.links.contains(&(inner(0), Some(a.clone()))));
    let mut restored = Tree::default();
    restored.set_expansion(saved.clone());
    assert_eq!(restored.expansion(), saved);
    // A branch collapsed while its first page is read ignores the result.
    let ticket = Ticket::default();
    let generation = ticket.next();
    let pending = Some(inner(1));
    view.tree
        .pending
        .insert(pending.clone(), Request(ticket.clone()));
    view.tree.collapse_all(Some(a.clone()), &None);
    assert!(!ticket.is_current(generation));
    let leaves = Branch::page(view.children[&pending].clone(), None);
    view.tree.receive(&pending, &Fetch::First, leaves, None);
    view.project();
    assert!(!view.tree.branches.contains_key(&pending) || !view.tree.has_open_inside(&Some(a)));
    assert_eq!(view.len, 2);
}

#[test]
fn saved_exceptions_keep_unloaded_indexed_ancestors_across_subtree_commands() {
    use crate::collection::DiscId;
    let collection = TreeId::Collection(7);
    let release = TreeId::Release(DiscId::Library(18));
    let disc = TreeId::Disc(DiscId::Library(19));
    let other = TreeId::Disc(DiscId::Library(43));
    let mut tree = Tree::default();
    tree.deep.insert(None);
    tree.closed.extend([disc.clone(), other.clone()]);
    tree.links.extend([
        (disc.clone(), Some(release.clone())),
        (release, Some(collection.clone())),
        (collection.clone(), None),
        (other.clone(), Some(TreeId::Collection(8))),
        (TreeId::Collection(8), None),
    ]);
    tree.reconcile();
    let mut restored = Tree::default();
    restored.set_expansion(tree.expansion());
    restored.expand_all(Some(collection.clone()), &None);
    assert!(!restored.closed.contains(&disc));
    assert!(restored.closed.contains(&other));
    restored.collapse_all(Some(collection.clone()), &None);
    assert!(restored.closed.contains(&collection));
    assert!(restored.closed.contains(&other));
}

#[test]
#[ignore = "measures expanded residency over 40,960 folders"]
fn expanded_pages_release_empty_and_nested_branches_and_reload_backward() {
    const PAGES: usize = 10;
    fn batch(index: usize) -> Branch {
        let nodes = (index * PAGE..(index + 1) * PAGE)
            .map(|i| Node::folder(None, format!(r"C:\fixture\folder-{i}").into()))
            .collect();
        Branch::page(
            nodes,
            (index + 1 < PAGES).then_some(Cursor::Offset((index + 1) * PAGE)),
        )
    }
    let mut tree = Tree::default();
    tree.expand_all(None, &None);
    let mut peak = 0;
    let mut branches = 0;
    for index in 0..PAGES {
        let page = batch(index);
        let fetch = if index == 0 {
            Fetch::First
        } else {
            Fetch::Next(Cursor::Offset(index * PAGE))
        };
        assert!(tree.receive(&None, &fetch, page, None));
        let ids: Vec<_> = tree.branches[&None].pages[index]
            .nodes
            .as_ref()
            .unwrap()
            .iter()
            .map(|node| node.id.clone())
            .collect();
        for (offset, id) in ids.into_iter().enumerate() {
            let children = if offset % 16 == 0 {
                vec![Node::folder(
                    None,
                    format!(r"C:\fixture\folder-{}\child", index * PAGE + offset).into(),
                )]
            } else {
                Vec::new()
            };
            tree.branches.insert(Some(id), Branch::page(children, None));
        }
        tree.reconcile();
        peak = peak.max(tree.loaded());
        branches = branches.max(tree.branches.len());
        assert!(peak <= LOADED_NODE_LIMIT);
        assert!(branches <= 1 + RESIDENT_PAGES * PAGE);
    }
    let projected = tree.project(None).len;
    assert!(projected > PAGES * PAGE * 2);
    assert_eq!(tree.expansion().deep, vec![None]);
    assert!(tree.expansion().links.is_empty());
    for index in (0..PAGES - RESIDENT_PAGES).rev() {
        let cursor = tree.branches[&None].pages[index].start.clone();
        assert!(tree.receive(&None, &Fetch::Page(index, cursor), batch(index), None));
        let slot = &tree.branches[&None].pages[index];
        assert!(tree.is_open(&slot.nodes.as_ref().unwrap()[0].id, true));
        assert!(tree.loaded() <= LOADED_NODE_LIMIT);
    }
    eprintln!(
        "visited={} peak_nodes={peak} peak_branches={branches} page_descriptors={} persisted_links={} requests={}",
        PAGES * PAGE,
        tree.branches[&None].pages.len(),
        tree.expansion().links.len(),
        tree.pending.len()
    );
}

#[test]
#[ignore = "measures residency of 36,864 authored releases"]
fn expanded_indexed_releases_reload_their_authored_discs_after_eviction() {
    use crate::{
        collection::{Disc, Discs, Shelf},
        store::{AlbumRow, query::AlbumRelease},
    };
    use std::sync::Arc;
    fn release(i: usize) -> Node {
        let first = 2 * i as i64 + 1;
        let folder = PathBuf::from(format!("fixture/release-{i}"));
        let discs = (1..=2)
            .map(|number| {
                Arc::new(Disc::from_album(AlbumRow {
                    id: first + i64::from(number) - 1,
                    root: Some(1),
                    title: format!("CD {number}"),
                    album_artist: None,
                    year: None,
                    disc_number: Some(number),
                    authored: true,
                    folder: folder.join(format!("CD {number}")),
                    cover: None,
                    cue_image: None,
                    count: 0,
                    duration: 0.,
                    complete: true,
                    format: "WAV".into(),
                    release: Some(AlbumRelease {
                        first,
                        title: format!("Release {i}"),
                        number,
                        subtitle: String::new(),
                    }),
                }))
            })
            .collect();
        let Shelf::Release(group) = Discs::shelve(discs).remove(0) else {
            panic!("release header");
        };
        Node::release(group, folder)
    }
    const PAGES: usize = 9;
    fn batch(i: usize) -> Branch {
        Branch::page(
            (i * PAGE..(i + 1) * PAGE).map(release).collect(),
            (i + 1 < PAGES).then_some(Cursor::Offset((i + 1) * PAGE)),
        )
    }
    let mut tree = Tree::default();
    tree.expand_all(None, &None);
    let mut peak_nodes = 0;
    let mut peak_branches = 0;
    for i in 0..PAGES {
        let fetch = if i == 0 {
            Fetch::First
        } else {
            Fetch::Next(Cursor::Offset(i * PAGE))
        };
        assert!(tree.receive(&None, &fetch, batch(i), None));
        // Exercise expanded and hydrated disc branches in each visited window.
        // Most cohorts remain logically expanded before their first payload arrives.
        let groups: Vec<_> = tree.branches[&None].pages[i]
            .nodes
            .as_ref()
            .unwrap()
            .iter()
            .step_by(16)
            .map(|node| (node.id.clone(), node.source()))
            .collect();
        for (id, group) in groups {
            let Some(Source::Loaded(discs)) = group else {
                panic!("authored disc list");
            };
            for disc in discs.iter() {
                tree.branches
                    .insert(Some(disc.id.clone()), Branch::page(vec![], None));
            }
            tree.branches
                .insert(Some(id), Branch::page(discs.to_vec(), None));
        }
        tree.reconcile();
        peak_nodes = peak_nodes.max(tree.loaded());
        peak_branches = peak_branches.max(tree.branches.len());
        assert!(tree.loaded() <= LOADED_NODE_LIMIT);
    }
    let first = release(0);
    assert!(!tree.branches.contains_key(&Some(first.id.clone())));
    assert!(tree.receive(&None, &Fetch::Page(0, None), batch(0), None));
    let restored = &tree.branches[&None].pages[0].nodes.as_ref().unwrap()[0];
    assert_eq!(restored.id, first.id);
    assert!(tree.is_open(&restored.id, true));
    let Some(Source::Loaded(discs)) = restored.source() else {
        panic!("rehydrated release");
    };
    assert_eq!(discs.len(), 2);
    assert!(discs.iter().all(|node| tree.is_open(&node.id, true)));
    eprintln!(
        "visited_releases={} peak_nodes={peak_nodes} peak_branches={peak_branches} root_slots={} outstanding={} intent_links={}",
        PAGES * PAGE,
        tree.branches[&None].pages.len(),
        tree.pending.len(),
        tree.expansion().links.len()
    );
}

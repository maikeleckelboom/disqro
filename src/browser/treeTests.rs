use super::{
    Branch, Cursor, Descendants, EXPANSION_LIMIT, Fetch, Item, LOADED_NODE_LIMIT, Node, PAGE,
    RESIDENT_PAGES, Request, Source, Tree, TreeId, TreeRow, Want, indexed_branch, physical_branch,
};
use crate::{
    store::{Mark, Scope, Store, Ticket, index_root},
    table::Field,
};
use gpui_kit::component::table::ColumnSort;
use std::{
    fs,
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
    tree.toggle(parent_id.clone());
    assert_eq!(tree.next_request().unwrap().0, Some(parent_id.clone()));
    let nested = physical_branch(&root.join("Nested"), None, &|| true).unwrap();
    let folder = nested.nodes().next().unwrap().id.clone();
    tree.branches.insert(Some(parent_id.clone()), nested);
    tree.toggle(folder.clone());
    let branch = physical_branch(&deep, None, &|| true).unwrap();
    assert_eq!(branch.nodes().count(), 1);
    let disc = branch.nodes().next().unwrap().clone();
    let Some(Source::Loaded(children)) = disc.source() else {
        panic!("Expected an authored CUE");
    };
    assert_eq!(children.len(), 20);
    let key = children[19].id.clone();
    tree.branches.insert(Some(folder), branch);
    tree.toggle(disc.id.clone());
    tree.branches
        .insert(Some(disc.id.clone()), Branch::page(children, None));
    let descending = tree.visible(Some((Field::Title, ColumnSort::Descending)), "", &|_| {
        Mark::default()
    });
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
    tree.toggle(parent_id);
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
    tree.set_expanded(serde_json::from_slice(&saved).unwrap());
    assert_eq!(tree.expanded.len(), EXPANSION_LIMIT);
    assert!(tree.expanded.contains(&TreeId::Collection(0)));
    tree.toggle(TreeId::Collection(500));
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

/// Serves every request the tree makes for the loaded list `all`.
fn serve(tree: &mut Tree, all: &[Node], filtering: bool) {
    while let Some((id, _, fetch)) = tree.next_request() {
        let offset = match fetch.cursor() {
            Some(Cursor::Offset(offset)) => *offset,
            _ => 0,
        };
        let page = Branch::slice(all, offset);
        assert!(page.nodes().count() <= PAGE);
        // A batch for a position the branch has moved past is ignored.
        if fetch != Fetch::First {
            assert!(!tree.receive(
                &id,
                &Fetch::Next(Cursor::Offset(offset + 1)),
                Branch::slice(all, 1),
                filtering
            ));
        }
        assert!(tree.receive(&id, &fetch, page, filtering));
    }
}

#[test]
fn batches_load_as_the_viewport_approaches_and_released_rows_keep_their_place() {
    let all = folders(RESIDENT_PAGES * PAGE + PAGE + 7);
    let mut tree = Tree {
        root: Some(Source::Loaded(all.clone())),
        ..Default::default()
    };
    serve(&mut tree, &all, false);
    let (rows, boundaries) = tree.visible_with(None, "", None, &|_| Default::default());
    assert_eq!(rows.len(), PAGE);
    // Nothing more loads while the viewport is far from the boundary.
    assert!(!boundaries.iter().any(|b| b.near(&(0..40))));
    // Scrolling to the end requests each next batch in turn.
    loop {
        let (rows, boundaries) = tree.visible_with(None, "", None, &|_| Default::default());
        let end = rows.len().saturating_sub(40)..rows.len();
        let wants: Vec<_> = boundaries.iter().filter(|b| b.near(&end)).collect();
        if wants.is_empty() {
            break;
        }
        for boundary in wants {
            tree.want(&boundary.branch, boundary.want);
        }
        serve(&mut tree, &all, false);
    }
    let (rows, boundaries) = tree.visible_with(None, "", None, &|_| Default::default());
    // Every item keeps its row; released ones are placeholders.
    assert_eq!(rows.len(), all.len());
    assert_eq!(
        tree.branches[&None].resident(),
        (RESIDENT_PAGES - 1) * PAGE + 7
    );
    assert!(matches!(rows[0], TreeRow::Released { .. }));
    assert_eq!(
        titles(&rows).last(),
        Some(&all.last().unwrap().value(Field::Title))
    );
    // Returning to the top reads the first page again in place.
    let top: Vec<_> = boundaries.iter().filter(|b| b.near(&(0..40))).collect();
    assert_eq!(top.len(), 1);
    assert!(tree.want(&top[0].branch, top[0].want));
    serve(&mut tree, &all, false);
    let (rows, _) = tree.visible_with(None, "", None, &|_| Default::default());
    assert_eq!(rows.len(), all.len());
    assert_eq!(
        titles(&rows[..2]),
        [all[0].value(Field::Title), all[1].value(Field::Title)]
    );
    assert!(
        tree.branches[&None]
            .pages
            .iter()
            .filter(|p| p.nodes.is_some())
            .count()
            <= RESIDENT_PAGES
    );
}

#[test]
fn a_filter_reads_on_without_scrolling_and_keeps_what_it_searched() {
    let all = folders(3 * PAGE + 5);
    let mut tree = Tree {
        root: Some(Source::Loaded(all.clone())),
        ..Default::default()
    };
    serve(&mut tree, &all, true);
    loop {
        let (_, boundaries) =
            tree.visible_with(None, "folder 12290", None, &|_| Default::default());
        let wants: Vec<_> = boundaries.iter().filter(|b| b.near(&(0..10))).collect();
        if wants.is_empty() {
            break;
        }
        for boundary in wants {
            tree.want(&boundary.branch, boundary.want);
        }
        serve(&mut tree, &all, true);
    }
    let rows = tree.visible(None, "folder 12290", &|_| Default::default());
    assert_eq!(titles(&rows), ["Folder 12290"]);
    assert_eq!(tree.branches[&None].resident(), all.len());
}

#[test]
fn loading_respects_the_loaded_total_and_cancels_with_its_branch() {
    let mut tree = Tree::default();
    let big = folders(LOADED_NODE_LIMIT);
    let id = Some(TreeId::Collection(1));
    tree.insert_branch(id.clone(), Branch::slice(&big, 0));
    let mut filler = folders(LOADED_NODE_LIMIT - PAGE);
    filler.iter_mut().enumerate().for_each(|(i, node)| {
        node.id = TreeId::Collection(1000 + i as i64);
    });
    tree.branches.insert(None, Branch::page(filler, None));
    assert!(tree.want(&id, Want::Next));
    // Already waiting: a second request is not queued.
    assert!(!tree.want(&id, Want::Next));
    let fetch = Fetch::Next(Cursor::Offset(PAGE));
    assert!(tree.receive(&id, &fetch, Branch::slice(&big, PAGE), false));
    // Browsing keeps the loaded total by releasing the branch's own pages.
    assert!(tree.loaded() <= LOADED_NODE_LIMIT);
    assert_eq!(tree.branches[&id].pages.len(), 2);

    let ticket = crate::store::Ticket::default();
    let generation = ticket.next();
    tree.pending.insert(id.clone(), Request(ticket.clone()));
    tree.toggle(TreeId::Collection(999));
    tree.prune();
    assert!(!ticket.is_current(generation));
}

#[test]
fn folder_listing_pages_keep_their_order_and_bounded_reads() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for i in 0..205 {
        fs::create_dir(root.join(format!("Folder {i}"))).unwrap();
    }
    fs::create_dir(root.join(".hidden")).unwrap();
    for i in 0..23 {
        write_wav(&root.join(format!("Track {i}.wav")), 0);
    }
    let mut after = None;
    let mut dirs = Vec::new();
    let mut pages = 0;
    let files = loop {
        let page = crate::library::list_page(root, after.as_ref(), 50, true, || true).unwrap();
        pages += 1;
        assert!(page.dirs.len() <= 50);
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
    assert_eq!(names.len(), 205);
    assert_eq!(names[..3], ["Folder 0", "Folder 1", "Folder 2"]);
    assert_eq!(names[204], "Folder 204");
    let crate::library::ListedFiles::All { audio, sheets } = files else {
        panic!("A small folder groups all of its files");
    };
    assert_eq!((audio.len(), sheets.len()), (23, 0));
    let cancelled = crate::library::list_page(root, None, 50, true, || false);
    assert!(cancelled.is_err());
}

#[test]
fn a_folder_with_too_many_recordings_to_group_lists_them_by_page() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let count = crate::library::GROUPED_FILES + 5;
    for i in 0..count {
        fs::write(root.join(format!("Take {i}.wav")), b"").unwrap();
    }
    cue(&root.join("Program"), "Program", 2, false);
    fs::rename(
        root.join("Program").join("Program.cue"),
        root.join("Program.cue"),
    )
    .unwrap();
    fs::rename(
        root.join("Program").join("Source 01.wav"),
        root.join("Source 01.wav"),
    )
    .unwrap();
    let mut after = None;
    let mut names = Vec::new();
    loop {
        let page = crate::library::list_page(root, after.as_ref(), 1000, true, || true).unwrap();
        let Some(crate::library::ListedFiles::Page {
            audio,
            sheets,
            claims,
            next,
        }) = page.files
        else {
            panic!("Expected a page of files");
        };
        assert!(audio.len() + sheets.len() <= 1000);
        assert_eq!(claims.len(), 1);
        names.extend(audio.iter().chain(&sheets).map(|p| crate::library::name(p)));
        match next {
            Some(next) => after = Some(crate::library::ListingCursor::Files(next)),
            None => break,
        }
    }
    // Every recording and the sheet, each once.
    assert_eq!(names.len(), count + 2);
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
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
    let (folders, truncated) = store.tree_matches(Some(id), "needle", 4096).unwrap();
    assert!(!truncated);
    let matches = Descendants::new("needle".into(), folders, truncated);
    assert_eq!(
        titles(
            &tree
                .visible_with(None, "needle", Some(&matches), &|_| Mark::default())
                .0
        ),
        ["Artists"]
    );
    let (all, _) = store.tree_matches(None, "needle", 4096).unwrap();
    assert_eq!(all.len(), 1);
    let (_, truncated) = store.tree_matches(Some(id), "wav", 1).unwrap();
    assert!(truncated);
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

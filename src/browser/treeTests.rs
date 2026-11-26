use super::{
    Branch, EXPANSION_LIMIT, Item, Node, Request, Source, Tree, TreeId, TreeRow, indexed_branch,
    merge_immediate_folders, physical_branch,
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
    let start = physical_branch(root, &|| true).unwrap();
    assert_eq!(start.nodes.len(), 3);
    assert!(
        start
            .nodes
            .iter()
            .all(|n| !matches!(n.item, Item::Disc { .. }))
    );
    let Node { id, .. } = start
        .nodes
        .iter()
        .find(|n| n.value(Field::Title) == "Nested")
        .unwrap();
    let parent_id = id.clone();
    let mut tree = Tree::default();
    tree.branches.insert(None, start);
    assert!(tree.next_request().is_none());
    tree.toggle(parent_id.clone());
    assert_eq!(tree.next_request().unwrap().0, Some(parent_id.clone()));
    let nested = physical_branch(&root.join("Nested"), &|| true).unwrap();
    let folder = nested.nodes[0].id.clone();
    tree.branches.insert(Some(parent_id.clone()), nested);
    tree.toggle(folder.clone());
    let branch = physical_branch(&deep, &|| true).unwrap();
    assert_eq!(branch.nodes.len(), 1);
    let disc = branch.nodes[0].clone();
    let Some(Source::Loaded(children)) = disc.source() else {
        panic!("Expected an authored CUE");
    };
    assert_eq!(children.len(), 20);
    let key = children[19].id.clone();
    tree.branches.insert(Some(folder), branch);
    tree.toggle(disc.id.clone());
    tree.branches.insert(
        Some(disc.id.clone()),
        Branch {
            nodes: children,
            notice: None,
            limited: false,
        },
    );
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
    let multi = physical_branch(&root.join("Multipart"), &|| true).unwrap();
    let Some(Source::Loaded(children)) = multi.nodes[0].source() else {
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
    let mut top = indexed_branch(&store, id, &root).unwrap();
    merge_immediate_folders(&mut top, id, &root, &|| true);
    assert_eq!(
        top.nodes
            .iter()
            .map(|n| n.value(Field::Title))
            .collect::<Vec<_>>(),
        ["Empty Folder", "Live Set", "Loose"]
    );
    let branch = indexed_branch(&store, id, &release).unwrap();
    assert_eq!(branch.nodes.len(), 1);
    let Some(Source::Loaded(discs)) = branch.nodes[0].source() else {
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
    assert_eq!(branch.nodes[0].id, again.nodes[0].id);
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
        let branch = physical_branch(&image_cd, &|| true).unwrap();
        assert_eq!(branch.nodes.len(), 1);
        let Some(Source::Loaded(chapters)) = branch.nodes[0].source() else {
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

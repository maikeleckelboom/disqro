use super::*;
use std::fs;

fn collect(path: &Path, recursive: bool, cache: Option<&Cache>) -> (Vec<Arc<Track>>, Vec<String>) {
    let (tx, rx) = async_channel::unbounded();
    scan(path, recursive, cache, &tx);
    drop(tx);
    let mut slots = Vec::new();
    let mut notices = Vec::new();
    while let Ok(batch) = rx.try_recv() {
        notices.extend(batch.notice);
        for (i, t) in batch.rows {
            if i == slots.len() {
                slots.push(t);
            } else {
                slots[i] = t;
            }
        }
    }
    (slots.into_iter().flatten().collect(), notices)
}

#[test]
fn nested_programs_deduplicate_sources_and_keep_distinct_editions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("a.wav"), []).unwrap();
    let deep = (0..45).fold(root.to_owned(), |p, _| p.join("d"));
    fs::create_dir_all(&deep).unwrap();
    let image = deep.join("b.wav");
    fs::write(&image, []).unwrap();
    let program = format!(
        "TITLE Archive\nFILE \"{}\" WAVE\nTRACK 01 AUDIO\nTITLE First\nINDEX 01 00:00:00\nFILE b.wav WAVE\nTRACK 02 AUDIO\nTITLE Second\nINDEX 01 00:00:00",
        root.join("a.wav").display()
    );
    for sheet in ["a.cue", "copy.cue"] {
        fs::write(deep.join(sheet), &program).unwrap();
    }
    fs::write(
        deep.join("edition.cue"),
        program.replace("Archive", "Alternative"),
    )
    .unwrap();
    fs::write(deep.join("broken.cue"), "TRACK 10 AUDIO").unwrap();
    assert_eq!(collect(root, false, None).0.len(), 1);
    let (first, notices) = collect(root, true, None);
    assert_eq!(first.len(), 4);
    assert!(first.iter().all(|t| t.disc.is_some()));
    assert!(!notices.is_empty());
    fs::remove_file(&image).unwrap();
    assert_eq!(collect(root, true, None).0.len(), 2);
    fs::write(&image, []).unwrap();
    let restored = collect(root, true, None).0;
    assert!(first.iter().zip(restored).all(|(a, b)| a.same_item(&b)));
    assert!(!collect(&root.join("missing"), true, None).1.is_empty());
}

#[test]
fn closing_receiver_releases_backpressured_walk() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..10 {
        let folder = dir.path().join(i.to_string());
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("song.flac"), []).unwrap();
    }
    let path = dir.path().to_owned();
    let (tx, rx) = async_channel::bounded(1);
    let (done, ack) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        scan(&path, true, None, &tx);
        done.send(()).unwrap();
    });
    assert!(rx.recv_blocking().unwrap().root.is_some());
    drop(rx);
    ack.recv_timeout(Duration::from_secs(3)).unwrap();
    worker.join().unwrap();
}

#[cfg(windows)]
#[test]
fn directory_junction_is_not_descended() {
    let dir = tempfile::tempdir().unwrap();
    let child = dir.path().join("disc");
    fs::create_dir(&child).unwrap();
    let link = child.join("again");
    let made = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/j"])
        .arg(&link)
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(library::browse(&child).unwrap().dirs.is_empty());
    fs::remove_dir(link).unwrap();
}

#[test]
fn audio_headers_invalidate_without_changing_logical_identity() {
    let dir = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let cache = Cache::open(db.path()).unwrap();
    let path = dir.path().join("music.wav");
    crate::audio::tests::fixture(&path, 44100, 4410);
    let old = Stamp::read(&path).unwrap();
    let first = collect(dir.path(), true, Some(&cache)).0;
    assert!(cache.tags(&path, &old).is_some());
    crate::audio::tests::fixture(&path, 44100, 8820);
    assert!(cache.tags(&path, &Stamp::read(&path).unwrap()).is_none());
    let new = collect(dir.path(), true, Some(&cache)).0;
    assert!(first[0].same_item(&new[0]));
    assert!(new[0].duration.unwrap() > first[0].duration.unwrap());
}

#[test]
#[ignore = "creates a 100,089-track synthetic collection for scale measurements"]
fn six_figure_collection_projection() {
    // An opt-in retained fixture lets the native player exercise the exact measured catalog.
    let keep_in = std::env::var_os("DISQRO_SCALE_FIXTURES");
    let dir = match &keep_in {
        Some(parent) => tempfile::tempdir_in(parent),
        None => tempfile::tempdir(),
    }
    .unwrap();
    let media = dir.path().join("Discs");
    fs::create_dir(&media).unwrap();
    let mut lines = String::new();
    for i in 1..=99 {
        let minute = i - 1;
        lines.push_str(&format!(
            "TRACK {i:02} AUDIO\nTITLE \"Track {i}\"\nINDEX 01 {minute:02}:00:00\n"
        ));
    }
    for i in 0..1011 {
        let disc = media.join(i.to_string());
        fs::create_dir(&disc).unwrap();
        fs::write(disc.join("disc.wav"), []).unwrap();
        fs::write(
            disc.join("disc.cue"),
            format!("TITLE \"Volume {i}\"\nFILE disc.wav WAVE\n{lines}"),
        )
        .unwrap();
    }
    let start = Instant::now();
    let (rows, notices) = collect(&media, true, None);
    let scan = start.elapsed();
    assert!(notices.is_empty());
    assert_eq!(rows.len(), 100089);
    let start = Instant::now();
    let groups = crate::collection::Discs::group(&rows, &Default::default(), &media);
    let grouping = start.elapsed();
    assert_eq!(groups.len(), 1011);
    let start = Instant::now();
    let terms = crate::store::Term::parse("format:wav album:777");
    let found = rows
        .iter()
        .filter(|t| crate::collection::matches(t, &terms, Default::default))
        .count();
    assert_eq!(found, 99);
    eprintln!(
        "Synthetic: {} tracks; discovery {:?}, grouping {:?}, query {:?}",
        rows.len(),
        scan,
        grouping,
        start.elapsed()
    );
    let mut store = crate::store::Store::open(&dir.path().join("library.sqlite3")).unwrap();
    let root = store.add_root(&media).unwrap();
    let indexed = crate::store::index_root(&mut store, root, None, &|| true, &mut |_| {}).unwrap();
    assert_eq!(indexed.tracks, 100089);
    for term in ["Volume 777", "Track 99", "Track"] {
        let start = Instant::now();
        let results = store.search_library(term, 5, 0).unwrap();
        assert!(results.more);
        assert!(
            results
                .hits
                .iter()
                .any(|h| matches!(h.target, crate::store::search::Target::Track(_)))
        );
        eprintln!(
            "Catalog search {term:?}: {:?} ({} grouped hits)",
            start.elapsed(),
            results.hits.len()
        );
    }
    drop(store);
    if keep_in.is_some() {
        eprintln!("Scale fixture profile: {}", dir.keep().display());
    }
}

#[test]
fn dot_folders_are_skipped_but_open_directly() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for folder in ["Album", ".git", r".local\fixtures"] {
        fs::create_dir_all(root.join(folder)).unwrap();
    }
    crate::audio::tests::fixture(&root.join("Album").join("song.wav"), 44100, 441);
    crate::audio::tests::fixture(&root.join(r".local\fixtures\probe.wav"), 44100, 441);
    let names = |dirs: Vec<PathBuf>| {
        dirs.iter()
            .map(|p| library::folder_name(p))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(library::subfolders(root)), ["Album"]);
    assert_eq!(names(library::browse(root).unwrap().dirs), ["Album"]);
    let (found, _) = collect(root, true, None);
    assert_eq!(found.len(), 1);
    assert!(found[0].source.ends_with("song.wav"));
    // Navigating straight into a dot-folder still lists it.
    assert_eq!(
        collect(&root.join(r".local\fixtures"), false, None).0.len(),
        1
    );
}

#[test]
fn folders_and_files_sort_like_explorer() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["CD 10", "cd 2", "CD 1"] {
        fs::create_dir(dir.path().join(name)).unwrap();
    }
    for name in ["10.wav", "2.wav", "1.wav"] {
        fs::write(dir.path().join(name), []).unwrap();
    }
    let folder = library::browse(dir.path()).unwrap();
    let dirs: Vec<_> = folder
        .dirs
        .iter()
        .map(|p| library::folder_name(p))
        .collect();
    assert_eq!(dirs, ["CD 1", "cd 2", "CD 10"]);
    let files: Vec<_> = folder.tracks.iter().map(|t| t.title.clone()).collect();
    assert_eq!(files, ["1", "2", "10"]);
}

#[test]
fn unreadable_audio_is_told_apart_from_untagged_audio() {
    let dir = tempfile::tempdir().unwrap();
    crate::audio::tests::fixture(&dir.path().join("untagged.wav"), 44100, 4410);
    fs::write(dir.path().join("empty.wav"), []).unwrap();
    let (tx, rx) = async_channel::unbounded();
    scan(dir.path(), false, None, &tx);
    drop(tx);
    let mut unreadable = vec![];
    let mut durations = std::collections::HashMap::new();
    while let Ok(batch) = rx.try_recv() {
        unreadable.extend(batch.unreadable);
        for t in batch.rows.into_iter().filter_map(|(_, t)| t) {
            durations.insert(library::name(&t.source), t.duration);
        }
    }
    assert_eq!(unreadable.len(), 1);
    assert!(unreadable[0].ends_with("empty.wav"));
    assert!(durations["untagged"].is_some_and(|d| d > 0.));
    assert_eq!(durations["empty"], None);
}

#[test]
fn loose_files_and_untagged_numbered_disc_directories_have_distinct_playback_contexts() {
    let root = Path::new("loose");
    let mut file = Track::file(root.join("single.mp3"));
    assert!(!file.has_disc_context());
    file.album = "A tagged album".into();
    assert!(file.has_disc_context());
    let cd = Track::file(root.join("Disc 2").join("track.wav"));
    assert!(cd.has_disc_context());
    assert!(!Track::file(root.join("CD samples").join("one.wav")).has_disc_context());
}

#[test]
fn loose_wavs_require_an_explicit_cd_selection_to_continue() {
    use library::DiscContext::{Inferred, Selected};
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.wav", "b.wav", "c.wav"] {
        crate::audio::tests::fixture(&dir.path().join(name), 44100, 441);
    }
    let tracks = collect(dir.path(), false, None).0;
    assert_eq!(tracks.len(), 3);
    let first = &tracks[0];
    assert!(!Inferred.allows(first));
    assert!(
        local_program(dir.path(), first, Inferred, None)
            .unwrap()
            .tracks
            .is_empty()
    );

    let mut queue = crate::queue::Queue::default();
    queue.replace(std::iter::once(first.clone()).collect(), 0);
    let id = queue.current_entry().unwrap().id;
    let revision = queue.revision;
    assert!(!queue.continue_disc(id, &tracks, Inferred));
    assert!(queue.upcoming().is_empty());
    assert_eq!(queue.revision, revision);

    let explicit = local_program(dir.path(), first, Selected, None)
        .unwrap()
        .tracks;
    assert_eq!(&*explicit, tracks);
    let duplicates = queue.add(&[tracks[1].clone(), tracks[1].clone()].into_iter().collect());
    assert!(queue.continue_disc(id, &explicit, Selected));
    assert_eq!(queue.current_entry().unwrap().id, id);
    assert_eq!(queue.upcoming()[0].id, duplicates[0]);
    assert_eq!(queue.upcoming()[1].track, tracks[2]);
    assert_eq!(queue.upcoming()[2].id, duplicates[1]);
    assert_eq!(queue.upcoming()[0].disc_context, Selected);
    assert_eq!(queue.upcoming()[2].disc_context, Inferred);
    assert!(!queue.continue_disc(id, &explicit, Selected));
}

#[test]
fn tagged_wavs_continue_in_track_order_within_the_same_album_and_disc() {
    use library::DiscContext::Inferred;
    use lofty::{
        config::WriteOptions,
        prelude::*,
        tag::{Tag, TagType},
    };
    let dir = tempfile::tempdir().unwrap();
    for (file, album, disc, number) in [
        ("a.wav", "Album", 1, 2),
        ("b.wav", "Album", 1, 1),
        ("c.wav", "Album", 2, 3),
        ("d.wav", "Other", 1, 4),
    ] {
        let path = dir.path().join(file);
        crate::audio::tests::fixture(&path, 44100, 441);
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_album(album.into());
        tag.set_disk(disc);
        tag.set_track(number);
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }
    let first = collect(dir.path(), false, None).0.remove(1);
    assert!(Inferred.allows(&first));
    let program = local_program(dir.path(), &first, Inferred, None).unwrap();
    assert!(program.has_sibling_disc);
    let program = program.tracks;
    assert_eq!(
        program.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["b", "a"]
    );
    let mut queue = crate::queue::Queue::default();
    queue.replace(program[..1].iter().cloned().collect(), 0);
    assert!(queue.continue_disc(queue.current_entry().unwrap().id, &program, Inferred));
    assert_eq!(queue.upcoming()[0].track.title, "a");
}

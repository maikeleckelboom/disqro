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
    let dir = tempfile::tempdir().unwrap();
    let mut lines = String::new();
    for i in 1..=99 {
        let minute = i - 1;
        lines.push_str(&format!(
            "TRACK {i:02} AUDIO\nTITLE \"Track {i}\"\nINDEX 01 {minute:02}:00:00\n"
        ));
    }
    for i in 0..1011 {
        let disc = dir.path().join(i.to_string());
        fs::create_dir(&disc).unwrap();
        fs::write(disc.join("disc.wav"), []).unwrap();
        fs::write(
            disc.join("disc.cue"),
            format!("TITLE \"Volume {i}\"\nFILE disc.wav WAVE\n{lines}"),
        )
        .unwrap();
    }
    let start = Instant::now();
    let (rows, notices) = collect(dir.path(), true, None);
    let scan = start.elapsed();
    assert!(notices.is_empty());
    assert_eq!(rows.len(), 100089);
    let start = Instant::now();
    let groups = crate::collection::Discs::group(&rows, &Default::default(), dir.path());
    let grouping = start.elapsed();
    assert_eq!(groups.len(), 1011);
    let start = Instant::now();
    let found = rows
        .iter()
        .filter(|t| crate::collection::matches(t, "format:wav album:777"))
        .count();
    assert_eq!(found, 99);
    eprintln!(
        "Synthetic: {} tracks; discovery {:?}, grouping {:?}, query {:?}",
        rows.len(),
        scan,
        grouping,
        start.elapsed()
    );
}

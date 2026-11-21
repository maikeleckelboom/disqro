use super::*;
use std::fs;

fn index(s: &mut Store, root: RootId) -> Summary {
    index_root(s, root, None, &|| true, &mut |_| {}).unwrap()
}
fn all(s: &Store, scope: Scope) -> Vec<Row> {
    let q = Query {
        scope,
        search: String::new(),
        order: None,
    };
    s.rows(scope, &s.keys(&q).unwrap()).unwrap()
}
fn chapter<'a>(rows: &'a [Row], sheet: &str, n: u8) -> &'a Row {
    rows.iter()
        .find(|r| {
            r.track.disc.as_ref().is_some_and(|d| {
                d.number == n && d.sheet.file_name().unwrap().to_str() == Some(sheet)
            })
        })
        .unwrap()
}
fn titled<'a>(rows: &'a [Row], title: &str) -> &'a Row {
    rows.iter().find(|r| r.track.title == title).unwrap()
}

const ALBUM_A: &str = "TITLE \"Album A\"\nPERFORMER \"Artist\"\nFILE \"a.flac\" WAVE\n\
    TRACK 01 AUDIO\nTITLE \"One\"\nINDEX 01 00:02:00\n\
    TRACK 02 AUDIO\nTITLE \"Two\"\nINDEX 01 01:00:00\n\
    TRACK 03 AUDIO\nTITLE \"Three\"\nINDEX 01 02:00:00\n";

/// A single-image CUE disc with a recorded lead-in, a multi-file CUE disc
/// and two standalone files: 8 logical tracks.
fn collection(root: &Path) {
    let a = root.join("Album A");
    let b = root.join("Album B");
    let singles = root.join("Singles");
    for d in [&a, &b, &singles] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(a.join("a.cue"), ALBUM_A).unwrap();
    fs::write(a.join("a.flac"), b"image of album a").unwrap();
    fs::write(
        b.join("b.cue"),
        "TITLE \"Album B\"\nFILE \"b1.wav\" WAVE\nTRACK 01 AUDIO\nTITLE \"B1\"\nINDEX 01 00:00:00\n\
         FILE \"b2.wav\" WAVE\nTRACK 02 AUDIO\nTITLE \"B2\"\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    fs::write(b.join("b1.wav"), b"first file of b").unwrap();
    fs::write(b.join("b2.wav"), b"second file of album b").unwrap();
    fs::write(singles.join("x.mp3"), b"single x").unwrap();
    fs::write(singles.join("y.flac"), b"single y, longer").unwrap();
}

#[test]
fn identities_survive_rescans_sheet_edits_and_verified_moves() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut s = Store::memory();
    let r = s.add_root(&root).unwrap();
    let first = index(&mut s, r);
    assert_eq!((first.tracks, first.added, first.missing), (8, 8, 0));
    let rows = all(&s, Scope::Library(Some(r)));
    assert_eq!(rows.len(), 8);
    let lead_in = chapter(&rows, "a.cue", 0);
    assert_eq!(lead_in.track.disc.as_ref().unwrap().end, Some(150));
    let two = chapter(&rows, "a.cue", 2).id;
    let x = titled(&rows, "x").id;
    assert_ne!(lead_in.id, chapter(&rows, "a.cue", 1).id);

    // Each CUE track carries its own marks and playlist entries.
    s.set_liked(&[two], true).unwrap();
    s.set_rating(&[two], 4).unwrap();
    let mix = s.create_playlist("Mix", &[two, x, two]).unwrap();

    let again = index(&mut s, r);
    assert_eq!((again.added, again.missing), (0, 0));
    let ids = |s: &Store| -> Vec<TrackId> {
        all(s, Scope::Library(Some(r)))
            .iter()
            .map(|r| r.id)
            .collect()
    };
    let before = ids(&s);

    // Correcting a title keeps the identity.
    fs::write(
        root.join("Album A/a.cue"),
        ALBUM_A.replace("\"Two\"", "\"Deux\""),
    )
    .unwrap();
    index(&mut s, r);
    let rows = all(&s, Scope::Library(Some(r)));
    assert_eq!(chapter(&rows, "a.cue", 2).id, two);
    assert_eq!(chapter(&rows, "a.cue", 2).track.title, "Deux");

    // A renamed album folder keeps its sheet, image and every track.
    fs::rename(root.join("Album A"), root.join("Album A (Remaster)")).unwrap();
    let moved = index(&mut s, r);
    assert_eq!((moved.relocated, moved.added, moved.missing), (2, 0, 0));
    assert_eq!(ids(&s), before);
    let rows = all(&s, Scope::Library(Some(r)));
    let two_row = chapter(&rows, "a.cue", 2);
    assert!(two_row.available);
    assert!(
        two_row
            .track
            .source
            .starts_with(root.join("Album A (Remaster)"))
    );
    let liked = all(&s, Scope::Liked);
    assert_eq!(liked.iter().map(|r| r.id).collect::<Vec<_>>(), [two]);
    let entries = all(&s, Scope::Playlist(mix));
    assert_eq!(
        entries.iter().map(|r| r.id).collect::<Vec<_>>(),
        [two, x, two]
    );
    let marks = s.marks().unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(
        marks[0].2,
        Mark {
            liked: true,
            rating: 4
        }
    );
    assert_eq!(marks[0].1, Some(Locator::of(&two_row.track)));

    // Untagged titles follow the filename even when the playlist identity survives.
    fs::rename(root.join("Singles/x.mp3"), root.join("Singles/Renamed.mp3")).unwrap();
    assert_eq!(index(&mut s, r).relocated, 1);
    let entries = all(&s, Scope::Playlist(mix));
    assert_eq!(entries[1].id, x);
    assert_eq!(entries[1].track.title, "Renamed");
}

#[test]
fn missing_files_keep_their_place_until_they_return() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut s = Store::memory();
    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let rows = all(&s, Scope::Library(Some(r)));
    let y = titled(&rows, "y").id;
    let b2 = chapter(&rows, "b.cue", 2).id;
    let list = s.create_playlist("Keep", &[y, b2]).unwrap();
    s.set_liked(&[y], true).unwrap();

    let y_path = root.join("Singles/y.flac");
    let bytes = fs::read(&y_path).unwrap();
    fs::remove_file(&y_path).unwrap();
    fs::remove_file(root.join("Album B/b2.wav")).unwrap();

    // A scan that stops early marks nothing missing.
    let stopped = index_root(&mut s, r, None, &|| false, &mut |_| {});
    assert!(is_cancelled(&stopped.unwrap_err()));
    assert!(all(&s, Scope::Playlist(list)).iter().all(|r| r.available));

    let gone = index(&mut s, r);
    assert_eq!(gone.missing, 2);
    assert_eq!(all(&s, Scope::Library(Some(r))).len(), 6);
    let entries = all(&s, Scope::Playlist(list));
    assert_eq!(entries.len(), 2, "entries of missing tracks stay");
    assert!(entries.iter().all(|r| !r.available));
    assert!(!all(&s, Scope::Liked)[0].available);

    fs::write(&y_path, bytes).unwrap();
    fs::write(root.join("Album B/b2.wav"), b"second file of album b").unwrap();
    let back = index(&mut s, r);
    assert_eq!((back.added, back.missing), (0, 0));
    let entries = all(&s, Scope::Playlist(list));
    assert_eq!(entries.iter().map(|r| r.id).collect::<Vec<_>>(), [y, b2]);
    assert!(entries.iter().all(|r| r.available));
}

#[test]
fn ambiguous_moves_and_unreadable_sheets_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut s = Store::memory();
    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let rows = all(&s, Scope::Library(Some(r)));
    let x = titled(&rows, "x").id;
    let b1 = chapter(&rows, "b.cue", 1).id;
    s.set_liked(&[x, b1], true).unwrap();

    // The single disappears and two identical copies appear: no guess.
    let x_path = root.join("Singles/x.mp3");
    let bytes = fs::read(&x_path).unwrap();
    fs::remove_file(&x_path).unwrap();
    for copy in ["Copy 1", "Copy 2"] {
        fs::create_dir_all(root.join(copy)).unwrap();
        fs::write(root.join(copy).join("x.mp3"), &bytes).unwrap();
    }
    // A sheet that cannot be read leaves its tracks as they were.
    fs::write(root.join("Album B/b.cue"), "TRACK 99 AUDIO").unwrap();

    let summary = index(&mut s, r);
    assert_eq!((summary.relocated, summary.ambiguous), (0, 2));
    let liked = all(&s, Scope::Liked);
    let x_row = liked.iter().find(|r| r.id == x).unwrap();
    assert!(!x_row.available);
    assert!(x_row.track.source.ends_with("Singles/x.mp3"));
    assert!(liked.iter().find(|r| r.id == b1).unwrap().available);
}

#[test]
fn removing_a_collection_keeps_personal_tracks_and_rejoins_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut s = Store::memory();
    assert!(s.add_root(&root.join("Singles")).is_ok());
    assert!(s.add_root(&root).is_err(), "collections may not overlap");
    let r = s.roots().unwrap()[0].id;
    s.remove_root(r).unwrap();
    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let rows = all(&s, Scope::Library(Some(r)));
    let three = chapter(&rows, "a.cue", 3).id;
    s.set_rating(&[three], 5).unwrap();

    s.remove_root(r).unwrap();
    assert!(s.roots().unwrap().is_empty());
    let count = |s: &Store| -> i64 {
        s.db.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count(&s), 1, "only the rated track remains");
    assert_eq!(s.marks().unwrap()[0].2.rating, 5);

    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let rows = all(&s, Scope::Library(Some(r)));
    assert_eq!(rows.len(), 8);
    assert_eq!(chapter(&rows, "a.cue", 3).id, three);
}

#[test]
fn browsed_tracks_get_identities_that_a_later_collection_adopts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let folder = crate::library::browse(&root.join("Album A")).unwrap();
    let browsed: Vec<Arc<Track>> = folder.tracks.into_iter().map(Arc::new).collect();
    assert_eq!(browsed.len(), 4);
    let mut s = Store::memory();
    let ids = s.resolve(&browsed[2..3]).unwrap();
    s.set_liked(&ids, true).unwrap();
    assert_eq!(s.resolve(&browsed[2..3]).unwrap(), ids);
    assert_eq!(s.marks().unwrap()[0].1, Some(Locator::of(&browsed[2])));

    let r = s.add_root(&root).unwrap();
    let summary = index(&mut s, r);
    assert_eq!(
        summary.added, 4,
        "the disc's 4 tracks already had identities"
    );
    let rows = all(&s, Scope::Library(Some(r)));
    assert_eq!(chapter(&rows, "a.cue", 2).id, ids[0]);
}

#[test]
fn playlists_order_duplicates_and_queries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut s = Store::memory();
    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let rows = all(&s, Scope::Library(Some(r)));
    let (one, two, x) = (
        chapter(&rows, "a.cue", 1).id,
        chapter(&rows, "a.cue", 2).id,
        titled(&rows, "x").id,
    );
    let p = s.create_playlist("Set", &[one, two]).unwrap();
    let added = s.add_entries(p, &[x, one], Some(1)).unwrap();
    let order =
        |s: &Store| -> Vec<TrackId> { all(s, Scope::Playlist(p)).iter().map(|r| r.id).collect() };
    assert_eq!(order(&s), [one, x, one, two]);
    s.move_entries(p, &[added[0]], None).unwrap();
    assert_eq!(order(&s), [one, one, two, x]);
    s.remove_entries(p, &[added[1]]).unwrap();
    assert_eq!(order(&s), [one, two, x]);
    assert_eq!(s.unique_playlist_name("Set").unwrap(), "Set 2");
    let copy = s.duplicate_playlist(p, "Set 2").unwrap();
    s.rename_playlist(p, "Renamed").unwrap();
    assert_eq!(
        order(&s),
        all(&s, Scope::Playlist(copy))
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    );

    s.set_rating(&[two, x], 3).unwrap();
    s.set_rating(&[x], 5).unwrap();
    let query = |search: &str, order| Query {
        scope: Scope::Library(Some(r)),
        search: search.into(),
        order,
    };
    assert_eq!(s.keys(&query("rating:4", None)).unwrap(), [x]);
    assert_eq!(s.keys(&query("rating:3", None)).unwrap().len(), 2);
    assert_eq!(s.keys(&query("rating:0", None)).unwrap().len(), 6);
    assert_eq!(s.keys(&query("format:wav", None)).unwrap().len(), 2);
    assert_eq!(
        s.keys(&query("singles", None)).unwrap().len(),
        2,
        "paths match"
    );
    let by_title = s.keys(&query("", Some((Order::Title, true)))).unwrap();
    let titles: Vec<String> = s
        .rows(Scope::Library(Some(r)), &by_title)
        .unwrap()
        .iter()
        .map(|r| r.track.title.to_lowercase())
        .collect();
    assert!(titles.windows(2).all(|w| w[0] >= w[1]));
    let albums = s.albums(Some(r), "").unwrap();
    assert_eq!(albums.len(), 3);
    assert_eq!(albums.iter().map(|a| a.count).sum::<usize>(), 8);
    assert_eq!(s.albums(Some(r), "b1").unwrap()[0].count, 1);
}

#[test]
fn copying_playlist_occurrences_keeps_chapters_order_and_missing_sources() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut store = Store::memory();
    let collection = store.add_root(&root).unwrap();
    index(&mut store, collection);
    let tracks = all(&store, Scope::Library(Some(collection)));
    let first = chapter(&tracks, "a.cue", 1).id;
    let second = chapter(&tracks, "a.cue", 2).id;
    let source = store
        .create_playlist("Source", &[first, second, first])
        .unwrap();
    let destination = store.create_playlist("Destination", &[]).unwrap();
    let occurrences = store.entries(source).unwrap();

    fs::remove_file(root.join("Album A/a.flac")).unwrap();
    index(&mut store, collection);
    let selection = [occurrences[1].0, occurrences[0].0, occurrences[2].0];
    let ids = store
        .track_ids(Scope::Playlist(source), &selection)
        .unwrap();
    let added = store.add_entries(destination, &ids, None).unwrap();
    assert_eq!(store.entries(source).unwrap(), occurrences);
    assert_eq!(ids, [second, first, first]);
    assert_eq!(
        store.entries(destination).unwrap(),
        added.iter().copied().zip(ids).collect::<Vec<_>>()
    );
    assert!(added.iter().all(|entry| !selection.contains(entry)));
}

#[test]
fn search_and_disc_playback_share_exact_chapter_identity_across_relocation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let mut s = Store::memory();
    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let query = Query {
        scope: Scope::Library(Some(r)),
        search: "title:b2".into(),
        order: None,
    };
    let (listing, sections) = s.table_listing(&query, true).unwrap();
    assert_eq!(
        (sections.len(), sections[0].count, listing.keys.len()),
        (1, 1, 1)
    );
    let selected = s.rows(query.scope, &listing.keys).unwrap().pop().unwrap();
    let disc = s.registered_disc(&selected.track).unwrap().unwrap();
    assert_eq!(
        (disc.root, disc.album, disc.track),
        (r, sections[0].id, selected.id)
    );
    let cd = s.disc_keys(Scope::Album(disc.album)).unwrap();
    assert_eq!(cd.len(), 2);
    assert_eq!(cd[1], selected.id);
    fs::rename(root.join("Album B"), root.join("Moved B")).unwrap();
    index(&mut s, r);
    let moved = s.rows(query.scope, &[selected.id]).unwrap().pop().unwrap();
    assert_ne!(moved.track.source, selected.track.source);
    assert!(selected.track.same_item(&moved.track));
    assert_eq!(
        s.registered_disc(&selected.track).unwrap().unwrap().track,
        moved.id
    );
}

#[test]
fn same_prefix_and_filename_are_insufficient_relocation_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Música 日本");
    let first = root.join("disc 1/song.wav");
    let second = root.join("disc 2/song.wav");
    fs::create_dir_all(first.parent().unwrap()).unwrap();
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    let mut bytes = vec![0; 96 * 1024];
    fs::write(&first, &bytes).unwrap();
    let mut store = Store::memory();
    let id = store.add_root(&root).unwrap();
    index(&mut store, id);
    let original = all(&store, Scope::Library(Some(id)))[0].id;
    store.set_liked(&[original], true).unwrap();
    // Allocate the second file while the first file object still exists.
    *bytes.last_mut().unwrap() = 1;
    fs::write(&second, &bytes).unwrap();
    fs::rename(&first, dir.path().join("offline.wav")).unwrap();
    assert_eq!(index(&mut store, id).relocated, 0);
    let liked = all(&store, Scope::Liked);
    assert_eq!(liked[0].id, original);
    assert!(!liked[0].available);
    assert_ne!(all(&store, Scope::Library(Some(id)))[0].id, original);
}

#[test]
fn a_superseded_scan_cannot_publish_after_root_removal_or_a_new_scan() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    collection(&root);
    let path = dir.path().join("library.sqlite3");
    let mut older = Store::open(&path).unwrap();
    let mut newer = Store::open(&path).unwrap();
    let id = older.add_root(&root).unwrap();
    index(&mut older, id);
    let track = all(&older, Scope::Library(Some(id)))[0].id;
    newer.set_rating(&[track], 4).unwrap();
    let mut replace = |p| {
        if matches!(p, Progress::Walking) {
            index(&mut newer, id);
        }
    };
    let error = index_root(&mut older, id, None, &|| true, &mut replace).unwrap_err();
    assert!(is_cancelled(&error));
    let mut remove = |p| {
        if matches!(p, Progress::Walking) {
            newer.remove_root(id).unwrap();
        }
    };
    let error = index_root(&mut older, id, None, &|| true, &mut remove).unwrap_err();
    assert!(is_cancelled(&error));
    drop(older);
    let reopened = Store::open(&path).unwrap();
    assert!(reopened.roots().unwrap().is_empty());
    assert_eq!(reopened.marks_for(&[track]).unwrap()[0].2.rating, 4);
}

#[test]
fn stale_playlist_keys_cannot_target_another_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::memory();
    let file = dir.path().join("single.wav");
    fs::write(&file, b"sample").unwrap();
    let ids = s.resolve(&[Arc::new(Track::file(file))]).unwrap();
    let p = s.create_playlist("First", &ids).unwrap();
    let old = s.entries(p).unwrap()[0].0;
    let q = s.create_playlist("Second", &ids).unwrap();
    assert!(s.rows(Scope::Playlist(q), &[old]).unwrap().is_empty());
    assert!(s.track_ids(Scope::Playlist(q), &[old]).is_err());
    s.remove_entries(p, &[old]).unwrap();
    let current = s.add_entries(p, &ids, None).unwrap()[0];
    assert_ne!(current, old);
    assert!(s.move_entries(p, &[current], Some(old)).is_err());
    assert_eq!(s.entries(p).unwrap(), [(current, ids[0])]);
}

#[test]
fn replacing_pending_queries_keeps_the_new_read_after_an_intervening_edit() {
    use std::{sync::mpsc, time::Duration};
    let dir = tempfile::tempdir().unwrap();
    let worker = Handle::spawn(dir.path().join("library.sqlite3"));
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let timeout = Duration::from_secs(5);
    let _busy = worker.call(move |_| {
        entered.send(())?;
        wait.recv_timeout(timeout)?;
        Ok(())
    });
    ready.recv_timeout(timeout).unwrap();
    let ticket = Ticket::default();
    for _ in 0..300 {
        let _superseded =
            worker.query::<(), _>(&ticket, ticket.next(), |_| panic!("stale query ran"));
    }
    let _edit = worker.call(|s| s.create_playlist("Saved", &[]));
    let (result, observed) = mpsc::channel();
    let _latest = worker.query(&ticket, ticket.next(), move |s| {
        result.send(s.playlists()?)?;
        Ok(())
    });
    release.send(()).unwrap();
    assert_eq!(observed.recv_timeout(timeout).unwrap()[0].name, "Saved");
}

fn tagged_wave(title: &[u8; 8], value: i16) -> Vec<u8> {
    let mut wave = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
    wave.extend(44100u32.to_le_bytes());
    wave.extend(88200u32.to_le_bytes());
    wave.extend(b"\x02\0\x10\0LIST\x18\0\0\0INFOINAM\x0c\0\0\0");
    wave.extend(title);
    wave.extend([0; 4]);
    wave.extend(b"data");
    wave.extend(96000u32.to_le_bytes());
    for _ in 0..48000 {
        wave.extend(value.to_le_bytes());
    }
    let size = wave.len() as u32 - 8;
    wave[4..8].copy_from_slice(&size.to_le_bytes());
    wave
}

#[test]
fn tag_corrections_moves_and_same_path_replacement_keep_personal_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::memory();
    let root = s.add_root(dir.path()).unwrap();
    let path = dir.path().join("first.wav");
    fs::write(&path, tagged_wave(b"Misspelt", 500)).unwrap();
    index(&mut s, root);
    let old = all(&s, Scope::Library(Some(root)))[0].id;
    s.set_liked(&[old], true).unwrap();
    s.set_rating(&[old], 4).unwrap();
    let mix = s.create_playlist("Mix", &[old, old]).unwrap();
    let positions = s.entries(mix).unwrap();

    fs::write(&path, tagged_wave(b"Repaired", 500)).unwrap();
    index(&mut s, root);
    assert_eq!(all(&s, Scope::Library(Some(root)))[0].id, old);
    let moved = dir.path().join("renamed.wav");
    fs::rename(path, &moved).unwrap();
    assert_eq!(index(&mut s, root).relocated, 1);
    assert_eq!(all(&s, Scope::Library(Some(root)))[0].id, old);

    // Keep the same object, size, tag, prefix and modification timestamp.
    // A change in the sampled tail still separates the two recordings.
    let time = fs::metadata(&moved).unwrap().modified().unwrap();
    let mut other = tagged_wave(b"Repaired", 500);
    other[90000..].fill(0x55);
    fs::write(&moved, other).unwrap();
    fs::File::options()
        .write(true)
        .open(&moved)
        .unwrap()
        .set_modified(time)
        .unwrap();
    index(&mut s, root);
    let current = all(&s, Scope::Library(Some(root)));
    assert_eq!(current.len(), 1);
    assert_ne!(current[0].id, old);
    assert_eq!(s.entries(mix).unwrap(), positions);
    assert!(all(&s, Scope::Playlist(mix)).iter().all(|r| !r.available));
    assert_eq!(
        s.marks().unwrap(),
        [(
            old,
            None,
            Mark {
                liked: true,
                rating: 4
            }
        )]
    );
    assert_eq!(
        s.resolve(&[Arc::new(Track::file(moved))]).unwrap(),
        [current[0].id]
    );
}

#[test]
fn reauthored_program_keeps_only_matching_source_and_frame_ranges() {
    let dir = tempfile::tempdir().unwrap();
    collection(dir.path());
    let mut s = Store::memory();
    let root = s.add_root(dir.path()).unwrap();
    index(&mut s, root);
    let rows = all(&s, Scope::Library(Some(root)));
    let two = chapter(&rows, "a.cue", 2);
    let three = chapter(&rows, "a.cue", 3);
    s.set_liked(&[two.id, three.id], true).unwrap();
    let mix = s.create_playlist("Archive", &[two.id, three.id]).unwrap();
    let sheet = dir.path().join("Album A/a.cue");
    fs::write(&sheet, ALBUM_A.replace("01:00:00", "01:20:00")).unwrap();
    index(&mut s, root);
    let shifted = all(&s, Scope::Library(Some(root)));
    assert_ne!(chapter(&shifted, "a.cue", 2).id, two.id);
    assert_eq!(chapter(&shifted, "a.cue", 3).id, three.id);
    assert!(!all(&s, Scope::Playlist(mix))[0].available);
    assert!(s.resolve(std::slice::from_ref(&two.track)).is_err());

    fs::write(sheet.with_file_name("other.flac"), b"different music").unwrap();
    fs::write(&sheet, ALBUM_A.replace("a.flac", "other.flac")).unwrap();
    index(&mut s, root);
    let switched = all(&s, Scope::Library(Some(root)));
    assert_ne!(chapter(&switched, "a.cue", 3).id, three.id);
    assert!(all(&s, Scope::Playlist(mix)).iter().all(|r| !r.available));

    // Putting the exact original program back rejoins its original entries.
    fs::write(sheet, ALBUM_A).unwrap();
    index(&mut s, root);
    assert!(all(&s, Scope::Playlist(mix)).iter().all(|r| r.available));
    assert_eq!(all(&s, Scope::Playlist(mix))[0].id, two.id);
}

#[test]
fn detached_tracks_are_checked_before_playback_and_personal_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("single.wav");
    fs::write(&path, b"first").unwrap();
    let mut s = Store::memory();
    let track = [Arc::new(Track::file(path.clone()))];
    let old = s.resolve(&track).unwrap();
    let mix = s.create_playlist("Keep", &old).unwrap();
    let row = all(&s, Scope::Playlist(mix)).remove(0);
    assert!(DetachedFiles::default().available(&row));
    fs::write(path, b"other").unwrap();
    assert!(!DetachedFiles::default().available(&row));
    assert_ne!(s.resolve(&track).unwrap(), old);
    assert!(!all(&s, Scope::Playlist(mix))[0].available);
}

/// Two two-disc volumes side by side and a single CD: discs of one volume
/// form a release, the volumes and the single stay apart.
fn releases(root: &Path) {
    for (folder, title) in [
        ("Vol I/CD1", "Vol I (CD1)"),
        ("Vol I/CD2", "Vol I (CD2)"),
        ("Vol II/CD1", "Vol II (CD1)"),
        ("Vol II/CD2", "Vol II (CD2)"),
        ("Single/CD", "Single"),
    ] {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("disc.cue"),
            format!(
                "TITLE \"{title}\"\nFILE \"disc.flac\" WAVE\nTRACK 01 AUDIO\nTITLE \"{title} one\"\n\
                 INDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"{title} two\"\nINDEX 01 01:00:00\n"
            ),
        )
        .unwrap();
        fs::write(dir.join("disc.flac"), format!("image of {title}")).unwrap();
    }
}

#[test]
fn indexed_releases_group_discs_of_one_folder_and_list_them_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    releases(&root);
    let mut s = Store::memory();
    let r = s.add_root(&root).unwrap();
    index(&mut s, r);
    let albums = s.albums(Some(r), "").unwrap();
    assert_eq!(albums.len(), 5);
    let by_title = |t: &str| albums.iter().find(|a| a.title == t).unwrap();
    let (one, two) = (by_title("Vol I (CD1)"), by_title("Vol I (CD2)"));
    let release = two.release.clone().unwrap();
    assert_eq!(
        (release.first, release.number, release.title.as_str()),
        (one.id, 2, "Vol I")
    );
    assert_eq!(
        by_title("Vol II (CD1)").release.as_ref().unwrap().title,
        "Vol II"
    );
    assert_ne!(
        by_title("Vol II (CD2)").release.as_ref().unwrap().first,
        one.id
    );
    assert!(by_title("Single").release.is_none());
    assert_eq!(s.release_discs(two.id).unwrap(), [one.id, two.id]);
    assert_eq!(s.release_discs(by_title("Single").id).unwrap().len(), 1);
    assert_eq!(s.membership(one.id).unwrap().unwrap().number, 1);

    // Play Release lists CD 1 then CD 2 in authored order, and only them.
    let keys = s.disc_keys(Scope::Release(one.id)).unwrap();
    let titles: Vec<_> = s
        .rows(Scope::Release(one.id), &keys)
        .unwrap()
        .into_iter()
        .map(|r| r.track.title.clone())
        .collect();
    assert_eq!(
        titles,
        [
            "Vol I (CD1) one",
            "Vol I (CD1) two",
            "Vol I (CD2) one",
            "Vol I (CD2) two"
        ]
    );
    // A release page groups its discs; a search keeps the matching disc.
    let query = Query {
        scope: Scope::Release(one.id),
        search: "two".into(),
        order: None,
    };
    let (listing, sections) = s.table_listing(&query, true).unwrap();
    assert_eq!((listing.keys.len(), sections.len()), (2, 2));
    let query = Query {
        search: "title:cd2".into(),
        ..query
    };
    let (listing, sections) = s.table_listing(&query, true).unwrap();
    assert_eq!((listing.keys.len(), sections.len()), (2, 1));
    assert_eq!(sections[0].id, two.id);

    // Grouping is derived on every read: personal marks keep their tracks.
    let liked = keys[2];
    s.set_liked(&[liked], true).unwrap();
    index(&mut s, r);
    assert_eq!(s.disc_keys(Scope::Release(one.id)).unwrap()[2], liked);
    assert_eq!(all(&s, Scope::Liked)[0].id, liked);
}

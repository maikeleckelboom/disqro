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
fn listening_list_keeps_the_displayed_occurrences_after_navigation() {
    use crate::catalog::ListeningList;
    use gpui_kit::block_on;

    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    collection(&music);
    let handle = Handle::spawn(dir.path().join("library.sqlite3"));
    let (liked, playlist, occurrences, filtered, one, three, single) =
        block_on(handle.call(move |s| {
            let root = s.add_root(&music)?;
            index(s, root);
            let scope = Scope::Library(Some(root));
            let rows = all(s, scope);
            let one = chapter(&rows, "a.cue", 1).id;
            let three = chapter(&rows, "a.cue", 3).id;
            let single = titled(&rows, "x").id;
            s.set_liked(&[one, three, single], true)?;
            let keys = |scope, search: &str, descending| {
                s.keys(&Query {
                    scope,
                    search: search.into(),
                    order: Some((Order::Title, descending)),
                })
            };
            let liked = keys(Scope::Liked, "", true)?;
            let filtered = keys(scope, "artist:artist", true)?;
            let playlist = s.create_playlist("Repeats", &[single, one, single, three])?;
            let occurrences = s.keys(&Query {
                scope: Scope::Playlist(playlist),
                search: String::new(),
                order: Some((Order::Title, true)),
            })?;
            Ok((liked, playlist, occurrences, filtered, one, three, single))
        }))
        .unwrap();
    let resolve = |list: ListeningList, key| {
        block_on(list.resolve(
            handle.clone(),
            Some(key),
            Ticket::default(),
            0,
            Default::default(),
        ))
        .unwrap()
    };
    // The browser can issue another query and the listener can even unlike
    // a row after activation. The captured occurrence keys still own order.
    block_on(handle.call(move |s| s.set_liked(&[one], false))).unwrap();
    let (tracks, start, skipped) = resolve(
        ListeningList::for_row(Scope::Liked, &liked.clone().into(), three, false),
        three,
    );
    let tracks = tracks.tracks;
    let ids = |tracks: &crate::library::TrackList| {
        tracks
            .iter()
            .map(|t| t.library_id.unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        (ids(&tracks), start, skipped),
        (vec![single, three, one], 1, 0)
    );
    assert!(tracks[1].disc.is_some() && tracks[2].disc.is_some());
    let chosen = occurrences[1];
    let (tracks, start, _) = resolve(
        ListeningList::for_row(
            Scope::Playlist(playlist),
            &occurrences.clone().into(),
            chosen,
            false,
        ),
        chosen,
    );
    assert_eq!(
        (ids(&tracks.tracks), start),
        (vec![single, single, three, one], 1)
    );
    let mut queue = crate::queue::Queue::default();
    queue.replace(tracks, start);
    assert_eq!(
        queue
            .entries
            .iter()
            .map(|entry| entry.playlist_occurrence.unwrap().entry)
            .collect::<Vec<_>>(),
        occurrences
    );
    let restored: crate::queue::Queue =
        serde_json::from_str(&serde_json::to_string(&queue).unwrap()).unwrap();
    let current = restored.current_entry().unwrap();
    let occurrence = current.playlist_occurrence.unwrap();
    let saved = block_on(
        handle.call(move |s| s.rows(Scope::Playlist(occurrence.playlist), &[occurrence.entry])),
    )
    .unwrap();
    assert!(current.track.library_id.is_none());
    assert!(saved[0].track.same_item(&current.track));
    assert_eq!(occurrence.entry, chosen);
    let (tracks, start, _) = resolve(
        ListeningList::Displayed(Scope::Library(None), filtered.clone().into()),
        three,
    );
    assert_eq!(ids(&tracks.tracks), filtered);
    assert_eq!(tracks.tracks[start].library_id, Some(three));
    let (tracks, start, _) = resolve(
        ListeningList::for_row(Scope::Liked, &liked.into(), three, true),
        three,
    );
    assert_eq!((ids(&tracks.tracks), start), (vec![three], 0));
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
    assert!(is_canceled(&stopped.unwrap_err()));
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
    let (listing, _) = s
        .table_listing(
            &Query {
                scope: Scope::Playlist(p),
                search: String::new(),
                order: None,
            },
            false,
            None,
        )
        .unwrap();
    assert_eq!(listing.playlist_tracks, [one, x, one, two]);
    assert_ne!(listing.keys[0], listing.keys[2]);
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
    assert_eq!(s.albums(Some(r), "title:b1").unwrap()[0].count, 1);
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
    let (listing, sections) = s.table_listing(&query, true, None).unwrap();
    assert_eq!(
        (sections.len(), sections[0].count, listing.keys.len()),
        (1, 1, 1)
    );
    // Filtering down to one file of a multi-file CUE does not turn its
    // embedded art into the section's cover.
    assert!(sections[0].cue_image.is_none());
    let (_, image_section) = s
        .table_listing(
            &Query {
                search: "title:two".into(),
                ..query.clone()
            },
            true,
            None,
        )
        .unwrap();
    assert_eq!(image_section.len(), 1);
    assert_eq!(image_section[0].count, 1);
    assert!(image_section[0].cover.is_none());
    assert_eq!(
        image_section[0].cue_image,
        Some(root.join("Album A/a.flac"))
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
    assert!(is_canceled(&error));
    let mut remove = |p| {
        if matches!(p, Progress::Walking) {
            newer.remove_root(id).unwrap();
        }
    };
    let error = index_root(&mut older, id, None, &|| true, &mut remove).unwrap_err();
    assert!(is_canceled(&error));
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
    let recording = Arc::new(Track::file(file));
    let ids = s.resolve(std::slice::from_ref(&recording)).unwrap();
    let p = s.create_playlist("First", &ids).unwrap();
    let old = s.entries(p).unwrap()[0].0;
    let q = s.create_playlist("Second", &ids).unwrap();
    assert!(s.rows(Scope::Playlist(q), &[old]).unwrap().is_empty());
    assert!(s.track_ids(Scope::Playlist(q), &[old]).is_err());
    s.remove_entries(p, &[old]).unwrap();
    assert!(s.rows(Scope::Playlist(p), &[old]).unwrap().is_empty());
    assert_eq!(s.find(&Locator::of(&recording)).unwrap(), Some(ids[0]));
    let current = s.add_entries(p, &ids, None).unwrap()[0];
    assert_ne!(current, old);
    assert!(s.move_entries(p, &[current], Some(old)).is_err());
    assert_eq!(s.entries(p).unwrap(), [(current, ids[0])]);
    s.delete_playlist(p).unwrap();
    assert!(s.rows(Scope::Playlist(p), &[current]).unwrap().is_empty());
    assert_eq!(s.find(&Locator::of(&recording)).unwrap(), Some(ids[0]));
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
    // A delayed hydration batch from an older view must not evict the winner.
    let stale = worker.query(&ticket, 0, |_| Ok(()));
    assert!(is_canceled(&gpui_kit::block_on(stale).unwrap_err()));
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
fn tagged_compilation_rescans_preserve_recording_and_album_identity() {
    use lofty::{
        config::WriteOptions,
        tag::{Accessor, ItemKey, Tag, TagExt, TagType},
    };
    let media = tempfile::tempdir().unwrap();
    let mut s = Store::memory();
    let root = s.add_root(media.path()).unwrap();
    let put = |file, title, value, performer: &str, credit: Option<&str>| {
        let path = media.path().join(file);
        fs::write(&path, tagged_wave(title, value)).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_artist(performer.to_owned());
        tag.set_album("One Title".into());
        if let Some(name) = credit {
            tag.insert_text(ItemKey::AlbumArtist, name.into());
        }
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    };
    put("a.wav", b"Song One", 31, "Alpha", Some(" Various Artists "));
    put("b.wav", b"Song Two", 53, "Beta", Some("Various Artists"));
    index(&mut s, root);
    let tracks = all(&s, Scope::Library(None));
    let original = tracks.iter().map(|r| r.id).collect::<Vec<_>>();
    let album = &s.albums(Some(root), "").unwrap()[0];
    assert_eq!(album.album_artist.as_deref(), Some("Various Artists"));
    let various = &s.artists("Various").unwrap()[0];
    assert_eq!((various.tracks, various.releases), (0, 1));
    s.set_liked(&original, true).unwrap();
    index(&mut s, root);
    assert_eq!(s.artists("Various").unwrap()[0].id, various.id);

    put("b.wav", b"Song Two", 53, "Beta", Some("Conflicting Credit"));
    index(&mut s, root);
    assert!(s.artists("Various").unwrap().is_empty());
    let updated = &s.albums(Some(root), "").unwrap()[0];
    assert!(updated.album_artist.is_none());
    assert_eq!(updated.id, album.id);
    let keys = s.disc_keys(Scope::Album(album.id)).unwrap();
    assert_eq!(keys, original);
    assert_eq!(all(&s, Scope::Liked).len(), 2);
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
        search: "title:two".into(),
        order: None,
    };
    let (listing, sections) = s.table_listing(&query, true, None).unwrap();
    assert_eq!((listing.keys.len(), sections.len()), (2, 2));
    let query = Query {
        search: "title:cd2".into(),
        ..query
    };
    let (listing, sections) = s.table_listing(&query, true, None).unwrap();
    assert_eq!((listing.keys.len(), sections.len()), (2, 1));
    assert_eq!(sections[0].id, two.id);
    let flat = Query {
        scope: Scope::Library(Some(r)),
        order: Some((Order::Title, true)),
        ..query
    };
    let child = Scope::Release(one.id);
    let (listing, _) = s.table_listing(&flat, false, Some(child)).unwrap();
    let disc_keys = s
        .keys(&Query {
            scope: Scope::Album(two.id),
            ..flat
        })
        .unwrap();
    assert_eq!(listing.child_row, Some((child, disc_keys[0])));

    // Grouping is derived on every read: personal marks keep their tracks.
    let liked = keys[2];
    s.set_liked(&[liked], true).unwrap();
    index(&mut s, r);
    assert_eq!(s.disc_keys(Scope::Release(one.id)).unwrap()[2], liked);
    assert_eq!(all(&s, Scope::Liked)[0].id, liked);
}

#[test]
fn indexed_navigation_uses_the_unfiltered_shelf_and_excludes_removed_media() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Collection");
    let nested = root.join("Archive/Volumes/Early");
    let other = dir.path().join("Another collection");
    releases(&nested);
    releases(&other);
    fs::create_dir_all(root.join("Empty")).unwrap();
    fs::write(root.join("Loose.wav"), b"loose").unwrap();
    let mut s = Store::memory();
    let root_id = s.add_root(&root).unwrap();
    let other_id = s.add_root(&other).unwrap();
    index(&mut s, root_id);
    index(&mut s, other_id);

    let albums = s.albums(Some(root_id), "").unwrap();
    let start = albums
        .iter()
        .find(|album| album.title == "Vol I (CD1)")
        .unwrap();
    let (scope, nodes) = s.navigation_albums(None, start.id).unwrap().unwrap();
    assert_eq!(scope, root_id);
    assert_eq!(
        nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
        albums.iter().map(|album| album.id).collect::<Vec<_>>()
    );
    assert_eq!(nodes.iter().filter(|node| node.is_disc).count(), 5);
    assert!(nodes.iter().all(|node| node.title != "Empty"));
    assert_eq!(
        nodes
            .iter()
            .find(|node| node.id == start.id)
            .unwrap()
            .release
            .as_ref()
            .unwrap()
            .number,
        1
    );

    fs::remove_file(nested.join("Single/CD/disc.flac")).unwrap();
    index(&mut s, root_id);
    let (_, nodes) = s
        .navigation_albums(Some(root_id), start.id)
        .unwrap()
        .unwrap();
    assert_eq!(nodes.iter().filter(|node| node.is_disc).count(), 4);
    assert!(nodes.iter().all(|node| node.title != "Single"));
}

#[test]
fn collection_playback_keeps_release_discs_together_even_when_their_years_differ() {
    let dir = tempfile::tempdir().unwrap();
    releases(dir.path());
    let mut s = Store::memory();
    let root = s.add_root(dir.path()).unwrap();
    index(&mut s, root);
    s.db.execute(
        "UPDATE albums SET year = CASE title WHEN 'Vol I (CD1)' THEN 1980 WHEN 'Vol I (CD2)' THEN 2010 ELSE 1995 END",
        [],
    ).unwrap();
    let keys = s
        .collection_keys(Scope::Library(Some(root)), "two".into())
        .unwrap();
    let titles: Vec<_> = s
        .rows(Scope::Library(Some(root)), &keys)
        .unwrap()
        .into_iter()
        .map(|r| r.track.title.clone())
        .collect();
    assert_eq!(
        titles,
        [
            "Vol I (CD1) two",
            "Vol I (CD2) two",
            "Single two",
            "Vol II (CD1) two",
            "Vol II (CD2) two"
        ]
    );
}

#[test]
fn catalog_search_follows_cue_identity_and_catalog_edits() {
    use search::Target;
    let dir = tempfile::tempdir().unwrap();
    collection(dir.path());
    let mut s = Store::memory();
    let root = s.add_root(dir.path()).unwrap();
    index(&mut s, root);
    let track = chapter(&all(&s, Scope::Library(Some(root))), "a.cue", 2).id;
    let playlist = s.create_playlist("Two mixes", &[track, track]).unwrap();
    let matches = s.search_library("Tw", 5, 0).unwrap().hits;
    assert!(matches.iter().any(|h| h.target == Target::Track(track)
        && h.detail.contains("a.cue")
        && h.detail.contains("Chapter 2")));
    assert!(
        matches
            .iter()
            .any(|h| h.target == Target::Playlist(playlist))
    );
    assert!(
        s.search_library("art", 5, 0)
            .unwrap()
            .hits
            .iter()
            .any(|h| matches!(h.target, Target::Artist(_)) && h.title == "Artist")
    );
    fs::write(
        dir.path().join("Album A/a.cue"),
        ALBUM_A.replace("Two", "Deux"),
    )
    .unwrap();
    index(&mut s, root);
    assert!(
        s.search_library("Deux", 5, 0)
            .unwrap()
            .hits
            .iter()
            .any(|h| h.target == Target::Track(track))
    );
    assert!(
        !s.search_library("Two", 5, 0)
            .unwrap()
            .hits
            .iter()
            .any(|h| h.target == Target::Track(track))
    );
    s.remove_root(root).unwrap();
    assert!(s.search_library("Deux", 5, 0).unwrap().hits.is_empty());
    assert_eq!(
        s.search_library("Two", 5, 0).unwrap().hits[0].target,
        Target::Playlist(playlist)
    );
}

#[test]
fn reveal_resolves_the_exact_occurrence_and_unfiltered_scope() {
    use crate::queue::PlaylistOccurrence;
    let dir = tempfile::tempdir().unwrap();
    collection(dir.path());
    let mut s = Store::memory();
    let root = s.add_root(dir.path()).unwrap();
    index(&mut s, root);
    let rows = all(&s, Scope::Library(None));
    let first = chapter(&rows, "a.cue", 1);
    let second = chapter(&rows, "a.cue", 2);
    let playlist = s
        .create_playlist("Duplicates", &[first.id, second.id, first.id])
        .unwrap();
    let occurrences = s.entries(playlist).unwrap();
    let releases = s.collection_albums(Scope::Releases, "").unwrap();
    assert_eq!(
        releases
            .iter()
            .map(|disc| disc.title.as_str())
            .collect::<Vec<_>>(),
        ["Album A", "Album B"]
    );
    let exact = PlaylistOccurrence {
        playlist,
        entry: occurrences[2].0,
    };
    assert_eq!(
        s.reveal_key(Scope::Playlist(playlist), first.id, Some(exact))
            .unwrap(),
        Some(exact.entry)
    );
    assert_eq!(
        s.reveal_key(Scope::Playlist(playlist), second.id, Some(exact))
            .unwrap(),
        None
    );
    let disc = s.registered_disc(&first.track).unwrap().unwrap();
    assert_eq!(
        s.reveal_key(Scope::Album(disc.album), second.id, None)
            .unwrap(),
        Some(second.id)
    );
    assert_eq!(
        s.reveal_key(Scope::CollectionFolder(root), first.id, None)
            .unwrap(),
        None
    );
}

#[test]
#[ignore = "six-figure catalog playback qualification"]
fn six_figure_listening_snapshot() {
    use crate::catalog::ListeningList;
    use gpui_kit::block_on;
    let dir = tempfile::tempdir().unwrap();
    let store = Handle::spawn(dir.path().join("library.sqlite3"));
    let path = dir.path().to_owned();
    let keys = block_on(store.call(move |s|{
        let root=s.add_root(&path)?;
        let tx=s.db.transaction()?;
        tx.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100001)
          INSERT INTO files(id,path,path_key,name,kind,root_id,size,modified)
          SELECT i,printf('music/%06d.wav',i),printf('music/%06d.wav',i),'fixture.wav',0,?1,400,1 FROM n",[root])?;
        tx.execute("INSERT INTO artists(id,key,name)
          SELECT id,printf('performer %04d',id),printf('Performer %04d',id) FROM files WHERE id <= 1000",[])?;
        tx.execute("INSERT INTO albums(id,key,root_id,title,folder,sort_key,is_release)
          SELECT id,printf('release:%04d',id),?1,printf('Release %04d',id),'music',printf('release%04d',id),1
          FROM artists",[root])?;
        tx.execute("INSERT INTO tracks(anchor,part,source,root_id,album_id,title,artist,album,format,title_key,artist_key,album_key,search,added_at)
          SELECT f.id,0,f.id,f.root_id,a.id,f.path,p.name,a.title,'WAV',f.path,p.key,lower(a.title),f.path,1
          FROM files f JOIN artists p ON p.id = f.id % 1000 + 1 JOIN albums a ON a.id=p.id",[])?;
        tx.commit()?;
        let start = std::time::Instant::now();
        let artists = s.artists("")?;
        assert_eq!(artists.len(), 1000);
        eprintln!("1,000 artist summaries from 100,001 recordings in {:?}", start.elapsed());
        let chosen = artists[700].id;
        let start = std::time::Instant::now();
        let tracks = s.keys(&Query {scope:Scope::Artist(chosen),search:String::new(),order:None})?;
        assert_eq!(tracks.len(), 100);
        assert_eq!(s.collection_albums(Scope::ArtistReleases(chosen), "")?.len(), 1);
        assert_eq!(s.reveal_key(Scope::Artist(chosen), tracks[50], None)?, Some(tracks[50]));
        eprintln!("Scoped tracks, releases and exact reveal in {:?}", start.elapsed());
        let mut plan = s.db.prepare(&format!("EXPLAIN QUERY PLAN SELECT id FROM tracks t WHERE {}", super::artists::tracks_of(1)))?;
        let operations = plan.query_map([chosen.0], |r| r.get::<_, String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?;
        assert!(operations.iter().any(|operation| operation.contains("tracks_performer")), "{operations:?}");
        eprintln!("Artist query plan {operations:?}");
        s.keys(&Query {scope:Scope::Library(None),search:String::new(),order:Some((Order::Title,true))})
    })).unwrap();
    assert_eq!(keys.len(), 100001);
    let chosen = keys[60000];
    let start = std::time::Instant::now();
    let (tracks, i, skipped) = block_on(
        ListeningList::Displayed(Scope::Library(None), keys.into()).resolve(
            store,
            Some(chosen),
            Ticket::default(),
            0,
            Default::default(),
        ),
    )
    .unwrap();
    assert_eq!((tracks.tracks.len(), i, skipped), (100001, 60000, 0));
    assert_eq!(tracks.tracks[i].library_id, Some(chosen));
    eprintln!(
        "100,001 sorted recordings resolved in {:?}",
        start.elapsed()
    );
}

#[test]
fn collection_folder_scope_uses_indexed_anchors_and_keeps_playback_in_scope() {
    let temp = tempfile::tempdir().unwrap();
    let music = temp.path().join("Music");
    collection(&music);
    fs::write(music.join("root.wav"), b"loose recording").unwrap();
    let mut store = Store::open(&temp.path().join("library.sqlite3")).unwrap();
    let root = store.add_root(&music).unwrap();
    index(&mut store, root);

    let direct = Scope::CollectionFolder(root);
    let rows = all(&store, direct);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].track.title, "root");
    assert_eq!(all(&store, Scope::Library(Some(root))).len(), 9);
    let shelf = store.collection_albums(direct, "").unwrap();
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].count, 1);
    assert_eq!(
        store.collection_keys(direct, String::new()).unwrap(),
        [rows[0].key]
    );
}

#[test]
fn artists_separate_track_and_album_artists_and_keep_compilations_open() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Music");
    let sheet = |folder: &str, file: &str, text: &str| {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), text).unwrap();
        fs::write(dir.join("image.wav"), b"compilation image").unwrap();
    };
    // A compilation: credited to Various Artists, performed by others,
    // including a duo whose name is never split.
    sheet(
        "Compilation",
        "c.cue",
        "TITLE \"Hard Hits\"\nPERFORMER \"Various Artists\"\nFILE \"image.wav\" WAVE\n\
         TRACK 01 AUDIO\nTITLE \"First\"\nPERFORMER \"Alpha\"\nINDEX 01 00:00:00\n\
         TRACK 02 AUDIO\nTITLE \"Second\"\nPERFORMER \"Beta\"\nINDEX 01 00:10:00\n\
         TRACK 03 AUDIO\nTITLE \"Third\"\nPERFORMER \"Alpha & Gamma\"\nINDEX 01 00:20:00\n",
    );
    // An album of Alpha's own, and its two discs of a live release.
    sheet(
        "Alpha Album",
        "a.cue",
        "TITLE \"Own Work\"\nPERFORMER \"Alpha\"\nFILE \"image.wav\" WAVE\n\
         TRACK 01 AUDIO\nTITLE \"Own One\"\nINDEX 01 00:00:00\n\
         TRACK 02 AUDIO\nTITLE \"Own Two\"\nINDEX 01 00:10:00\n",
    );
    for n in [1, 2] {
        sheet(
            &format!("Live/CD {n}"),
            &format!("Live (CD{n}).cue"),
            &format!(
                "TITLE \"Live (CD{n})\"\nPERFORMER \"alpha\"\nFILE \"image.wav\" WAVE\n\
                 TRACK 01 AUDIO\nTITLE \"Live {n}\"\nINDEX 01 00:00:00\n"
            ),
        );
    }
    // A recording nobody is credited for.
    sheet(
        "Anonymous",
        "u.cue",
        "TITLE \"Untitled\"\nFILE \"image.wav\" WAVE\n\
         TRACK 01 AUDIO\nTITLE \"Nobody\"\nINDEX 01 00:00:00\n",
    );
    let mut s = Store::open(&temp.path().join("library.sqlite3")).unwrap();
    let id = s.add_root(&root).unwrap();
    index(&mut s, id);
    let artists = s.artists("").unwrap();
    let names: Vec<_> = artists.iter().map(|a| a.name.as_str()).collect();
    assert!(
        names.contains(&"Beta") && names.contains(&"Alpha & Gamma"),
        "{names:?}"
    );
    assert!(names.contains(&"Various Artists"), "{names:?}");
    assert_eq!(names.len(), 4, "{names:?}");
    // One artist however its name is cased.
    assert_eq!(
        names
            .iter()
            .filter(|n| n.eq_ignore_ascii_case("alpha"))
            .count(),
        1
    );
    let alpha = artists
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("alpha"))
        .unwrap();
    assert_eq!(s.artists(" ALPHA ").unwrap()[0].id, alpha.id);
    // Its compilation track, its album and the live release, counted once.
    assert_eq!(alpha.tracks, 1 + 2 + 2);
    assert_eq!(alpha.releases, 3);
    let various = artists
        .iter()
        .find(|a| a.name == "Various Artists")
        .unwrap();
    assert_eq!((various.tracks, various.releases), (0, 1));
    // Filtering by name.
    assert_eq!(
        s.artists("gamma")
            .unwrap()
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>(),
        ["Alpha & Gamma"]
    );
    // An artist's tracks and releases, through the same views as a
    // collection's.
    let tracks = all(&s, Scope::Artist(alpha.id));
    let mut titles: Vec<_> = tracks.iter().map(|r| r.track.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["First", "Live 1", "Live 2", "Own One", "Own Two"]);
    let releases = s
        .collection_albums(Scope::ArtistReleases(alpha.id), "")
        .unwrap();
    assert_eq!(releases.len(), 4);
    assert_eq!(
        releases.iter().filter(|r| r.release.is_some()).count(),
        2,
        "the live discs keep their release"
    );
    assert!(all(&s, Scope::Artist(various.id)).is_empty());
    let compilation = s
        .collection_albums(Scope::ArtistReleases(various.id), "")
        .unwrap();
    assert_eq!(compilation.len(), 1);
    assert_eq!(
        (compilation[0].title.as_str(), compilation[0].count),
        ("Hard Hits", 3)
    );
    assert_eq!(
        compilation[0].album_artist.as_deref(),
        Some("Various Artists")
    );
    let hits = s.search_library("Various", 20, 0).unwrap().hits;
    assert!(
        hits.iter()
            .any(|h| h.target == search::Target::Artist(various.id))
    );
    let missing = all(&s, Scope::Library(None));
    assert!(
        missing
            .iter()
            .any(|row| row.track.title == "Nobody" && row.track.album_artist.is_none())
    );
    index(&mut s, id);
    assert_eq!(s.artists("Various").unwrap()[0].id, various.id);
}

#[test]
#[ignore = "high-cardinality artist and release qualification"]
fn independent_large_artist_and_release_shelves_page_in_both_directions() {
    use crate::{
        collection::{Shelf, ShelfIndex},
        table::{Field, RowKey},
    };
    use gpui_kit::component::table::ColumnSort;
    const N: usize = 40_960;
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("library.sqlite3")).unwrap();
    let root = store.add_root(dir.path()).unwrap();
    let tx = store.db.transaction().unwrap();
    tx.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?2)
        INSERT INTO files(id,path,path_key,name,kind,root_id,size,modified)
        SELECT i,printf('music/%06d.wav',i),printf('music/%06d.wav',i),'fixture.wav',0,?1,400,1 FROM n",
        rusqlite::params![root, N as i64]).unwrap();
    tx.execute_batch("INSERT INTO artists(id,key,name) SELECT id,printf('artist %06d',id),printf('Artist %06d',id) FROM files;
        INSERT INTO albums(id,key,root_id,title,folder,sort_key,is_release)
            SELECT id,path,root_id,printf('Release %06d',id),'music',path,1 FROM files;
        INSERT INTO tracks(anchor,part,source,root_id,album_id,title,artist,album,format,title_key,artist_key,album_key,search,added_at)
            SELECT f.id,0,f.id,f.root_id,a.id,f.path,p.name,a.title,'WAV',f.path,p.key,lower(a.title),f.path,1
            FROM files f JOIN artists p ON p.id=f.id JOIN albums a ON a.id=f.id;").unwrap();
    tx.commit().unwrap();
    for artists in [true, false] {
        let ticket = Ticket::default();
        let generation = ticket.next();
        let start = std::time::Instant::now();
        let reader = store.reader(ticket.clone(), generation).unwrap();
        let index = ShelfIndex::build(
            &reader,
            artists,
            "",
            Some((Field::Title, ColumnSort::Descending)),
            ticket,
            generation,
        )
        .unwrap();
        let elapsed = start.elapsed();
        assert_eq!((index.snapshot.len, index.count), (N, N));
        let start = std::time::Instant::now();
        for offset in [0, N - 256, 256, 0] {
            let page = index.snapshot.rows::<Shelf>(offset, 256).unwrap();
            assert_eq!(page.len(), 256);
            let id = (N - offset) as i64;
            let key = if artists {
                RowKey::Artist(ArtistId(id))
            } else {
                RowKey::Disc(crate::collection::DiscId::Library(id))
            };
            assert_eq!(page[0].key(), key);
            assert_eq!(index.snapshot.position(&key).unwrap(), Some(offset));
        }
        eprintln!(
            "artists={artists} cardinality={N} index={elapsed:?} four_pages_and_ranks={:?}",
            start.elapsed()
        );
    }
}

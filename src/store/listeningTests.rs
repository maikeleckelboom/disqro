use super::*;
use crate::{
    cache::Stamp,
    library::Track,
    listening::{End, Occurrence, Recording},
    store::{Order, Query},
};
use std::{path::Path, sync::Arc, time::Instant};

const SECOND: i64 = 1_000_000;

#[test]
fn qualification_uses_the_smaller_of_half_the_recording_or_thirty_seconds() {
    for (duration, expected) in [
        (Some(10.), 5),
        (Some(30.), 15),
        (Some(60.), 30),
        (Some(240.), 30),
        (Some(4500.), 30),
        (Some(0.), 30),
        (Some(f64::NAN), 30),
        (None, 30),
    ] {
        assert_eq!(crate::listening::qualification(duration), expected * SECOND);
    }
}

#[test]
#[ignore = "requires the Amsterdam system time zone for native DST qualification"]
fn local_daylight_saving_boundaries() {
    let spring = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
    let autumn = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
    for (day, length) in [(spring, 23), (autumn, 25)] {
        let next = day.succ_opt().unwrap();
        let boundary = midnight(&chrono::Local, next);
        assert_eq!(
            boundary - midnight(&chrono::Local, day),
            length * 3600 * SECOND
        );
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::memory();
        let at = boundary - 10 * SECOND;
        let mut occurrence = Occurrence::new(recording(&dir.path().join("transition.wav"), ""), at);
        credit(&mut s, &mut occurrence, Some(30.), at, 20);
        let stats = s
            .statistics_in(&chrono::Local, next, Period::Week, true)
            .unwrap();
        assert_eq!(&stats.days[5..], &[(day, 10 * SECOND), (next, 10 * SECOND)]);
        assert_eq!(stats.totals.plays, 1);
    }
}

fn recording(path: &Path, artist: &str) -> Recording {
    std::fs::write(path, b"fixture recording bytes").unwrap();
    let mut track = Track::file(path.into());
    track.artist = artist.into();
    Recording {
        track: Arc::new(track),
        source: Stamp::read(path),
        sheet: None,
    }
}

fn credit(s: &mut Store, o: &mut Occurrence, duration: Option<f64>, at: i64, seconds: i64) {
    if let Some(c) = o.credit(duration, at, seconds * SECOND) {
        assert!(s.checkpoint_listening(&c).unwrap().is_some());
        assert!(s.checkpoint_listening(&c).unwrap().is_none());
    }
}

fn recent(s: &Store) -> Vec<i64> {
    s.keys(&Query {
        scope: Scope::RecentlyPlayed,
        search: String::new(),
        order: None,
    })
    .unwrap()
}

#[test]
fn listening_qualification_and_removal_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let audio = recording(&dir.path().join("study.wav"), "Ensemble");
    let start = 1700000000 * SECOND;
    let mut s = Store::memory();
    let mut first = Occurrence::new(audio.clone(), start);
    credit(&mut s, &mut first, Some(240.), start, 10);
    assert!(recent(&s).is_empty());
    // A paused or stalled hour contributes nothing. This is a fresh span, not
    // the difference between source positions before and after a seek.
    credit(&mut s, &mut first, Some(240.), start + 3600 * SECOND, 25);
    let id = recent(&s)[0];
    assert_eq!(
        s.listening_totals(id).unwrap(),
        Totals {
            plays: 1,
            last_played: Some(start + 3620 * SECOND),
            credited_us: 35 * SECOND,
        }
    );
    credit(&mut s, &mut first, Some(9.), start + 4000 * SECOND, 5);
    let old = s
        .history_page((i64::MIN, i64::MAX), "ensemble", false, None)
        .unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].credited_us, 40 * SECOND);
    assert!(old[0].qualified);
    assert!(old[0].track.is_some());
    let last = first.finish(start + 4020 * SECOND, End::Completed);
    s.erase_listening(Some(old[0].id)).unwrap();
    assert!(s.checkpoint_listening(&last).unwrap().is_none());
    assert!(recent(&s).is_empty());
    assert_eq!(s.listening_totals(id).unwrap(), Totals::default());
}

#[test]
fn listen_splits_credit_at_local_midnight_and_counts_in_qualification_period() {
    let dir = tempfile::tempdir().unwrap();
    let audio = recording(&dir.path().join("mix.wav"), "Artist");
    let today = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
    let zone = chrono::FixedOffset::east_opt(5 * 3600 + 45 * 60).unwrap();
    let edge = midnight(&zone, today - Days::new(6));
    let at = edge - 20 * SECOND;
    let mut s = Store::memory();
    let mut one = Occurrence::new(audio.clone(), at);
    credit(&mut s, &mut one, None, at, 35);
    let mut two = Occurrence::new(audio, edge + 20 * SECOND);
    credit(&mut s, &mut two, Some(10.), edge + 20 * SECOND, 6);

    let stats = s.statistics_in(&zone, today, Period::Week, true).unwrap();
    assert_eq!(stats.totals.credited_us, 21 * SECOND);
    assert_eq!(stats.totals.plays, 2);
    assert_eq!(stats.days.len(), 7);
    assert_eq!(stats.days[0].1, 21 * SECOND);
    assert_eq!((stats.recordings, stats.artists, stats.releases), (1, 1, 0));
    assert_eq!(
        (stats.tracks[0].plays, stats.performers[0].credited_us),
        (2, 21 * SECOND)
    );
    let all = s.statistics_in(&zone, today, Period::All, false).unwrap();
    assert_eq!(all.totals.credited_us, 41 * SECOND);
    assert_eq!(all.days[0].1, 20 * SECOND);
    assert_eq!(all.days[1].1, 21 * SECOND);
    assert_eq!(recent(&s).len(), 1);
    let history = s.history_page((edge, i64::MAX), "mix", true, None).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].started_at, edge + 20 * SECOND);
}

#[test]
fn recovered_occurrences_keep_the_last_committed_checkpoint_and_catalog_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let audio = recording(&dir.path().join("disc.wav"), "");
    let mut s = Store::open(&path).unwrap();
    let start = 1700000000 * SECOND;
    let mut occurrence = Occurrence::new(audio.clone(), start);
    credit(&mut s, &mut occurrence, Some(10.), start, 6);
    let id = recent(&s)[0];
    s.set_liked(&[id], true).unwrap();
    let playlist = s.create_playlist("Keep", &[id]).unwrap();
    assert!(
        occurrence
            .credit(Some(10.), start + 6 * SECOND, SECOND)
            .is_none()
    );
    drop(s);
    drop(occurrence);
    let mut s = Store::open(&path).unwrap();
    s.recover_listening().unwrap();
    s.recover_listening().unwrap();
    assert_eq!(
        s.db.query_row(
            "SELECT reason,ended_at,credited_us FROM listening_occurrences",
            [],
            |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?
            )),
        )
        .unwrap(),
        ("interrupted".into(), start + 6 * SECOND, 6 * SECOND)
    );
    assert_eq!(s.listening_totals(id).unwrap().plays, 1);
    std::fs::remove_file(&audio.track.source).unwrap();
    let page = s
        .history_page((i64::MIN, i64::MAX), "", false, None)
        .unwrap();
    assert_eq!(page[0].title, "disc");
    assert!(page[0].track.is_none());
    s.erase_listening(None).unwrap();
    assert!(recent(&s).is_empty());
    assert!(s.marks_for(&[id]).unwrap()[0].2.liked);
    assert_eq!(s.entries(playlist).unwrap()[0].1, id);
}

#[test]
fn concurrent_catalog_and_history_startup_upgrade_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let mut db = rusqlite::Connection::open(&path).unwrap();
    let old = &super::super::schema::MIGRATIONS[..3];
    super::super::schema::migrate(&mut db, Some(&path), old).unwrap();
    db.execute("INSERT INTO playlists VALUES (1,'Preserved',0,0)", [])
        .unwrap();
    drop(db);
    let start = Arc::new(std::sync::Barrier::new(3));
    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                start.wait();
                let store = Store::open(&path).unwrap();
                assert_eq!(super::super::schema::version(&store.db).unwrap(), 4);
                assert_eq!(store.playlists().unwrap()[0].name, "Preserved");
            });
        }
    });
    assert!(dir.path().join("library-schema3-backup.sqlite3").is_file());
    assert!(
        !std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("library-upgrade-"))
    );
}

#[test]
fn listening_writer_recovers_from_delayed_sqlite_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let mut reader = Store::open(&path).unwrap();
    let (writer, updates) = crate::listening::Writer::spawn(path);
    writer.barrier().unwrap();
    let lock = reader
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let mut occurrence = Occurrence::new(recording(&dir.path().join("held.wav"), ""), 0);
    writer.save(occurrence.credit(None, 0, SECOND).unwrap());
    writer.save(occurrence.finish(SECOND, End::Stopped));
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(updates.changed.is_empty());
    drop(lock);
    writer.barrier().unwrap();
    assert_eq!(
        reader
            .history_page((i64::MIN, i64::MAX), "", false, None)
            .unwrap()[0]
            .credited_us,
        SECOND
    );
}

#[test]
fn listening_writer_retains_ordered_checkpoints_after_a_write_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let reader = Store::open(&path).unwrap();
    let (writer, _) = crate::listening::Writer::spawn(path);
    writer.barrier().unwrap();
    reader
        .db
        .execute_batch(
            "CREATE TRIGGER fail_checkpoint BEFORE INSERT ON listening_occurrences
         BEGIN SELECT RAISE(ABORT, 'qualification write error'); END;",
        )
        .unwrap();
    let mut occurrence = Occurrence::new(recording(&dir.path().join("retry.wav"), ""), 0);
    writer.save(occurrence.credit(Some(2.), 0, SECOND).unwrap());
    writer.save(occurrence.finish(SECOND, End::Stopped));
    assert!(
        writer
            .barrier()
            .unwrap_err()
            .to_string()
            .contains("qualification write error")
    );
    reader
        .db
        .execute_batch("DROP TRIGGER fail_checkpoint")
        .unwrap();
    writer.barrier().unwrap();
    let page = reader
        .history_page((i64::MIN, i64::MAX), "", false, None)
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].credited_us, SECOND);
    assert!(page[0].qualified);
}

#[test]
#[ignore = "one million occurrence qualification dataset"]
fn listening_scale() {
    let export = std::env::var_os("DISQRO_LISTENING_SCALE_DIR").map(std::path::PathBuf::from);
    let dir = match &export {
        Some(directory) => tempfile::Builder::new()
            .prefix("listening-scale-")
            .tempdir_in(directory),
        None => tempfile::tempdir(),
    }
    .unwrap();
    let path = dir.path().join("library.sqlite3");
    let mut s = Store::open(&path).unwrap();
    let start = Instant::now();
    let today = chrono::Local::now().date_naive();
    let base = midnight(&chrono::Local, today - Days::new(1824));
    let tx = s.db.transaction().unwrap();
    tx.execute_batch(
        "INSERT INTO roots VALUES(1,'music','music','Fixture',0,0,0);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<100000)
         INSERT INTO files(id,path,path_key,name,kind,root_id,size,modified)
         SELECT i,'music/'||i,'music/'||i,'File '||i,0,1,1,0 FROM n;
         INSERT INTO tracks(id,anchor,part,source,root_id,title,artist,album,format,title_key,artist_key,album_key,search,added_at)
         SELECT id,id,0,id,1,name,'Artist '||(id%3000),'Album '||(id%6000),'WAV',name,'','','',0 FROM files;",
    ).unwrap();
    tx.execute(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i<999999)
         INSERT INTO listening_occurrences(token,track_id,title,artist,artist_key,album,release_key,snapshot,
             started_at,activity_at,ended_at,reason,credited_us,qualified_at,checkpoint)
         SELECT 'fixture'||i,t.id,t.title,t.artist,t.artist,t.album,'release'||(t.id%6000),'{}',
             ?1+(i*113%1825)*86400000000+(i%1440)*60000000,
             ?1+(i*113%1825)*86400000000+(i%1440)*60000000+90000000,
             ?1+(i*113%1825)*86400000000+(i%1440)*60000000+90000000,
             'completed',90000000,?1+(i*113%1825)*86400000000+(i%1440)*60000000+30000000,1
         FROM n JOIN tracks t ON t.id=(i/10)+1",
        [base],
    ).unwrap();
    tx.execute_batch(
        "INSERT INTO listening_minutes SELECT id,started_at,60000000 FROM listening_occurrences;
         INSERT INTO listening_minutes SELECT id,started_at+60000000,30000000 FROM listening_occurrences;",
    ).unwrap();
    tx.commit().unwrap();
    s.db.execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    println!(
        "fixture {:.2}s, database {:.1} MiB",
        start.elapsed().as_secs_f64(),
        path.metadata().unwrap().len() as f64 / 1048576.
    );

    let elapsed = |start: Instant| start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    assert_eq!(recent(&s).len(), 100_000);
    println!("Recently Played 100k keys {:.1} ms", elapsed(start));
    let start = Instant::now();
    assert_eq!(
        s.keys(&Query {
            scope: Scope::Library(None),
            search: String::new(),
            order: Some((Order::Plays, true))
        })
        .unwrap()
        .len(),
        100_000
    );
    println!("Sort 100k tracks by plays {:.1} ms", elapsed(start));
    let start = Instant::now();
    let page = s
        .history_page((i64::MIN, i64::MAX), "", false, None)
        .unwrap();
    assert_eq!(page.len(), HISTORY_PAGE + 1);
    println!("First page including availability {:.1} ms", elapsed(start));
    let middle: HistoryKey = s.db.query_row("SELECT started_at,id FROM listening_occurrences ORDER BY started_at DESC,id DESC LIMIT 1 OFFSET 500000", [], |r| Ok(HistoryKey(r.get(0)?, r.get(1)?))).unwrap();
    let start = Instant::now();
    let page = s
        .history_page((i64::MIN, i64::MAX), "", false, Some(middle))
        .unwrap();
    assert_eq!(page.len(), HISTORY_PAGE + 1);
    println!("Cursor page at 500k {:.1} ms", elapsed(start));

    for period in [Period::Week, Period::Month, Period::All] {
        let start = Instant::now();
        let stats = s
            .statistics_in(&chrono::Local, today, period, false)
            .unwrap();
        if period == Period::All {
            assert_eq!(stats.totals.plays, 1_000_000);
            assert_eq!(stats.totals.credited_us, 90_000_000_000_000);
            assert_eq!(
                (stats.recordings, stats.artists, stats.releases),
                (100_000, 3000, 6000)
            );
        }
        println!(
            "{period:?} statistics {:.1} ms, {} plays",
            elapsed(start),
            stats.totals.plays
        );
    }
    let start = Instant::now();
    let mut occurrence = Occurrence::new(recording(&dir.path().join("new.wav"), ""), base);
    credit(&mut s, &mut occurrence, None, base, 31);
    println!(
        "New unindexed recording and qualifying checkpoint {:.1} ms",
        elapsed(start)
    );
    let start = Instant::now();
    credit(&mut s, &mut occurrence, None, base + 31 * SECOND, 4);
    println!("Incremental checkpoint {:.1} ms", elapsed(start));
    if export.is_some() {
        s.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(s);
        println!("Fixture for native qualification {}", dir.keep().display());
    }
}

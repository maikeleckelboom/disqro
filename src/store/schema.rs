//! Versioned schema of the library database.
//!
//! Installation and production upgrades apply the final listening schema in
//! one transaction. Existing production libraries are backed up first.
use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use std::path::Path;

pub const VERSION: usize = 5;
const PRODUCTION_MIGRATIONS: &[(usize, &str)] = &[
    (1, V1),
    (2, include_str!("recording-identity.sql")),
    (3, include_str!("catalog-search.sql")),
];
const LISTENING_SCHEMA: &str = include_str!("listening-history.sql");

const V1: &str = "
-- Registered music collections. Removing one never touches files on disk.
CREATE TABLE roots(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    path_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    added_at INTEGER NOT NULL,
    scanned_at INTEGER
);

-- Physical files: audio sources (kind 0) and CUE sheets (kind 1). A row
-- keeps its last known location while the file is missing. `seen` is the
-- root generation that last found it; `tagged` is the modification time
-- whose tags the tracks hold. Identity evidence (volume, file_index, digest)
-- supports verified relocation.
CREATE TABLE files(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    path_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    kind INTEGER NOT NULL,
    root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    volume INTEGER,
    file_index INTEGER,
    digest INTEGER,
    tagged INTEGER,
    present INTEGER NOT NULL DEFAULT 1,
    seen INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX files_root ON files(root_id, present);
CREATE INDEX files_missing ON files(kind, size) WHERE present = 0;

-- Discs and albums: an authored CUE program, or tagged files sharing a
-- folder, album and disc number. Rebuilt by scans.
CREATE TABLE albums(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE,
    root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    year INTEGER,
    folder TEXT NOT NULL,
    cover TEXT,
    sort_key TEXT NOT NULL
);
CREATE INDEX albums_root ON albums(root_id);

-- Logical tracks. Identity is (anchor, part): the audio file and part 0 for
-- a whole file; the CUE sheet and the chapter number, or its negation for
-- the recorded lead-in before that chapter. Metadata columns are refreshed
-- by every scan; the row itself outlives its file.
CREATE TABLE tracks(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    anchor INTEGER NOT NULL REFERENCES files(id),
    part INTEGER NOT NULL,
    source INTEGER NOT NULL REFERENCES files(id),
    first_tick INTEGER,
    last_tick INTEGER,
    root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
    album_id INTEGER REFERENCES albums(id) ON DELETE SET NULL,
    ordinal INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT NOT NULL,
    number INTEGER,
    disc_number INTEGER,
    year INTEGER,
    duration REAL,
    format TEXT NOT NULL,
    title_key TEXT NOT NULL,
    artist_key TEXT NOT NULL,
    album_key TEXT NOT NULL,
    search TEXT NOT NULL,
    present INTEGER NOT NULL DEFAULT 1,
    seen INTEGER NOT NULL DEFAULT 0,
    added_at INTEGER NOT NULL,
    UNIQUE(anchor, part)
);
CREATE INDEX tracks_root ON tracks(root_id, present);
CREATE INDEX tracks_album ON tracks(album_id, ordinal);
CREATE INDEX tracks_source ON tracks(source);
CREATE INDEX tracks_title ON tracks(title_key);
CREATE INDEX tracks_artist ON tracks(artist_key);

-- User-owned data. RESTRICT keeps every referenced track alive through
-- rescans, collection removal and cleanup.
CREATE TABLE likes(
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE RESTRICT,
    liked_at INTEGER NOT NULL
);
CREATE TABLE ratings(
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE RESTRICT,
    stars INTEGER NOT NULL CHECK (stars BETWEEN 1 AND 5),
    rated_at INTEGER NOT NULL
);
CREATE TABLE playlists(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    modified_at INTEGER NOT NULL
);
CREATE TABLE playlist_entries(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL,
    added_at INTEGER NOT NULL
);
CREATE INDEX playlist_order ON playlist_entries(playlist_id, position);
CREATE INDEX playlist_tracks ON playlist_entries(track_id);
";

pub fn version(db: &Connection) -> Result<usize> {
    Ok(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize)
}

/// The caller serializes on-disk initialization across connections and processes.
pub fn initialize(db: &mut Connection, path: Option<&Path>) -> Result<()> {
    let current = version(db)?;
    ensure!(
        current <= VERSION,
        "This library was written by a newer version of Disqro (schema {current})."
    );
    if current == VERSION {
        return Ok(());
    }
    ensure!(
        current != 4,
        "This library uses an obsolete development schema (4). Preserve the database and use a new profile or explicitly reset the development database."
    );
    migrate(
        db,
        path,
        current,
        PRODUCTION_MIGRATIONS,
        Some(LISTENING_SCHEMA),
    )
}

fn migrate(
    db: &mut Connection,
    path: Option<&Path>,
    current: usize,
    production: &[(usize, &str)],
    listening: Option<&str>,
) -> Result<()> {
    if current > 0
        && let Some(path) = path
    {
        let backup = path.with_file_name(format!(
            "{}-schema{current}-backup.sqlite3",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ));
        let temp = tempfile::Builder::new()
            .prefix("library-upgrade-")
            .suffix(".sqlite3")
            .tempfile_in(backup.parent().unwrap_or(Path::new(".")))?
            .into_temp_path();
        db.execute("VACUUM INTO ?1", [temp.to_string_lossy()])
            .context("Could not back up the library before upgrading it")?;
        match temp.persist_noclobber(backup) {
            Ok(()) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                e.path.keep()?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    // SQLite requires foreign key enforcement to be disabled on this
    // connection while replacing parent tables. Check every relationship
    // before committing and restore enforcement even after a rollback.
    db.pragma_update(None, "foreign_keys", false)?;
    let result = (|| -> Result<()> {
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (version, sql) in production
            .iter()
            .copied()
            .filter(|(v, _)| *v > current)
            .chain(listening.map(|sql| (VERSION, sql)))
        {
            tx.execute_batch(sql)
                .with_context(|| format!("Library migration {version} failed"))?;
            tx.pragma_update(None, "user_version", version as i64)?;
        }
        ensure!(
            !tx.prepare("PRAGMA foreign_key_check")?.exists([])?,
            "Library migration would break a personal or library reference"
        );
        tx.commit()?;
        Ok(())
    })();
    db.pragma_update(None, "foreign_keys", true)?;
    result
}

#[cfg(test)]
pub fn initialize_production(db: &mut Connection) -> Result<()> {
    migrate(db, None, 0, PRODUCTION_MIGRATIONS, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db
    }

    #[test]
    fn upgrade_keeps_user_data_backs_up_and_rolls_back_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite3");
        let mut db = open(&path);
        migrate(&mut db, None, 0, &[(1, V1)], None).unwrap();
        db.execute_batch(
            "INSERT INTO playlists VALUES(1, 'Keep', 0, 0);
             INSERT INTO files(id, path, path_key, name, kind, size, modified) VALUES(1, 'a', 'a', 'a', 0, 1, 1);
             INSERT INTO tracks(id, anchor, part, source, title, artist, album, format, title_key, artist_key, album_key, search, added_at)
                 VALUES(1, 1, 0, 1, 't', '', '', 'FLAC', 't', '', '', 't', 0);
             INSERT INTO likes VALUES(1, 5);",
        )
        .unwrap();

        // A later migration that fails leaves the user's schema and rows as they were.
        let broken = "ALTER TABLE likes ADD COLUMN note TEXT; SELECT * FROM missing;";
        assert!(migrate(&mut db, Some(&path), 1, PRODUCTION_MIGRATIONS, Some(broken)).is_err());
        assert_eq!(version(&db).unwrap(), 1);
        assert!(db.prepare("SELECT note FROM likes").is_err());

        initialize(&mut db, Some(&path)).unwrap();
        assert_eq!(version(&db).unwrap(), VERSION);
        let liked: i64 = db
            .query_row("SELECT liked_at FROM likes WHERE track_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(liked, 5);
        let backup = open(&dir.path().join("library-schema1-backup.sqlite3"));
        assert_eq!(version(&backup).unwrap(), 1);
    }

    #[test]
    fn obsolete_development_and_newer_libraries_are_preserved() {
        for version_number in [4, VERSION + 1] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("library.sqlite3");
            let mut db = open(&path);
            initialize_production(&mut db).unwrap();
            db.execute("INSERT INTO playlists VALUES (1,'Preserved',0,0)", [])
                .unwrap();
            db.pragma_update(None, "user_version", version_number as i64)
                .unwrap();
            let error = initialize(&mut db, Some(&path)).unwrap_err();
            let message = if version_number == 4 {
                "obsolete development schema"
            } else {
                "newer version"
            };
            assert!(error.to_string().contains(message));
            assert_eq!(version(&db).unwrap(), version_number);
            assert_eq!(
                db.query_row("SELECT name FROM playlists", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "Preserved"
            );
        }
    }

    #[test]
    fn recording_identity_upgrade_preserves_ids_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite3");
        let mut db = open(&path);
        migrate(&mut db, None, 0, &[(1, V1)], None).unwrap();
        db.execute_batch(
            "INSERT INTO files(id,path,path_key,name,kind,size,modified) VALUES(3,'a','a','a',0,1,1);
             INSERT INTO tracks(id,anchor,part,source,title,artist,album,format,title_key,artist_key,album_key,search,added_at)
                 VALUES(8,3,0,3,'t','','','FLAC','t','','','t',0);
             INSERT INTO likes VALUES(8,17);
             INSERT INTO ratings VALUES(8,4,17);
             INSERT INTO playlists VALUES(2,'Saved',0,0);
             INSERT INTO playlist_entries VALUES(9,2,8,0,0),(11,2,8,1,0);
             UPDATE sqlite_sequence SET seq=20 WHERE name='tracks';"
        ).unwrap();
        initialize(&mut db, Some(&path)).unwrap();
        assert_eq!(version(&db).unwrap(), VERSION);
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM playlist_entries e JOIN ratings r USING(track_id)
             JOIN likes l USING(track_id) JOIN tracks t ON t.id=e.track_id
             WHERE e.id IN (9,11) AND t.id=8 AND t.source=3 AND e.playlist_id=2 AND r.stars=4",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            db.query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='tracks'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            20
        );
        assert!(db.execute("DELETE FROM tracks WHERE id=8", []).is_err());
        assert_eq!(
            version(&open(&dir.path().join("library-schema1-backup.sqlite3"))).unwrap(),
            1
        );
    }

    #[test]
    fn rows_with_user_data_cannot_be_deleted() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        initialize(&mut db, None).unwrap();
        db.execute_batch(
            "INSERT INTO files(id, path, path_key, name, kind, size, modified) VALUES(1, 'a', 'a', 'a', 0, 1, 1);
             INSERT INTO tracks(id, anchor, part, source, title, artist, album, format, title_key, artist_key, album_key, search, added_at)
                 VALUES(1, 1, 0, 1, 't', '', '', 'FLAC', 't', '', '', 't', 0);
             INSERT INTO playlists VALUES(1, 'Mix', 0, 0);
             INSERT INTO playlist_entries VALUES(1, 1, 1, 0, 0);",
        )
        .unwrap();
        assert!(db.execute("DELETE FROM tracks WHERE id = 1", []).is_err());
        assert!(db.execute("DELETE FROM files WHERE id = 1", []).is_err());
    }
}

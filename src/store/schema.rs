//! Versioned schema of the library database.
//!
//! Each migration runs in its own transaction together with the version
//! bump, so a failure leaves the previous version intact. Before an existing
//! library is upgraded, a copy is written next to it. A library created by a
//! newer Disqro is never opened for writing.
use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use std::path::Path;

/// Migrations in order. The database's `user_version` counts those applied.
pub const MIGRATIONS: &[&str] = &[V1];

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

/// Brings the database at `path` (or in memory) up to date.
pub fn migrate(db: &mut Connection, path: Option<&Path>, migrations: &[&str]) -> Result<()> {
    let current = version(db)?;
    ensure!(
        current <= migrations.len(),
        "This library was written by a newer version of Disqro (schema {current})."
    );
    if current == migrations.len() {
        return Ok(());
    }
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
    for (i, sql) in migrations.iter().enumerate().skip(current) {
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version = version(&tx)?;
        ensure!(
            version <= migrations.len(),
            "This library requires a newer Disqro"
        );
        if version > i {
            continue;
        }
        tx.execute_batch(sql)
            .with_context(|| format!("Library migration {} failed", i + 1))?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
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
        migrate(&mut db, Some(&path), MIGRATIONS).unwrap();
        db.execute_batch(
            "INSERT INTO playlists VALUES(1, 'Keep', 0, 0);
             INSERT INTO files(id, path, path_key, name, kind, size, modified) VALUES(1, 'a', 'a', 'a', 0, 1, 1);
             INSERT INTO tracks(id, anchor, part, source, title, artist, album, format, title_key, artist_key, album_key, search, added_at)
                 VALUES(1, 1, 0, 1, 't', '', '', 'FLAC', 't', '', '', 't', 0);
             INSERT INTO likes VALUES(1, 5);",
        )
        .unwrap();

        // A later migration that fails leaves the user's schema and rows as they were.
        let broken = [
            V1,
            "ALTER TABLE likes ADD COLUMN note TEXT; SELECT * FROM missing;",
        ];
        assert!(migrate(&mut db, Some(&path), &broken).is_err());
        assert_eq!(version(&db).unwrap(), 1);
        assert!(db.prepare("SELECT note FROM likes").is_err());

        let next = [V1, "ALTER TABLE likes ADD COLUMN note TEXT"];
        migrate(&mut db, Some(&path), &next).unwrap();
        assert_eq!(version(&db).unwrap(), 2);
        let liked: i64 = db
            .query_row("SELECT liked_at FROM likes WHERE track_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(liked, 5);
        let backup = open(&dir.path().join("library-schema1-backup.sqlite3"));
        assert_eq!(version(&backup).unwrap(), 1);

        // A newer library is refused instead of being rewritten.
        drop(db);
        let mut db = open(&path);
        assert!(migrate(&mut db, Some(&path), MIGRATIONS).is_err());
    }

    #[test]
    fn rows_with_user_data_cannot_be_deleted() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        migrate(&mut db, None, MIGRATIONS).unwrap();
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

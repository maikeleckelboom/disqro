//! The current library schema. Existing libraries require an explicit conversion.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, TransactionBehavior};

pub const VERSION: usize = 6;
const SCHEMA: &str = concat!(
    include_str!("library.sql"),
    include_str!("catalog-search.sql"),
    include_str!("listening-history.sql"),
);

pub fn version(db: &Connection) -> Result<usize> {
    Ok(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize)
}

pub fn initialize(db: &mut Connection) -> Result<()> {
    let check = |db: &Connection| {
        let current = version(db)?;
        ensure!(
            current == 0 || current == VERSION,
            "Library schema {current} cannot be opened by this build (schema {VERSION}). Preserve the existing profile and use a converted copy or a separate profile."
        );
        Ok(current)
    };
    if check(db)? == VERSION {
        return Ok(());
    }
    // Catalog and listening connections can open a new profile concurrently.
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if check(&tx)? == 0 {
        ensure!(
            !tx.prepare("SELECT 1 FROM sqlite_schema LIMIT 1")?
                .exists([])?,
            "The unversioned library is not empty. Preserve the existing profile."
        );
        tx.execute_batch(SCHEMA)
            .context("Create the library schema")?;
        tx.pragma_update(None, "user_version", VERSION as i64)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn refuses_other_schemas_without_changing_the_database() {
        for schema in [0, 3, 5, VERSION + 1] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("library.sqlite3");
            let db = Connection::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE playlists(name TEXT); INSERT INTO playlists VALUES ('Preserved');",
            )
            .unwrap();
            db.pragma_update(None, "user_version", schema as i64)
                .unwrap();
            drop(db);
            let original = std::fs::read(&path).unwrap();
            assert!(Store::open(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn rows_with_user_data_cannot_be_deleted() {
        let store = Store::memory();
        let db = &store.db;
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

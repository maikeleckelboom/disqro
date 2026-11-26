//! Disk-backed ranks for large browser result sets. A writer belongs to one
//! request. Readers use immutable, indexed ranges after publication.
use crate::store::Ticket;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use std::sync::Arc;
use tempfile::TempPath;

pub(crate) struct Snapshot {
    file: TempPath,
    pub(crate) len: usize,
}

pub(crate) struct Writer {
    db: Connection,
    file: TempPath,
}

impl Writer {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let file = tempfile::NamedTempFile::new()?.into_temp_path();
        let db = Connection::open(&file)?;
        db.execute_batch(
            "PRAGMA journal_mode=OFF;
            PRAGMA temp_store=FILE;
            PRAGMA cache_size=-2048;
            CREATE TABLE items(sequence INTEGER PRIMARY KEY, identity BLOB UNIQUE NOT NULL,
                number INTEGER NOT NULL, label TEXT NOT NULL, family INTEGER NOT NULL, member INTEGER NOT NULL, payload BLOB NOT NULL);
            CREATE TABLE positions(rank INTEGER PRIMARY KEY, item INTEGER UNIQUE NOT NULL);",
        )?;
        db.execute_batch("BEGIN")?;
        Ok(Self { db, file })
    }

    pub(crate) fn append<T: Serialize, K: Serialize>(
        &self,
        key: &K,
        value: &T,
        order: (u64, String),
        family: usize,
        member: usize,
    ) -> anyhow::Result<()> {
        self.db
            .prepare_cached(
                "INSERT INTO items(identity,number,label,family,member,payload)
            VALUES(?,?,?,?,?,?) ON CONFLICT(identity) DO NOTHING",
            )?
            .execute(params![
                serde_json::to_vec(key)?,
                order.0 as i64,
                order.1,
                i64::try_from(family)?,
                i64::try_from(member)?,
                serde_json::to_vec(value)?
            ])?;
        Ok(())
    }

    pub(crate) fn finish(
        self,
        descending: bool,
        ticket: Ticket,
        generation: u64,
    ) -> anyhow::Result<Arc<Snapshot>> {
        self.db
            .progress_handler(2048, Some(move || !ticket.is_current(generation)))?;
        let order = if descending {
            "number DESC, label DESC, family DESC, member, sequence"
        } else {
            "number, label, family, member, sequence"
        };
        let len = self.db.execute(
            &format!("INSERT INTO positions(item) SELECT sequence FROM items ORDER BY {order}"),
            [],
        )?;
        self.db.execute_batch("COMMIT")?;
        Ok(Arc::new(Snapshot {
            file: self.file,
            len,
        }))
    }
}

impl Snapshot {
    fn open(&self) -> anyhow::Result<Connection> {
        Ok(Connection::open_with_flags(
            &self.file,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    }

    pub(crate) fn position(&self, id: &impl Serialize) -> anyhow::Result<Option<usize>> {
        Ok(self
            .open()?
            .query_row(
                "SELECT rank - 1 FROM positions JOIN items ON sequence=item WHERE identity=?",
                [serde_json::to_vec(id)?],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(usize::try_from)
            .transpose()?)
    }

    pub(crate) fn rows<T: serde::de::DeserializeOwned>(
        &self,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<T>> {
        let db = self.open()?;
        let mut query = db.prepare(
            "SELECT payload FROM positions JOIN items ON item=sequence
            WHERE rank > ? ORDER BY rank LIMIT ?",
        )?;
        let mut rows = query.query(params![i64::try_from(offset)?, i64::try_from(limit)?])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(serde_json::from_slice(row.get_ref(0)?.as_blob()?)?);
        }
        Ok(nodes)
    }
}

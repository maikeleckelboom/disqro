//! A disposable ordering index for mixed folder, program and recording results.
//! Input pages are consumed once. Both the ordered range and identity lookup
//! use SQLite indexes; only the requested payload page comes back to the UI.
use super::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use tempfile::TempPath;

pub(super) struct Snapshot {
    file: TempPath,
    len: usize,
}

pub(super) struct Writer {
    db: Connection,
    file: TempPath,
}

impl Writer {
    pub(super) fn new() -> anyhow::Result<Self> {
        let file = tempfile::NamedTempFile::new()?.into_temp_path();
        let db = Connection::open(&file)?;
        db.execute_batch(
            "PRAGMA journal_mode=OFF;
            PRAGMA temp_store=FILE;
            PRAGMA cache_size=-2048;
            CREATE TABLE items(sequence INTEGER PRIMARY KEY, identity BLOB UNIQUE NOT NULL,
                number INTEGER NOT NULL, label TEXT NOT NULL, payload BLOB NOT NULL);
            CREATE TABLE positions(rank INTEGER PRIMARY KEY, item INTEGER UNIQUE NOT NULL);",
        )?;
        Ok(Self { db, file })
    }

    pub(super) fn append(&mut self, page: Branch, sort: (Field, ColumnSort)) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO items(identity,number,label,payload)
                VALUES(?,?,?,?) ON CONFLICT(identity) DO NOTHING",
            )?;
            for node in page.nodes() {
                let (number, label) = order_value(node, sort.0);
                insert.execute(params![
                    serde_json::to_vec(&node.id)?,
                    number as i64,
                    label,
                    serde_json::to_vec(node)?
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn finish(
        self,
        direction: ColumnSort,
        ticket: Ticket,
        generation: u64,
    ) -> anyhow::Result<Arc<Snapshot>> {
        self.db
            .progress_handler(2048, Some(move || !ticket.is_current(generation)))?;
        let order = if direction == ColumnSort::Descending {
            "number DESC, label DESC, sequence"
        } else {
            "number, label, sequence"
        };
        let len = self.db.execute(
            &format!("INSERT INTO positions(item) SELECT sequence FROM items ORDER BY {order}"),
            [],
        )?;
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

    pub(super) fn position(&self, id: &TreeId) -> anyhow::Result<Option<usize>> {
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

    pub(super) fn page(self: &Arc<Self>, offset: usize) -> anyhow::Result<Branch> {
        let db = self.open()?;
        let mut query = db.prepare(
            "SELECT payload FROM positions JOIN items ON item=sequence
            WHERE rank > ? ORDER BY rank LIMIT ?",
        )?;
        let mut rows = query.query(params![i64::try_from(offset)?, PAGE as i64])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            let mut node: Node = serde_json::from_slice(row.get_ref(0)?.as_blob()?)?;
            // Persistent queues omit database-local IDs. This temporary result
            // has the authoritative recording identity alongside the payload.
            if let (TreeId::IndexedTrack(id), Item::Track { track, .. }) =
                (&node.id, &mut node.item)
            {
                Arc::make_mut(track).library_id = Some(*id);
            }
            nodes.push(node);
        }
        let end = offset + nodes.len();
        let mut page = Branch::page(nodes, (end < self.len).then_some(Cursor::Offset(end)));
        page.snapshot = Some(self.clone());
        Ok(page)
    }
}

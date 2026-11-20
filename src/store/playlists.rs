//! Ordered personal playlists. Each occurrence has its own entry ID.
use super::{EntryId, Playlist, PlaylistId, Store, TrackId, now};
use anyhow::{Result, ensure};
use rusqlite::{Transaction, params};

impl Store {
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut stmt = self.db.prepare_cached(
            "SELECT p.id, p.name, (SELECT count(*) FROM playlist_entries e WHERE e.playlist_id = p.id)
             FROM playlists p ORDER BY p.name COLLATE NOCASE, p.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Playlist {
                id: r.get(0)?,
                name: r.get(1)?,
                entries: r.get::<_, i64>(2)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn create_playlist(&mut self, name: &str, tracks: &[TrackId]) -> Result<PlaylistId> {
        let name = clean_name(name)?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let at = now();
        tx.execute(
            "INSERT INTO playlists(name, created_at, modified_at) VALUES(?1, ?2, ?2)",
            params![name, at],
        )?;
        let id = tx.last_insert_rowid();
        insert_entries(&tx, id, tracks, 0)?;
        tx.commit()?;
        Ok(id)
    }
    pub fn rename_playlist(&mut self, id: PlaylistId, name: &str) -> Result<()> {
        let changed = self.db.execute(
            "UPDATE playlists SET name = ?2, modified_at = ?3 WHERE id = ?1",
            params![id, clean_name(name)?, now()],
        )?;
        ensure!(changed == 1, "The playlist no longer exists");
        Ok(())
    }
    pub fn delete_playlist(&mut self, id: PlaylistId) -> Result<()> {
        self.db
            .execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        Ok(())
    }
    pub fn duplicate_playlist(&mut self, id: PlaylistId, name: &str) -> Result<PlaylistId> {
        let tracks: Vec<TrackId> = self
            .entries(id)?
            .into_iter()
            .map(|(_, track)| track)
            .collect();
        self.create_playlist(name, &tracks)
    }
    /// Entry ids and their tracks, in playlist order.
    pub fn entries(&self, id: PlaylistId) -> Result<Vec<(EntryId, TrackId)>> {
        let mut stmt = self.db.prepare_cached(
            "SELECT id, track_id FROM playlist_entries WHERE playlist_id = ?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    /// Inserts tracks at `at` (the end by default). A track may occur in a
    /// playlist more than once.
    pub fn add_entries(
        &mut self,
        id: PlaylistId,
        tracks: &[TrackId],
        at: Option<usize>,
    ) -> Result<Vec<EntryId>> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some(at) = at else {
            let first: i64 = tx.query_row("SELECT coalesce(max(position) + 1, 0) FROM playlist_entries WHERE playlist_id = ?1", [id], |r| r.get(0))?;
            let added = insert_entries(&tx, id, tracks, first.try_into()?)?;
            tx.commit()?;
            return Ok(added);
        };
        let order = ordered_entries(&tx, id)?;
        let at = at.min(order.len());
        let new = insert_entries(&tx, id, tracks, order.len())?;
        let mut order = order;
        order.splice(at..at, new.iter().copied());
        renumber(&tx, id, &order)?;
        tx.commit()?;
        Ok(new)
    }
    pub fn remove_entries(&mut self, id: PlaylistId, entries: &[EntryId]) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut delete = tx.prepare_cached(
                "DELETE FROM playlist_entries WHERE playlist_id = ?1 AND id = ?2",
            )?;
            for e in entries {
                delete.execute(params![id, e])?;
            }
        }
        let order = ordered_entries(&tx, id)?;
        renumber(&tx, id, &order)?;
        tx.commit()?;
        Ok(())
    }
    /// Moves entries, keeping their relative order, so they sit before the
    /// destination entry (or at the end).
    pub fn move_entries(
        &mut self,
        id: PlaylistId,
        entries: &[EntryId],
        before_entry: Option<EntryId>,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let order = ordered_entries(&tx, id)?;
        let entries: std::collections::HashSet<_> = entries.iter().copied().collect();
        let to = match before_entry {
            Some(entry) => order
                .iter()
                .position(|e| *e == entry)
                .ok_or_else(|| anyhow::anyhow!("The destination entry no longer exists"))?,
            None => order.len(),
        };
        let before = order[..to].iter().filter(|e| entries.contains(e)).count();
        let (moving, mut rest): (Vec<EntryId>, Vec<EntryId>) =
            order.into_iter().partition(|e| entries.contains(e));
        ensure!(
            moving.len() == entries.len(),
            "The playlist selection changed. Select the entries again."
        );
        let at = to - before;
        rest.splice(at..at, moving);
        renumber(&tx, id, &rest)?;
        tx.commit()?;
        Ok(())
    }
    pub fn unique_playlist_name(&self, base: &str) -> Result<String> {
        let taken: Vec<String> = self.playlists()?.into_iter().map(|p| p.name).collect();
        let free = |n: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
        if free(base) {
            return Ok(base.into());
        }
        Ok((2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| free(n))
            .unwrap())
    }
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "A playlist needs a name");
    Ok(name.chars().take(200).collect())
}

fn ordered_entries(tx: &Transaction, id: PlaylistId) -> Result<Vec<EntryId>> {
    let mut stmt = tx.prepare_cached(
        "SELECT id FROM playlist_entries WHERE playlist_id = ?1 ORDER BY position, id",
    )?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn insert_entries(
    tx: &Transaction,
    id: PlaylistId,
    tracks: &[TrackId],
    first: usize,
) -> Result<Vec<EntryId>> {
    let mut insert = tx.prepare_cached(
        "INSERT INTO playlist_entries(playlist_id, track_id, position, added_at) VALUES(?1, ?2, ?3, ?4)",
    )?;
    let at = now();
    let mut ids = Vec::with_capacity(tracks.len());
    for (i, track) in tracks.iter().enumerate() {
        insert.execute(params![id, track, (first + i) as i64, at])?;
        ids.push(tx.last_insert_rowid());
    }
    if !tracks.is_empty() {
        tx.execute(
            "UPDATE playlists SET modified_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
    }
    Ok(ids)
}

fn renumber(tx: &Transaction, id: PlaylistId, order: &[EntryId]) -> Result<()> {
    let mut update = tx.prepare_cached(
        "UPDATE playlist_entries SET position = ?3 WHERE playlist_id = ?1 AND id = ?2 AND position != ?3",
    )?;
    for (i, e) in order.iter().enumerate() {
        update.execute(params![id, e, i as i64])?;
    }
    tx.execute(
        "UPDATE playlists SET modified_at = ?2 WHERE id = ?1",
        params![id, now()],
    )?;
    Ok(())
}

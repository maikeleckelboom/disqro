//! Indexed performer and album-artist credits.
use super::{Store, Track};
use anyhow::Result;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtistId(pub i64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtistRow {
    pub id: ArtistId,
    pub name: String,
    /// Available recordings with this performer credit.
    pub tracks: usize,
    /// Distinct releases it performs on or is credited for as album artist.
    pub releases: usize,
}

pub(super) struct Credits {
    pub performer_key: String,
    pub album_artist: Option<i64>,
}

fn record(tx: &Transaction<'_>, name: &str, key: &str) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO artists(key, name) VALUES (?1, ?2) ON CONFLICT(key) DO NOTHING",
    )?
    .execute((key, name))?;
    Ok(())
}

pub(super) fn record_credits(tx: &Transaction<'_>, track: &Track) -> Result<Credits> {
    let performer = track.artist.trim();
    let performer_key = performer.to_lowercase();
    if !performer.is_empty() {
        record(tx, performer, &performer_key)?;
    }
    let album_artist = if let Some(name) = track
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let key = name.to_lowercase();
        record(tx, name, &key)?;
        Some(
            tx.prepare_cached("SELECT id FROM artists WHERE key = ?1")?
                .query_row([key], |r| r.get(0))?,
        )
    } else {
        None
    };
    Ok(Credits {
        performer_key,
        album_artist,
    })
}

// All statements share the performer index and the published album credit.
// A release counts once even if the artist has several credits across discs.
pub(super) const PERFORMS: &str = "t.present = 1 AND t.root_id IS NOT NULL AND t.artist_key <> ''";
const CREDITED_ALBUMS: &str =
    "a.album_artist_id = p.id AND a.is_release = 1 AND a.root_id IS NOT NULL AND EXISTS (
        SELECT 1 FROM tracks t WHERE t.album_id = a.id AND t.present = 1
          AND t.root_id = a.root_id)";

pub(super) fn active() -> String {
    format!(
        "(EXISTS (SELECT 1 FROM tracks t WHERE {PERFORMS} AND t.artist_key = p.key)
          OR EXISTS (SELECT 1 FROM albums a WHERE {CREDITED_ALBUMS}))"
    )
}

impl Store {
    #[cfg(test)]
    pub fn artists(&self, search: &str) -> Result<Vec<ArtistRow>> {
        let mut rows = Vec::new();
        self.visit_artists(search, |row| {
            rows.push(row);
            Ok(())
        })?;
        Ok(rows)
    }

    pub(crate) fn visit_artists(
        &self,
        search: &str,
        mut visit: impl FnMut(ArtistRow) -> Result<()>,
    ) -> Result<()> {
        let sql = format!(
            "SELECT p.id, p.name,
                (SELECT count(*) FROM tracks t WHERE {PERFORMS} AND t.artist_key = p.key),
                (SELECT count(*) FROM (
                    SELECT coalesce(d.first_id, t.album_id)
                    FROM tracks t JOIN albums a ON a.id = t.album_id
                    LEFT JOIN release_discs d ON d.album_id = a.id
                    WHERE {PERFORMS} AND t.artist_key = p.key AND a.is_release = 1
                    UNION
                    SELECT coalesce(d.first_id, a.id)
                    FROM albums a LEFT JOIN release_discs d ON d.album_id = a.id
                    WHERE {CREDITED_ALBUMS}
                ))
             FROM artists p WHERE instr(p.key, ?1) > 0 AND {}
             ORDER BY p.key, p.id",
            active(),
        );
        let mut stmt = self.db.prepare_cached(&sql)?;
        let rows = stmt.query_map([search.trim().to_lowercase()], |r| {
            Ok(ArtistRow {
                id: ArtistId(r.get(0)?),
                name: r.get(1)?,
                tracks: r.get::<_, i64>(2)? as usize,
                releases: r.get::<_, i64>(3)? as usize,
            })
        })?;
        for row in rows {
            visit(row?)?;
        }
        Ok(())
    }
}

pub(super) fn tracks_of(arg: usize) -> String {
    format!("{PERFORMS} AND t.artist_key = (SELECT key FROM artists WHERE id = ?{arg})")
}

pub(super) fn albums_of(arg: usize) -> String {
    let performs = tracks_of(arg);
    format!(
        "WITH credited(album) AS MATERIALIZED (
            SELECT DISTINCT t.album_id FROM tracks t JOIN albums a ON a.id = t.album_id
            WHERE {performs} AND a.is_release = 1
            UNION
            SELECT a.id FROM artists p JOIN albums a ON a.album_artist_id = p.id
            WHERE p.id = ?{arg} AND {CREDITED_ALBUMS}
         )
         SELECT album FROM credited
         UNION
         SELECT d.album_id FROM release_discs d WHERE d.first_id IN (
            SELECT m.first_id FROM credited c JOIN release_discs m ON m.album_id = c.album
         )"
    )
}

//! Disqro's persistent music library.
//!
//! The database holds three kinds of data with different lifetimes:
//!
//! - **Identities.** Physical files (audio sources and CUE sheets) and the
//!   logical tracks anchored on them. A whole file is one track; a CUE sheet
//!   anchors one track per authored chapter and per recorded lead-in. A track
//!   is identified by its anchor, part and source-frame slice, independently of title or
//!   sort order, and keeps its identity when its file is missing.
//! - **Index metadata.** Titles, albums, durations, search keys and
//!   availability, refreshed by every scan.
//! - **User-owned data.** Likes, ratings and playlists. They reference tracks
//!   with `ON DELETE RESTRICT`, so no scan or cleanup can remove a track the
//!   listener still refers to.
//!
//! Rebuildable analysis (parsed sheets, tags, waveform peaks) stays in
//! [`crate::cache`]. Deleting that cache never touches this database.
use crate::library::Track;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

mod identity;
mod index;
mod playlists;
pub mod query;
mod schema;
#[cfg(test)]
mod tests;
mod worker;

pub use index::{Progress, Summary, index_root};
pub use query::{AlbumRow, Order, Query, Scope, Term};
pub use worker::{Cancelled, Handle, Ticket, is_cancelled};

pub type TrackId = i64;
pub type RootId = i64;
pub type PlaylistId = i64;
pub type EntryId = i64;
pub type AlbumId = i64;

/// Files are compared the way the file system compares them: lexically
/// normalized and, on Windows, without regard to case.
pub fn path_key(path: &Path) -> String {
    let mut normal = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            c => normal.push(c.as_os_str()),
        }
    }
    let text = normal.to_string_lossy();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Finds a track met while browsing a folder, by the files it plays from.
/// `chapter` is the CUE track number, or 0 for a whole file or a lead-in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Locator {
    anchor: String,
    source: String,
    chapter: u8,
    start: Option<i64>,
    end: Option<i64>,
}
impl Locator {
    pub fn of(t: &Track) -> Self {
        match &t.disc {
            Some(d) => Self {
                anchor: path_key(&d.sheet),
                source: path_key(&t.source),
                chapter: d.number,
                start: Some(d.start as i64),
                end: d.end.map(|tick| tick as i64),
            },
            None => {
                let key = path_key(&t.source);
                Self {
                    anchor: key.clone(),
                    source: key,
                    chapter: 0,
                    start: None,
                    end: None,
                }
            }
        }
    }
}

/// The listener's marks on a track. Rating 0 means unrated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    pub liked: bool,
    pub rating: u8,
}

/// A row of a library view: a track, or a playlist entry of one.
#[derive(Clone, Debug)]
pub struct Row {
    /// The track id, or the playlist entry id in a playlist.
    pub key: i64,
    pub id: TrackId,
    pub track: Arc<Track>,
    /// False when the last scan could not find the track or its source.
    pub available: bool,
    pub root: Option<RootId>,
    pub source_root: Option<RootId>,
    revision: identity::Revision,
}

/// Per-page observations for sources outside the registered scanner.
#[derive(Default)]
pub struct DetachedFiles {
    audio: std::collections::HashMap<PathBuf, Option<(crate::cache::Stamp, identity::Evidence)>>,
    sheets: std::collections::HashMap<PathBuf, std::collections::HashSet<Locator>>,
}
impl DetachedFiles {
    pub fn available(&mut self, row: &Row) -> bool {
        if row.source_root.is_none() {
            let observation = self
                .audio
                .entry(row.track.source.clone())
                .or_insert_with(|| {
                    let stamp = crate::cache::Stamp::read(&row.track.source)?;
                    let evidence = identity::Evidence::read(&row.track.source, false, &stamp)?;
                    Some((stamp, evidence))
                });
            if !observation
                .as_ref()
                .is_some_and(|(stamp, e)| row.revision.matches(stamp.bytes, stamp.modified, e))
            {
                return false;
            }
        }
        if row.root.is_none()
            && let Some(d) = &row.track.disc
        {
            return self
                .sheets
                .entry(d.sheet.clone())
                .or_insert_with(|| {
                    crate::cue::read(&d.sheet)
                        .unwrap_or_default()
                        .iter()
                        .map(Locator::of)
                        .collect()
                })
                .contains(&Locator::of(&row.track));
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub id: RootId,
    pub path: PathBuf,
    pub name: String,
    pub tracks: usize,
    pub scanned: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub entries: usize,
}

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn location() -> Option<PathBuf> {
        crate::profile::location().map(|p| p.with_file_name("library.sqlite3"))
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&mut db, Some(path), schema::MIGRATIONS)?;
        configure(&db)?;
        Ok(Self { db })
    }
    #[cfg(test)]
    pub fn memory() -> Self {
        let mut db = Connection::open_in_memory().unwrap();
        configure(&db).unwrap();
        schema::migrate(&mut db, None, schema::MIGRATIONS).unwrap();
        Self { db }
    }

    // Registered collections

    pub fn roots(&self) -> Result<Vec<Root>> {
        let mut stmt = self.db.prepare_cached(
            "SELECT r.id, r.path, r.name, r.scanned_at IS NOT NULL,
                (SELECT count(*) FROM tracks t WHERE t.root_id = r.id AND t.present = 1)
             FROM roots r ORDER BY r.name COLLATE NOCASE, r.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Root {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                name: r.get(2)?,
                scanned: r.get(3)?,
                tracks: r.get::<_, i64>(4)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    /// Registers a folder. Collections may not overlap.
    pub fn add_root(&mut self, path: &Path) -> Result<RootId> {
        let text = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("{} has an unsupported name", path.display()))?;
        let key = path_key(path);
        let inside = |outer: &str, inner: &str| {
            inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['\\', '/']))
                || outer.ends_with(['\\', '/']) && inner.starts_with(outer)
        };
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let registrations = {
            let mut stmt = tx.prepare("SELECT path_key, name FROM roots")?;
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (other, name) in registrations {
            if inside(&other, &key) {
                bail!(
                    "{} is already in the library as part of {}",
                    path.display(),
                    name
                );
            }
            if inside(&key, &other) {
                bail!("{} contains the collection {}", path.display(), name);
            }
        }
        tx.execute(
            "INSERT INTO roots(path, path_key, name, added_at) VALUES(?1, ?2, ?3, ?4)",
            params![text, key, crate::library::display_name(path), now()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }
    /// Forgets a collection. Files on disk are untouched. Tracks the listener
    /// liked, rated or added to a playlist stay, detached, and return to the
    /// collection if its folder is added again.
    pub fn remove_root(&mut self, id: RootId) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE tracks SET root_id = NULL WHERE root_id = ?1", [id])?;
        tx.execute("UPDATE files SET root_id = NULL WHERE root_id = ?1", [id])?;
        tx.execute("UPDATE albums SET root_id = NULL WHERE root_id = ?1", [id])?;
        tx.execute("DELETE FROM roots WHERE id = ?1", [id])?;
        collect_garbage(&tx)?;
        tx.commit()?;
        Ok(())
    }

    // Likes and ratings

    pub fn marks(&self) -> Result<Vec<TrackMark>> {
        let mut stmt = self.db.prepare_cached(&format!("{MARK_SQL} WHERE t.id IN (SELECT track_id FROM likes UNION SELECT track_id FROM ratings)"))?;
        Ok(stmt
            .query_map([], read_mark)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Includes cleared marks so the interface can remove just those rows.
    pub fn marks_for(&self, ids: &[TrackId]) -> Result<Vec<TrackMark>> {
        let mut stmt = self
            .db
            .prepare_cached(&format!("{MARK_SQL} WHERE t.id = ?1"))?;
        Ok(ids
            .iter()
            .map(|id| stmt.query_row([id], read_mark))
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn set_liked(&mut self, ids: &[TrackId], liked: bool) -> Result<()> {
        self.change_likes(ids, Some(liked))
    }
    pub fn toggle_liked(&mut self, ids: &[TrackId]) -> Result<()> {
        self.change_likes(ids, None)
    }
    fn change_likes(&mut self, ids: &[TrackId], requested: Option<bool>) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let liked = match requested {
            Some(liked) => liked,
            None => {
                let mut stmt =
                    tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM likes WHERE track_id = ?1)")?;
                let mut all = true;
                for id in ids {
                    if !stmt.query_row([id], |r| r.get::<_, bool>(0))? {
                        all = false;
                        break;
                    }
                }
                !all
            }
        };
        {
            let mut like = tx.prepare_cached(
                "INSERT INTO likes(track_id, liked_at) VALUES(?1, ?2) ON CONFLICT DO NOTHING",
            )?;
            let mut unlike = tx.prepare_cached("DELETE FROM likes WHERE track_id = ?1")?;
            let at = now();
            for id in ids {
                if liked {
                    like.execute(params![id, at])?;
                } else {
                    unlike.execute([id])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Sets 1 to 5 stars, or clears the rating with 0.
    pub fn set_rating(&mut self, ids: &[TrackId], stars: u8) -> Result<()> {
        ensure!(stars <= 5, "A rating has at most five stars");
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut rate = tx.prepare_cached(
                "INSERT INTO ratings(track_id, stars, rated_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(track_id) DO UPDATE SET stars = excluded.stars, rated_at = excluded.rated_at",
            )?;
            let mut clear = tx.prepare_cached("DELETE FROM ratings WHERE track_id = ?1")?;
            let at = now();
            for &id in ids {
                if stars == 0 {
                    clear.execute([id])?;
                } else {
                    rate.execute(params![id, stars, at])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The identities of tracks met while browsing. A track outside every
    /// registered collection gets an identity of its own, so liking it or
    /// adding it to a playlist never requires importing its folder.
    pub fn resolve(&mut self, tracks: &[Arc<Track>]) -> Result<Vec<TrackId>> {
        let mut checked = std::collections::HashSet::new();
        let mut ids = Vec::with_capacity(tracks.len());
        for t in tracks {
            let locator = Locator::of(t);
            if checked.insert(locator.anchor.clone()) {
                index::adopt(&mut self.db, t)?;
            }
            ids.push(self.find(&locator)?.with_context(|| {
                format!(
                    "{} changed or is unavailable. Refresh the folder and try again.",
                    t.title
                )
            })?);
        }
        Ok(ids)
    }
    pub fn find(&self, l: &Locator) -> Result<Option<TrackId>> {
        Ok(self
            .db
            .prepare_cached(
                "SELECT t.id FROM tracks t JOIN files a ON a.id = t.anchor JOIN files s ON s.id = t.source
                 WHERE a.path_key = ?1 AND s.path_key = ?2
                   AND t.present AND a.present AND s.present AND t.first_tick IS ?4 AND t.last_tick IS ?5
                   AND max(t.part, 0) = ?3
                 ORDER BY t.part DESC LIMIT 1",
            )?
            .query_row(params![l.anchor, l.source, l.chapter, l.start, l.end], |r| r.get(0))
            .optional()?)
    }
}

pub type TrackMark = (TrackId, Option<Locator>, Mark);
const MARK_SQL: &str =
    "SELECT t.id, a.path, s.path, t.part, l.track_id IS NOT NULL, coalesce(r.stars, 0),
        t.present AND a.present AND s.present, t.first_tick, t.last_tick
    FROM tracks t JOIN files a ON a.id = t.anchor JOIN files s ON s.id = t.source
    LEFT JOIN likes l ON l.track_id = t.id LEFT JOIN ratings r ON r.track_id = t.id";

fn read_mark(r: &rusqlite::Row<'_>) -> rusqlite::Result<TrackMark> {
    let part: i64 = r.get(3)?;
    Ok((
        r.get(0)?,
        if r.get(6)? {
            Some(Locator {
                anchor: path_key(Path::new(&r.get::<_, String>(1)?)),
                source: path_key(Path::new(&r.get::<_, String>(2)?)),
                chapter: part.max(0) as u8,
                start: r.get(7)?,
                end: r.get(8)?,
            })
        } else {
            None
        },
        Mark {
            liked: r.get(4)?,
            rating: r.get(5)?,
        },
    ))
}

fn configure(db: &Connection) -> Result<()> {
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}

/// Removes detached rows nothing refers to. Tracks with likes, ratings or
/// playlist entries always stay.
fn collect_garbage(tx: &Transaction) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM tracks WHERE root_id IS NULL AND id NOT IN (
             SELECT track_id FROM likes UNION SELECT track_id FROM ratings
             UNION SELECT track_id FROM playlist_entries);
         DELETE FROM files WHERE root_id IS NULL AND id NOT IN (
             SELECT anchor FROM tracks UNION SELECT source FROM tracks);
         DELETE FROM albums WHERE id NOT IN (
             SELECT album_id FROM tracks WHERE album_id IS NOT NULL);",
    )?;
    Ok(())
}

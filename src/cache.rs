//! Rebuildable data only. The versioned database can be removed independently
//! of the workspace and queue. Paths use the OS encoding without lossy conversion.
use crate::library::{DiscTrack, Track};
use rusqlite::{Connection, params};
use std::{fs, path::Path, time::UNIX_EPOCH};

pub struct Cache(pub(crate) Connection);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Stamp {
    pub(crate) bytes: i64,
    pub(crate) modified: i64,
}
impl Stamp {
    pub fn read(path: &Path) -> Option<Self> {
        Self::from_metadata(&fs::metadata(path).ok()?)
    }
    pub fn from_metadata(m: &fs::Metadata) -> Option<Self> {
        Some(Self {
            bytes: m.len().try_into().ok()?,
            modified: m
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos()
                .try_into()
                .ok()?,
        })
    }
}
impl Cache {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(dir)?;
        let db = Connection::open(dir.join("music-cache-v1.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_millis(200))?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS sheets(path BLOB PRIMARY KEY, bytes INTEGER, modified INTEGER) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS chapters(sheet BLOB REFERENCES sheets(path) ON DELETE CASCADE,
                ordinal INTEGER, source TEXT NOT NULL, number INTEGER, first_tick INTEGER, last_tick INTEGER,
                title TEXT, artist TEXT, album TEXT, year INTEGER, disc_number INTEGER, PRIMARY KEY(sheet,ordinal)) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS chapters_artist ON chapters(artist COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS chapters_album ON chapters(album COLLATE NOCASE);
            CREATE TABLE IF NOT EXISTS tags(path BLOB PRIMARY KEY, bytes INTEGER, modified INTEGER, data BLOB) WITHOUT ROWID;
            DROP TABLE IF EXISTS wave_pages_v1;
            DROP TABLE IF EXISTS wave_pages_v2;
            DROP TABLE IF EXISTS wave_evidence_v1;
            CREATE TABLE IF NOT EXISTS wave_evidence_v2(path BLOB, requested BLOB, size INTEGER, modified INTEGER,
                volume INTEGER, object INTEGER, data BLOB, UNIQUE(path,requested));
            CREATE TABLE IF NOT EXISTS disc_sources_v1(sheet BLOB, sheet_bytes INTEGER, sheet_modified INTEGER,
                path BLOB, bytes INTEGER, modified INTEGER, rate INTEGER, frames INTEGER, PRIMARY KEY(sheet,path)) WITHOUT ROWID;")?;
        Ok(Self(db))
    }
    pub fn program(&self, path: &Path) -> anyhow::Result<Vec<Track>> {
        let Some(stamp) = Stamp::read(path) else {
            return crate::cue::read(path);
        };
        if let Ok(rows) = self.cached_program(path, &stamp)
            && !rows.is_empty()
        {
            return Ok(rows);
        }
        let tracks = crate::cue::read(path)?;
        if Stamp::read(path).as_ref() == Some(&stamp)
            && tracks.iter().all(|t| t.source.to_str().is_some())
        {
            let _ = self.store_program(path, &stamp, &tracks);
        }
        Ok(tracks)
    }
    fn cached_program(&self, path: &Path, stamp: &Stamp) -> rusqlite::Result<Vec<Track>> {
        let mut stmt = self.0.prepare_cached("SELECT source,number,first_tick,last_tick,title,artist,album,year,disc_number
            FROM sheets JOIN chapters ON chapters.sheet=sheets.path WHERE sheets.path=? AND bytes=? AND modified=? ORDER BY ordinal")?;
        stmt.query_map(
            params![
                path.as_os_str().as_encoded_bytes(),
                stamp.bytes,
                stamp.modified
            ],
            |r| {
                let mut track = Track::file(std::path::PathBuf::from(r.get::<_, String>(0)?));
                let start = r.get::<_, i64>(2)? as u64;
                let end = r.get::<_, Option<i64>>(3)?.map(|v| v as u64);
                track.duration = end
                    .and_then(|e| e.checked_sub(start))
                    .map(|ticks| ticks as f64 / 75.);
                track.disc = Some(DiscTrack {
                    sheet: path.into(),
                    number: r.get(1)?,
                    start,
                    end,
                });
                track.title = r.get(4)?;
                track.artist = r.get(5)?;
                track.album = r.get(6)?;
                track.year = r.get(7)?;
                track.disc_number = r.get(8)?;
                Ok(track)
            },
        )?
        .collect()
    }
    fn store_program(&self, path: &Path, stamp: &Stamp, tracks: &[Track]) -> anyhow::Result<()> {
        let tx = self.0.unchecked_transaction()?;
        let sheet = path.as_os_str().as_encoded_bytes();
        tx.execute(
            "INSERT OR REPLACE INTO sheets VALUES(?,?,?)",
            params![sheet, stamp.bytes, stamp.modified],
        )?;
        {
            let mut insert =
                tx.prepare_cached("INSERT INTO chapters VALUES(?,?,?,?,?,?,?,?,?,?,?)")?;
            for (i, t) in tracks.iter().enumerate() {
                let d = t
                    .disc
                    .as_ref()
                    .expect("CUE parser returns authored segments");
                insert.execute(params![
                    sheet,
                    i as i64,
                    t.source.to_str(),
                    d.number,
                    d.start as i64,
                    d.end.map(|e| e as i64),
                    t.title,
                    t.artist,
                    t.album,
                    t.year,
                    t.disc_number
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn tags(&self, path: &Path, stamp: &Stamp) -> Option<Track> {
        let bytes: Vec<u8> = self
            .0
            .prepare_cached("SELECT data FROM tags WHERE path=? AND bytes=? AND modified=?")
            .ok()?
            .query_row(
                params![
                    path.as_os_str().as_encoded_bytes(),
                    stamp.bytes,
                    stamp.modified
                ],
                |r| r.get(0),
            )
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    pub fn store_tags(&self, track: &Track, stamp: &Stamp) -> anyhow::Result<()> {
        self.0
            .prepare_cached("INSERT OR REPLACE INTO tags VALUES(?,?,?,?)")?
            .execute(params![
                track.source.as_os_str().as_encoded_bytes(),
                stamp.bytes,
                stamp.modified,
                serde_json::to_vec(track)?
            ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_sheet_atomically_replaces_all_chapters_and_keeps_numbering() {
        let dir = tempfile::tempdir().unwrap();
        let cd = dir.path().join("CD2");
        fs::create_dir(&cd).unwrap();
        let path = cd.join("music.cue");
        let one = "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00";
        fs::write(&path, format!("{one}\nTRACK 02 AUDIO\nINDEX 01 00:01:00")).unwrap();
        let db = Cache::open(dir.path()).unwrap();
        let old = db.program(&path).unwrap();
        assert_eq!(old.len(), 2);
        assert_eq!(
            db.cached_program(&path, &Stamp::read(&path).unwrap())
                .unwrap(),
            old
        );
        fs::write(&path, one).unwrap();
        let new = db.program(&path).unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].disc_number, Some(2));
        assert_eq!(
            db.0.query_row("SELECT count(*) FROM chapters", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

//! Persistent occurrence checkpoints and listening projections.
use super::{DetachedFiles, Scope, Store, TrackId};
use crate::listening::{Change, Checkpoint, Period, Totals, midnight};
use anyhow::{Result, ensure};
use chrono::{Days, NaiveDate, TimeZone};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
#[path = "listeningTests.rs"]
mod tests;

struct RecordingIdentity {
    track: TrackId,
    release: Option<String>,
    album: String,
}

impl Store {
    pub fn recover_listening(&mut self) -> Result<()> {
        self.db.execute(
            "UPDATE listening_occurrences SET ended_at = activity_at, reason = 'interrupted'
             WHERE ended_at IS NULL",
            [],
        )?;
        Ok(())
    }

    pub fn checkpoint_listening(&mut self, c: &Checkpoint) -> Result<Option<Change>> {
        let existing = self
            .db
            .query_row(
                "SELECT id, track_id, checkpoint, qualified_at IS NOT NULL
             FROM listening_occurrences WHERE token = ?1",
                [&c.token],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<TrackId>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()?;
        let track = &c.recording.track;
        let first = existing.is_none() && c.sequence == 1;
        let resolved = if first {
            self.listening_identity(&c.recording)?
        } else {
            None
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ended = c.end.map(|(at, _)| at);
        let reason = c.end.map(|(_, reason)| reason.as_str());
        let (id, track_id, ordered) = match existing {
            Some((_, _, sequence, _)) if sequence >= c.sequence => return Ok(None),
            Some((id, track_id, sequence, qualified)) => {
                ensure!(
                    sequence + 1 == c.sequence,
                    "A listening checkpoint is missing"
                );
                tx.execute(
                    "UPDATE listening_occurrences SET activity_at=?2, ended_at=?3, reason=?4,
                     credited_us=?5, qualified_at=?6, checkpoint=?7 WHERE id=?1",
                    params![
                        id,
                        c.activity_at,
                        ended,
                        reason,
                        c.credited_us,
                        c.qualified_at,
                        c.sequence
                    ],
                )?;
                (id, track_id, !qualified && c.qualified_at.is_some())
            }
            None if first => {
                let track_id = resolved.as_ref().map(|identity| identity.track);
                let release = resolved
                    .as_ref()
                    .and_then(|identity| identity.release.as_deref());
                let album = resolved
                    .as_ref()
                    .map_or(track.album.as_str(), |identity| identity.album.as_str());
                let artist_key =
                    (!track.artist.trim().is_empty()).then(|| track.artist.trim().to_lowercase());
                let search = format!("{} {} {} {}", track.title, track.artist, track.album, album)
                    .to_lowercase();
                tx.execute(
                    "INSERT INTO listening_occurrences(token, track_id, title, artist, artist_key,
                     album, release_key, snapshot, started_at, activity_at, ended_at, reason, search,
                     credited_us, qualified_at, checkpoint)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1)",
                    params![
                        c.token,
                        track_id,
                        track.title,
                        track.artist,
                        artist_key,
                        album,
                        release,
                        serde_json::to_string(track.as_ref())?,
                        c.started_at,
                        c.activity_at,
                        ended,
                        reason,
                        search,
                        c.credited_us,
                        c.qualified_at
                    ],
                )?;
                (tx.last_insert_rowid(), track_id, c.qualified_at.is_some())
            }
            // A user removed this occurrence after its initial checkpoint.
            None => return Ok(None),
        };
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO listening_minutes VALUES(?1,?2,?3)
                 ON CONFLICT DO UPDATE SET credited_us = credited_us + excluded.credited_us",
            )?;
            for (&minute, &credited) in &c.minutes {
                insert.execute(params![id, minute, credited])?;
            }
        }
        tx.commit()?;
        Ok(Some(Change {
            track: track_id,
            order: ordered,
            removed: false,
        }))
    }

    fn listening_identity(
        &mut self,
        recording: &crate::listening::Recording,
    ) -> Result<Option<RecordingIdentity>> {
        use super::Locator;
        use crate::cache::Stamp;
        let track = &recording.track;
        let source = Stamp::read(&track.source);
        let same_object = |path: &std::path::Path, expected: Option<super::NativeFile>| {
            expected.is_none_or(|expected| {
                std::fs::File::open(path)
                    .ok()
                    .as_ref()
                    .and_then(super::NativeFile::of)
                    == Some(expected)
            })
        };
        let same_paths = source.is_some()
            && source == recording.source
            && same_object(&track.source, recording.source_object)
            && track.disc.as_ref().is_none_or(|d| {
                recording.sheet.is_some()
                    && Stamp::read(&d.sheet) == recording.sheet
                    && same_object(&d.sheet, recording.sheet_object)
            });
        let id = if !same_paths {
            let Some(id) = self.relocated_listening_track(recording)? else {
                return Ok(None);
            };
            id
        } else {
            let found = self.find(&Locator::of(track))?;
            let current = if let Some(id) = found {
                let source = source.as_ref().unwrap();
                let (size, modified, a_size, a_modified): (i64, i64, i64, i64) =
                    self.db.query_row(
                        "SELECT s.size,s.modified,a.size,a.modified FROM tracks t
                     JOIN files s ON s.id=t.source JOIN files a ON a.id=t.anchor WHERE t.id=?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )?;
                (source.bytes == size
                    && source.modified == modified
                    && recording
                        .sheet
                        .as_ref()
                        .is_none_or(|s| s.bytes == a_size && s.modified == a_modified))
                .then_some(id)
            } else {
                None
            };
            match current {
                Some(id) => id,
                None => {
                    if let Err(error) = super::index::adopt(&mut self.db, track) {
                        if error.downcast_ref::<rusqlite::Error>().is_some() {
                            return Err(error);
                        }
                        return Ok(None);
                    }
                    let Some(id) = self.find(&Locator::of(track))? else {
                        return Ok(None);
                    };
                    id
                }
            }
        };
        let album: Option<i64> = self
            .db
            .query_row("SELECT album_id FROM tracks WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        // A tagged album or authored sheet is meaningful release evidence.
        let release = self.listening_release(
            id,
            album.filter(|_| !track.album.trim().is_empty() || track.disc.is_some()),
        )?;
        let (key, name) = release.map_or((None, track.album.clone()), |(id, label)| {
            (Some(format!("album:{id}")), label)
        });
        Ok(Some(RecordingIdentity {
            track: id,
            release: key,
            album: name,
        }))
    }

    fn relocated_listening_track(
        &self,
        recording: &crate::listening::Recording,
    ) -> Result<Option<TrackId>> {
        let Some((source, stamp)) = recording.source_object.zip(recording.source.as_ref()) else {
            return Ok(None);
        };
        let anchor = if recording.track.disc.is_some() {
            recording.sheet_object.zip(recording.sheet.as_ref())
        } else {
            Some((source, stamp))
        };
        let Some((anchor, anchor_stamp)) = anchor else {
            return Ok(None);
        };
        let loc = super::Locator::of(&recording.track);
        let mut stmt = self.db.prepare_cached(
            "SELECT t.id FROM files s
             JOIN tracks t ON t.source=s.id JOIN files a ON a.id=t.anchor
             WHERE s.present=1 AND s.volume=?1 AND s.file_index=?2 AND s.size=?3 AND s.modified=?4
               AND a.present=1 AND a.volume=?5 AND a.file_index=?6 AND a.size=?7 AND a.modified=?8
               AND t.present=1 AND max(t.part,0)=?9 AND t.first_tick IS ?10 AND t.last_tick IS ?11
             LIMIT 2",
        )?;
        let mut ids = stmt.query(params![
            source.0,
            source.1,
            stamp.bytes,
            stamp.modified,
            anchor.0,
            anchor.1,
            anchor_stamp.bytes,
            anchor_stamp.modified,
            loc.chapter,
            loc.start,
            loc.end,
        ])?;
        let first = ids.next()?.map(|r| r.get(0)).transpose()?;
        Ok(if ids.next()?.is_none() { first } else { None })
    }

    pub fn erase_listening(&mut self, id: Option<i64>) -> Result<Change> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let track = if let Some(id) = id {
            let track = tx
                .query_row(
                    "SELECT track_id FROM listening_occurrences WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            tx.execute("DELETE FROM listening_occurrences WHERE id = ?1", [id])?;
            track
        } else {
            tx.execute_batch("DELETE FROM listening_totals; DELETE FROM listening_groups;")?;
            tx.execute("DELETE FROM listening_occurrences", [])?;
            None
        };
        tx.commit()?;
        Ok(Change {
            track,
            order: true,
            removed: true,
        })
    }

    #[cfg(test)]
    pub fn listening_totals(&self, id: TrackId) -> Result<Totals> {
        Ok(self
            .db
            .query_row(
                "SELECT plays,last_played,credited_us FROM listening_totals WHERE track_id=?1",
                [id],
                |r| {
                    Ok(Totals {
                        plays: r.get(0)?,
                        last_played: r.get(1)?,
                        credited_us: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn listening_index(
        &self,
        ids: Option<Vec<TrackId>>,
    ) -> Result<Vec<(TrackId, Option<super::Locator>, Totals)>> {
        const SELECT: &str = "SELECT t.id, a.path, s.path, t.part,
            t.present AND a.present AND s.present, t.first_tick, t.last_tick,
            coalesce(lt.plays,0), lt.last_played, coalesce(lt.credited_us,0)
            FROM listening_totals lt JOIN tracks t ON t.id=lt.track_id
            JOIN files a ON a.id=t.anchor JOIN files s ON s.id=t.source";
        let read = |r: &rusqlite::Row<'_>| {
            let locator = if r.get(4)? {
                Some(super::Locator {
                    anchor: super::path_key(std::path::Path::new(&r.get::<_, String>(1)?)),
                    source: super::path_key(std::path::Path::new(&r.get::<_, String>(2)?)),
                    chapter: r.get::<_, i64>(3)?.max(0) as u8,
                    start: r.get(5)?,
                    end: r.get(6)?,
                })
            } else {
                None
            };
            Ok((
                r.get(0)?,
                locator,
                Totals {
                    plays: r.get(7)?,
                    last_played: r.get(8)?,
                    credited_us: r.get(9)?,
                },
            ))
        };
        let Some(ids) = ids else {
            return Ok(self
                .db
                .prepare_cached(SELECT)?
                .query_map([], read)?
                .collect::<rusqlite::Result<_>>()?);
        };
        let mut stmt = self.db.prepare_cached(&format!("{SELECT} WHERE t.id=?1"))?;
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            result.push(stmt.query_row([id], read).optional()?.unwrap_or((
                id,
                None,
                Totals::default(),
            )));
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryKey(pub i64, pub i64);

pub struct HistoryRow {
    pub id: i64,
    pub started_at: i64,
    pub credited_us: i64,
    pub qualified: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track: Option<super::Row>,
}

impl HistoryRow {
    pub fn key(&self) -> HistoryKey {
        HistoryKey(self.started_at, self.id)
    }
}

pub const HISTORY_PAGE: usize = 100;

impl Store {
    pub fn history_page(
        &self,
        range: (i64, i64),
        search: &str,
        oldest_first: bool,
        cursor: Option<HistoryKey>,
    ) -> Result<Vec<HistoryRow>> {
        let (order, op, edge) = if oldest_first {
            ("ASC", ">", i64::MIN)
        } else {
            ("DESC", "<", i64::MAX)
        };
        let cursor = cursor.unwrap_or(HistoryKey(edge, edge));
        let pattern = search.trim().to_lowercase();
        let sql = format!(
            "SELECT id, started_at, credited_us, qualified_at IS NOT NULL, title, artist, album, track_id
             FROM listening_occurrences
             WHERE started_at >= ?1 AND started_at < ?2 AND (started_at, id) {op} (?3, ?4)
             AND (?5 = '' OR instr(search, ?5) > 0)
             ORDER BY started_at {order}, id {order} LIMIT ?6"
        );
        let mut stmt = self.db.prepare_cached(&sql)?;
        let mut ids = Vec::new();
        let mut page = stmt
            .query_map(
                params![
                    range.0,
                    range.1,
                    cursor.0,
                    cursor.1,
                    pattern,
                    HISTORY_PAGE as i64 + 1
                ],
                |r| {
                    ids.push(r.get::<_, Option<i64>>(7)?);
                    Ok(HistoryRow {
                        id: r.get(0)?,
                        started_at: r.get(1)?,
                        credited_us: r.get(2)?,
                        qualified: r.get(3)?,
                        title: r.get(4)?,
                        artist: r.get(5)?,
                        album: r.get(6)?,
                        track: None,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let tracks: HashMap<_, _> = self
            .rows(
                Scope::Library(None),
                &ids.iter().flatten().copied().collect::<Vec<_>>(),
            )?
            .into_iter()
            .map(|row| (row.id, row))
            .collect();
        let mut files = DetachedFiles::default();
        for (row, id) in page.iter_mut().zip(ids) {
            row.track = id
                .and_then(|id| tracks.get(&id))
                .filter(|track| {
                    track.available && track.track.source.is_file() && files.available(track)
                })
                .cloned();
        }
        Ok(page)
    }
}

pub fn period_range(period: Period) -> (i64, i64) {
    let today = chrono::Local::now().date_naive();
    (
        period
            .start(today)
            .map_or(i64::MIN, |d| midnight(&chrono::Local, d)),
        if period == Period::All {
            i64::MAX
        } else {
            midnight(&chrono::Local, today.succ_opt().unwrap())
        },
    )
}

#[derive(Default, Debug)]
pub struct Statistics {
    pub totals: Totals,
    pub recordings: i64,
    pub artists: i64,
    pub releases: i64,
    pub days: Vec<(NaiveDate, i64)>,
    pub tracks: Vec<Rank>,
    pub performers: Vec<Rank>,
    pub albums: Vec<Rank>,
}

#[derive(Debug)]
pub struct Rank {
    pub name: String,
    pub detail: String,
    pub plays: i64,
    pub credited_us: i64,
    pub track: Option<TrackId>,
}

impl Store {
    pub fn listening_statistics(&self, period: Period, by_time: bool) -> Result<Statistics> {
        self.statistics_in(
            &chrono::Local,
            chrono::Local::now().date_naive(),
            period,
            by_time,
        )
    }

    fn statistics_in(
        &self,
        zone: &impl TimeZone,
        today: NaiveDate,
        period: Period,
        by_time: bool,
    ) -> Result<Statistics> {
        let from = period.start(today).map_or(i64::MIN, |d| midnight(zone, d));
        let to = if period == Period::All {
            i64::MAX
        } else {
            midnight(zone, today.succ_opt().unwrap())
        };
        let db = self.db.unchecked_transaction()?;
        let mut stats = if period == Period::All {
            all_time(&db, by_time)?
        } else {
            // One row per matching occurrence, shared by all dated rankings.
            db.execute_batch(
                "CREATE TEMP TABLE listening_period(id INTEGER PRIMARY KEY, time INTEGER NOT NULL, plays INTEGER NOT NULL);",
            )?;
            db.execute(
                "INSERT INTO listening_period
                 SELECT occurrence_id, sum(credited_us), 0 FROM listening_minutes
                 WHERE minute >= ?1 AND minute < ?2 GROUP BY occurrence_id",
                params![from, to],
            )?;
            db.execute(
                "INSERT INTO listening_period SELECT id, 0, 1 FROM listening_occurrences
                 WHERE qualified_at >= ?1 AND qualified_at < ?2
                 ON CONFLICT DO UPDATE SET plays = 1",
                params![from, to],
            )?;
            let stats = dated(&db, by_time)?;
            db.execute_batch("DROP TABLE listening_period")?;
            stats
        };
        stats.days = {
            let mut days = BTreeMap::new();
            if let Some(start) = period.start(today) {
                for d in start.iter_days().take_while(|d| *d <= today) {
                    days.insert(d, 0);
                }
            }
            let mut stmt = db.prepare(
                "SELECT minute,sum(credited_us) FROM listening_minutes
                 WHERE minute >= ?1 AND minute < ?2 GROUP BY minute ORDER BY minute",
            )?;
            let mut rows = stmt.query(params![from, to])?;
            let mut boundary = i64::MIN;
            let mut day = today;
            while let Some(row) = rows.next()? {
                let minute: i64 = row.get(0)?;
                if minute >= boundary {
                    day = chrono::DateTime::from_timestamp_micros(minute)
                        .unwrap()
                        .with_timezone(zone)
                        .date_naive();
                    boundary = midnight(zone, day.checked_add_days(Days::new(1)).unwrap());
                }
                *days.entry(day).or_default() += row.get::<_, i64>(1)?;
            }
            days.into_iter().collect()
        };
        db.commit()?;
        Ok(stats)
    }
}

fn rank_order(by_time: bool) -> &'static str {
    if by_time {
        "time DESC, plays DESC"
    } else {
        "plays DESC, time DESC"
    }
}

fn ranks(db: &rusqlite::Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Rank>> {
    Ok(db
        .prepare_cached(sql)?
        .query_map(args, |r| {
            Ok(Rank {
                name: r.get(0)?,
                detail: r.get(1)?,
                plays: r.get(2)?,
                credited_us: r.get(3)?,
                track: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn dated(db: &rusqlite::Connection, by_time: bool) -> Result<Statistics> {
    // Drive from the selected period. A week never scans the full history.
    let joined = "listening_period p CROSS JOIN listening_occurrences o ON o.id=p.id";
    let (time, plays, recordings, artists, releases) = db.query_row(
        &format!(
            "SELECT coalesce(sum(time),0),coalesce(sum(plays),0),
         count(DISTINCT coalesce(track_id,-o.id)) FILTER (WHERE time>0),
         count(DISTINCT artist_key) FILTER (WHERE time>0),
         count(DISTINCT release_key) FILTER (WHERE time>0) FROM {joined}"
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let order = rank_order(by_time);
    let metric = if by_time { "p.time" } else { "p.plays" };
    let rank = |group: &str, name: &str, detail: &str, track: &str| {
        ranks(
            db,
            &format!(
                "SELECT max({name}),max({detail}),sum(p.plays) plays,sum(p.time) time,{track}
         FROM {joined} WHERE {group} IS NOT NULL GROUP BY {group}
         HAVING sum({metric}) > 0 ORDER BY {order}, {group} LIMIT 10"
            ),
            [],
        )
    };
    Ok(Statistics {
        totals: Totals {
            plays,
            credited_us: time,
            last_played: None,
        },
        recordings,
        artists,
        releases,
        tracks: rank(
            "coalesce(track_id,-o.id)",
            "title",
            "artist",
            "max(track_id)",
        )?,
        performers: rank("artist_key", "artist", "''", "NULL")?,
        albums: rank("release_key", "album", "''", "NULL")?,
        days: Vec::new(),
    })
}

fn all_time(db: &rusqlite::Connection, by_time: bool) -> Result<Statistics> {
    let (time, plays, recordings) = db.query_row(
        "SELECT sum(time),sum(plays),sum(count) FROM (
            SELECT coalesce(sum(credited_us),0) time,coalesce(sum(plays),0) plays,count(*) count
            FROM listening_totals
            UNION ALL
            SELECT coalesce(sum(credited_us),0),count(qualified_at),count(*)
            FROM listening_occurrences WHERE track_id IS NULL
         )",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let count = |kind| -> Result<i64> {
        Ok(db.query_row(
            "SELECT count(*) FROM (
            SELECT key FROM listening_groups WHERE kind=?1 GROUP BY key
        )",
            [kind],
            |r| r.get(0),
        )?)
    };
    let order = rank_order(by_time);
    let metric = if by_time { "time" } else { "plays" };
    let tracks = ranks(db, &format!(
        "SELECT coalesce(t.title,o.title),coalesce(t.artist,o.artist),s.plays,s.time,t.id
         FROM (
            SELECT track_id id,plays,credited_us time FROM listening_totals
            UNION ALL
            SELECT -id,qualified_at IS NOT NULL,credited_us FROM listening_occurrences WHERE track_id IS NULL
            ORDER BY {order}, id LIMIT 10
         ) s LEFT JOIN tracks t ON t.id=s.id LEFT JOIN listening_occurrences o ON o.id=-s.id
         WHERE s.{metric}>0 ORDER BY {order}, s.id"
    ), [])?;
    let groups = |kind| {
        ranks(
            db,
            &format!(
                "SELECT max(label),'',sum(plays) AS plays,sum(credited_us) AS time,NULL
         FROM listening_groups WHERE kind=?1 GROUP BY key
         HAVING {} > 0 ORDER BY {order}, key LIMIT 10",
                if by_time {
                    "sum(credited_us)"
                } else {
                    "sum(plays)"
                },
            ),
            [kind],
        )
    };
    Ok(Statistics {
        totals: Totals {
            plays,
            credited_us: time,
            last_played: None,
        },
        recordings,
        artists: count(0)?,
        releases: count(1)?,
        tracks,
        performers: groups(0)?,
        albums: groups(1)?,
        days: Vec::new(),
    })
}

//! Library views as SQL.
//!
//! A view is materialized as the ordered list of its row keys: track ids, or
//! playlist entry ids. Keys are a few bytes per row, so even a six-figure
//! collection costs one indexed scan and a sort per search or sort change.
//! The table then fetches only the pages it shows, selection follows keys
//! rather than positions, and counts are free.
use super::{AlbumId, PlaylistId, RootId, Row, Store, TrackId};
use crate::library::{DiscTrack, Track};
use anyhow::Result;
use rusqlite::{ToSql, params_from_iter};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// One registered collection, or all of them.
    Library(Option<RootId>),
    Album(AlbumId),
    Liked,
    Playlist(PlaylistId),
}
impl Scope {
    /// Playlist rows are entries, so one track can appear twice.
    pub fn keys_are_entries(self) -> bool {
        matches!(self, Self::Playlist(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Order {
    Number,
    Title,
    Artist,
    Album,
    Length,
    Format,
    Disc,
    Year,
    Path,
    Rating,
    Liked,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub scope: Scope,
    pub search: String,
    /// A column order, descending when true. `None` is the view's own order.
    pub order: Option<(Order, bool)>,
}

/// One search term. Plain words match titles, artists, albums and paths;
/// `artist:`, `album:`, `title:`, `format:`, `year:`, `rating:` and
/// `liked:` narrow to a field. `rating:4` means four stars or more and
/// `rating:0` unrated.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Any(String),
    Title(String),
    Artist(String),
    Album(String),
    Format(String),
    Year(u32),
    Rating(u8),
    Liked(bool),
}
impl Term {
    pub fn parse(search: &str) -> Vec<Self> {
        search
            .split_whitespace()
            .map(|word| {
                let lower = word.to_lowercase();
                let Some((field, value)) = lower.split_once(':') else {
                    return Self::Any(lower);
                };
                let value = value.to_owned();
                match field {
                    "title" => Self::Title(value),
                    "artist" => Self::Artist(value),
                    "album" => Self::Album(value),
                    "format" => Self::Format(value.to_uppercase()),
                    "year" => value.parse().map_or(Self::Any(lower.clone()), Self::Year),
                    "rating" => value
                        .trim_start_matches(">=")
                        .parse::<u8>()
                        .map_or(Self::Any(lower.clone()), |n| Self::Rating(n.min(5))),
                    "liked" | "is" if ["yes", "true", "liked"].contains(&value.as_str()) => {
                        Self::Liked(true)
                    }
                    "liked" if ["no", "false"].contains(&value.as_str()) => Self::Liked(false),
                    _ => Self::Any(lower),
                }
            })
            .collect()
    }
    /// Whether a track met while browsing matches. `mark` is only consulted
    /// for `rating:` and `liked:` terms.
    pub fn matches(&self, t: &Track, mark: impl Fn() -> super::Mark) -> bool {
        let has = |s: &str, v: &str| s.to_lowercase().contains(v);
        match self {
            Self::Any(v) => {
                [&t.title, &t.artist, &t.album].iter().any(|s| has(s, v))
                    || has(&t.source.to_string_lossy(), v)
            }
            Self::Title(v) => has(&t.title, v),
            Self::Artist(v) => has(&t.artist, v),
            Self::Album(v) => has(&t.album, v),
            Self::Format(v) => t.format() == *v,
            Self::Year(y) => t.year == Some(*y),
            Self::Rating(0) => mark().rating == 0,
            Self::Rating(n) => mark().rating >= *n,
            Self::Liked(l) => mark().liked == *l,
        }
    }
    fn sql(&self, args: &mut Vec<Box<dyn ToSql>>) -> String {
        let mut arg = |v: Box<dyn ToSql>| {
            args.push(v);
            format!("?{}", args.len())
        };
        match self {
            Self::Any(v) => {
                let p = arg(Box::new(v.clone()));
                format!("(instr(t.search, {p}) > 0 OR instr(s.path_key, {p}) > 0)")
            }
            Self::Title(v) => format!("instr(t.title_key, {}) > 0", arg(Box::new(v.clone()))),
            Self::Artist(v) => format!("instr(t.artist_key, {}) > 0", arg(Box::new(v.clone()))),
            Self::Album(v) => format!("instr(t.album_key, {}) > 0", arg(Box::new(v.clone()))),
            Self::Format(v) => format!("t.format = {}", arg(Box::new(v.clone()))),
            Self::Year(y) => format!("t.year = {}", arg(Box::new(*y))),
            Self::Rating(0) => "r.stars IS NULL".into(),
            Self::Rating(n) => format!("coalesce(r.stars, 0) >= {}", arg(Box::new(*n))),
            Self::Liked(true) => "l.track_id IS NOT NULL".into(),
            Self::Liked(false) => "l.track_id IS NULL".into(),
        }
    }
}

fn order_sql(order: Order) -> &'static str {
    match order {
        Order::Number => "t.number",
        Order::Title => "t.title_key",
        Order::Artist => "t.artist_key",
        Order::Album => "t.album_key",
        Order::Length => "t.duration",
        Order::Format => "t.format",
        Order::Disc => "t.disc_number",
        Order::Year => "t.year",
        Order::Path => "s.path_key",
        Order::Rating => "coalesce(r.stars, 0)",
        Order::Liked => "(l.track_id IS NOT NULL)",
    }
}

/// A disc or album of a collection view.
#[derive(Clone, Debug)]
pub struct AlbumRow {
    pub id: AlbumId,
    pub title: String,
    pub year: Option<u32>,
    /// The folder, relative to its collection.
    pub folder: PathBuf,
    pub cover: Option<PathBuf>,
    pub count: usize,
    pub duration: f64,
    pub complete: bool,
    pub format: String,
}

/// A materialized view: its ordered keys and their total duration.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub keys: Vec<i64>,
    pub duration: f64,
}

impl Store {
    /// The ordered keys of a view.
    #[cfg(test)]
    pub fn keys(&self, q: &Query) -> Result<Vec<i64>> {
        Ok(self.listing(q)?.keys)
    }
    pub fn listing(&self, q: &Query) -> Result<Listing> {
        let mut args: Vec<Box<dyn ToSql>> = vec![];
        let (key, from, mut filter, default) = match q.scope {
            Scope::Library(root) => (
                "t.id",
                "tracks t LEFT JOIN albums al ON al.id = t.album_id",
                vec![match root {
                    Some(root) => {
                        args.push(Box::new(root));
                        format!("t.root_id = ?{} AND t.present = 1", args.len())
                    }
                    None => "t.root_id IS NOT NULL AND t.present = 1".into(),
                }],
                "al.sort_key, t.ordinal, t.id",
            ),
            Scope::Album(album) => {
                args.push(Box::new(album));
                (
                    "t.id",
                    "tracks t",
                    vec![format!("t.album_id = ?{} AND t.present = 1", args.len())],
                    "t.ordinal, t.id",
                )
            }
            Scope::Liked => (
                "t.id",
                "likes lk JOIN tracks t ON t.id = lk.track_id",
                vec![],
                "lk.liked_at DESC, t.id",
            ),
            Scope::Playlist(p) => {
                args.push(Box::new(p));
                (
                    "e.id",
                    "playlist_entries e JOIN tracks t ON t.id = e.track_id",
                    vec![format!("e.playlist_id = ?{}", args.len())],
                    "e.position, e.id",
                )
            }
        };
        filter.extend(Term::parse(&q.search).iter().map(|t| t.sql(&mut args)));
        let order = match q.order {
            Some((o, desc)) => format!(
                "{} {}, {default}",
                order_sql(o),
                if desc { "DESC" } else { "ASC" }
            ),
            None => default.into(),
        };
        let sql = format!(
            "SELECT {key}, t.duration FROM {from}
             JOIN files s ON s.id = t.source
             LEFT JOIN ratings r ON r.track_id = t.id
             LEFT JOIN likes l ON l.track_id = t.id
             {} ORDER BY {order}",
            where_clause(&filter)
        );
        let mut stmt = self.db.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(args.iter()))?;
        let mut listing = Listing::default();
        while let Some(r) = rows.next()? {
            listing.keys.push(r.get(0)?);
            listing.duration += r.get::<_, Option<f64>>(1)?.unwrap_or(0.);
        }
        Ok(listing)
    }

    /// The rows for `keys`, in the same order. Keys that no longer exist are
    /// left out.
    pub fn rows(&self, scope: Scope, keys: &[i64]) -> Result<Vec<Row>> {
        let mut found: HashMap<i64, Row> = HashMap::with_capacity(keys.len());
        let (key, from, scope_filter) = if let Scope::Playlist(id) = scope {
            (
                "e.id",
                "playlist_entries e JOIN tracks t ON t.id = e.track_id",
                format!("AND e.playlist_id = {id}"),
            )
        } else {
            ("t.id", "tracks t", String::new())
        };
        for chunk in keys.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT {key}, t.id, s.path, a.path, a.kind, t.part, t.first_tick, t.last_tick,
                    t.title, t.artist, t.album, t.number, t.disc_number, t.year, t.duration,
                    t.present AND s.present AND (a.present OR a.id = s.id), t.root_id, s.root_id,
                    s.size, s.modified, s.volume, s.file_index, s.digest, s.sample
                 FROM {from} JOIN files s ON s.id = t.source JOIN files a ON a.id = t.anchor
                 WHERE {key} IN ({marks}) {scope_filter}"
            );
            let mut stmt = self.db.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
                let source = PathBuf::from(r.get::<_, String>(2)?);
                let mut track = Track::file(source);
                let part: i64 = r.get(5)?;
                if r.get::<_, i64>(4)? == 1 {
                    track.disc = Some(DiscTrack {
                        sheet: PathBuf::from(r.get::<_, String>(3)?),
                        number: part.max(0) as u8,
                        start: r.get::<_, Option<i64>>(6)?.unwrap_or(0) as u64,
                        end: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                    });
                } else {
                    track.number = r.get(11)?;
                }
                track.title = r.get(8)?;
                track.artist = r.get(9)?;
                track.album = r.get(10)?;
                track.disc_number = r.get(12)?;
                track.year = r.get(13)?;
                track.duration = r.get(14)?;
                Ok(Row {
                    key: r.get(0)?,
                    id: r.get(1)?,
                    track: Arc::new(track),
                    available: r.get(15)?,
                    root: r.get(16)?,
                    source_root: r.get(17)?,
                    revision: super::identity::Revision::columns(r, 18)?,
                })
            })?;
            for row in rows {
                let row = row?;
                found.insert(row.key, row);
            }
        }
        Ok(keys.iter().filter_map(|k| found.remove(k)).collect())
    }

    /// The track ids behind view keys, in order.
    pub fn track_ids(&self, scope: Scope, keys: &[i64]) -> Result<Vec<TrackId>> {
        let Scope::Playlist(playlist) = scope else {
            return Ok(keys.to_vec());
        };
        let mut lookup = self.db.prepare_cached(
            "SELECT track_id FROM playlist_entries WHERE playlist_id = ?1 AND id = ?2",
        )?;
        keys.iter()
            .map(|key| Ok(lookup.query_row(rusqlite::params![playlist, key], |row| row.get(0))?))
            .collect()
    }

    /// The discs of a collection view that hold matching tracks.
    pub fn albums(&self, root: Option<RootId>, search: &str) -> Result<Vec<AlbumRow>> {
        let mut args: Vec<Box<dyn ToSql>> = vec![];
        let mut filter = vec![match root {
            Some(root) => {
                args.push(Box::new(root));
                format!("t.root_id = ?{} AND t.present = 1", args.len())
            }
            None => "t.root_id IS NOT NULL AND t.present = 1".into(),
        }];
        filter.extend(Term::parse(search).iter().map(|t| t.sql(&mut args)));
        let sql = format!(
            "SELECT al.id, al.title, al.year, al.folder, al.cover, count(t.id),
                total(t.duration), count(t.duration) = count(t.id),
                CASE WHEN min(t.format) = max(t.format) THEN min(t.format) ELSE 'Mixed' END,
                ro.path
             FROM tracks t JOIN albums al ON al.id = t.album_id
             JOIN files s ON s.id = t.source
             LEFT JOIN roots ro ON ro.id = t.root_id
             LEFT JOIN ratings r ON r.track_id = t.id
             LEFT JOIN likes l ON l.track_id = t.id
             {} GROUP BY al.id
             ORDER BY coalesce(al.year, 4294967295), al.sort_key, al.id",
            where_clause(&filter)
        );
        let mut stmt = self.db.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let folder = PathBuf::from(r.get::<_, String>(3)?);
            let root: Option<String> = r.get(9)?;
            let folder = root
                .and_then(|root| folder.strip_prefix(root).ok().map(PathBuf::from))
                .unwrap_or(folder);
            Ok(AlbumRow {
                id: r.get(0)?,
                title: r.get(1)?,
                year: r.get(2)?,
                folder,
                cover: r.get::<_, Option<String>>(4)?.map(PathBuf::from),
                count: r.get::<_, i64>(5)? as usize,
                duration: r.get(6)?,
                complete: r.get(7)?,
                format: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn where_clause(filter: &[String]) -> String {
    if filter.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filter.join(" AND "))
    }
}

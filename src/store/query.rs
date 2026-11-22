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
use rusqlite::{OptionalExtension, ToSql, params_from_iter};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// One registered collection, or all of them.
    Library(Option<RootId>),
    Album(AlbumId),
    /// Every disc of the release whose first disc this is, in disc order.
    Release(AlbumId),
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
    ExactArtist(String),
    Album(String),
    Format(String),
    Year(u32),
    Rating(u8),
    Liked(bool),
}
impl Term {
    pub fn parse(search: &str) -> Vec<Self> {
        if let Some(name) = search.strip_prefix("artist:=").filter(|v| !v.is_empty()) {
            return vec![Self::ExactArtist(name.to_lowercase())];
        }
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
            Self::ExactArtist(v) => t.artist.to_lowercase() == *v,
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
            Self::ExactArtist(v) => format!("t.artist_key = {}", arg(Box::new(v.clone()))),
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
    }
}

/// A disc or album of a collection view.
#[derive(Clone, Debug)]
pub struct AlbumRow {
    pub id: AlbumId,
    pub title: String,
    pub year: Option<u32>,
    pub disc_number: Option<u32>,
    /// The folder, relative to its collection.
    pub folder: PathBuf,
    pub cover: Option<PathBuf>,
    /// A CUE program whose chapters all use the same audio image.
    pub cue_image: Option<PathBuf>,
    pub count: usize,
    pub duration: f64,
    pub complete: bool,
    pub format: String,
    /// Its place in a release of several discs.
    pub release: Option<AlbumRelease>,
}

/// A disc's place in a release, as [`crate::release`] finds it among the
/// discs of its collection.
#[derive(Clone, Debug, PartialEq)]
pub struct AlbumRelease {
    pub first: AlbumId,
    pub title: String,
    pub number: u32,
    pub subtitle: String,
}

/// Which discs an album query reads.
#[derive(Clone)]
enum Albums<'a> {
    Root(Option<RootId>),
    Ids(&'a [AlbumId]),
}

pub struct RegisteredDisc {
    pub root: RootId,
    pub album: AlbumId,
    pub title: String,
    pub track: TrackId,
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
        let release_order;
        let (key, from, mut filter, default) = match q.scope {
            Scope::Release(first) => {
                // Album ids come from the database, so they are inlined.
                let discs = self.release_discs(first)?;
                let ids = discs.iter().map(i64::to_string).collect::<Vec<_>>();
                release_order = format!(
                    "CASE t.album_id {} END, t.ordinal, coalesce(t.number, 4294967295), s.path_key, t.id",
                    ids.iter()
                        .enumerate()
                        .map(|(i, id)| format!("WHEN {id} THEN {i}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                (
                    "t.id",
                    "tracks t",
                    vec![format!(
                        "t.album_id IN ({}) AND t.present = 1",
                        ids.join(",")
                    )],
                    release_order.as_str(),
                )
            }
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
                "coalesce(al.year, 4294967295), al.sort_key, al.id, t.ordinal, coalesce(t.number, 4294967295), s.path_key, t.id",
            ),
            Scope::Album(album) => {
                args.push(Box::new(album));
                (
                    "t.id",
                    "tracks t",
                    vec![format!("t.album_id = ?{} AND t.present = 1", args.len())],
                    "t.ordinal, coalesce(t.number, 4294967295), s.path_key, t.id",
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

    /// Read keys and section counts from the same SQLite snapshot during indexing.
    pub fn table_listing(&self, query: &Query, grouped: bool) -> Result<(Listing, Vec<AlbumRow>)> {
        let snapshot = self.db.unchecked_transaction()?;
        let listing = self.listing(query)?;
        let sections = match query.scope {
            Scope::Library(root) if grouped => self.albums(root, &query.search)?,
            Scope::Release(first) if grouped => {
                let discs = self.release_discs(first)?;
                self.album_rows(Albums::Ids(&discs), &query.search)?
            }
            _ => vec![],
        };
        snapshot.commit()?;
        Ok((listing, sections))
    }

    pub fn registered_disc(&self, track: &Track) -> Result<Option<RegisteredDisc>> {
        let key = match track.library_id {
            Some(key) => Some(key),
            None => self.find(&super::Locator::of(track))?,
        };
        let Some(key) = key else { return Ok(None) };
        let disc = self.db.prepare_cached(
            "SELECT t.root_id, al.id, al.title FROM tracks t JOIN albums al ON al.id = t.album_id
             WHERE t.id = ?1 AND t.present"
        )?.query_row([key], |row| {
            let root: Option<RootId> = row.get(0)?;
            let album = row.get(1)?;
            let title = row.get(2)?;
            Ok(root.map(|root| RegisteredDisc { root, album, title, track: key }))
        }).optional()?;
        anyhow::ensure!(
            track.library_id.is_none() || disc.is_some(),
            "The selected recording is unavailable."
        );
        Ok(disc.flatten())
    }

    /// The tracks of a disc, or with [`Scope::Release`] of every disc of a
    /// release, in authored order.
    pub fn disc_keys(&self, scope: Scope) -> Result<Vec<i64>> {
        Ok(self
            .listing(&Query {
                scope,
                search: String::new(),
                order: None,
            })?
            .keys)
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
                    s.size, s.modified, s.volume, s.file_index, s.digest, s.sample, al.cover
                 FROM {from} JOIN files s ON s.id = t.source JOIN files a ON a.id = t.anchor
                 LEFT JOIN albums al ON al.id = t.album_id
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
                track.library_id = Some(r.get(1)?);
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
                    cover: r.get::<_, Option<String>>(24)?.map(PathBuf::from),
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
        self.album_rows(Albums::Root(root), search)
    }

    /// Whether an indexed disc belongs to a release of several discs.
    pub fn membership(&self, album: AlbumId) -> Result<Option<AlbumRelease>> {
        Ok(match self.album_root(album)? {
            Some(root) => self.releases(Some(root))?.remove(&album),
            None => None,
        })
    }

    /// The discs of one release in disc order, or the album alone.
    pub fn release_discs(&self, album: AlbumId) -> Result<Vec<AlbumId>> {
        let Some(root) = self.album_root(album)? else {
            return Ok(vec![album]);
        };
        let releases = self.releases(Some(root))?;
        let Some(first) = releases.get(&album).map(|r| r.first) else {
            return Ok(vec![album]);
        };
        let mut discs: Vec<_> = releases
            .iter()
            .filter(|(_, r)| r.first == first)
            .map(|(id, r)| (r.number, *id))
            .collect();
        discs.sort();
        Ok(discs.into_iter().map(|(_, id)| id).collect())
    }

    /// The release memberships of a collection's discs, from all of its
    /// available discs whatever a view's search shows.
    pub(super) fn releases(&self, root: Option<RootId>) -> Result<HashMap<AlbumId, AlbumRelease>> {
        let (filter, args): (&str, Vec<RootId>) = match root {
            Some(root) => ("al.root_id = ?1", vec![root]),
            None => ("al.root_id IS NOT NULL", vec![]),
        };
        // One indexed look-up per disc rather than a pass over every track.
        let sql = format!(
            "SELECT al.id, al.folder, al.title,
                (SELECT min(t.disc_number) FROM tracks t INDEXED BY tracks_album
                 WHERE t.album_id = al.id AND t.present = 1 AND t.root_id = al.root_id)
             FROM albums al
             WHERE {filter} AND EXISTS(SELECT 1 FROM tracks t INDEXED BY tracks_album
                 WHERE t.album_id = al.id AND t.present = 1 AND t.root_id = al.root_id)
             ORDER BY al.id"
        );
        let mut stmt = self.db.prepare_cached(&sql)?;
        let discs = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok((
                    r.get::<_, AlbumId>(0)?,
                    PathBuf::from(r.get::<_, String>(1)?),
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<u32>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let candidates: Vec<_> = discs
            .iter()
            .map(|(_, folder, title, number)| crate::release::Candidate {
                folder,
                title,
                number: *number,
            })
            .collect();
        let mut out = HashMap::new();
        for g in crate::release::releases(&candidates) {
            let first = discs[g.discs[0]].0;
            for (n, &i) in g.discs.iter().enumerate() {
                out.insert(
                    discs[i].0,
                    AlbumRelease {
                        first,
                        title: g.title.clone(),
                        number: n as u32 + 1,
                        subtitle: g.subtitles[n].clone(),
                    },
                );
            }
        }
        Ok(out)
    }

    fn album_rows(&self, albums: Albums, search: &str) -> Result<Vec<AlbumRow>> {
        let mut args: Vec<Box<dyn ToSql>> = vec![];
        let mut filter = vec![match &albums {
            Albums::Root(Some(root)) => {
                args.push(Box::new(*root));
                format!("t.root_id = ?{} AND t.present = 1", args.len())
            }
            Albums::Root(None) => "t.root_id IS NOT NULL AND t.present = 1".into(),
            Albums::Ids(ids) => format!(
                "t.album_id IN ({}) AND t.present = 1",
                ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            ),
        }];
        filter.extend(Term::parse(search).iter().map(|t| t.sql(&mut args)));
        // Section artwork belongs to the complete CUE, independent of
        // which chapters pass the outer query's filter.
        let sql = format!(
            "SELECT al.id, al.title, al.year, al.folder, al.cover, count(t.id),
                total(t.duration), count(t.duration) = count(t.id),
                CASE WHEN min(t.format) = max(t.format) THEN min(t.format) ELSE 'Mixed' END,
                ro.path, min(t.disc_number),
                CASE WHEN min(t.first_tick) IS NOT NULL THEN (
                    SELECT CASE WHEN min(ch.source) = max(ch.source) THEN min(audio.path) END
                    FROM tracks ch JOIN files audio ON audio.id = ch.source
                    WHERE ch.album_id = al.id AND ch.present = 1
                ) END
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
                disc_number: r.get(10)?,
                cover: r.get::<_, Option<String>>(4)?.map(PathBuf::from),
                cue_image: r.get::<_, Option<String>>(11)?.map(PathBuf::from),
                count: r.get::<_, i64>(5)? as usize,
                duration: r.get(6)?,
                complete: r.get(7)?,
                format: r.get(8)?,
                release: None,
            })
        })?;
        let mut rows: Vec<AlbumRow> = rows.collect::<rusqlite::Result<_>>()?;
        let mut releases = match albums {
            Albums::Root(root) => self.releases(root)?,
            Albums::Ids(ids) => {
                let order: HashMap<AlbumId, usize> =
                    ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
                rows.sort_by_key(|r| order.get(&r.id).copied());
                match ids.first() {
                    Some(&id) => match self.album_root(id)? {
                        Some(root) => self.releases(Some(root))?,
                        None => HashMap::new(),
                    },
                    None => HashMap::new(),
                }
            }
        };
        for row in &mut rows {
            row.release = releases.remove(&row.id);
        }
        Ok(rows)
    }
    fn album_root(&self, album: AlbumId) -> Result<Option<RootId>> {
        Ok(self
            .db
            .prepare_cached(
                "SELECT t.root_id FROM tracks t WHERE t.album_id = ?1 AND t.present = 1 LIMIT 1",
            )?
            .query_row([album], |r| r.get(0))
            .optional()?
            .flatten())
    }
}

fn where_clause(filter: &[String]) -> String {
    if filter.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filter.join(" AND "))
    }
}

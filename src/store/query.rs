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
use rusqlite::{OptionalExtension, ToSql, params, params_from_iter};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// One registered collection, or all of them.
    Library(Option<RootId>),
    CollectionFolder(RootId),
    Album(AlbumId),
    /// Every disc of the release whose first disc this is, in disc order.
    Release(AlbumId),
    Liked,
    RecentlyPlayed,
    Playlist(PlaylistId),
    /// Recordings with this performer credit.
    Artist(super::ArtistId),
    /// Recordings on releases associated with the artist.
    ArtistReleases(super::ArtistId),
    /// Recordings on releases in the global catalog.
    Releases,
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
    Plays,
    LastPlayed,
    ListeningTime,
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
        Order::Plays => "coalesce(lt.plays, 0)",
        Order::LastPlayed => "lt.last_played",
        Order::ListeningTime => "coalesce(lt.credited_us, 0)",
    }
}

/// A disc or album of a collection view.
#[derive(Clone, Debug)]
pub struct AlbumRow {
    pub id: AlbumId,
    pub root: Option<RootId>,
    pub title: String,
    pub album_artist: Option<String>,
    pub year: Option<u32>,
    pub disc_number: Option<u32>,
    /// The folder, relative to its collection.
    pub authored: bool,
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

/// The unfiltered collection shelf, without track rows, covers or durations.
pub struct AlbumStructure {
    pub id: AlbumId,
    pub title: String,
    pub is_disc: bool,
    pub release: Option<AlbumRelease>,
}

struct AlbumCandidate {
    id: AlbumId,
    folder: PathBuf,
    title: String,
    number: Option<u32>,
    authored: bool,
    release: Option<AlbumRelease>,
}

impl AlbumCandidate {
    fn is_release(&self) -> bool {
        self.authored
            || self.number.is_some_and(|number| number > 0)
            || self
                .folder
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::release::disc_folder)
                .is_some()
    }
}

fn joined_release(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Option<AlbumRelease>> {
    Ok(match r.get(at)? {
        Some(first) => Some(AlbumRelease {
            first,
            title: r.get(at + 1)?,
            number: r.get(at + 2)?,
            subtitle: r.get(at + 3)?,
        }),
        None => None,
    })
}

/// One page of the recordings a collection folder holds itself.
pub(crate) struct IndexedBranch {
    pub programs: Vec<AlbumRow>,
    pub loose: Vec<Arc<Track>>,
    /// Where the next page starts when more recordings follow.
    pub next: Option<ContentsCursor>,
}

/// A position among a folder's albums and, within a loose album, its tracks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContentsCursor {
    pub album: usize,
    pub track: usize,
}

/// Which discs an album query reads.
#[derive(Clone)]
enum Albums<'a> {
    Disc(AlbumId),
    Root(Option<RootId>),
    CollectionFolder(RootId),
    TreeFolders(RootId, &'a [PathBuf]),
    Release(&'a ReleaseContents),
    Artist(super::ArtistId),
    Performed(super::ArtistId),
    Catalog,
}

struct ReleaseContents {
    discs: Vec<AlbumId>,
}

enum ListingContext<'a> {
    FlatTable {
        child: Option<Scope>,
        albums: &'a [AlbumId],
    },
    Default,
    Release(&'a [AlbumId]),
    Collection(&'a HashMap<AlbumId, usize>),
}

pub struct RegisteredDisc {
    pub root: RootId,
    pub album: AlbumId,
    pub title: String,
    pub track: TrackId,
    pub release: Option<AlbumRelease>,
}

impl RegisteredDisc {
    pub(crate) fn shelf_key(&self) -> crate::table::RowKey {
        use crate::{collection::DiscId, table::RowKey};
        match &self.release {
            Some(release) => RowKey::Release(DiscId::Library(release.first)),
            None => RowKey::Disc(DiscId::Library(self.album)),
        }
    }
}

/// A materialized view: its ordered keys and their total duration.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub keys: Vec<i64>,
    pub duration: f64,
    /// Playlist occurrences have distinct keys. Keep their recording IDs in display order.
    pub playlist_tracks: Vec<TrackId>,
    /// First matching row of an explicitly requested child in a flat table.
    pub child_row: Option<(Scope, i64)>,
}

impl Store {
    /// The ordered keys of a view.
    #[cfg(test)]
    pub fn keys(&self, q: &Query) -> Result<Vec<i64>> {
        Ok(self.listing(q)?.keys)
    }
    pub fn listing(&self, q: &Query) -> Result<Listing> {
        self.listing_in_order(q, ListingContext::Default)
    }

    /// Follow the same structural order as the collection shelf, retaining
    /// its search and including loose recordings in authored collection play.
    pub fn collection_keys(&self, scope: Scope, search: String) -> Result<Vec<i64>> {
        let root = match scope {
            Scope::Library(root) => root,
            Scope::CollectionFolder(root) => Some(root),
            _ => anyhow::bail!("Expected a collection scope"),
        };
        let snapshot = self.db.unchecked_transaction()?;
        let albums = self.album_structure(root)?;
        let mut ranks = HashMap::with_capacity(albums.len());
        for item in crate::release::ordered(
            albums
                .iter()
                .map(|a| a.release.as_ref().map(|m| (m.first, m.number))),
        ) {
            match item {
                crate::release::Ordered::Disc(i) => {
                    ranks.insert(albums[i].id, ranks.len());
                }
                crate::release::Ordered::Release(discs) => {
                    for i in discs {
                        ranks.insert(albums[i].id, ranks.len());
                    }
                }
            }
        }
        let keys = self
            .listing_in_order(
                &Query {
                    scope,
                    search,
                    order: None,
                },
                ListingContext::Collection(&ranks),
            )?
            .keys;
        snapshot.commit()?;
        Ok(keys)
    }

    fn listing_in_order(&self, q: &Query, context: ListingContext<'_>) -> Result<Listing> {
        if q.scope == Scope::RecentlyPlayed && q.search.trim().is_empty() && q.order.is_none() {
            let mut stmt = self.db.prepare_cached(
                "SELECT recent.track_id, t.duration FROM listening_totals recent
                 CROSS JOIN tracks t ON t.id = recent.track_id
                 WHERE recent.last_played IS NOT NULL
                 ORDER BY recent.last_played DESC, recent.track_id",
            )?;
            let mut listing = Listing::default();
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                listing.keys.push(r.get(0)?);
                listing.duration += r.get::<_, Option<f64>>(1)?.unwrap_or(0.);
            }
            return Ok(listing);
        }
        let mut args: Vec<Box<dyn ToSql>> = vec![];
        let release_order;
        let (key, from, mut filter, default) = match q.scope {
            Scope::Release(first) => {
                // Album ids come from the database, so they are inlined.
                let resolved;
                let discs = if let ListingContext::Release(discs) = context {
                    discs
                } else {
                    resolved = self.release_discs(first)?;
                    &resolved
                };
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
            scope @ (Scope::Library(_)
            | Scope::Artist(_)
            | Scope::ArtistReleases(_)
            | Scope::Releases) => (
                "t.id",
                "tracks t LEFT JOIN albums al ON al.id = t.album_id",
                vec![match scope {
                    Scope::Library(Some(root)) => {
                        args.push(Box::new(root));
                        format!("t.root_id = ?{} AND t.present = 1", args.len())
                    }
                    Scope::Library(None) => "t.root_id IS NOT NULL AND t.present = 1".into(),
                    Scope::Releases => {
                        "t.root_id IS NOT NULL AND t.present = 1 AND al.is_release = 1".into()
                    }
                    Scope::Artist(id) => {
                        args.push(Box::new(id.0));
                        super::artists::tracks_of(args.len())
                    }
                    Scope::ArtistReleases(id) => {
                        args.push(Box::new(id.0));
                        let related = super::artists::albums_of(args.len());
                        format!(
                            "t.root_id IS NOT NULL AND t.present = 1 AND t.album_id IN ({related})"
                        )
                    }
                    _ => unreachable!(),
                }],
                "coalesce(al.year, 4294967295), al.sort_key, al.id, t.ordinal, coalesce(t.number, 4294967295), s.path_key, t.id",
            ),
            Scope::CollectionFolder(root) => {
                args.push(Box::new(root));
                (
                    "t.id",
                    "tracks t JOIN albums al ON al.id = t.album_id",
                    vec![COLLECTION_FOLDER.into()],
                    "al.sort_key, al.id, t.ordinal, coalesce(t.number, 4294967295), s.path_key, t.id",
                )
            }
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
            Scope::RecentlyPlayed => (
                "t.id",
                "listening_totals recent JOIN tracks t ON t.id = recent.track_id",
                vec!["recent.last_played IS NOT NULL".into()],
                "recent.last_played DESC, recent.track_id",
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
            "SELECT {key}, t.duration, t.album_id, t.id FROM {from}
             JOIN files s ON s.id = t.source
             LEFT JOIN ratings r ON r.track_id = t.id
             LEFT JOIN likes l ON l.track_id = t.id
             LEFT JOIN listening_totals lt ON lt.track_id = t.id
             {} ORDER BY {order}",
            where_clause(&filter)
        );
        let mut stmt = self.db.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(args.iter()))?;
        let mut listing = Listing::default();
        let mut ranked = Vec::new();
        while let Some(r) = rows.next()? {
            let key = r.get(0)?;
            if let ListingContext::FlatTable { child, albums } = context {
                if q.scope.keys_are_entries() {
                    listing.playlist_tracks.push(r.get(3)?);
                }
                if listing.child_row.is_none()
                    && let Some(child) = child
                    && let Some(album) = r.get::<_, Option<AlbumId>>(2)?
                    && albums.contains(&album)
                {
                    listing.child_row = Some((child, key));
                }
            }
            if let ListingContext::Collection(ranks) = context {
                let album: Option<AlbumId> = r.get(2)?;
                let rank = album
                    .and_then(|id| ranks.get(&id).copied())
                    .unwrap_or(usize::MAX);
                ranked.push((rank, key));
            } else {
                listing.keys.push(key);
            }
            listing.duration += r.get::<_, Option<f64>>(1)?.unwrap_or(0.);
        }
        if matches!(context, ListingContext::Collection(_)) {
            ranked.sort_by_key(|(rank, _)| *rank);
            listing.keys = ranked.into_iter().map(|(_, key)| key).collect();
        }
        Ok(listing)
    }

    /// Read keys and section counts from the same SQLite snapshot during indexing.
    pub fn table_listing(
        &self,
        query: &Query,
        grouped: bool,
        child: Option<Scope>,
    ) -> Result<(Listing, Vec<AlbumRow>)> {
        let snapshot = self.db.unchecked_transaction()?;
        let (listing, sections) = match query.scope {
            scope @ (Scope::Library(_) | Scope::CollectionFolder(_)) if grouped => (
                self.listing(query)?,
                self.collection_albums(scope, &query.search)?,
            ),
            Scope::Artist(id) if grouped => (
                self.listing(query)?,
                self.album_rows(Albums::Performed(id), &query.search)?,
            ),
            Scope::Release(first) if grouped => {
                let release = self.release_contents(first)?;
                (
                    self.listing_in_order(query, ListingContext::Release(&release.discs))?,
                    self.album_rows(Albums::Release(&release), &query.search)?,
                )
            }
            Scope::Album(id) if grouped => (
                self.listing(query)?,
                self.album_rows(Albums::Disc(id), &query.search)?,
            ),
            _ => {
                let albums = match child {
                    Some(Scope::Album(id)) => vec![id],
                    Some(Scope::Release(id)) => self.release_discs(id)?,
                    _ => Vec::new(),
                };
                (
                    self.listing_in_order(
                        query,
                        ListingContext::FlatTable {
                            child,
                            albums: &albums,
                        },
                    )?,
                    vec![],
                )
            }
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
        let disc = self
            .db
            .prepare_cached(
                "SELECT t.root_id, al.id, al.title, d.first_id, d.title, d.number, d.subtitle
             FROM tracks t JOIN albums al ON al.id = t.album_id
             LEFT JOIN release_discs d ON d.album_id = al.id
             WHERE t.id = ?1 AND t.present = 1",
            )?
            .query_row([key], |row| {
                let root: Option<RootId> = row.get(0)?;
                let album = row.get(1)?;
                let title = row.get(2)?;
                let release = joined_release(row, 3)?;
                Ok(root.map(|root| RegisteredDisc {
                    root,
                    album,
                    title,
                    track: key,
                    release,
                }))
            })
            .optional()?;
        anyhow::ensure!(
            track.library_id.is_none() || disc.is_some(),
            "The selected recording is unavailable."
        );
        Ok(disc.flatten())
    }

    /// Resolve one recording against the unfiltered view. A playlist keeps
    /// its own occurrence identity, even when other entries play the same track.
    pub(crate) fn reveal_key(
        &self,
        scope: Scope,
        track: TrackId,
        occurrence: Option<crate::queue::PlaylistOccurrence>,
    ) -> Result<Option<i64>> {
        if let Scope::Playlist(playlist) = scope {
            let exact = occurrence
                .filter(|entry| entry.playlist == playlist)
                .map(|entry| entry.entry);
            return Ok(self
                .db
                .prepare_cached(
                    "SELECT id FROM playlist_entries WHERE playlist_id = ?1 AND track_id = ?2
                   AND (?3 IS NULL OR id = ?3) ORDER BY position, id LIMIT 1",
                )?
                .query_row(params![playlist, track, exact], |r| r.get(0))
                .optional()?);
        }
        let (parameter, condition) = match scope {
            Scope::Library(None) => (0, "t.root_id IS NOT NULL".into()),
            Scope::Library(Some(root)) => (root, "t.root_id = ?1".into()),
            Scope::CollectionFolder(root) => (root, COLLECTION_FOLDER.into()),
            Scope::Album(album) => (album, "t.album_id = ?1".into()),
            Scope::Release(first) => (first, "coalesce(d.first_id, t.album_id) = ?1".into()),
            Scope::ArtistReleases(artist) => (artist.0, format!("t.root_id IS NOT NULL AND t.album_id IN ({})", super::artists::albums_of(1))),
            Scope::Artist(artist) => (artist.0, super::artists::tracks_of(1)),
            Scope::Releases => (0, "t.root_id IS NOT NULL AND al.is_release = 1".into()),
            Scope::Liked => (0, "EXISTS (SELECT 1 FROM likes WHERE track_id = t.id)".into()),
            Scope::RecentlyPlayed => (0, "EXISTS (SELECT 1 FROM listening_totals WHERE track_id = t.id AND last_played IS NOT NULL)".into()),
            Scope::Playlist(_) => unreachable!("playlists use entry identities"),
        };
        let query = format!(
            "SELECT t.id FROM tracks t
             LEFT JOIN albums al ON al.id = t.album_id
             LEFT JOIN release_discs d ON d.album_id = t.album_id
             WHERE t.id = ?2 AND t.present = 1 AND {condition}"
        );
        Ok(self
            .db
            .prepare_cached(&query)?
            .query_row(params![parameter, track], |r| r.get(0))
            .optional()?)
    }

    pub(crate) fn tree_album(&self, id: AlbumId) -> Result<Option<AlbumRow>> {
        Ok(self.album_rows(Albums::Disc(id), "")?.pop())
    }

    pub(crate) fn root_location(&self, id: RootId) -> Result<(PathBuf, String)> {
        Ok(self
            .db
            .prepare_cached("SELECT path, name FROM roots WHERE id = ?1")?
            .query_row([id], |r| {
                Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?))
            })?)
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

    pub(crate) fn disc_page(
        &self,
        id: AlbumId,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Row>, Option<usize>)> {
        let mut stmt = self.db.prepare_cached(
            "SELECT t.id FROM tracks t JOIN files s ON s.id = t.source
             WHERE t.album_id = ?1 AND t.present = 1
             ORDER BY t.ordinal, coalesce(t.number, 4294967295), s.path_key, t.id
             LIMIT ?2 OFFSET ?3",
        )?;
        let mut keys = stmt
            .query_map(params![id, limit as i64 + 1, offset as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = (keys.len() > limit).then_some(offset + limit);
        keys.truncate(limit);
        Ok((self.rows(Scope::Album(id), &keys)?, next))
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
                    s.size, s.modified, s.volume, s.file_index, s.digest, s.sample, al.cover, credit.name
                 FROM {from} JOIN files s ON s.id = t.source JOIN files a ON a.id = t.anchor
                 LEFT JOIN albums al ON al.id = t.album_id
                 LEFT JOIN artists credit ON credit.id = t.album_artist_id
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
                track.album_artist = r.get(25)?;
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

    pub fn collection_albums(&self, scope: Scope, search: &str) -> Result<Vec<AlbumRow>> {
        match scope {
            Scope::Library(root) => self.albums(root, search),
            Scope::CollectionFolder(root) => {
                self.album_rows(Albums::CollectionFolder(root), search)
            }
            Scope::ArtistReleases(id) => self.album_rows(Albums::Artist(id), search),
            Scope::Releases => self.album_rows(Albums::Catalog, search),
            _ => anyhow::bail!("Expected a collection scope"),
        }
    }

    /// Whether an indexed disc belongs to a release of several discs.
    #[cfg(test)]
    pub fn membership(&self, album: AlbumId) -> Result<Option<AlbumRelease>> {
        Ok(self
            .db
            .prepare_cached(
                "SELECT first_id, title, number, subtitle FROM release_discs WHERE album_id = ?1",
            )?
            .query_row([album], |r| joined_release(r, 0))
            .optional()?
            .flatten())
    }

    /// The discs of one release in disc order, or the album alone.
    pub fn release_discs(&self, album: AlbumId) -> Result<Vec<AlbumId>> {
        Ok(self.release_contents(album)?.discs)
    }

    fn release_contents(&self, album: AlbumId) -> Result<ReleaseContents> {
        let mut stmt = self.db.prepare_cached(
            "SELECT album_id FROM release_discs
             WHERE first_id = (SELECT first_id FROM release_discs WHERE album_id = ?1)
             ORDER BY number",
        )?;
        let mut discs = stmt
            .query_map([album], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if discs.is_empty() {
            discs.push(album);
        }
        Ok(ReleaseContents { discs })
    }

    pub(super) fn listening_release(
        &self,
        track: TrackId,
        album: Option<AlbumId>,
    ) -> Result<Option<(AlbumId, String)>> {
        if let Some(release) = self
            .db
            .query_row(
                "SELECT release_id,title FROM listening_release_evidence WHERE track_id=?1",
                [track],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        {
            return Ok(Some(release));
        }
        let Some(album) = album else { return Ok(None) };
        Ok((
            album,
            self.db
                .query_row("SELECT title FROM albums WHERE id=?1", [album], |r| {
                    r.get(0)
                })?,
        )
            .into())
    }

    pub(super) fn remember_listening_releases(
        &self,
        tx: &rusqlite::Transaction<'_>,
        root: RootId,
    ) -> Result<()> {
        let mut releases = std::collections::BTreeMap::new();
        let mut stmt = tx.prepare_cached(
            "SELECT d.album_id, d.first_id, d.title FROM release_discs d
             JOIN albums a ON a.id = d.album_id WHERE a.root_id = ?1",
        )?;
        for member in stmt.query_map([root], |r| {
            Ok((
                r.get::<_, AlbumId>(0)?,
                r.get::<_, AlbumId>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (album, first, title) = member?;
            releases
                .entry(first)
                .or_insert_with(Vec::new)
                .push((album, title));
        }
        for (first, discs) in releases {
            let mut known = std::collections::BTreeSet::new();
            for (album, _) in &discs {
                let mut stmt = tx.prepare_cached(
                    "SELECT DISTINCT e.release_id FROM tracks t
                     JOIN listening_release_evidence e ON e.track_id=t.id
                     WHERE t.album_id=?1 AND t.root_id=?2 AND t.present",
                )?;
                for id in stmt.query_map(params![album, root], |r| r.get::<_, AlbumId>(0))? {
                    known.insert(id?);
                }
            }
            // Earlier distinct releases can coexist after tag edits. Keep the
            // established assignments when new evidence is ambiguous.
            if known.len() > 1 {
                continue;
            }
            let release = known.first().copied().unwrap_or(first);
            for (album, title) in discs {
                tx.prepare_cached(
                    "INSERT INTO listening_release_evidence
                     SELECT id,?3,?4 FROM tracks WHERE album_id=?1 AND root_id=?2 AND present
                     ON CONFLICT(track_id) DO NOTHING",
                )?
                .execute(params![album, root, release, title])?;
                if album != release {
                    // Repair earlier standalone attribution only for recordings
                    // with evidence in this specific authored disc.
                    tx.prepare_cached(
                        "UPDATE listening_occurrences SET release_key=?3
                         WHERE track_id IN (
                            SELECT id FROM tracks WHERE album_id=?1 AND root_id=?2 AND present
                         ) AND release_key=?4",
                    )?
                    .execute(params![
                        album,
                        root,
                        format!("album:{release}"),
                        format!("album:{album}")
                    ])?;
                }
            }
        }
        Ok(())
    }

    pub fn navigation_albums(
        &self,
        root: Option<RootId>,
        album: AlbumId,
    ) -> Result<Option<(RootId, Vec<AlbumStructure>)>> {
        let root = match root {
            Some(root) => Some(root),
            None => self.album_root(album)?,
        };
        root.map(|root| Ok((root, self.album_structure(Some(root))?)))
            .transpose()
    }

    fn album_structure(&self, root: Option<RootId>) -> Result<Vec<AlbumStructure>> {
        Ok(Self::album_candidates(&self.db, root)?
            .into_iter()
            .map(|disc| {
                let is_disc = disc.is_release();
                AlbumStructure {
                    id: disc.id,
                    title: disc.title,
                    is_disc,
                    release: disc.release,
                }
            })
            .collect())
    }

    fn album_candidates(
        db: &rusqlite::Connection,
        root: Option<RootId>,
    ) -> Result<Vec<AlbumCandidate>> {
        let (filter, args): (&str, Vec<RootId>) = match root {
            Some(root) => ("al.root_id = ?1", vec![root]),
            None => ("al.root_id IS NOT NULL", vec![]),
        };
        // One indexed look-up per disc rather than a pass over every track.
        let sql = format!(
            "SELECT al.id, al.folder, al.title,
                (SELECT min(t.disc_number) FROM tracks t INDEXED BY tracks_album
                 WHERE t.album_id = al.id AND t.present = 1 AND t.root_id = al.root_id),
                al.key LIKE 'sheet:%' OR EXISTS(SELECT 1 FROM tracks t INDEXED BY tracks_album
                 WHERE t.album_id = al.id AND t.present = 1 AND t.root_id = al.root_id
                   AND trim(t.album) <> ''),
                d.first_id, d.title, d.number, d.subtitle
             FROM albums al LEFT JOIN release_discs d ON d.album_id = al.id
             WHERE {filter} AND EXISTS(SELECT 1 FROM tracks t INDEXED BY tracks_album
                 WHERE t.album_id = al.id AND t.present = 1 AND t.root_id = al.root_id)
             ORDER BY coalesce(al.year, 4294967295), al.sort_key, al.id"
        );
        let mut stmt = db.prepare_cached(&sql)?;
        let discs = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(AlbumCandidate {
                    id: r.get(0)?,
                    folder: PathBuf::from(r.get::<_, String>(1)?),
                    title: r.get(2)?,
                    number: r.get(3)?,
                    authored: r.get(4)?,
                    release: joined_release(r, 5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(discs)
    }

    pub(super) fn refresh_release_index(
        tx: &rusqlite::Transaction<'_>,
        root: RootId,
    ) -> Result<()> {
        tx.execute(
            "UPDATE albums AS a SET is_release = 0, album_artist_id = (
                SELECT min(t.album_artist_id) FROM tracks t
                WHERE t.album_id = a.id AND t.root_id = a.root_id AND t.present = 1
                HAVING min(t.album_artist_id) = max(t.album_artist_id)
             ) WHERE a.root_id = ?1",
            [root],
        )?;
        tx.execute(
            "DELETE FROM release_discs WHERE album_id IN (SELECT id FROM albums WHERE root_id = ?1)",
            [root],
        )?;
        let discs = Self::album_candidates(tx, Some(root))?;
        let mut mark = tx.prepare_cached("UPDATE albums SET is_release = 1 WHERE id = ?1")?;
        for disc in &discs {
            if disc.is_release() {
                mark.execute([disc.id])?;
            }
        }
        let candidates: Vec<_> = discs
            .iter()
            .map(|disc| crate::release::Candidate {
                folder: &disc.folder,
                title: &disc.title,
                number: disc.number,
            })
            .collect();
        let mut insert = tx.prepare_cached(
            "INSERT INTO release_discs(album_id,first_id,title,number,subtitle)
             VALUES(?1,?2,?3,?4,?5)",
        )?;
        for g in crate::release::releases(&candidates) {
            let first = discs[g.discs[0]].id;
            for (n, &i) in g.discs.iter().enumerate() {
                insert.execute(params![
                    discs[i].id,
                    first,
                    g.title,
                    n as u32 + 1,
                    g.subtitles[n]
                ])?;
            }
        }
        Ok(())
    }

    /// The musical programs and loose recordings owned by this directory,
    /// at most `limit` of them from `at`. Named CD siblings can form a
    /// release when the catalog confirms they account for every program
    /// under the directory. Every statement is bounded by `limit`.
    pub(crate) fn tree_folder(
        &self,
        root: RootId,
        path: &Path,
        at: ContentsCursor,
        limit: usize,
    ) -> Result<IndexedBranch> {
        let limit = limit.max(1);
        let snapshot = self.db.unchecked_transaction()?;
        let mut folders = vec![path.to_owned()];
        let page = Some((limit + 1, at.album));
        let mut albums = self.album_page(Albums::TreeFolders(root, &folders), "", page)?;
        if at == ContentsCursor::default() && albums.is_empty() {
            let dirs = self.disc_candidates(root, path)?;
            if !dirs.is_empty() {
                folders = dirs;
                albums = self.album_page(Albums::TreeFolders(root, &folders), "", page)?;
            }
        }
        let more_albums = albums.len() > limit;
        albums.truncate(limit);
        let mut programs = Vec::new();
        let mut loose = Vec::new();
        let mut next = None;
        for (index, album) in albums.into_iter().enumerate() {
            let position = at.album + index;
            if programs.len() + loose.len() >= limit {
                next = Some(ContentsCursor {
                    album: position,
                    track: 0,
                });
                break;
            }
            if album.authored
                || album
                    .folder
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(crate::release::disc_folder)
                    .is_some()
            {
                programs.push(album);
                continue;
            }
            let first = if position == at.album { at.track } else { 0 };
            let room = limit - programs.len() - loose.len();
            let (rows, continuation) = self.disc_page(album.id, first, room)?;
            loose.extend(
                rows.into_iter()
                    .filter(|row| row.available)
                    .map(|row| row.track),
            );
            if let Some(continuation) = continuation {
                next = Some(ContentsCursor {
                    album: position,
                    track: continuation,
                });
                break;
            }
        }
        if next.is_none() && more_albums {
            next = Some(ContentsCursor {
                album: at.album + limit,
                track: 0,
            });
        }
        snapshot.commit()?;
        Ok(IndexedBranch {
            programs,
            loose,
            next,
        })
    }

    pub(crate) fn visit_matching_folders(
        &self,
        root: Option<RootId>,
        search: &str,
        mut visit: impl FnMut(PathBuf) -> Result<()>,
    ) -> Result<()> {
        let mut args: Vec<Box<dyn ToSql>> = vec![];
        let mut filter = vec!["t.present = 1".to_owned()];
        match root {
            Some(root) => {
                args.push(Box::new(root));
                filter.push(format!("t.root_id = ?{}", args.len()));
            }
            None => filter.push("t.root_id IS NOT NULL".into()),
        }
        let terms = Term::parse(search);
        if terms.is_empty() {
            return Ok(());
        }
        filter.extend(terms.iter().map(|t| t.sql(&mut args)));
        let sql = format!(
            "SELECT DISTINCT al.folder FROM tracks t JOIN albums al ON al.id = t.album_id
             JOIN files s ON s.id = t.source
             LEFT JOIN ratings r ON r.track_id = t.id
             LEFT JOIN likes l ON l.track_id = t.id
             {}",
            where_clause(&filter)
        );
        let mut stmt = self.db.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(args.iter()))?;
        while let Some(row) = rows.next()? {
            visit(PathBuf::from(row.get::<_, String>(0)?))?;
        }
        Ok(())
    }

    /// Named CD folders directly below `path` that hold every indexed
    /// program under it, so they can form one release. At most 32 are read.
    fn disc_candidates(&self, root: RootId, path: &Path) -> Result<Vec<PathBuf>> {
        let sep = std::path::MAIN_SEPARATOR;
        let prefix = format!("{}{sep}", super::path_key(path).trim_end_matches(sep));
        let next = char::from_u32(u32::from(sep) + 1).expect("path separator");
        let upper = format!("{}{next}", prefix.trim_end_matches(sep));
        let mut statement = self.db.prepare_cached(
            "SELECT DISTINCT substr(al.folder, length(?2) + 1,
                        instr(substr(al.folder, length(?2) + 1) || ?4, ?4) - 1) AS child
             FROM albums al WHERE al.root_id = ?1 AND al.sort_key >= ?2 AND al.sort_key < ?3
               AND child <> '' AND EXISTS(SELECT 1 FROM tracks t
                   WHERE t.album_id = al.id AND t.root_id = al.root_id AND t.present)
             ORDER BY child COLLATE NOCASE LIMIT 33",
        )?;
        let dirs: Vec<PathBuf> = statement
            .query_map(params![root, prefix, upper, sep.to_string()], |r| {
                Ok(path.join(r.get::<_, String>(0)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let named = (2..=32).contains(&dirs.len())
            && dirs.iter().all(|dir| {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(crate::release::disc_folder)
                    .is_some()
            });
        if !named {
            return Ok(Vec::new());
        }
        let count: i64 = self.db.query_row(
            "SELECT count(*) FROM albums al
             WHERE al.root_id = ?1 AND al.sort_key >= ?2 AND al.sort_key < ?3
               AND EXISTS(SELECT 1 FROM tracks t WHERE t.album_id = al.id AND t.root_id = al.root_id AND t.present)",
            params![root, prefix, upper],
            |r| r.get(0),
        )?;
        Ok(if count == dirs.len() as i64 {
            dirs
        } else {
            Vec::new()
        })
    }

    fn album_rows(&self, albums: Albums, search: &str) -> Result<Vec<AlbumRow>> {
        self.album_page(albums, search, None)
    }

    /// Album rows, at most `page.0` of them after skipping `page.1`.
    fn album_page(
        &self,
        albums: Albums,
        search: &str,
        page: Option<(usize, usize)>,
    ) -> Result<Vec<AlbumRow>> {
        let mut rows = Vec::new();
        self.visit_albums(albums.clone(), search, page, false, |row| {
            rows.push(row);
            Ok(())
        })?;
        if matches!(albums, Albums::Release(_)) {
            rows.sort_by_key(|row| row.release.as_ref().map(|r| r.number));
        }
        Ok(rows)
    }

    pub(crate) fn visit_releases(
        &self,
        search: &str,
        visit: impl FnMut(AlbumRow) -> Result<()>,
    ) -> Result<()> {
        self.visit_albums(Albums::Catalog, search, None, true, visit)
    }

    fn visit_albums(
        &self,
        albums: Albums,
        search: &str,
        page: Option<(usize, usize)>,
        families: bool,
        mut visit: impl FnMut(AlbumRow) -> Result<()>,
    ) -> Result<()> {
        let mut args: Vec<Box<dyn ToSql>> = vec![];
        let mut filter = vec![match &albums {
            Albums::Disc(id) => {
                args.push(Box::new(*id));
                format!("t.album_id = ?{} AND t.present = 1", args.len())
            }
            Albums::Root(Some(root)) => {
                args.push(Box::new(*root));
                format!("t.root_id = ?{} AND t.present = 1", args.len())
            }
            Albums::Root(None) => "t.root_id IS NOT NULL AND t.present = 1".into(),
            Albums::Catalog => {
                "t.root_id IS NOT NULL AND t.present = 1 AND al.is_release = 1".into()
            }
            Albums::CollectionFolder(root) => {
                args.push(Box::new(*root));
                COLLECTION_FOLDER.into()
            }
            Albums::TreeFolders(root, folders) => {
                args.push(Box::new(*root));
                let values = folders
                    .iter()
                    .map(|path| {
                        args.push(Box::new(super::path_key(path)));
                        format!("?{}", args.len())
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "t.root_id = ?1 AND al.root_id = ?1 AND t.present = 1 AND
                    substr(al.sort_key, 1, instr(al.sort_key, char(1)) - 1) IN ({values})"
                )
            }
            Albums::Release(release) => format!(
                "t.album_id IN ({}) AND t.present = 1",
                release
                    .discs
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Albums::Artist(artist) => {
                args.push(Box::new(artist.0));
                let albums = super::artists::albums_of(args.len());
                format!("t.present = 1 AND t.root_id IS NOT NULL AND t.album_id IN ({albums})")
            }
            Albums::Performed(artist) => {
                args.push(Box::new(artist.0));
                super::artists::tracks_of(args.len())
            }
        }];
        filter.extend(Term::parse(search).iter().map(|t| t.sql(&mut args)));
        // Section artwork belongs to the complete CUE, independent of
        // which chapters pass the outer query's filter.
        let family_keys = if families {
            ", coalesce(d.first_id, al.id) AS family, coalesce(d.number, 0) AS member,
            row_number() OVER (ORDER BY coalesce(al.year, 4294967295), al.sort_key, al.id) AS ordinal"
        } else {
            ""
        };
        let sql = format!(
            "SELECT al.id, al.title, al.year, al.folder, al.cover, count(t.id),
                total(t.duration), count(t.duration) = count(t.id),
                CASE WHEN min(t.format) = max(t.format) THEN min(t.format) ELSE 'Mixed' END,
                ro.path, min(t.disc_number), ro.id,
                CASE WHEN min(t.first_tick) IS NOT NULL THEN (
                    SELECT CASE WHEN min(ch.source) = max(ch.source) THEN min(audio.path) END
                    FROM tracks ch JOIN files audio ON audio.id = ch.source
                    WHERE ch.album_id = al.id AND ch.present = 1
                ) END,
                al.key LIKE 'sheet:%' OR max(trim(t.album) <> '') OR max(coalesce(t.disc_number,0)) > 0,
                credit.name, d.first_id, d.title, d.number, d.subtitle{family_keys}
             FROM tracks t JOIN albums al ON al.id = t.album_id
             LEFT JOIN release_discs d ON d.album_id = al.id
             LEFT JOIN artists credit ON credit.id = al.album_artist_id
             JOIN files s ON s.id = t.source
             LEFT JOIN roots ro ON ro.id = t.root_id
             LEFT JOIN ratings r ON r.track_id = t.id
             LEFT JOIN likes l ON l.track_id = t.id
             {} GROUP BY al.id
             ORDER BY coalesce(al.year, 4294967295), al.sort_key, al.id{}",
            where_clause(&filter),
            page.map_or_else(String::new, |(limit, offset)| format!(
                " LIMIT {limit} OFFSET {offset}"
            )),
        );
        // Visit complete families in the position of their first numbered
        // member, matching the shelf's default order even for filtered CDs.
        let sql = if families {
            format!("WITH discs AS ({sql}) SELECT * FROM discs ORDER BY
                first_value(ordinal) OVER (PARTITION BY family ORDER BY member, ordinal), member, ordinal")
        } else {
            sql
        };
        let mut stmt = self.db.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let folder = PathBuf::from(r.get::<_, String>(3)?);
            let root: Option<String> = r.get(9)?;
            let folder = root
                .filter(|_| !matches!(albums, Albums::TreeFolders(..)))
                .and_then(|root| folder.strip_prefix(root).ok().map(PathBuf::from))
                .unwrap_or(folder);
            Ok(AlbumRow {
                id: r.get(0)?,
                root: r.get(11)?,
                title: r.get(1)?,
                album_artist: r.get(14)?,
                year: r.get(2)?,
                folder,
                authored: r.get(13)?,
                disc_number: r.get(10)?,
                cover: r.get::<_, Option<String>>(4)?.map(PathBuf::from),
                cue_image: r.get::<_, Option<String>>(12)?.map(PathBuf::from),
                count: r.get::<_, i64>(5)? as usize,
                duration: r.get(6)?,
                complete: r.get(7)?,
                format: r.get(8)?,
                release: if matches!(albums, Albums::TreeFolders(..)) {
                    None
                } else {
                    joined_release(r, 15)?
                },
            })
        })?;
        for row in rows {
            visit(row?)?;
        }
        Ok(())
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

// The album directory belongs to the logical program's anchor, which can
// differ from the source directory for a multi-file CUE.
const COLLECTION_FOLDER: &str = "t.root_id = ?1 AND t.present = 1
    AND substr(al.sort_key, 1, instr(al.sort_key, char(1)) - 1)
        = (SELECT path_key FROM roots WHERE id = ?1)";

fn where_clause(filter: &[String]) -> String {
    if filter.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filter.join(" AND "))
    }
}

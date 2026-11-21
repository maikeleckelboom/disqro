//! Ranked library lookup. FTS rowids are the catalog's durable ids. Resolve
//! display metadata in the same read transaction as the match.
use super::{AlbumId, PlaylistId, RootId, Scope, Store, TrackId};
use anyhow::Result;
use rusqlite::params;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Track(TrackId),
    Artist(String),
    Album(RootId, AlbumId),
    Release(RootId, AlbumId),
    Playlist(PlaylistId),
    Collection(RootId),
}
impl Target {
    pub fn group(&self) -> &'static str {
        match self {
            Self::Track(_) => "Tracks",
            Self::Artist(_) => "Artists",
            Self::Album(..) => "Albums and CDs",
            Self::Release(..) => "Releases",
            Self::Playlist(_) => "Playlists",
            Self::Collection(_) => "Collections",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Hit {
    pub target: Target,
    pub title: String,
    pub detail: String,
}
#[derive(Default)]
pub struct Matches {
    pub hits: Vec<Hit>,
    pub more: bool,
}

impl Store {
    pub fn search_library(&self, text: &str, limit: usize, offset: usize) -> Result<Matches> {
        let expression = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(16)
            .map(|word| format!("\"{word}\"*"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut result = Matches::default();
        if expression.is_empty() {
            return Ok(result);
        }
        let snapshot = self.db.unchecked_transaction()?;
        let args = params![
            expression,
            i64::try_from(limit)? + 1,
            i64::try_from(offset)?
        ];
        let mut ids: Vec<i64> = self
            .db
            .prepare_cached(
                "SELECT t.id FROM tracks_fts(?1) JOIN tracks t ON t.id=tracks_fts.rowid
             WHERE t.present=1 AND t.root_id IS NOT NULL
             ORDER BY rank, t.id LIMIT ?2 OFFSET ?3",
            )?
            .query_map(args, |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        result.more = ids.len() > limit;
        ids.truncate(limit);
        for row in self.rows(Scope::Library(None), &ids)? {
            let t = row.track;
            let source = t.disc.as_ref().map_or_else(
                || t.source.display().to_string(),
                |d| format!("Chapter {} · {}", d.number, d.sheet.display()),
            );
            result.hits.push(Hit {
                target: Target::Track(row.id),
                title: t.title.clone(),
                detail: [t.artist.as_str(), t.album.as_str(), &source]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            });
        }
        let artists = self
            .db
            .prepare_cached(
                "SELECT min(t.artist) FROM tracks_fts('artist : (' || ?1 || ')')
             JOIN tracks t ON t.id=tracks_fts.rowid WHERE t.present=1 AND t.root_id IS NOT NULL
             GROUP BY t.artist_key ORDER BY t.artist_key LIMIT ?2 OFFSET ?3",
            )?
            .query_map(args, |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.more |= artists.len() > limit;
        result
            .hits
            .extend(artists.into_iter().take(limit).map(|name| Hit {
                target: Target::Artist(name.clone()),
                title: name,
                detail: String::new(),
            }));
        let album_sql = "SELECT a.root_id, a.id, a.title, a.folder
            FROM albums_fts(?1) JOIN albums a ON a.id = albums_fts.rowid
            WHERE a.root_id IS NOT NULL AND EXISTS (
                SELECT 1 FROM tracks t INDEXED BY tracks_album
                WHERE t.album_id = a.id AND t.present = 1 AND t.root_id = a.root_id
            ) ORDER BY rank, a.id LIMIT ?2 OFFSET ?3";
        let albums = self
            .db
            .prepare_cached(album_sql)?
            .query_map(args, |r| {
                Ok((
                    r.get::<_, RootId>(0)?,
                    r.get::<_, AlbumId>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.more |= albums.len() > limit;
        let mut roots = HashMap::new();
        let mut releases = Vec::new();
        let mut seen = HashSet::new();
        for (root, id, title, detail) in albums.into_iter().take(limit) {
            if let std::collections::hash_map::Entry::Vacant(entry) = roots.entry(root) {
                entry.insert(self.releases(Some(root))?);
            }
            if let Some(release) = roots[&root].get(&id)
                && seen.insert(release.first)
            {
                releases.push(Hit {
                    target: Target::Release(root, release.first),
                    title: release.title.clone(),
                    detail: String::new(),
                });
            }
            result.hits.push(Hit {
                target: Target::Album(root, id),
                title,
                detail,
            });
        }
        result.hits.extend(releases);
        for (index, playlist) in [("playlists_fts", true), ("roots_fts", false)] {
            let sql = format!(
                "SELECT rowid,name FROM {index}(?1) ORDER BY rank,rowid LIMIT ?2 OFFSET ?3"
            );
            let names = self
                .db
                .prepare_cached(&sql)?
                .query_map(args, |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result.more |= names.len() > limit;
            result
                .hits
                .extend(names.into_iter().take(limit).map(|(id, title)| Hit {
                    target: if playlist {
                        Target::Playlist(id)
                    } else {
                        Target::Collection(id)
                    },
                    title,
                    detail: String::new(),
                }));
        }
        snapshot.commit()?;
        Ok(result)
    }
}

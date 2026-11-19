//! Indexing a registered collection, in three steps:
//!
//! 1. **Walk.** The walker that browses folders finds every playable track,
//!    with the same CUE program deduplication. Nothing is written yet.
//! 2. **Reconcile.** Known paths keep their identities. Files the walk did
//!    not find become missing, unless their folder or sheet could not be
//!    read. A new path takes over a missing file's identity only with
//!    verified evidence and a one-to-one match. Anything ambiguous becomes a
//!    new identity, and the missing one stays missing together with its
//!    likes, ratings and playlist entries.
//! 3. **Enrich.** Tags, durations and identity evidence are read for new or
//!    changed files in small batches, after the collection is browsable.
//!
//! Nothing here writes to music files or sheets.
use super::{AlbumId, RootId, Store, now, path_key};
use crate::{
    cache::{Cache, Stamp},
    library::{self, Track},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

const AUDIO: i64 = 0;
const SHEET: i64 = 1;
/// Tracks written per transaction, so browsing and user actions never wait
/// long for the indexer's write lock.
const CHUNK: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub tracks: usize,
    pub added: usize,
    pub missing: usize,
    pub relocated: usize,
    pub ambiguous: usize,
    /// False when part of the collection could not be read; nothing under
    /// those folders was marked missing.
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub enum Progress {
    Walking,
    /// The collection is browsable; headers are still being read.
    Indexed(Summary),
    Reading {
        done: usize,
        total: usize,
    },
}

/// Evidence that a file at a new path is a file the library already knows.
#[derive(Clone, Debug, PartialEq)]
struct Evidence {
    volume: Option<i64>,
    index: Option<i64>,
    digest: i64,
}

struct FileInfo {
    path: PathBuf,
    key: String,
    name: String,
    kind: i64,
    size: i64,
    modified: i64,
    /// The collection whose folder contains the file.
    owner: Option<RootId>,
}
impl FileInfo {
    fn read(path: &Path, kind: i64, root: Option<(&str, RootId)>) -> Option<Self> {
        let stamp = Stamp::read(path)?;
        path.to_str()?;
        let key = path_key(path);
        let owner = root.and_then(|(prefix, id)| inside(prefix, &key).then_some(id));
        Some(Self {
            name: library::folder_name(path),
            path: path.to_owned(),
            key,
            kind,
            size: stamp.bytes,
            modified: stamp.modified,
            owner,
        })
    }
}

fn inside(folder: &str, key: &str) -> bool {
    key.strip_prefix(folder)
        .is_some_and(|rest| rest.starts_with(['\\', '/']) || folder.ends_with(['\\', '/']))
}

/// One anchor and the tracks it anchors, in program order.
struct Group {
    anchor: PathBuf,
    sheet: bool,
    tracks: Vec<Arc<Track>>,
}
fn groups(tracks: Vec<Arc<Track>>) -> Vec<Group> {
    let mut at = HashMap::new();
    let mut out: Vec<Group> = vec![];
    for t in tracks {
        let (anchor, sheet) = match &t.disc {
            Some(d) => (d.sheet.clone(), true),
            None => (t.source.clone(), false),
        };
        let i = *at.entry(anchor.clone()).or_insert_with(|| {
            out.push(Group {
                anchor,
                sheet,
                tracks: vec![],
            });
            out.len() - 1
        });
        out[i].tracks.push(t);
    }
    out
}

/// The part code of each track in a program: 0 for a whole file, the
/// chapter number, or minus the number of the chapter a lead-in precedes.
fn parts(group: &Group) -> Vec<i64> {
    let t = &group.tracks;
    (0..t.len())
        .map(|i| match &t[i].disc {
            None => 0,
            Some(d) if d.number > 0 => i64::from(d.number),
            Some(_) => -t[i + 1..]
                .iter()
                .find_map(|n| n.disc.as_ref().map(|d| i64::from(d.number)))
                .unwrap_or(100),
        })
        .collect()
}

fn digest(bytes: &[u8], size: u64) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in size.to_le_bytes().iter().chain(bytes) {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash as i64
}

/// Reads the file's identity on its volume and a digest of its first 64 KiB
/// (all of a sheet). A file that changed since `expected` was taken gives none.
fn evidence(path: &Path, kind: i64, expected: (i64, i64)) -> Option<Evidence> {
    let file = fs::File::open(path).ok()?;
    let (volume, index) = file_identity(&file);
    let limit = if kind == SHEET { 1 << 20 } else { 64 << 10 };
    let mut head = Vec::with_capacity(limit as usize);
    file.take(limit).read_to_end(&mut head).ok()?;
    let stamp = Stamp::read(path)?;
    ((stamp.bytes, stamp.modified) == expected).then(|| Evidence {
        volume,
        index,
        digest: digest(&head, expected.0 as u64),
    })
}

#[cfg(windows)]
fn file_identity(file: &fs::File) -> (Option<i64>, Option<i64>) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle belongs to `file`, which outlives the call.
    match unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) } {
        Ok(()) => (
            Some(i64::from(info.dwVolumeSerialNumber)),
            Some(((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)) as i64),
        ),
        Err(_) => (None, None),
    }
}
#[cfg(unix)]
fn file_identity(file: &fs::File) -> (Option<i64>, Option<i64>) {
    use std::os::unix::fs::MetadataExt;
    file.metadata().map_or((None, None), |m| {
        (Some(m.dev() as i64), Some(m.ino() as i64))
    })
}

/// A file the library knows that this scan did not find.
struct Missing {
    id: i64,
    kind: i64,
    size: i64,
    modified: i64,
    name: String,
    path: PathBuf,
    volume: Option<i64>,
    index: Option<i64>,
    digest: Option<i64>,
}
impl Missing {
    // A content sample identifies possible copies, never a relocation on its own.
    fn plausible(&self, new: &FileInfo, e: &Evidence) -> bool {
        self.kind == new.kind
            && self.size == new.size
            && self.digest == Some(e.digest)
            && (self.modified == new.modified || self.name.eq_ignore_ascii_case(&new.name))
    }

    fn verified(&self, new: &FileInfo, e: &Evidence) -> bool {
        e.volume.is_some()
            && e.index.is_some_and(|id| id != 0)
            && (self.volume, self.index) == (e.volume, e.index)
            && self.modified == new.modified
            && self.plausible(new, e)
    }
}

fn scan_transaction<'db>(
    db: &'db mut Connection,
    root: RootId,
    generation: i64,
    alive: &dyn Fn() -> bool,
) -> Result<Transaction<'db>> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM roots WHERE id = ?1 AND generation = ?2)",
        params![root, generation],
        |row| row.get(0),
    )?;
    if !current || !alive() {
        return Err(super::worker::Cancelled.into());
    }
    Ok(tx)
}

/// Indexes a registered collection. `alive` turning false cancels the scan;
/// a cancelled or partly unreadable scan never marks anything missing.
pub fn index_root(
    store: &mut Store,
    root: RootId,
    cache: Option<&Cache>,
    alive: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let db = &mut store.db;
    let (path, root_key): (String, String) = db
        .query_row(
            "SELECT path, path_key FROM roots WHERE id = ?1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .context("The collection is no longer registered")?;
    let path = PathBuf::from(path);
    ensure!(path.is_dir(), "{} is not available", path.display());
    let generation: i64 = db.query_row(
        "UPDATE roots SET generation = ?2 WHERE id = ?1 RETURNING generation",
        params![root, fastrand::i64(1..i64::MAX)],
        |r| r.get(0),
    )?;
    progress(Progress::Walking);
    let found = walk(&path, cache, alive).ok_or(super::worker::Cancelled)?;
    let mut uncertain = found.failed_sheets;
    // Every file the walk found, with its size and time.
    let groups = groups(found.tracks);
    let owner = Some((root_key.as_str(), root));
    let mut files: HashMap<String, FileInfo> = HashMap::new();
    let mut by_path: HashMap<PathBuf, String> = HashMap::new();
    for g in &groups {
        let anchor_kind = if g.sheet { SHEET } else { AUDIO };
        let paths = std::iter::once((&g.anchor, anchor_kind))
            .chain(g.tracks.iter().map(|t| (&t.source, AUDIO)));
        for (p, kind) in paths {
            if by_path.contains_key(p) {
                continue;
            }
            if let Some(info) = FileInfo::read(p, kind, owner) {
                by_path.insert(p.clone(), info.key.clone());
                files.entry(info.key.clone()).or_insert(info);
            } else {
                uncertain.insert(path_key(p));
            }
        }
    }

    let complete = found.incomplete.is_empty() && uncertain.is_empty();
    let unreadable = |key: &str| {
        uncertain.contains(key) || found.incomplete.iter().any(|d| d == key || inside(d, key))
    };
    // Known paths, and the collection's files this walk did not find.
    let mut known = HashSet::new();
    {
        let mut find = db.prepare_cached("SELECT id FROM files WHERE path_key = ?1")?;
        for key in files.keys() {
            if find.exists([key])? {
                known.insert(key.clone());
            }
        }
    }
    let mut missing: HashMap<(i64, i64), Vec<Missing>> = HashMap::new();
    {
        let mut stmt = db.prepare(
            "SELECT id, kind, size, modified, name, volume, file_index, digest, path_key, path
             FROM files WHERE root_id = ?1 AND present = 1",
        )?;
        let rows = stmt.query_map([root], |r| {
            Ok((
                Missing {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    size: r.get(2)?,
                    modified: r.get(3)?,
                    name: r.get(4)?,
                    volume: r.get(5)?,
                    index: r.get(6)?,
                    digest: r.get(7)?,
                    path: PathBuf::from(r.get::<_, String>(9)?),
                },
                r.get::<_, String>(8)?,
            ))
        })?;
        for row in rows {
            let (m, key) = row?;
            if !files.contains_key(&key) && !unreadable(&key) {
                missing.entry((m.kind, m.size)).or_default().push(m);
            }
        }
    }

    // Verified relocation: a one-to-one match between a new path and a file
    // the library can no longer find, here or anywhere else.
    let mut relocated: HashMap<String, (i64, Evidence)> = HashMap::new();
    let mut ambiguous = 0;
    {
        let mut elsewhere = db.prepare_cached(
            "SELECT id, kind, size, modified, name, volume, file_index, digest, path
             FROM files WHERE present = 0 AND kind = ?1 AND size = ?2",
        )?;
        let mut matches = Vec::new();
        let mut claims: HashMap<i64, usize> = HashMap::new();
        for (key, info) in files.iter().filter(|(k, _)| !known.contains(*k)) {
            let candidates: Vec<Missing> = elsewhere
                .query_map(params![info.kind, info.size], |r| {
                    Ok(Missing {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        size: r.get(2)?,
                        modified: r.get(3)?,
                        name: r.get(4)?,
                        volume: r.get(5)?,
                        index: r.get(6)?,
                        digest: r.get(7)?,
                        path: PathBuf::from(r.get::<_, String>(8)?),
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            let local = missing.get(&(info.kind, info.size)).into_iter().flatten();
            let candidates: Vec<&Missing> = local.chain(candidates.iter()).collect();
            if candidates.is_empty() {
                continue;
            }
            let Some(e) = evidence(&info.path, info.kind, (info.size, info.modified)) else {
                continue;
            };
            let hits: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|c| c.plausible(info, &e) && matches!(c.path.try_exists(), Ok(false)))
                .collect();
            for hit in &hits {
                *claims.entry(hit.id).or_default() += 1;
            }
            match hits.as_slice() {
                [] => {}
                [old] => {
                    let verified = old.verified(info, &e);
                    matches.push((key.clone(), old.id, e, verified));
                }
                _ => ambiguous += 1,
            }
        }
        for (key, old, e, verified) in matches {
            if verified && claims[&old] == 1 {
                relocated.insert(key, (old, e));
            } else {
                ambiguous += 1;
            }
        }
    }

    // Write in chunks. Relocations first, so their tracks keep identities.
    let before: i64 = db.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))?;
    {
        let tx = scan_transaction(db, root, generation, alive)?;
        for (key, (old, e)) in &relocated {
            let f = &files[key];
            tx.execute(
                "UPDATE files SET path = ?2, path_key = ?3, name = ?4, root_id = coalesce(?5, root_id),
                    size = ?6, modified = ?7, volume = ?8, file_index = ?9, digest = ?10,
                    present = 1, seen = ?11 WHERE id = ?1",
                params![
                    old,
                    f.path.to_str(),
                    f.key,
                    f.name,
                    f.owner,
                    f.size,
                    f.modified,
                    e.volume,
                    e.index,
                    e.digest,
                    generation
                ],
            )?;
        }
        tx.commit()?;
    }
    let mut pending = groups.iter().peekable();
    while pending.peek().is_some() {
        if !alive() {
            return Err(super::worker::Cancelled.into());
        }
        let mut prepared = Vec::new();
        let mut count = 0;
        while count < CHUNK
            && let Some(g) = pending.next()
        {
            let Some(anchor) = by_path.get(&g.anchor).and_then(|k| files.get(k)) else {
                continue;
            };
            let cover = g
                .anchor
                .parent()
                .and_then(|dir| found.covers.get(dir))
                .map(PathBuf::as_path);
            let tracks = g
                .tracks
                .iter()
                .map(|t| {
                    enriched(
                        db,
                        t,
                        cache,
                        by_path.get(&t.source).and_then(|k| files.get(k)),
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            count += g.tracks.len();
            prepared.push((g, tracks, anchor, cover));
        }
        let tx = scan_transaction(db, root, generation, alive)?;
        for (g, tracks, anchor, cover) in prepared {
            write_group(
                &tx,
                g,
                &tracks,
                anchor,
                &|p| by_path.get(p).and_then(|k| files.get(k)),
                cover,
                generation,
            )?;
        }
        tx.commit()?;
    }

    // Missing files and tracks, unless their folder or sheet was unreadable.
    let mut newly_missing = 0;
    {
        let tx = scan_transaction(db, root, generation, alive)?;
        let stale_files: Vec<(i64, String)> = tx
            .prepare(
                "SELECT id, path_key FROM files WHERE root_id = ?1 AND present = 1 AND seen != ?2",
            )?
            .query_map(params![root, generation], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, key) in stale_files {
            if !unreadable(&key) {
                tx.execute("UPDATE files SET present = 0 WHERE id = ?1", [id])?;
            }
        }
        let stale_tracks: Vec<(i64, String, String)> = tx
            .prepare(
                "SELECT t.id, a.path_key, s.path_key FROM tracks t JOIN files a ON a.id = t.anchor JOIN files s ON s.id = t.source
                 WHERE t.root_id = ?1 AND t.present = 1 AND t.seen != ?2",
            )?
            .query_map(params![root, generation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, key, source) in stale_tracks {
            if !unreadable(&key) && !unreadable(&source) {
                tx.execute("UPDATE tracks SET present = 0 WHERE id = ?1", [id])?;
                newly_missing += 1;
            }
        }
        tx.execute(
            "DELETE FROM albums WHERE id NOT IN (SELECT album_id FROM tracks WHERE album_id IS NOT NULL)",
            [],
        )?;
        tx.execute(
            "UPDATE roots SET scanned_at = ?2 WHERE id = ?1",
            params![root, now()],
        )?;
        tx.commit()?;
    }
    let after: i64 = db.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))?;
    let summary = Summary {
        tracks: groups.iter().map(|g| g.tracks.len()).sum(),
        added: (after - before).max(0) as usize,
        missing: newly_missing,
        relocated: relocated.len(),
        ambiguous,
        complete,
    };
    progress(Progress::Indexed(summary.clone()));
    enrich(db, root, generation, &found.covers, cache, alive, progress)?;
    Ok(summary)
}

struct Found {
    tracks: Vec<Arc<Track>>,
    covers: HashMap<PathBuf, PathBuf>,
    incomplete: Vec<String>,
    failed_sheets: HashSet<String>,
}
fn walk(root: &Path, cache: Option<&Cache>, alive: &dyn Fn() -> bool) -> Option<Found> {
    let mut slots: Vec<Option<Arc<Track>>> = vec![];
    let mut found = Found {
        tracks: vec![],
        covers: HashMap::new(),
        incomplete: vec![],
        failed_sheets: HashSet::new(),
    };
    crate::discovery::walk(root, true, cache, alive, &mut |batch| {
        for (i, t) in batch.rows {
            if i == slots.len() {
                slots.push(t);
            } else {
                slots[i] = t;
            }
        }
        for (folder, cover) in batch.covers {
            found.covers.insert(folder, cover.path().to_owned());
        }
        found
            .incomplete
            .extend(batch.incomplete.iter().map(|p| path_key(p)));
        found
            .failed_sheets
            .extend(batch.failed_sheets.iter().map(|p| path_key(p)));
        true
    })?;
    found.tracks = slots.into_iter().flatten().collect();
    Some(found)
}

/// The best metadata available without reading the file: cached tags for
/// its current revision, or what the library recorded for an unchanged file.
fn enriched(
    db: &Connection,
    t: &Arc<Track>,
    cache: Option<&Cache>,
    source: Option<&FileInfo>,
) -> Result<Arc<Track>> {
    let Some(source) = source else {
        return Ok(t.clone());
    };
    let stamp = Stamp {
        bytes: source.size,
        modified: source.modified,
    };
    if let Some(d) = &t.disc {
        if d.end.is_some() || t.duration.is_some() {
            return Ok(t.clone());
        }
        let mut t = (**t).clone();
        t.duration = cache
            .and_then(|c| c.tags(&source.path, &stamp))
            .and_then(|tags| tags.duration)
            .map(|s| (s - d.start as f64 / 75.).max(0.));
        return Ok(Arc::new(t));
    }
    if let Some(mut tags) = cache.and_then(|c| c.tags(&source.path, &stamp)) {
        tags.source = t.source.clone();
        return Ok(Arc::new(tags));
    }
    let recorded = db
        .prepare_cached(
            "SELECT t.title, t.artist, t.album, t.number, t.disc_number, t.year, t.duration
             FROM tracks t JOIN files f ON f.id = t.anchor
             WHERE f.path_key = ?1 AND t.part = 0 AND f.tagged = ?2 AND f.size = ?3",
        )?
        .query_row(params![source.key, source.modified, source.size], |r| {
            let mut t = (**t).clone();
            t.title = r.get(0)?;
            t.artist = r.get(1)?;
            t.album = r.get(2)?;
            t.number = r.get(3)?;
            t.disc_number = r.get(4)?;
            t.year = r.get(5)?;
            t.duration = r.get(6)?;
            Ok(t)
        })
        .optional()?;
    Ok(recorded.map_or_else(|| t.clone(), Arc::new))
}

/// Writes one anchor and its tracks. Identities come from `(anchor, part)`;
/// everything else is refreshed.
fn write_group<'a>(
    tx: &Transaction,
    group: &Group,
    tracks: &[Arc<Track>],
    anchor: &FileInfo,
    file: &dyn Fn(&Path) -> Option<&'a FileInfo>,
    cover: Option<&Path>,
    generation: i64,
) -> Result<()> {
    let anchor_id = upsert_file(tx, anchor, generation)?;
    let parts = parts(group);
    for (ordinal, (t, part)) in tracks.iter().zip(parts).enumerate() {
        let Some(source) = file(&t.source) else {
            continue;
        };
        let source_id = if source.key == anchor.key {
            anchor_id
        } else {
            upsert_file(tx, source, generation)?
        };
        let album = album_for(tx, t, group, anchor_id, anchor.owner, cover)?;
        upsert_track(
            tx,
            anchor_id,
            part,
            source_id,
            t,
            ordinal,
            album,
            anchor.owner,
            generation,
        )?;
    }
    Ok(())
}

fn upsert_file(tx: &Transaction, f: &FileInfo, generation: i64) -> Result<i64> {
    let existing: Option<i64> = tx
        .prepare_cached("SELECT id FROM files WHERE path_key = ?1")?
        .query_row([&f.key], |r| r.get(0))
        .optional()?;
    if let Some(id) = existing {
        // A changed file keeps its identity; its evidence is read again.
        tx.prepare_cached(
            "UPDATE files SET path = ?2, name = ?3, kind = ?4, root_id = coalesce(?5, root_id),
                volume = CASE WHEN size = ?6 AND modified = ?7 THEN volume END,
                file_index = CASE WHEN size = ?6 AND modified = ?7 THEN file_index END,
                digest = CASE WHEN size = ?6 AND modified = ?7 THEN digest END,
                tagged = CASE WHEN size = ?6 AND modified = ?7 THEN tagged END,
                size = ?6, modified = ?7, present = 1, seen = CASE WHEN ?5 IS NULL THEN seen ELSE ?8 END
             WHERE id = ?1",
        )?
        .execute(params![
            id,
            f.path.to_str(),
            f.name,
            f.kind,
            f.owner,
            f.size,
            f.modified,
            generation
        ])?;
        return Ok(id);
    }
    tx.prepare_cached(
        "INSERT INTO files(path, path_key, name, kind, root_id, size, modified, present, seen)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
    )?
    .execute(params![
        f.path.to_str(),
        f.key,
        f.name,
        f.kind,
        f.owner,
        f.size,
        f.modified,
        generation
    ])?;
    Ok(tx.last_insert_rowid())
}

/// A CUE program is its own disc. Tagged files group by folder, album and
/// disc number, so identical album names never merge editions.
fn album_for(
    tx: &Transaction,
    t: &Track,
    group: &Group,
    anchor_id: i64,
    root: Option<RootId>,
    cover: Option<&Path>,
) -> Result<AlbumId> {
    let folder = group.anchor.parent().unwrap_or(Path::new(""));
    let (key, title) = if group.sheet {
        let title = if t.album.is_empty() {
            library::name(&group.anchor)
        } else {
            t.album.clone()
        };
        (format!("sheet:{anchor_id}"), title)
    } else {
        let title = if t.album.is_empty() {
            library::folder_name(folder)
        } else {
            t.album.clone()
        };
        (
            format!(
                "dir:{}\u{1}{}\u{1}{}",
                path_key(folder),
                t.album.to_lowercase(),
                t.disc_number.unwrap_or(0)
            ),
            title,
        )
    };
    let sort_key = format!("{}\u{1}{}", path_key(folder), title.to_lowercase());
    Ok(tx
        .prepare_cached(
            "INSERT INTO albums(key, root_id, title, artist, year, folder, cover, sort_key)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(key) DO UPDATE SET root_id = coalesce(excluded.root_id, root_id),
                title = excluded.title, artist = excluded.artist, year = excluded.year,
                folder = excluded.folder, cover = coalesce(excluded.cover, cover),
                sort_key = excluded.sort_key
             RETURNING id",
        )?
        .query_row(
            params![
                key,
                root,
                title,
                t.artist,
                t.year,
                folder.to_string_lossy(),
                cover.map(|c| c.to_string_lossy()),
                sort_key
            ],
            |r| r.get(0),
        )?)
}

#[allow(clippy::too_many_arguments)]
fn upsert_track(
    tx: &Transaction,
    anchor: i64,
    part: i64,
    source: i64,
    t: &Track,
    ordinal: usize,
    album: AlbumId,
    root: Option<RootId>,
    generation: i64,
) -> Result<()> {
    let (first, last) = t.disc.as_ref().map_or((None, None), |d| {
        (Some(d.start as i64), d.end.map(|e| e as i64))
    });
    tx.prepare_cached(
        "INSERT INTO tracks(anchor, part, source, first_tick, last_tick, root_id, album_id, ordinal,
            title, artist, album, number, disc_number, year, duration, format,
            title_key, artist_key, album_key, search, present, seen, added_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, 1, ?21, ?22)
         ON CONFLICT(anchor, part) DO UPDATE SET source = excluded.source,
            first_tick = excluded.first_tick, last_tick = excluded.last_tick,
            root_id = coalesce(excluded.root_id, root_id), album_id = excluded.album_id,
            ordinal = excluded.ordinal, title = excluded.title, artist = excluded.artist,
            album = excluded.album, number = excluded.number, disc_number = excluded.disc_number,
            year = excluded.year, duration = coalesce(excluded.duration, duration),
            format = excluded.format, title_key = excluded.title_key,
            artist_key = excluded.artist_key, album_key = excluded.album_key,
            search = excluded.search, present = 1, seen = CASE WHEN excluded.root_id IS NULL THEN seen ELSE excluded.seen END",
    )?
    .execute(params![
        anchor,
        part,
        source,
        first,
        last,
        root,
        album,
        ordinal as i64,
        t.title,
        t.artist,
        t.album,
        t.display_number(),
        t.disc_number,
        t.year,
        t.duration,
        t.format(),
        t.title.to_lowercase(),
        t.artist.to_lowercase(),
        t.album.to_lowercase(),
        search_text(t),
        generation,
        now()
    ])?;
    Ok(())
}

fn search_text(t: &Track) -> String {
    format!("{}\u{1}{}\u{1}{}", t.title, t.artist, t.album).to_lowercase()
}

/// Reads tags, durations and identity evidence for the files of this scan
/// that need them.
fn enrich(
    db: &mut Connection,
    root: RootId,
    generation: i64,
    covers: &HashMap<PathBuf, PathBuf>,
    cache: Option<&Cache>,
    alive: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    struct Todo {
        id: i64,
        path: PathBuf,
        kind: i64,
        size: i64,
        modified: i64,
        tags: bool,
        evidence: bool,
    }
    let todo: Vec<Todo> = db
        .prepare(
            "SELECT id, path, kind, size, modified,
                kind = 0 AND (tagged IS NULL OR tagged != modified), digest IS NULL
             FROM files WHERE root_id = ?1 AND present = 1 AND seen = ?2
               AND ((kind = 0 AND (tagged IS NULL OR tagged != modified)) OR digest IS NULL)
             ORDER BY path_key",
        )?
        .query_map(params![root, generation], |r| {
            Ok(Todo {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                kind: r.get(2)?,
                size: r.get(3)?,
                modified: r.get(4)?,
                tags: r.get(5)?,
                evidence: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let total = todo.len();
    for (n, chunk) in todo.chunks(64).enumerate() {
        if !alive() {
            return Err(super::worker::Cancelled.into());
        }
        let read: Vec<_> = chunk
            .iter()
            .map(|f| {
                let stamp = Stamp {
                    bytes: f.size,
                    modified: f.modified,
                };
                let e = f
                    .evidence
                    .then(|| evidence(&f.path, f.kind, (f.size, f.modified)))
                    .flatten();
                let tags = f.tags.then(|| {
                    cache.and_then(|c| c.tags(&f.path, &stamp)).or_else(|| {
                        let mut t = Track::file(f.path.clone());
                        if !t.read_tags() {
                            return None;
                        }
                        if Stamp::read(&f.path).as_ref() != Some(&stamp) {
                            return None;
                        }
                        if let Some(c) = cache {
                            let _ = c.store_tags(&t, &stamp);
                        }
                        Some(t)
                    })
                });
                (e, tags)
            })
            .collect();
        let tx = scan_transaction(db, root, generation, alive)?;
        for (f, (e, tags)) in chunk.iter().zip(read) {
            if let Some(e) = e {
                tx.prepare_cached(
                    "UPDATE files SET volume = ?2, file_index = ?3, digest = ?4
                     WHERE id = ?1 AND size = ?5 AND modified = ?6",
                )?
                .execute(params![
                    f.id, e.volume, e.index, e.digest, f.size, f.modified
                ])?;
            }
            let Some(tags) = tags else { continue };
            tx.prepare_cached("UPDATE files SET tagged = ?2 WHERE id = ?1 AND modified = ?2")?
                .execute(params![f.id, f.modified])?;
            let Some(mut t) = tags else { continue };
            t.source = f.path.clone();
            if let Some(seconds) = t.duration {
                tx.prepare_cached(
                    "UPDATE tracks SET duration = max(0.0, ?2 - first_tick / 75.0)
                     WHERE source = ?1 AND first_tick IS NOT NULL AND last_tick IS NULL",
                )?
                .execute(params![f.id, seconds])?;
            }
            let whole: Option<(i64, i64)> = tx
                .prepare_cached("SELECT id, ordinal FROM tracks WHERE anchor = ?1 AND part = 0")?
                .query_row([f.id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if let Some((id, ordinal)) = whole {
                let group = Group {
                    anchor: f.path.clone(),
                    sheet: false,
                    tracks: vec![],
                };
                let cover = f
                    .path
                    .parent()
                    .and_then(|d| covers.get(d))
                    .map(PathBuf::as_path);
                let album = album_for(&tx, &t, &group, f.id, Some(root), cover)?;
                tx.prepare_cached(
                    "UPDATE tracks SET title = ?2, artist = ?3, album = ?4, number = ?5,
                        disc_number = ?6, year = ?7, duration = ?8, title_key = ?9,
                        artist_key = ?10, album_key = ?11, search = ?12, album_id = ?13,
                        ordinal = ?14
                     WHERE id = ?1",
                )?
                .execute(params![
                    id,
                    t.title,
                    t.artist,
                    t.album,
                    t.display_number(),
                    t.disc_number,
                    t.year,
                    t.duration,
                    t.title.to_lowercase(),
                    t.artist.to_lowercase(),
                    t.album.to_lowercase(),
                    search_text(&t),
                    album,
                    ordinal
                ])?;
            }
        }
        if (n + 1) * 64 >= total {
            tx.execute(
                "DELETE FROM albums WHERE id NOT IN (SELECT album_id FROM tracks WHERE album_id IS NOT NULL)",
                [],
            )?;
        }
        tx.commit()?;
        progress(Progress::Reading {
            done: ((n + 1) * 64).min(total),
            total,
        });
    }
    Ok(())
}

/// Gives a track met while browsing an identity outside any collection. A
/// CUE chapter brings its whole program, so every chapter has its own part.
pub(super) fn adopt(db: &mut Connection, t: &Track, cache: Option<&Cache>) -> Result<()> {
    let program: Vec<Arc<Track>> = match &t.disc {
        None => vec![Arc::new(t.clone())],
        Some(d) => {
            let program = match cache {
                Some(c) => c.program(&d.sheet)?,
                None => crate::cue::read(&d.sheet)?,
            };
            program
                .into_iter()
                .filter(|c| c.source.is_file())
                .map(Arc::new)
                .collect()
        }
    };
    let mut groups = groups(program);
    ensure!(groups.len() == 1, "{} could not be read", t.title);
    let group = groups.remove(0);
    let anchor_kind = if group.sheet { SHEET } else { AUDIO };
    let anchor = FileInfo::read(&group.anchor, anchor_kind, None)
        .with_context(|| format!("{} is not available", group.anchor.display()))?;
    let mut sources: HashMap<PathBuf, FileInfo> = HashMap::new();
    for track in &group.tracks {
        if !sources.contains_key(&track.source)
            && let Some(info) = FileInfo::read(&track.source, AUDIO, None)
        {
            sources.insert(track.source.clone(), info);
        }
    }
    let cover = group.anchor.parent().and_then(library::find_cover);
    let tx = db.transaction()?;
    let tracks = group.tracks.clone();
    write_group(
        &tx,
        &group,
        &tracks,
        &anchor,
        &|p| sources.get(p),
        cover.as_deref(),
        0,
    )?;
    tx.commit()?;
    Ok(())
}

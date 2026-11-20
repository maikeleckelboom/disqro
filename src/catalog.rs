//! The interface's model of the persistent library: registered collections,
//! playlists, the listener's likes and ratings, and indexing progress.
//!
//! Every surface that changes personal data (track menus, keyboard
//! shortcuts, the rating and like cells, the sidebar) calls these commands,
//! so there is one execution path per operation. Database work runs on the
//! library worker; this entity keeps what the interface draws.
use crate::{
    cache::Cache,
    library::{Track, TrackList},
    store::{
        self, Handle, Locator, Mark, PlaylistId, Progress, RootId, Row, Scope, Store, TrackId,
    },
};
use gpui_kit::{prelude::*, *};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Tracks to act on.
#[derive(Clone)]
pub enum Targets {
    /// Rows of a library view: track ids, or playlist entry ids.
    Keys(Scope, Vec<i64>),
    /// Tracks met while browsing folders. Their identities are found, or
    /// created outside any collection.
    Tracks(Vec<Arc<Track>>),
}
impl Targets {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Keys(_, k) => k.is_empty(),
            Self::Tracks(t) => t.is_empty(),
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Keys(_, k) => k.len(),
            Self::Tracks(t) => t.len(),
        }
    }
}

pub struct Collection {
    pub root: store::Root,
    pub online: bool,
    /// What the indexer is doing, while it is busy with this collection.
    pub status: Option<String>,
}

pub enum CatalogEvent {
    /// Collections or playlists changed.
    Changed,
    /// Library rows changed; open views should query again.
    Content,
    /// A playlist was just created and can be named.
    Created(PlaylistId),
    Notice(String),
}
impl EventEmitter<CatalogEvent> for Catalog {}

pub struct Catalog {
    store: Option<Handle>,
    cache_dir: Option<PathBuf>,
    pub collections: Vec<Collection>,
    pub playlists: Vec<store::Playlist>,
    marks: HashMap<TrackId, Mark>,
    located: HashMap<Locator, TrackId>,
    scanning: Option<(RootId, Arc<AtomicBool>)>,
    pending: VecDeque<RootId>,
    pub loaded: bool,
}

impl Catalog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = Store::location().map(Handle::spawn);
        let mut this = Self {
            store,
            cache_dir: crate::profile::location().map(|p| p.with_file_name("Cache")),
            collections: vec![],
            playlists: vec![],
            marks: HashMap::new(),
            located: HashMap::new(),
            scanning: None,
            pending: VecDeque::new(),
            loaded: false,
        };
        this.load(true, cx);
        this
    }

    /// Reads collections, playlists and marks; optionally rescans every
    /// collection that is online.
    fn load(&mut self, rescan: bool, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let loaded = store
                .call(|s| Ok((s.roots()?, s.playlists()?, s.marks()?)))
                .await;
            let (roots, playlists, marks) = match loaded {
                Ok(v) => v,
                Err(e) => {
                    let _ =
                        this.update(cx, |_, cx| cx.emit(CatalogEvent::Notice(format!("{e:#}"))));
                    return;
                }
            };
            let paths: Vec<_> = roots.iter().map(|r| (r.id, r.path.clone())).collect();
            let _ = this.update(cx, |c, cx| {
                c.set_roots(roots);
                c.playlists = playlists;
                c.set_marks(marks);
                c.loaded = true;
                cx.emit(CatalogEvent::Changed);
                cx.emit(CatalogEvent::Content);
                cx.notify();
            });
            let availability = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|(id, path)| (id, path.is_dir()))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |c, cx| {
                for (id, online) in availability {
                    if let Some(collection) = c.collections.iter_mut().find(|c| c.root.id == id) {
                        collection.online = online;
                        if rescan && online {
                            c.rescan(id, cx);
                        }
                    }
                }
                if c.collections.iter().any(|r| !r.online) {
                    cx.emit(CatalogEvent::Content);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn set_roots(&mut self, roots: Vec<store::Root>) {
        let previous = std::mem::take(&mut self.collections);
        self.collections = roots
            .into_iter()
            .map(|root| {
                let old = previous.iter().find(|c| c.root.id == root.id);
                Collection {
                    online: old.is_none_or(|c| c.online),
                    status: old.and_then(|c| c.status.clone()),
                    root,
                }
            })
            .collect();
    }
    fn set_marks(&mut self, marks: Vec<store::TrackMark>) {
        self.marks.clear();
        self.located.clear();
        self.apply_marks(marks);
    }
    fn apply_marks(&mut self, marks: Vec<store::TrackMark>) {
        for (id, locator, mark) in marks {
            if mark == Mark::default() {
                self.marks.remove(&id);
                if let Some(locator) = locator {
                    self.located.remove(&locator);
                }
            } else {
                self.marks.insert(id, mark);
                if let Some(locator) = locator {
                    self.located.insert(locator, id);
                }
            }
        }
    }
    pub fn collection(&self, id: RootId) -> Option<&Collection> {
        self.collections.iter().find(|c| c.root.id == id)
    }
    pub fn playlist(&self, id: PlaylistId) -> Option<&store::Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    pub fn mark(&self, id: TrackId) -> Mark {
        self.marks.get(&id).copied().unwrap_or_default()
    }
    /// The marks of a track met while browsing.
    pub fn mark_of(&self, t: &Track) -> Mark {
        if self.located.is_empty() {
            return Mark::default();
        }
        self.located
            .get(&Locator::of(t))
            .map_or_else(Mark::default, |id| self.mark(*id))
    }

    /// Runs a write on the library worker against the identities of
    /// `targets`, then refreshes marks or playlists.
    fn write<T: Send + 'static>(
        &mut self,
        targets: Targets,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Store, Vec<TrackId>) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let Some(store) = self.store.clone() else {
            cx.emit(CatalogEvent::Notice("The library is unavailable.".into()));
            return;
        };
        let pending = store.call(move |s| {
            let browsing = matches!(&targets, Targets::Tracks(tracks) if !tracks.is_empty());
            let ids = match targets {
                Targets::Tracks(tracks) => s.resolve(&tracks)?,
                Targets::Keys(scope, keys) => s.track_ids(scope, &keys)?,
            };
            let value = f(s, ids)?;
            let marks = browsing.then(|| s.marks()).transpose()?;
            Ok((value, marks))
        });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |c, cx| match result {
                Ok((value, marks)) => {
                    if let Some(marks) = marks {
                        c.set_marks(marks);
                    }
                    done(c, value, cx);
                    cx.emit(CatalogEvent::Content);
                    cx.notify();
                }
                Err(e) => cx.emit(CatalogEvent::Notice(format!("{e:#}"))),
            });
        })
        .detach();
    }

    fn edit_marks(
        &mut self,
        targets: Targets,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Store, &[TrackId]) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.write(
            targets,
            cx,
            move |s, ids| {
                edit(s, &ids)?;
                s.marks_for(&ids)
            },
            |c, marks, _| c.apply_marks(marks),
        );
    }

    fn edit_playlist<T: Send + 'static>(
        &mut self,
        targets: Targets,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Store, Vec<TrackId>) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        self.write(
            targets,
            cx,
            move |s, ids| {
                let value = edit(s, ids)?;
                Ok((value, s.playlists()?))
            },
            move |c, (value, playlists), cx| {
                c.playlists = playlists;
                done(c, value, cx);
                cx.emit(CatalogEvent::Changed);
            },
        );
    }

    /// Likes every target, or unlikes them all when all are liked.
    pub fn toggle_like(&mut self, targets: Targets, cx: &mut Context<Self>) {
        self.edit_marks(targets, cx, Store::toggle_liked);
    }
    pub fn all_liked(&self, targets: &Targets) -> bool {
        match targets {
            Targets::Tracks(t) => !t.is_empty() && t.iter().all(|t| self.mark_of(t).liked),
            Targets::Keys(scope, k) if !scope.keys_are_entries() => {
                !k.is_empty() && k.iter().all(|id| self.mark(*id).liked)
            }
            Targets::Keys(..) => false,
        }
    }
    pub fn set_liked(&mut self, targets: Targets, liked: bool, cx: &mut Context<Self>) {
        if targets.is_empty() {
            return;
        }
        self.edit_marks(targets, cx, move |s, ids| s.set_liked(ids, liked));
    }
    pub fn rate(&mut self, targets: Targets, stars: u8, cx: &mut Context<Self>) {
        if targets.is_empty() {
            return;
        }
        self.edit_marks(targets, cx, move |s, ids| s.set_rating(ids, stars));
    }

    pub fn add_to_playlist(&mut self, id: PlaylistId, targets: Targets, cx: &mut Context<Self>) {
        if targets.is_empty() {
            return;
        }
        self.edit_playlist(
            targets,
            cx,
            move |s, ids| s.add_entries(id, &ids, None).map(|_| ()),
            |_, _, _| {},
        );
    }
    /// Creates a playlist holding `targets`, ready to be named.
    pub fn new_playlist(&mut self, targets: Targets, cx: &mut Context<Self>) {
        self.edit_playlist(
            targets,
            cx,
            |s, ids| {
                let name = s.unique_playlist_name("New Playlist")?;
                s.create_playlist(&name, &ids)
            },
            |_, id, cx| cx.emit(CatalogEvent::Created(id)),
        );
    }
    pub fn rename_playlist(&mut self, id: PlaylistId, name: String, cx: &mut Context<Self>) {
        if name.trim().is_empty() || self.playlist(id).is_some_and(|p| p.name == name.trim()) {
            return;
        }
        self.edit_playlist(
            Targets::Tracks(vec![]),
            cx,
            move |s, _| s.rename_playlist(id, &name),
            |_, _, _| {},
        );
    }
    pub fn delete_playlist(&mut self, id: PlaylistId, cx: &mut Context<Self>) {
        self.edit_playlist(
            Targets::Tracks(vec![]),
            cx,
            move |s, _| s.delete_playlist(id),
            |_, _, _| {},
        );
    }
    pub fn duplicate_playlist(&mut self, id: PlaylistId, cx: &mut Context<Self>) {
        let base = self
            .playlist(id)
            .map_or_else(|| "Playlist".into(), |p| format!("{} Copy", p.name));
        self.edit_playlist(
            Targets::Tracks(vec![]),
            cx,
            move |s, _| {
                let name = s.unique_playlist_name(&base)?;
                s.duplicate_playlist(id, &name)
            },
            |_, id, cx| cx.emit(CatalogEvent::Created(id)),
        );
    }
    pub fn remove_entries(&mut self, id: PlaylistId, entries: Vec<i64>, cx: &mut Context<Self>) {
        self.edit_playlist(
            Targets::Tracks(vec![]),
            cx,
            move |s, _| s.remove_entries(id, &entries),
            |_, _, _| {},
        );
    }
    pub fn move_entries(
        &mut self,
        id: PlaylistId,
        entries: Vec<i64>,
        before: Option<store::EntryId>,
        cx: &mut Context<Self>,
    ) {
        self.edit_playlist(
            Targets::Tracks(vec![]),
            cx,
            move |s, _| s.move_entries(id, &entries, before),
            |_, _, _| {},
        );
    }

    /// Fetch metadata on the database worker. Detached files can live on a
    /// sleeping network share, so probe them separately from SQLite.
    pub fn rows(
        &self,
        scope: Scope,
        keys: Vec<i64>,
        ticket: &store::Ticket,
        generation: u64,
        cx: &App,
    ) -> impl Future<Output = anyhow::Result<Vec<Row>>> + use<> {
        let pending = self
            .store
            .as_ref()
            .map(|s| s.query(ticket, generation, move |s| s.rows(scope, &keys)));
        let offline: HashSet<RootId> = self
            .collections
            .iter()
            .filter(|c| !c.online)
            .map(|c| c.root.id)
            .collect();
        let ticket = ticket.clone();
        let background = cx.background_executor().clone();
        async move {
            let mut rows = pending
                .ok_or_else(|| anyhow::anyhow!("The library is unavailable."))?
                .await?;
            background
                .spawn(async move {
                    let mut files = store::DetachedFiles::default();
                    for row in &mut rows {
                        if !ticket.is_current(generation) {
                            return Err(store::Cancelled.into());
                        }
                        row.available &= !row.root.is_some_and(|id| offline.contains(&id))
                            && !row.source_root.is_some_and(|id| offline.contains(&id));
                        row.available = row.available && files.available(row);
                    }
                    Ok(rows)
                })
                .await
        }
    }

    pub fn tracks(
        &self,
        targets: Targets,
        cx: &App,
    ) -> impl Future<Output = anyhow::Result<TrackList>> + use<> {
        let (tracks, pending) = match targets {
            Targets::Tracks(t) => (t, None),
            Targets::Keys(scope, keys) => (
                vec![],
                Some(self.rows(scope, keys, &store::Ticket::default(), 0, cx)),
            ),
        };
        async move {
            match pending {
                None => Ok(tracks.into()),
                Some(rows) => Ok(rows
                    .await?
                    .into_iter()
                    .filter(|r| r.available)
                    .map(|r| r.track)
                    .collect()),
            }
        }
    }
    /// Resolve an authored sequence independently of the visible search, sort
    /// and loaded page. Unregistered music is read from its own directory.
    pub async fn program(
        catalog: Entity<Self>,
        track: Arc<Track>,
        cx: &AsyncApp,
    ) -> anyhow::Result<TrackList> {
        let store = catalog.read_with(cx, |c, _| c.handle());
        if let Some(store) = store {
            let target = track.clone();
            if let Some(disc) = store.call(move |s| s.registered_disc(&target)).await? {
                return Self::album_tracks(catalog, disc.album, cx).await;
            }
        }
        let anchor = track.disc.as_ref().map_or(&track.source, |d| &d.sheet);
        let folder = anchor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("The CD folder is unavailable."))?
            .to_owned();
        cx.background_spawn(async move { crate::discovery::local_program(&folder, &track) })
            .await
    }
    pub async fn album_tracks(
        catalog: Entity<Self>,
        album: store::AlbumId,
        cx: &AsyncApp,
    ) -> anyhow::Result<TrackList> {
        let store = catalog
            .read_with(cx, |c, _| c.handle())
            .ok_or_else(|| anyhow::anyhow!("The library is unavailable."))?;
        let keys = store.call(move |s| s.disc_keys(album)).await?;
        catalog
            .read_with(cx, |c, cx| {
                c.tracks(Targets::Keys(Scope::Album(album), keys), cx)
            })
            .await
    }
    pub fn handle(&self) -> Option<Handle> {
        self.store.clone()
    }

    // Collections

    pub fn add_root(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let pending = store.call(move |s| {
            let id = s.add_root(&path)?;
            Ok((id, s.roots()?))
        });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |c, cx| match result {
                Ok((id, roots)) => {
                    c.set_roots(roots);
                    c.rescan(id, cx);
                    cx.emit(CatalogEvent::Changed);
                    cx.notify();
                }
                Err(e) => cx.emit(CatalogEvent::Notice(format!("{e:#}"))),
            });
        })
        .detach();
    }
    pub fn remove_root(&mut self, id: RootId, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        self.pending.retain(|r| *r != id);
        if let Some((root, alive)) = &self.scanning
            && *root == id
        {
            alive.store(false, Ordering::SeqCst);
        }
        let pending = store.call(move |s| {
            s.remove_root(id)?;
            Ok((s.roots()?, s.marks()?))
        });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |c, cx| match result {
                Ok((roots, marks)) => {
                    c.set_roots(roots);
                    c.set_marks(marks);
                    cx.emit(CatalogEvent::Changed);
                    cx.emit(CatalogEvent::Content);
                    cx.notify();
                }
                Err(e) => cx.emit(CatalogEvent::Notice(format!("{e:#}"))),
            });
        })
        .detach();
    }
    pub fn is_scanning(&self, id: RootId) -> bool {
        self.scanning.as_ref().is_some_and(|(r, _)| *r == id) || self.pending.contains(&id)
    }
    /// Queues a scan. Scans run one at a time on their own thread and
    /// connection; the interface stays responsive and browsable throughout.
    pub fn rescan(&mut self, id: RootId, cx: &mut Context<Self>) {
        if self.is_scanning(id) {
            return;
        }
        self.pending.push_back(id);
        if let Some(c) = self.collections.iter_mut().find(|c| c.root.id == id) {
            c.status = Some("Waiting to scan".into());
        }
        self.next_scan(cx);
        cx.notify();
    }
    fn next_scan(&mut self, cx: &mut Context<Self>) {
        if self.scanning.is_some() {
            return;
        }
        let (Some(id), Some(store)) = (self.pending.pop_front(), self.store.clone()) else {
            return;
        };
        let alive = Arc::new(AtomicBool::new(true));
        self.scanning = Some((id, alive.clone()));
        let path = store.path().to_owned();
        let cache_dir = self.cache_dir.clone();
        let (tx, rx) =
            async_channel::bounded::<Result<Progress, anyhow::Result<store::Summary>>>(4);
        std::thread::Builder::new()
            .name("disqro-indexer".into())
            .spawn(move || {
                let result = (|| {
                    let mut store = Store::open(&path)?;
                    let cache = cache_dir.and_then(|d| Cache::open(&d).ok());
                    store::index_root(
                        &mut store,
                        id,
                        cache.as_ref(),
                        &|| alive.load(Ordering::SeqCst),
                        &mut |p| {
                            let _ = tx.send_blocking(Ok(p));
                        },
                    )
                })();
                let _ = tx.send_blocking(Err(result));
            })
            .expect("start library indexer");
        cx.spawn(async move |this, cx| {
            let mut last_refresh = std::time::Instant::now();
            while let Ok(message) = rx.recv().await {
                let finished = message.is_err();
                let refresh = match &message {
                    Ok(Progress::Indexed(_)) | Err(_) => true,
                    Ok(Progress::Reading { done, total }) => {
                        done == total || last_refresh.elapsed().as_millis() > 750
                    }
                    Ok(Progress::Walking) => false,
                };
                if refresh {
                    last_refresh = std::time::Instant::now();
                }
                let _ = this.update(cx, |c, cx| c.scan_progress(id, message, refresh, cx));
                if finished {
                    break;
                }
            }
        })
        .detach();
    }
    fn scan_progress(
        &mut self,
        id: RootId,
        message: Result<Progress, anyhow::Result<store::Summary>>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        let status = match &message {
            Ok(Progress::Walking) => Some("Scanning".to_string()),
            Ok(Progress::Indexed(s)) => Some(format!("Reading tags · {} tracks", s.tracks)),
            Ok(Progress::Reading { done, total }) => {
                Some(format!("Reading tags · {done} of {total}"))
            }
            Err(_) => None,
        };
        if let Some(c) = self.collections.iter_mut().find(|c| c.root.id == id) {
            c.status = status;
        }
        if let Err(result) = message {
            self.scanning = None;
            match result {
                Ok(summary) if summary.ambiguous > 0 => cx.emit(CatalogEvent::Notice(format!(
                    "{} moved or copied files could not be matched with certainty. Their earlier entries are kept as missing.",
                    summary.ambiguous
                ))),
                Err(e) if !store::is_cancelled(&e) => {
                    cx.emit(CatalogEvent::Notice(format!("Library scan failed: {e:#}")))
                }
                _ => {}
            }
            self.load(false, cx);
            self.next_scan(cx);
        }
        if refresh {
            cx.emit(CatalogEvent::Content);
        }
        cx.notify();
    }
}

impl Drop for Catalog {
    fn drop(&mut self) {
        if let Some((_, active)) = &self.scanning {
            active.store(false, Ordering::SeqCst);
        }
    }
}

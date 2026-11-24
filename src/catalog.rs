//! The interface's model of the persistent library: registered collections,
//! playlists, the listener's likes and ratings, and indexing progress.
//!
//! Every surface that changes personal data (track menus, keyboard
//! shortcuts, the rating and like cells, the sidebar) calls these commands,
//! so there is one execution path per operation. Database work runs on the
//! library worker; this entity keeps what the interface draws.
use crate::{
    cache::Cache,
    library::{DiscContext, Track, TrackList},
    queue::Selection,
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
    /// The playlist's own occurrences are reordered by its table. Other
    /// sources append references in selection order, including repeats.
    pub fn can_copy_to(&self, playlist: PlaylistId) -> bool {
        !self.is_empty() && !matches!(self, Self::Keys(Scope::Playlist(p), _) if *p == playlist)
    }
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

/// Source of the ordered occurrence keys for a playback request.
pub enum ListeningList {
    Displayed(Scope, Arc<[i64]>),
    Disc(Scope),
    Collection(Option<RootId>, String),
    Playlist(PlaylistId),
    Search(String),
}

impl ListeningList {
    pub fn for_row(scope: Scope, keys: &Arc<[i64]>, chosen: i64, track_only: bool) -> Self {
        if track_only {
            Self::Displayed(scope, Arc::from([chosen]))
        } else if matches!(scope, Scope::Album(_) | Scope::Release(_)) {
            Self::Disc(scope)
        } else {
            Self::Displayed(scope, keys.clone())
        }
    }

    pub(crate) async fn resolve(
        self,
        store: Handle,
        chosen: Option<i64>,
        ticket: store::Ticket,
        generation: u64,
        offline: HashSet<RootId>,
    ) -> anyhow::Result<(Selection, usize, usize)> {
        let (scope, keys) = store
            .query(&ticket, generation, move |s| self.keys(s))
            .await?;
        let mut tracks = Vec::with_capacity(keys.len());
        let mut playlist = match scope {
            Scope::Playlist(id) => Some((id, Vec::with_capacity(keys.len()))),
            _ => None,
        };
        let mut start = None;
        for chunk in keys.chunks(500) {
            let batch = chunk.to_vec();
            let rows = store
                .query(&ticket, generation, move |s| s.rows(scope, &batch))
                .await?;
            let mut files = store::DetachedFiles::default();
            for row in rows {
                anyhow::ensure!(ticket.is_current(generation), store::Cancelled);
                if Catalog::available(&row, &offline, &mut files)
                    && (scope != Scope::RecentlyPlayed || row.track.source.is_file())
                {
                    if Some(row.key) == chosen {
                        start = Some(tracks.len());
                    }
                    if let Some((_, entries)) = &mut playlist {
                        entries.push(row.key);
                    }
                    tracks.push(row.track);
                }
            }
        }
        let start = if chosen.is_some() {
            start.ok_or_else(|| anyhow::anyhow!("The selected recording is unavailable."))?
        } else {
            0
        };
        anyhow::ensure!(!tracks.is_empty(), "No available tracks in this list.");
        let skipped = keys.len() - tracks.len();
        Ok((
            Selection::from_indexed(tracks.into(), playlist),
            start,
            skipped,
        ))
    }

    fn source(&self, catalog: &Catalog) -> Option<String> {
        let scope = match self {
            Self::Displayed(scope, _) | Self::Disc(scope) => *scope,
            Self::Collection(root, _) => Scope::Library(*root),
            Self::Playlist(id) => Scope::Playlist(*id),
            Self::Search(query) => return Some(format!("search for “{query}”")),
        };
        match scope {
            Scope::Liked => Some("Liked Tracks".into()),
            Scope::RecentlyPlayed => Some("Recently Played".into()),
            Scope::Playlist(id) => catalog
                .playlists
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone()),
            Scope::Library(None) => Some("All Music".into()),
            Scope::Library(Some(id)) => catalog
                .collections
                .iter()
                .find(|c| c.root.id == id)
                .map(|c| c.root.name.clone()),
            Scope::Album(_) | Scope::Release(_) => None,
        }
    }

    fn keys(self, store: &Store) -> anyhow::Result<(Scope, Arc<[i64]>)> {
        Ok(match self {
            Self::Displayed(scope, keys) => (scope, keys),
            Self::Disc(scope) => (scope, store.disc_keys(scope)?.into()),
            Self::Collection(root, search) => (
                Scope::Library(root),
                store.collection_keys(root, search)?.into(),
            ),
            Self::Playlist(id) => (
                Scope::Playlist(id),
                store
                    .entries(id)?
                    .into_iter()
                    .map(|(entry, _)| entry)
                    .collect(),
            ),
            Self::Search(text) => (Scope::Library(None), store.search_track_keys(&text)?.into()),
        })
    }
}

pub struct Collection {
    pub root: store::Root,
    pub online: bool,
    /// What the indexer is doing, while it is busy with this collection.
    pub status: Option<String>,
}

pub enum CatalogEvent {
    PlayingDisc,
    /// Collections or playlists changed.
    Changed,
    /// Library rows changed; open views should query again.
    Content,
    Listening(Vec<crate::listening::Change>),
    ListeningRemoved,
    SourcesMoved(Vec<PathBuf>),
    /// A registered collection's navigation structure or availability changed.
    Structure(RootId),
    /// A playlist was just created and can be named.
    Created(PlaylistId),
    Notice(String),
}
impl EventEmitter<CatalogEvent> for Catalog {}

pub struct Catalog {
    store: Option<Handle>,
    listening_reads: Option<Handle>,
    statistics_reads: Option<Handle>,
    listening_changes: Option<crate::listening::Changes>,
    cache_dir: Option<PathBuf>,
    pub collections: Vec<Collection>,
    pub playlists: Vec<store::Playlist>,
    marks: Arc<HashMap<TrackId, Mark>>,
    located: Arc<HashMap<Locator, TrackId>>,
    listening: Arc<HashMap<TrackId, crate::listening::Totals>>,
    listening_locations: Arc<HashMap<Locator, TrackId>>,
    listening_keys: HashMap<TrackId, Locator>,
    scanning: Option<(RootId, Arc<AtomicBool>)>,
    pending: VecDeque<RootId>,
    pub loaded: bool,
    playing_lookup: store::Ticket,
    playing_track: Option<Arc<Track>>,
    playing_disc: Option<store::query::RegisteredDisc>,
}

impl Catalog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = Store::location().map(Handle::spawn);
        let mut this = Self {
            store,
            listening_reads: None,
            statistics_reads: None,
            listening_changes: None,
            cache_dir: crate::profile::location().map(|p| p.with_file_name("Cache")),
            collections: vec![],
            playlists: vec![],
            marks: Arc::default(),
            located: Arc::default(),
            listening: Arc::default(),
            listening_locations: Arc::default(),
            listening_keys: HashMap::new(),
            scanning: None,
            pending: VecDeque::new(),
            loaded: false,
            playing_lookup: Default::default(),
            playing_track: None,
            playing_disc: None,
        };
        this.load(true, cx);
        this
    }

    pub fn watch_listening(&mut self, updates: crate::listening::Updates, cx: &mut Context<Self>) {
        let Some(store) = self.listening_handle() else {
            return;
        };
        self.listening_changes = Some(updates.changes.clone());
        let pending = updates.changes.pending();
        let changed = updates.changed;
        let notice = updates.error;
        cx.spawn(async move |this, cx| {
            let mut initial = true;
            let mut index_error = None;
            while initial || changed.recv().await.is_ok() {
                let changes = std::mem::take(&mut *pending.lock().unwrap());
                let notice = notice.lock().unwrap().take();
                if this
                    .update(cx, |_, cx| {
                        if let Some(notice) = notice {
                            cx.emit(CatalogEvent::Notice(format!("Listening history: {notice}")));
                        }
                        if changes.iter().any(|c| c.removed) {
                            cx.emit(CatalogEvent::ListeningRemoved);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                let rebuild = initial || changes.iter().any(|c| c.track.is_none() && c.order);
                initial = false;
                let ids =
                    (!rebuild).then(|| changes.iter().filter_map(|c| c.track).collect::<Vec<_>>());
                let rows = if ids.as_ref().is_some_and(Vec::is_empty) {
                    Ok(Vec::new())
                } else {
                    store.call(move |s| s.listening_index(ids)).await
                };
                if this
                    .update(cx, |state, cx| match rows {
                        Ok(rows) => {
                            index_error = None;
                            if rebuild {
                                state.listening = Arc::default();
                                state.listening_locations = Arc::default();
                                state.listening_keys.clear();
                            }
                            for (id, locator, totals) in rows {
                                if let Some(key) = state.listening_keys.remove(&id)
                                    && state.listening_locations.get(&key) == Some(&id)
                                {
                                    Arc::make_mut(&mut state.listening_locations).remove(&key);
                                }
                                if totals.credited_us == 0 {
                                    Arc::make_mut(&mut state.listening).remove(&id);
                                } else {
                                    Arc::make_mut(&mut state.listening).insert(id, totals);
                                    if let Some(locator) = locator {
                                        state.listening_keys.insert(id, locator.clone());
                                        Arc::make_mut(&mut state.listening_locations)
                                            .insert(locator, id);
                                    }
                                }
                            }
                            if rebuild || !changes.is_empty() {
                                cx.emit(CatalogEvent::Listening(changes));
                            }
                        }
                        Err(e) => {
                            initial = true;
                            let message = format!("Listening history: {e:#}");
                            if index_error.as_ref() != Some(&message) {
                                cx.emit(CatalogEvent::Notice(message.clone()));
                                index_error = Some(message);
                            }
                            if !changes.is_empty() {
                                cx.emit(CatalogEvent::Listening(changes));
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
                if initial {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                }
            }
        })
        .detach();
    }
    pub fn listening_handle(&mut self) -> Option<Handle> {
        if self.listening_reads.is_none() {
            self.listening_reads = self
                .store
                .as_ref()
                .map(|s| Handle::spawn(s.path().to_path_buf()));
        }
        self.listening_reads.clone()
    }
    pub fn statistics_handle(&mut self) -> Option<Handle> {
        if self.statistics_reads.is_none() {
            self.statistics_reads = self
                .store
                .as_ref()
                .map(|s| Handle::spawn(s.path().to_path_buf()));
        }
        self.statistics_reads.clone()
    }
    fn refresh_listening_locations(&self) {
        if let Some(changes) = &self.listening_changes {
            changes.publish(crate::listening::Change {
                track: None,
                order: true,
                removed: false,
            });
        }
    }

    pub fn listening_of(&self, track: &Track) -> crate::listening::Totals {
        track
            .library_id
            .or_else(|| self.listening_locations.get(&Locator::of(track)).copied())
            .and_then(|id| self.listening.get(&id).copied())
            .unwrap_or_default()
    }
    pub fn listening_snapshot(&self) -> impl Fn(&Track) -> crate::listening::Totals + Send + use<> {
        let totals = self.listening.clone();
        let located = self.listening_locations.clone();
        move |track| {
            track
                .library_id
                .or_else(|| located.get(&Locator::of(track)).copied())
                .and_then(|id| totals.get(&id).copied())
                .unwrap_or_default()
        }
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
                        if collection.online != online {
                            cx.emit(CatalogEvent::Structure(id));
                        }
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
        self.marks = Arc::default();
        self.located = Arc::default();
        self.apply_marks(marks);
    }
    fn apply_marks(&mut self, marks: Vec<store::TrackMark>) {
        for (id, locator, mark) in marks {
            if mark == Mark::default() {
                Arc::make_mut(&mut self.marks).remove(&id);
                if let Some(locator) = locator {
                    Arc::make_mut(&mut self.located).remove(&locator);
                }
            } else {
                Arc::make_mut(&mut self.marks).insert(id, mark);
                if let Some(locator) = locator {
                    Arc::make_mut(&mut self.located).insert(locator, id);
                }
            }
        }
    }
    /// Resolve a physical queue entry once for all indexed browser headers.
    pub fn index_playing(
        &mut self,
        track: Option<Arc<Track>>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        if !refresh && self.playing_track == track {
            return;
        }
        let revision = self.playing_lookup.next();
        self.playing_track = track.clone();
        self.playing_disc = None;
        let (Some(store), Some(track)) = (&self.store, track) else {
            return;
        };
        let lookup = store.query(&self.playing_lookup, revision, move |s| {
            s.registered_disc(&track)
        });
        cx.spawn(async move |this, cx| {
            let disc = lookup.await.ok().flatten();
            let _ = this.update(cx, |c, cx| {
                if c.playing_lookup.is_current(revision) {
                    c.playing_disc = disc;
                    cx.emit(CatalogEvent::PlayingDisc);
                }
            });
        })
        .detach();
    }

    pub fn playing_album(&self, track: &Track) -> Option<store::AlbumId> {
        self.playing_track.as_ref().filter(|t| t.same_item(track))?;
        Some(self.playing_disc.as_ref()?.album)
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

    pub fn folder_marks(&self) -> impl Fn(&Track) -> Mark + use<> {
        let (located, marks) = (self.located.clone(), self.marks.clone());
        move |t| {
            located
                .get(&Locator::of(t))
                .and_then(|id| marks.get(id))
                .copied()
                .unwrap_or_default()
        }
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
        let offline = self.offline_roots();
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
                        row.available = Self::available(row, &offline, &mut files)
                            && (scope != Scope::RecentlyPlayed || row.track.source.is_file());
                    }
                    Ok(rows)
                })
                .await
        }
    }

    /// Capture order once. Only one batch of database rows and availability
    /// evidence is retained alongside the queue's shared track descriptors.
    pub fn listening_list(
        &self,
        list: ListeningList,
        chosen: Option<i64>,
        ticket: store::Ticket,
        generation: u64,
        cx: &App,
    ) -> impl Future<Output = anyhow::Result<(Selection, usize, usize)>> + use<> {
        let handle = self.handle();
        let offline = self.offline_roots();
        let source = list.source(self);
        cx.background_spawn(async move {
            let store = handle.ok_or_else(|| anyhow::anyhow!("The library is unavailable."))?;
            let (selection, start, skipped) = list
                .resolve(store, chosen, ticket, generation, offline)
                .await?;
            Ok((selection.named(source), start, skipped))
        })
    }

    fn offline_roots(&self) -> HashSet<RootId> {
        self.collections
            .iter()
            .filter(|c| !c.online)
            .map(|c| c.root.id)
            .collect()
    }

    fn available(row: &Row, offline: &HashSet<RootId>, files: &mut store::DetachedFiles) -> bool {
        row.available
            && !row.root.is_some_and(|id| offline.contains(&id))
            && !row.source_root.is_some_and(|id| offline.contains(&id))
            && files.available(row)
    }

    pub fn queue_selection(
        &self,
        targets: Targets,
        cx: &App,
    ) -> impl Future<Output = anyhow::Result<Selection>> + use<> {
        let (tracks, pending) = match targets {
            Targets::Tracks(tracks) => (tracks, None),
            Targets::Keys(scope, keys) => (
                Vec::new(),
                Some((
                    scope,
                    self.rows(scope, keys, &store::Ticket::default(), 0, cx),
                )),
            ),
        };
        async move {
            match pending {
                None => Ok(TrackList::from(tracks).into()),
                Some((scope, rows)) => Ok(Selection::from_rows(
                    scope,
                    rows.await?.into_iter().filter(|r| r.available),
                )),
            }
        }
    }
    /// Resolve an authored sequence independently of the visible search, sort
    /// and loaded page. Unregistered music is read from its own directory.
    pub async fn program(
        catalog: Entity<Self>,
        track: Arc<Track>,
        context: DiscContext,
        cx: &AsyncApp,
    ) -> anyhow::Result<TrackList> {
        if !context.allows(&track) {
            return Ok(Arc::default());
        }
        let store = catalog.read_with(cx, |c, _| c.handle());
        if let Some(store) = store {
            let target = track.clone();
            if let Some(disc) = store.call(move |s| s.registered_disc(&target)).await? {
                let tracks = Self::album_tracks(catalog, disc.album, cx).await?;
                if tracks
                    .iter()
                    .any(|t| t.same_item(&track) && context.allows(t))
                {
                    return Ok(tracks);
                }
                return Ok(Arc::default());
            }
        }
        let anchor = track.disc.as_ref().map_or(&track.source, |d| &d.sheet);
        let folder = anchor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("The CD folder is unavailable."))?
            .to_owned();
        let cache_dir = catalog.read_with(cx, |c, _| c.cache_dir.clone());
        cx.background_spawn(async move {
            let cache = cache_dir.and_then(|dir| Cache::open(&dir).ok());
            crate::discovery::local_program(&folder, &track, context, cache.as_ref())
                .map(|program| program.tracks)
        })
        .await
    }
    pub async fn album_tracks(
        catalog: Entity<Self>,
        album: store::AlbumId,
        cx: &AsyncApp,
    ) -> anyhow::Result<TrackList> {
        Self::authored_tracks(catalog, Scope::Album(album), cx).await
    }
    /// The available tracks of an indexed disc, or of every disc of a
    /// release with [`Scope::Release`], in authored order.
    pub async fn authored_tracks(
        catalog: Entity<Self>,
        scope: Scope,
        cx: &AsyncApp,
    ) -> anyhow::Result<TrackList> {
        let store = catalog
            .read_with(cx, |c, _| c.handle())
            .ok_or_else(|| anyhow::anyhow!("The library is unavailable."))?;
        let keys = store.call(move |s| s.disc_keys(scope)).await?;
        catalog
            .read_with(cx, |c, cx| {
                c.queue_selection(Targets::Keys(scope, keys), cx)
            })
            .await
            .map(|selection| selection.tracks)
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
                    c.refresh_listening_locations();
                    cx.emit(CatalogEvent::Changed);
                    cx.emit(CatalogEvent::Content);
                    cx.emit(CatalogEvent::Structure(id));
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
            // Scan transactions can commit file moves before later phases finish,
            // including when a scan is cancelled.
            self.refresh_listening_locations();
            cx.emit(CatalogEvent::Structure(id));
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
        } else if let Ok(Progress::Indexed(summary)) = message {
            self.refresh_listening_locations();
            if !summary.relocated_folders.is_empty() {
                cx.emit(CatalogEvent::SourcesMoved(summary.relocated_folders));
            }
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

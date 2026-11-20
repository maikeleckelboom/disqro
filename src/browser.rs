use crate::{
    catalog::{Catalog, CatalogEvent},
    collection::{Disc, DiscId, Discs, ViewPrefs},
    discovery::Scanner,
    folders::Folders,
    library::{self, Track, TrackList},
    player::{Player, PlayerEvent},
    store::{PlaylistId, RootId, Scope, Term, Ticket},
    table::{LayoutColumn, MusicTable, reselect, scroll_to_top, set_rows_in_place},
};
use gpui_kit::{
    component::{
        input::{InputEvent, InputState},
        table::{TableEvent, TableState},
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// A folder that has not listed anything by then says it is opening, rather
/// than flashing that message for every folder.
const OPENING_NOTICE: Duration = Duration::from_millis(250);

const HISTORY: usize = 100;

/// What the Recordings panel shows: a folder on disk, read as it is, or a
/// view of the persistent library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Location {
    Folder(PathBuf),
    /// One registered collection, or all of them.
    Library(Option<RootId>),
    Album {
        root: Option<RootId>,
        id: crate::store::AlbumId,
        title: String,
    },
    Liked,
    Playlist(PlaylistId),
}
impl Location {
    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

/// What to do with a file once its folder is open.
#[derive(Clone)]
pub enum Reveal {
    Play(PathBuf),
    Select(PathBuf),
    Track(Arc<Track>),
}

pub struct ViewChanged;
impl EventEmitter<ViewChanged> for Browser {}

pub struct Browser {
    /// The current folder, or the last one while a library view is shown.
    pub path: PathBuf,
    pub location: Location,
    pub album: Option<String>,
    pub folder_count: usize,
    pub notice: Option<String>,
    /// Discovery of the open folder is running.
    pub busy: bool,
    /// It has run long enough to say so.
    pub slow: bool,
    /// The open folder is shown with its subfolders, as a collection.
    pub subfolders: bool,
    pub folders: Entity<Folders>,
    pub table: Entity<TableState<MusicTable>>,
    pub search: Entity<InputState>,
    pub catalog: Entity<Catalog>,
    back: Vec<Location>,
    forward: Vec<Location>,
    raw: Vec<Option<Arc<Track>>>,
    pub prefs: ViewPrefs,
    pub discs: Entity<TableState<Discs>>,
    pub disc: Option<DiscId>,
    pub count: usize,
    pub duration: Option<f64>,
    covers: std::collections::HashMap<PathBuf, crate::artwork::CoverFile>,
    unreadable: Arc<std::collections::HashSet<PathBuf>>,
    equivalent_sheets: std::collections::HashMap<PathBuf, PathBuf>,
    scanner: Scanner,
    task: Option<Task<()>>,
    selected: Option<Arc<Track>>,
    generation: u64,
    query: String,
    albums: Ticket,
}
pub fn music_home() -> PathBuf {
    crate::places::known_path(crate::places::Known::Music)
        .or_else(|| crate::places::known_path(crate::places::Known::Home))
        .unwrap_or_else(|| PathBuf::from("."))
}
impl Browser {
    pub fn new(
        player: Entity<Player>,
        catalog: Entity<Catalog>,
        columns: Vec<LayoutColumn>,
        prefs: ViewPrefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folders = cx.new(Folders::new);
        let browser = cx.weak_entity();
        let table = cx.new(|cx| {
            TableState::new(
                MusicTable::new(player.clone(), catalog.clone(), columns, browser),
                window,
                cx,
            )
            .select_on_modifier_click(false)
            .col_selectable(false)
            .sortable(false)
        });
        let discs = cx.new(|cx| {
            TableState::new(Discs::default(), window, cx)
                .col_selectable(false)
                .sortable(false)
        });
        cx.subscribe_in(&discs, window, |b: &mut Self, _, event, window, cx| {
            if let TableEvent::DoubleClickedRow(i) = event {
                b.show_disc(*i, window, cx);
            }
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find tracks"));
        cx.subscribe(&player, |b: &mut Self, _, ev, cx| {
            b.table.update(cx, |t, cx| match ev {
                PlayerEvent::TrackChanged => cx.notify(),
                PlayerEvent::PlaySelection => t.delegate().play(t.selected_row().unwrap_or(0), cx),
                PlayerEvent::QueueChanged | PlayerEvent::StateChanged => {}
            })
        })
        .detach();
        cx.subscribe(&search, |s: &mut Self, e, ev, cx| {
            if matches!(ev, InputEvent::Change) {
                s.query = e.read(cx).value().trim().to_lowercase();
                s.filter(cx);
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&table, |_: &mut Self, t, event, cx| match event {
            TableEvent::SelectRow(_) => t.update(cx, |t, _| t.delegate_mut().clear_marks()),
            TableEvent::DoubleClickedRow(i) => t.update(cx, |t, cx| t.delegate().play(*i, cx)),
            TableEvent::ColumnWidthsChanged(widths) => {
                t.update(cx, |t, _| t.delegate_mut().resize(widths))
            }
            _ => {}
        })
        .detach();
        cx.subscribe(&catalog, |b: &mut Self, _, event, cx| match event {
            CatalogEvent::Content => b.library_changed(cx),
            CatalogEvent::Changed => {
                let catalog = b.catalog.read(cx);
                let roots: Option<Vec<PathBuf>> = catalog.loaded.then(|| {
                    catalog
                        .collections
                        .iter()
                        .map(|c| c.root.path.clone())
                        .collect()
                });
                if let Some(roots) = roots {
                    b.folders.update(cx, |f, cx| f.set_roots(roots, cx));
                }
                // A removed collection or playlist cannot stay open.
                let gone = match &b.location {
                    Location::Library(Some(r)) | Location::Album { root: Some(r), .. } => {
                        b.catalog.read(cx).collection(*r).is_none()
                    }
                    Location::Playlist(p) => b.catalog.read(cx).playlist(*p).is_none(),
                    _ => false,
                };
                if gone && b.catalog.read(cx).loaded {
                    b.location = Location::Library(None);
                    b.disc = None;
                    b.filter(cx);
                    cx.emit(ViewChanged);
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        Self {
            path: PathBuf::new(),
            location: Location::Folder(PathBuf::new()),
            album: None,
            folders,
            table,
            search,
            catalog,
            raw: vec![],
            prefs,
            discs,
            disc: None,
            count: 0,
            duration: None,
            covers: Default::default(),
            unreadable: Default::default(),
            equivalent_sheets: Default::default(),
            scanner: Scanner::new(),
            task: None,
            selected: None,
            generation: Default::default(),
            query: String::new(),
            back: vec![],
            forward: vec![],
            notice: None,
            busy: false,
            slow: false,
            subfolders: false,
            folder_count: 0,
            albums: Ticket::default(),
        }
    }
    pub fn revision(&self) -> u64 {
        self.generation
    }
    pub fn list_focus(&self, cx: &App) -> FocusHandle {
        if self.listing_discs() {
            self.discs.read(cx).focus_handle(cx)
        } else {
            self.table.read(cx).focus_handle(cx)
        }
    }
    /// Albums / CDs applies to collections: a folder shown with its
    /// subfolders, or a library collection. A folder on its own lists its
    /// recordings.
    pub fn can_list_discs(&self) -> bool {
        match self.location {
            // A folder without subfolders is one album or a loose set even
            // inside a collection: its tracks are what it has to show.
            Location::Folder(_) => self.subfolders && self.folder_count > 0,
            Location::Library(_) => true,
            _ => false,
        }
    }
    pub fn listing_discs(&self) -> bool {
        self.prefs.albums && self.disc.is_none() && self.can_list_discs()
    }
    /// The library view the table shows, if any.
    fn scope(&self) -> Option<Scope> {
        Some(match &self.location {
            Location::Folder(_) => return None,
            Location::Library(root) => Scope::Library(*root),
            Location::Album { id, .. } => Scope::Album(*id),
            Location::Liked => Scope::Liked,
            Location::Playlist(p) => Scope::Playlist(*p),
        })
    }
    pub fn show_disc(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.discs.read(cx).delegate().items.get(ix) {
            if let (Location::Library(root), Some(id)) = (&self.location, d.id.album()) {
                self.show(
                    Location::Album {
                        root: *root,
                        id,
                        title: d.title.clone(),
                    },
                    window,
                    cx,
                );
                return;
            }
            self.disc = Some(d.id.clone());
            self.table.update(cx, |t, _| scroll_to_top(t));
            self.filter(cx);
            self.list_focus(cx).focus(window, cx);
            cx.notify();
        }
    }
    /// Shows the open folder with or without its subfolders, and remembers
    /// the choice for it and the folders inside it.
    pub fn set_subfolders(&mut self, include: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Location::Folder(path) = &self.location {
            self.prefs.set_subfolders(path, include);
            cx.emit(ViewChanged);
            self.reload(window, cx);
        }
    }
    pub fn view(&mut self, albums: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.albums = albums;
        if let Location::Album { root, .. } = self.location {
            self.show(Location::Library(root), window, cx);
            return;
        }
        cx.emit(ViewChanged);
        self.disc = None;
        self.filter(cx);
        self.list_focus(cx).focus(window, cx);
        cx.notify();
    }
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
    fn filter(&mut self, cx: &mut Context<Self>) {
        self.albums.next();
        if let Some(scope) = self.scope() {
            self.filter_library(scope, cx);
        } else {
            self.filter_folder(false, cx);
        }
    }
    /// Projects the folder's tracks. `discovered` keeps the rows on screen
    /// in place while discovery adds to them.
    fn filter_folder(&mut self, discovered: bool, cx: &mut App) {
        let terms = Term::parse(&self.query);
        let catalog = self.catalog.read(cx);
        let rows: TrackList = self
            .raw
            .iter()
            .flatten()
            .filter(|t| {
                self.disc.as_ref().is_none_or(|d| d.contains(t))
                    && crate::collection::matches(t, &terms, || catalog.mark_of(t))
            })
            .cloned()
            .collect();
        self.count = rows.len();
        self.duration = rows.iter().map(|t| t.duration).sum();
        self.album = rows
            .first()
            .filter(|t| !t.album.is_empty() && rows.iter().all(|r| r.album == t.album))
            .map(|t| t.album.clone());
        if self.listing_discs() {
            let items = Discs::group(&rows, &self.covers, &self.path);
            self.set_discs(items, cx);
        }
        let only_folders = !self.busy
            && self.query.is_empty()
            && !self.subfolders
            && self.folder_count > 0
            && self.raw.iter().flatten().next().is_none();
        let empty: SharedString = if self.busy {
            if self.slow {
                "Opening folder…".into()
            } else {
                SharedString::default()
            }
        } else if !self.query.is_empty() {
            "No matching tracks".into()
        } else if self.notice.is_some() && self.raw.is_empty() && self.folder_count == 0 {
            SharedString::default()
        } else if only_folders {
            "No recordings in this folder".into()
        } else if self.subfolders {
            "No supported recordings in this folder or its subfolders".into()
        } else {
            "No supported recordings in this folder".into()
        };
        self.table.update(cx, |t, cx| {
            if let Some(selected) = t
                .selected_row()
                .and_then(|i| t.delegate().track(i))
                .cloned()
            {
                self.selected = Some(selected);
            }
            if t.delegate().has_marks() {
                self.selected = None;
            }
            let selected = self.selected.as_ref();
            let can_start = !rows.is_empty();
            t.delegate().player.update(cx, |p, cx| {
                if p.can_start != can_start {
                    p.can_start = can_start;
                    cx.notify();
                }
            });
            t.delegate_mut().empty = empty;
            t.delegate_mut().offer_subfolders = only_folders;
            t.delegate_mut().unreadable = self.unreadable.clone();
            if discovered {
                set_rows_in_place(t, rows, cx);
            } else {
                t.delegate_mut().set_rows(rows, cx);
            }
            let ix = selected.and_then(|item| {
                (0..t.delegate().len())
                    .find(|&i| t.delegate().track(i).is_some_and(|r| r.same_item(item)))
            });
            reselect(t, ix, cx);
            cx.notify();
        });
    }
    fn set_discs(&mut self, items: Vec<Disc>, cx: &mut App) {
        self.discs.update(cx, |table, cx| {
            let selected = table
                .selected_row()
                .and_then(|i| table.delegate().items.get(i))
                .map(|d| d.id.clone());
            table.delegate_mut().items = items;
            let ix = selected.and_then(|id| table.delegate().items.iter().position(|d| d.id == id));
            reselect(table, ix, cx);
            cx.notify();
        });
    }
    fn filter_library(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.album = None;
        self.busy = false;
        let empty: SharedString = if !self.query.is_empty() {
            "No matching tracks".into()
        } else {
            match &self.location {
                Location::Liked => {
                    "No liked tracks yet. Like a track from its menu or with Alt+L.".into()
                }
                Location::Playlist(_) => {
                    "This playlist is empty. Add tracks from their menu or drag them here.".into()
                }
                Location::Library(_) if self.catalog.read(cx).collections.is_empty() => {
                    "Add a music folder to the library to browse it here.".into()
                }
                _ => "No tracks found yet".into(),
            }
        };
        self.table.update(cx, |t, _| {
            t.delegate_mut().empty = empty;
            t.delegate_mut().offer_subfolders = false;
        });
        let search = self.query.clone();
        if self.listing_discs() {
            self.load_albums(cx);
        }
        self.table
            .update(cx, |t, cx| t.delegate_mut().show_library(scope, search, cx));
    }
    fn load_albums(&mut self, cx: &mut Context<Self>) {
        let (Location::Library(root), Some(store)) =
            (self.location.clone(), self.catalog.read(cx).handle())
        else {
            return;
        };
        let search = self.query.clone();
        let generation = self.albums.next();
        let albums = store.query(&self.albums, generation, move |s| s.albums(root, &search));
        let ticket = self.albums.clone();
        cx.spawn(async move |this, cx| {
            let result = match albums.await {
                Ok(albums) => {
                    cx.background_spawn(async move {
                        let mut items = Vec::with_capacity(albums.len());
                        for album in albums {
                            if !ticket.is_current(generation) {
                                return Err(crate::store::Cancelled.into());
                            }
                            items.push(Disc::from_album(album));
                        }
                        Ok(items)
                    })
                    .await
                }
                Err(e) => Err(e),
            };
            let _ = this.update(cx, |b, cx| {
                if !b.albums.is_current(generation)
                    || b.location != Location::Library(root)
                    || !b.listing_discs()
                {
                    return;
                }
                let items = match result {
                    Ok(items) => items,
                    Err(e) if crate::store::is_cancelled(&e) => return,
                    Err(e) => {
                        b.notice = Some(format!("{e:#}"));
                        Vec::new()
                    }
                };
                b.count = items.iter().map(|d| d.count).sum();
                b.duration = Some(items.iter().map(Disc::duration).sum());
                b.set_discs(items, cx);
                cx.notify();
            });
        })
        .detach();
    }
    /// The table finished listing a library view.
    pub fn library_listed(&mut self, count: usize, duration: f64, cx: &mut Context<Self>) {
        if self.scope().is_some() {
            self.table
                .read(cx)
                .delegate()
                .player
                .clone()
                .update(cx, |p, cx| {
                    if p.can_start != (count > 0) {
                        p.can_start = count > 0;
                        cx.notify();
                    }
                });
        }
        if self.scope().is_some() && !self.listing_discs() {
            self.count = count;
            self.duration = Some(duration);
            cx.notify();
        }
    }
    /// Likes, ratings, playlists or an index scan changed library rows.
    fn library_changed(&mut self, cx: &mut Context<Self>) {
        match self.scope() {
            Some(scope) if self.listing_discs() => self.filter_library(scope, cx),
            Some(_) => self.table.update(cx, |t, cx| t.delegate_mut().query(cx)),
            // Folder rows only need filtering again when marks decide them.
            None if Term::parse(&self.query)
                .iter()
                .any(|t| matches!(t, Term::Rating(_) | Term::Liked(_))) =>
            {
                self.filter_folder(false, cx)
            }
            None => self.table.update(cx, |_, cx| cx.notify()),
        }
    }

    /// Opens a folder and records the current view in the Back history.
    pub fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_revealing(path, None, window, cx);
    }
    pub fn open_revealing(
        &mut self,
        path: PathBuf,
        reveal: Option<Reveal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember(&Location::Folder(path.clone()));
        self.load(path, reveal, window, cx);
    }
    /// Opens a Place: the folder tree is rooted at it.
    pub fn open_place(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.remember(&Location::Folder(path.clone()));
        self.load_from(path, None, true, window, cx);
    }
    /// Shows a library view and records the current view in the history.
    pub fn show(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.remember(&location);
        self.enter(location, window, cx);
    }
    fn remember(&mut self, next: &Location) {
        let same = match (&self.location, next) {
            (Location::Folder(a), Location::Folder(b)) => {
                a.as_os_str().is_empty() || crate::places::same_path(a, b)
            }
            (a, b) => a == b,
        };
        if !same {
            self.back.push(self.location.clone());
            if self.back.len() > HISTORY {
                self.back.remove(0);
            }
            self.forward.clear();
        }
    }
    fn enter(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        match location {
            Location::Folder(path) => self.load(path, None, window, cx),
            location => {
                if self.location != location {
                    self.disc = None;
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.query.clear();
                    self.table.update(cx, |t, _| scroll_to_top(t));
                    self.discs.update(cx, |t, _| scroll_to_top(t));
                }
                self.task.take();
                self.generation += 1;
                self.busy = false;
                self.notice = None;
                self.raw.clear();
                self.location = location;
                self.count = 0;
                self.duration = None;
                // The tree keeps the last folder in view without marking it
                // as the current location.
                let path = self.path.clone();
                self.folders.update(cx, |f, cx| {
                    if !path.as_os_str().is_empty() && f.is_empty() {
                        f.show(&path, false, cx);
                    }
                    f.set_highlight(false, cx);
                });
                cx.emit(ViewChanged);
                self.filter(cx);
                self.list_focus(cx).focus(window, cx);
                cx.notify();
            }
        }
    }
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.back.pop() {
            self.forward.push(self.location.clone());
            self.enter(l, window, cx);
        }
    }
    pub fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.forward.pop() {
            self.back.push(self.location.clone());
            self.enter(l, window, cx);
        }
    }
    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.location {
            Location::Folder(_) => {
                if let Some(parent) = self.path.parent().map(Path::to_owned) {
                    self.open(parent, window, cx);
                }
            }
            Location::Album { root, .. } => self.show(Location::Library(*root), window, cx),
            _ if self.disc.is_some() => {
                self.disc = None;
                self.filter(cx);
                self.list_focus(cx).focus(window, cx);
                cx.notify();
            }
            _ => {}
        }
    }
    pub fn can_go_up(&self) -> bool {
        match self.location {
            Location::Folder(_) => self.path.parent().is_some(),
            Location::Album { .. } => true,
            _ => self.disc.is_some(),
        }
    }
    pub fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.revision();
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open music folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = result.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |b, w, cx| {
                    if b.revision() == revision {
                        b.open(path, w, cx);
                    }
                });
            }
        })
        .detach();
    }
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.location.clone() {
            Location::Folder(_) => {
                let path = self.path.clone();
                self.folders.update(cx, |f, cx| f.refresh(&path, cx));
                self.load(path, None, window, cx);
            }
            Location::Library(root) | Location::Album { root, .. } => {
                let ids: Vec<RootId> = match root {
                    Some(r) => vec![r],
                    None => self
                        .catalog
                        .read(cx)
                        .collections
                        .iter()
                        .map(|c| c.root.id)
                        .collect(),
                };
                self.catalog.update(cx, |c, cx| {
                    for id in ids {
                        c.rescan(id, cx);
                    }
                });
            }
            _ => self.library_changed(cx),
        }
    }
    pub fn load(
        &mut self,
        path: PathBuf,
        reveal: Option<Reveal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_from(path, reveal, false, window, cx);
    }
    /// Opens `path` and lists it. `place` roots the folder tree at it.
    fn load_from(
        &mut self,
        path: PathBuf,
        reveal: Option<Reveal>,
        place: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_folder = self.location.is_folder() && crate::places::same_path(&self.path, &path);
        if !same_folder {
            self.selected = None;
            self.table.update(cx, |t, cx| {
                t.delegate_mut().clear_marks();
                t.clear_selection(cx);
                scroll_to_top(t);
            });
            self.discs.update(cx, |t, cx| {
                t.clear_selection(cx);
                scroll_to_top(t);
            });
        }
        self.generation += 1;
        let revision = self.generation;
        let timing = std::env::var_os("DISQRO_TRACE_DISCOVERY").map(|_| std::time::Instant::now());
        self.task.take();
        self.subfolders = self.prefs.includes_subfolders(&path);
        let events = self.scanner.open(path.clone(), self.subfolders);
        self.location = Location::Folder(path.clone());
        cx.emit(ViewChanged);
        self.folders.update(cx, |f, cx| f.show(&path, place, cx));
        self.path = path;
        self.busy = true;
        self.slow = false;
        if !same_folder {
            self.disc = None;
        }
        self.raw.clear();
        self.covers.clear();
        self.unreadable = Default::default();
        self.equivalent_sheets.clear();
        self.notice = None;
        self.folder_count = 0;
        if !same_folder || reveal.is_some() {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            self.query.clear();
        }
        self.filter(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPENING_NOTICE).await;
            let _ = this.update(cx, |b, cx| {
                if b.generation == revision && b.busy {
                    b.slow = true;
                    b.filter_folder(true, cx);
                    cx.notify();
                }
            });
        })
        .detach();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut reveal = reveal;
            let mut first = true;
            let mut handling = (0usize, std::time::Duration::ZERO, std::time::Duration::ZERO);
            while let Ok(batch) = events.recv().await {
                let started = timing.map(|_| std::time::Instant::now());
                if this
                    .update_in(cx, |b, window, cx| {
                        if b.generation != revision {
                            return;
                        }
                        if let Some(root) = batch.root {
                            b.folder_count = root.dirs.len();
                            let path = b.path.clone();
                            b.folders
                                .update(cx, |f, cx| f.listed(&path, &root.dirs, cx));
                        }
                        b.covers.extend(batch.covers);
                        if !batch.unreadable.is_empty() {
                            Arc::make_mut(&mut b.unreadable).extend(batch.unreadable);
                        }
                        b.equivalent_sheets.extend(batch.equivalent_sheets);
                        if b.notice.is_none() {
                            b.notice = batch.notice;
                        }
                        for (i, track) in batch.rows {
                            if i == b.raw.len() {
                                b.raw.push(track);
                            } else {
                                b.raw[i] = track;
                            }
                        }
                        b.albums.next();
                        b.filter_folder(true, cx);
                        if first && b.count > 0 {
                            first = false;
                            if let Some(start) = timing {
                                eprintln!(
                                    "Discovery {revision} first table update {:?} ({} tracks)",
                                    start.elapsed(),
                                    b.count
                                );
                            }
                        }
                        if let Some(request) = &reveal
                            && b.reveal(request, window, cx)
                        {
                            reveal = None;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                if let Some(started) = started {
                    let spent = started.elapsed();
                    handling = (handling.0 + 1, handling.1 + spent, handling.2.max(spent));
                }
            }
            let _ = this.update(cx, |b, cx| {
                if b.generation == revision {
                    b.busy = false;
                    if let Some(start) = timing {
                        eprintln!(
                            "Discovery {revision} finished {:?} ({} tracks; {} batches, interface {:?}, longest {:?})",
                            start.elapsed(),
                            b.raw.iter().flatten().count(),
                            handling.0,
                            handling.1,
                            handling.2
                        );
                    }
                    if reveal.is_some() && b.notice.is_none() {
                        b.notice = Some("This recording is unavailable.".into());
                    }
                    b.filter(cx);
                    cx.notify();
                }
            });
        }));
    }
    fn reveal(&mut self, request: &Reveal, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let table = self.table.read(cx).delegate();
        let item = (0..table.len())
            .filter_map(|i| table.track(i))
            .find(|t| match request {
                Reveal::Track(wanted) => wanted.same_item(t),
                Reveal::Select(path) | Reveal::Play(path) => {
                    let path = self.equivalent_sheets.get(path).unwrap_or(path);
                    crate::places::same_path(&t.source, path)
                        || t.disc
                            .as_ref()
                            .is_some_and(|d| crate::places::same_path(&d.sheet, path))
                }
            })
            .cloned();
        let Some(item) = item else { return false };
        if self.prefs.albums && self.can_list_discs() {
            let list_had_focus = self.list_focus(cx).is_focused(window);
            self.disc = Some(DiscId::of(&item));
            self.filter(cx);
            if list_had_focus {
                self.list_focus(cx).focus(window, cx);
            }
        }
        self.table.update(cx, |t, cx| {
            let found = (0..t.delegate().len())
                .find(|&i| t.delegate().track(i).is_some_and(|t| t.same_item(&item)));
            if let Some(i) = found {
                t.set_selected_row(i, cx);
                t.scroll_to_row(i, cx);
                if matches!(request, Reveal::Play(_)) {
                    t.delegate().play(i, cx);
                }
            }
        });
        true
    }
    /// A short name for the current view.
    pub fn title(&self, cx: &App) -> String {
        let catalog = self.catalog.read(cx);
        match &self.location {
            Location::Folder(_) => self
                .album
                .clone()
                .unwrap_or_else(|| library::display_name(&self.path)),
            Location::Library(None) => "All Music".into(),
            Location::Library(Some(r)) => catalog
                .collection(*r)
                .map_or_else(|| "Collection".into(), |c| c.root.name.clone()),
            Location::Album { title, .. } => title.clone(),
            Location::Liked => "Liked Tracks".into(),
            Location::Playlist(p) => catalog
                .playlist(*p)
                .map_or_else(|| "Playlist".into(), |p| p.name.clone()),
        }
    }
}

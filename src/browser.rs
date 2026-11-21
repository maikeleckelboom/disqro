mod navigation;
use crate::{
    catalog::{Catalog, CatalogEvent},
    collection::{Disc, DiscId, Discs, Membership, Page, Shelf, ViewPrefs},
    discovery::Scanner,
    folders::Folders,
    library::{self, Track, TrackList},
    player::{Player, PlayerEvent},
    store::{PlaylistId, RootId, Scope, Term, Ticket},
    table::{
        ContextKind, Field, Layouts, MusicTable, RowKey, Sections, reselect, scroll_to_top,
        set_rows_in_place,
    },
};
use gpui_kit::{
    component::{
        input::{InputEvent, InputState},
        table::{ColumnSort, TableEvent, TableState},
    },
    prelude::*,
    *,
};
pub(crate) use navigation::SavedVisit;
pub use navigation::continue_cd;
use navigation::{Bookmark, Visit};
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
    /// Every disc of an indexed release, identified by its first disc.
    Release {
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

#[derive(Clone, Copy, PartialEq)]
pub enum Destination {
    Disc,
    Collection,
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
    /// The Place the open folder is shown under in Locations, when one was chosen.
    pub tree_root: Option<PathBuf>,
    /// The browser that receives navigation from Library and Locations.
    pub(crate) active: bool,
    back: Vec<Visit>,
    forward: Vec<Visit>,
    bookmark: Option<Bookmark>,
    reveal_key: Option<RowKey>,
    navigation: u64,
    programs: Arc<[Arc<Disc>]>,
    folder_ready: bool,
    raw: Vec<Option<Arc<Track>>>,
    pub prefs: ViewPrefs,
    pub discs: Entity<TableState<Discs>>,
    /// A CD or release page of the open folder's collection.
    pub page: Option<Page>,
    /// The release of the CD page shown, folder or indexed.
    pub membership: Option<Arc<Membership>>,
    pub count: usize,
    pub duration: Option<f64>,
    covers: Arc<std::collections::HashMap<PathBuf, crate::artwork::CoverFile>>,
    unreadable: Arc<std::collections::HashSet<PathBuf>>,
    equivalent_sheets: std::collections::HashMap<PathBuf, PathBuf>,
    scanner: Scanner,
    task: Option<Task<()>>,
    selected: Option<RowKey>,
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
        folders: Entity<Folders>,
        columns: Layouts,
        prefs: ViewPrefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let this = cx.weak_entity();
        let table = cx.new(|cx| {
            TableState::new(
                MusicTable::new(player.clone(), catalog.clone(), columns, this.clone()),
                window,
                cx,
            )
            .select_on_modifier_click(false)
            .col_selectable(false)
            // The heading resizes its own columns, refitting them live.
            .col_resizable(false)
            .sortable(false)
        });
        let discs = cx.new(|cx| {
            let discs = Discs {
                browser: Some(this.clone()),
                tracks: Some(table.downgrade()),
                ..Default::default()
            };
            TableState::new(discs, window, cx)
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
            // Only the active browser starts playback from its selection.
            if matches!(ev, PlayerEvent::PlaySelection) && !b.active {
                return;
            }
            b.table.update(cx, |t, cx| match ev {
                PlayerEvent::TrackChanged | PlayerEvent::StateChanged => cx.notify(),
                PlayerEvent::PlaySelection => t.delegate().play(
                    t.selected_row()
                        .filter(|&i| t.delegate().header(i).is_none())
                        .unwrap_or_else(|| t.delegate().first_track()),
                    cx,
                ),
                _ => {}
            })
        })
        .detach();
        cx.subscribe(&search, |s: &mut Self, e, ev, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.emit(ViewChanged);
                let query = e.read(cx).value().trim().to_lowercase();
                if s.query == query {
                    return;
                }
                s.cancel_reveal();
                s.query = query;
                s.filter(cx);
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(
            &table,
            window,
            |b: &mut Self, t, event, w, cx| match event {
                TableEvent::SelectRow(_) => t.update(cx, |t, _| t.delegate_mut().clear_marks()),
                TableEvent::DoubleClickedRow(i) => b.activate(*i, w, cx),
                TableEvent::MoveColumn(_, _) => cx.emit(ViewChanged),
                _ => {}
            },
        )
        .detach();
        cx.subscribe(&catalog, |b: &mut Self, _, event, cx| match event {
            CatalogEvent::Content => b.library_changed(cx),
            CatalogEvent::Changed => {
                // A removed collection or playlist cannot stay open.
                let gone = match &b.location {
                    Location::Library(Some(r))
                    | Location::Album { root: Some(r), .. }
                    | Location::Release { root: Some(r), .. } => {
                        b.catalog.read(cx).collection(*r).is_none()
                    }
                    Location::Playlist(p) => b.catalog.read(cx).playlist(*p).is_none(),
                    _ => false,
                };
                if gone && b.catalog.read(cx).loaded {
                    b.location = Location::Library(None);
                    b.page = None;
                    b.membership = None;
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
            tree_root: None,
            active: false,
            folder_ready: false,
            raw: vec![],
            prefs,
            discs,
            page: None,
            membership: None,
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
            programs: Arc::default(),
            bookmark: None,
            reveal_key: None,
            navigation: 0,
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
    /// Makes this the browser the address bar, Locations and the player's
    /// Play button address, or stops it being that.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        cx.notify();
        if !active {
            return;
        }
        self.sync_tree(cx);
        let can_start = self.count > 0;
        self.table
            .read(cx)
            .delegate()
            .player
            .clone()
            .update(cx, |p, cx| {
                if p.can_start != can_start {
                    p.can_start = can_start;
                    cx.notify();
                }
            });
    }
    /// Shows this browser's folder in Locations, or keeps its last folder in
    /// view without highlighting it while a library view is shown.
    fn sync_tree(&self, cx: &mut App) {
        let path = self.path.clone();
        let root = self.tree_root.clone();
        let folder = self.location.is_folder();
        self.folders.update(cx, |f, cx| {
            if path.as_os_str().is_empty() {
                return;
            }
            if folder {
                f.show(&path, root.as_deref(), cx);
            } else {
                if f.is_empty() {
                    f.show(&path, None, cx);
                }
                f.set_highlight(false, cx);
            }
        });
    }
    /// Opens a saved folder view, under the Place it was shown in.
    pub fn restore_folder(
        &mut self,
        path: PathBuf,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_from(path, None, root, window, cx);
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
        self.prefs.albums && self.page.is_none() && self.can_list_discs()
    }
    /// The library view the table shows, if any.
    fn scope(&self) -> Option<Scope> {
        Some(match &self.location {
            Location::Folder(_) => return None,
            Location::Library(root) => Scope::Library(*root),
            Location::Album { id, .. } => Scope::Album(*id),
            Location::Release { id, .. } => Scope::Release(*id),
            Location::Liked => Scope::Liked,
            Location::Playlist(p) => Scope::Playlist(*p),
        })
    }
    /// The collection a library page belongs to.
    fn library_root(&self) -> Option<RootId> {
        match self.location {
            Location::Library(root)
            | Location::Album { root, .. }
            | Location::Release { root, .. } => root,
            _ => None,
        }
    }
    /// A CD or release page of a collection, folder or indexed.
    pub fn is_page(&self) -> bool {
        self.page.is_some()
            || matches!(
                self.location,
                Location::Album { .. } | Location::Release { .. }
            )
    }
    /// The release shown, or the one the CD shown belongs to.
    pub fn release(&self) -> Option<(DiscId, String)> {
        match (&self.location, &self.page) {
            (Location::Release { id, title, .. }, _) => Some((DiscId::Library(*id), title.clone())),
            (_, Some(Page::Release(key))) => Some((key.clone(), self.title_of(key))),
            _ => self
                .membership
                .as_ref()
                .map(|m| (m.key.clone(), m.title.clone())),
        }
    }
    /// Whether the page shown lists every CD of the release `key`.
    pub fn shows_release(&self, key: &DiscId) -> bool {
        match (&self.location, &self.page) {
            (Location::Release { id, .. }, _) => *key == DiscId::Library(*id),
            (_, Some(Page::Release(shown))) => shown == key,
            _ => false,
        }
    }
    /// What a CD page is called beneath its release in the address bar:
    /// `CD 2` rather than the full title the panel already shows.
    pub fn crumb_title(&self, cx: &App) -> String {
        match &self.membership {
            Some(m) if m.subtitle.is_empty() => format!("CD {}", m.number),
            Some(m) => format!("CD {} · {}", m.number, m.subtitle),
            None => self.title(cx),
        }
    }
    fn title_of(&self, key: &DiscId) -> String {
        self.programs
            .iter()
            .find(|d| &d.id == key)
            .and_then(|d| d.release.as_ref())
            .map_or_else(String::new, |m| m.title.clone())
    }
    pub fn show_disc(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.discs.read(cx).delegate().rows.get(ix).cloned() {
            Some(Shelf::Disc(d)) => self.open_disc(&d, window, cx),
            Some(Shelf::Release(r)) => self.open_release(&r.key, r.title.clone(), window, cx),
            None => {}
        }
    }
    pub fn activate(&mut self, ix: usize, w: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.read(cx).delegate();
        if let Some(disc) = table.header(ix).cloned() {
            self.open_disc(&disc, w, cx);
        } else {
            self.table.update(cx, |t, cx| t.delegate().play(ix, cx));
        }
    }
    pub fn open_disc(&mut self, d: &Disc, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = d.id.album() {
            let root = self.library_root();
            self.show(
                Location::Album {
                    root,
                    id,
                    title: d.title.clone(),
                },
                w,
                cx,
            );
            // Known from the list it was opened from; the store confirms it.
            self.membership = d.release.clone();
        } else {
            self.push_visit(cx);
            self.project_page(Some(Page::Disc(d.id.clone())), w, cx);
        }
    }
    /// Opens the page listing every CD of a release.
    pub fn open_release(
        &mut self,
        key: &DiscId,
        title: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let DiscId::Library(id) = key {
            let root = self.library_root();
            self.show(
                Location::Release {
                    root,
                    id: *id,
                    title,
                },
                w,
                cx,
            );
        } else {
            self.push_visit(cx);
            self.project_page(Some(Page::Release(key.clone())), w, cx);
        }
    }
    /// Finds out whether an indexed CD belongs to a release.
    fn load_membership(&mut self, album: crate::store::AlbumId, cx: &mut Context<Self>) {
        let Some(store) = self.catalog.read(cx).handle() else {
            return;
        };
        let found = store.call(move |s| s.membership(album));
        cx.spawn(async move |this, cx| {
            let Ok(Some(m)) = found.await else { return };
            let _ = this.update(cx, |b, cx| {
                if matches!(b.location, Location::Album { id, .. } if id == album) {
                    b.membership = Some(Arc::new(Membership {
                        key: DiscId::Library(m.first),
                        title: m.title,
                        number: m.number,
                        subtitle: m.subtitle,
                    }));
                    cx.emit(ViewChanged);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn project_page(&mut self, page: Option<Page>, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.clear_search(window, cx);
        self.reset_table(cx);
        self.filter(cx);
        self.list_focus(cx).focus(window, cx);
        cx.emit(ViewChanged);
        cx.notify();
    }
    fn clear_search(&mut self, window: &mut Window, cx: &mut App) {
        self.query.clear();
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
    }
    fn reset_table(&mut self, cx: &mut App) {
        self.selected = None;
        self.table.update(cx, |t, cx| {
            t.delegate_mut().clear_marks();
            t.delegate_mut().sort = None;
            t.clear_selection(cx);
            scroll_to_top(t);
        });
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
        self.cancel_reveal();
        self.prefs.albums = albums;
        if let Location::Album { root, .. } | Location::Release { root, .. } = self.location {
            self.show(Location::Library(root), window, cx);
            return;
        }
        cx.emit(ViewChanged);
        self.page = None;
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
        let single_disc = self.programs.len() == 1
            && self.programs[0]
                .tracks
                .first()
                .is_some_and(|track| track.has_disc_context());
        let grouped = match &self.page {
            None => self.can_list_discs(),
            Some(Page::Release(_)) => true,
            Some(Page::Disc(_)) => false,
        };
        let mut groups = Vec::new();
        let mut rows = Vec::new();
        let matches =
            |t: &&Arc<Track>| crate::collection::matches(t, &terms, || catalog.mark_of(t));
        let discs: Vec<Arc<Disc>> = match &self.page {
            None if !grouped && !single_disc => vec![],
            None => self.programs.to_vec(),
            Some(Page::Disc(id)) => self
                .programs
                .iter()
                .filter(|d| &d.id == id)
                .cloned()
                .collect(),
            Some(Page::Release(key)) => crate::collection::release_discs(&self.programs, key),
        };
        if !grouped && !single_disc && self.page.is_none() {
            rows.extend(self.raw.iter().flatten().filter(matches).cloned());
        } else {
            for disc in &discs {
                let start = rows.len();
                rows.extend(disc.tracks.iter().filter(matches).cloned());
                if grouped && rows.len() > start {
                    groups.push((disc.clone(), rows.len() - start));
                }
            }
        }
        let rows: TrackList = rows.into();
        self.count = rows.len();
        self.duration = rows.iter().map(|t| t.duration).sum();
        self.membership = match &self.page {
            Some(Page::Disc(_)) => discs.first().and_then(|d| d.release.clone()),
            _ => None,
        };
        self.album = match &self.page {
            Some(Page::Disc(_)) => discs.first().map(|d| d.title.clone()),
            Some(Page::Release(_)) => discs
                .first()
                .and_then(|d| d.release.as_ref())
                .map(|m| m.title.clone()),
            None => None,
        }
        .or_else(|| {
            rows.first()
                .filter(|t| !t.album.is_empty() && rows.iter().all(|r| r.album == t.album))
                .map(|t| t.album.clone())
        });
        if self.listing_discs() {
            self.set_discs(groups.iter().map(|(d, _)| d.clone()).collect(), cx);
        }
        let sections = Sections::new(groups);
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
        let active = self.active;
        self.table.update(cx, |t, cx| {
            if let Some(selected) = t.selected_row().and_then(|i| t.delegate().selected_key(i)) {
                self.selected = Some(selected);
            }
            if t.delegate().has_marks() {
                self.selected = None;
            }
            let selected = self.selected.as_ref();
            let can_start = !rows.is_empty();
            if active {
                t.delegate().player.update(cx, |p, cx| {
                    if p.can_start != can_start {
                        p.can_start = can_start;
                        cx.notify();
                    }
                });
            }
            let context = if grouped || self.page.is_some() || single_disc {
                ContextKind::Disc
            } else {
                ContextKind::Mixed
            };
            MusicTable::set_context(t, context, cx);
            t.delegate_mut().artwork_revision = self.generation;
            t.delegate_mut().folder_covers = self.covers.clone();
            t.delegate_mut().programs = self.programs.clone();
            t.delegate_mut().folder_cd = self.page.is_some();
            t.delegate_mut().empty = empty;
            t.delegate_mut().offer_subfolders = only_folders;
            t.delegate_mut().offer_library = false;
            t.delegate_mut().unreadable = self.unreadable.clone();
            if discovered {
                set_rows_in_place(t, rows, sections, cx);
            } else {
                t.delegate_mut().set_rows(rows, sections, cx);
            }
            let ix = selected.and_then(|item| t.delegate().position(item));
            reselect(t, ix, cx);
            cx.notify();
        });
    }
    fn set_discs(&mut self, items: Vec<Arc<Disc>>, cx: &mut App) {
        let rows = Discs::shelve(items);
        self.discs.update(cx, |table, cx| {
            let selected = table
                .selected_row()
                .and_then(|i| table.delegate().rows.get(i))
                .map(Shelf::key);
            table.delegate_mut().rows = rows;
            let ix = selected.and_then(|k| table.delegate().rows.iter().position(|r| r.key() == k));
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
        let offer_library = matches!(self.location, Location::Library(_))
            && self.catalog.read(cx).loaded
            && self.catalog.read(cx).collections.is_empty();
        self.table.update(cx, |t, _| {
            t.delegate_mut().empty = empty;
            t.delegate_mut().offer_subfolders = false;
            t.delegate_mut().offer_library = offer_library;
        });
        let search = self.query.clone();
        if self.listing_discs() {
            self.load_albums(cx);
        }
        let grouped =
            matches!(scope, Scope::Library(Some(_)) | Scope::Release(_)) && !self.listing_discs();
        self.table.update(cx, |t, cx| {
            let context = match scope {
                Scope::Playlist(_) => ContextKind::Playlist,
                Scope::Album(_) | Scope::Release(_) | Scope::Library(Some(_)) => ContextKind::Disc,
                _ => ContextKind::Mixed,
            };
            MusicTable::set_context(t, context, cx);
            t.delegate_mut().show_library(scope, search, grouped, cx)
        });
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
                            items.push(Arc::new(Disc::from_album(album)));
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
                b.duration = Some(items.iter().map(|d| d.duration()).sum());
                b.set_discs(items, cx);
                b.restore_bookmark(cx);
                cx.notify();
            });
        })
        .detach();
    }
    /// The table finished listing a library view.
    pub fn library_listed(&mut self, count: usize, duration: f64, cx: &mut Context<Self>) {
        if self.scope().is_some() && self.active {
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
            self.restore_bookmark(cx);
            self.select_revealed(cx);
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
        self.remember(&Location::Folder(path.clone()), cx);
        self.page = None;
        self.load(path, reveal, window, cx);
    }
    /// Opens a folder as Locations shows it under `root`.
    pub fn open_in(
        &mut self,
        path: PathBuf,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember(&Location::Folder(path.clone()), cx);
        self.page = None;
        self.load_from(path, None, Some(root), window, cx);
    }
    /// Shows a library view and records the current view in the history.
    pub fn show(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.remember(&location, cx);
        self.enter(location, window, cx);
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
            Location::Library(root)
            | Location::Album { root, .. }
            | Location::Release { root, .. } => {
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
        self.load_from(path, reveal, None, window, cx);
    }
    /// Opens `path` and lists it. `root` is the Place Locations shows it in.
    fn load_from(
        &mut self,
        path: PathBuf,
        reveal: Option<Reveal>,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_folder = self.location.is_folder() && crate::places::same_path(&self.path, &path);
        if !same_folder {
            self.reset_table(cx);
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
        if self.active {
            self.folders
                .update(cx, |f, cx| f.show(&path, root.as_deref(), cx));
        }
        self.tree_root = root;
        self.path = path;
        self.folder_ready = false;
        self.busy = true;
        self.slow = false;
        if !same_folder {
            self.page = None;
        }
        self.raw.clear();
        self.programs = Arc::default();
        self.covers = Default::default();
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
                        if !batch.covers.is_empty() { Arc::make_mut(&mut b.covers).extend(batch.covers); }
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
                        b.programs = Discs::group(b.raw.iter().flatten(), &b.covers, &b.path).into();
                        b.albums.next();
                        b.filter_folder(true, cx);
                        b.select_revealed(cx);
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
                    b.folder_ready = true;
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
                    if (reveal.is_some() || b.reveal_key.is_some()) && b.notice.is_none() {
                        b.notice = Some("This recording is unavailable.".into());
                    }
                    b.filter(cx);
                    b.restore_bookmark(cx);
                    b.select_revealed(cx);
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
            self.page = Some(Page::Disc(DiscId::of(&item)));
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
            Location::Album { title, .. } | Location::Release { title, .. } => title.clone(),
            Location::Liked => "Liked Tracks".into(),
            Location::Playlist(p) => catalog
                .playlist(*p)
                .map_or_else(|| "Playlist".into(), |p| p.name.clone()),
        }
    }
}

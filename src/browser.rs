use crate::{
    collection::{DiscId, Discs, ViewPrefs},
    discovery::Scanner,
    library::{self, Track, TrackList},
    player::{Player, PlayerEvent},
    table::{LayoutColumn, MusicTable},
};
use gpui_kit::{
    component::{
        input::{InputEvent, InputState},
        table::{TableEvent, TableState},
        tree::{TreeItem, TreeState},
    },
    prelude::*,
    *,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const HISTORY: usize = 100;

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
    pub path: PathBuf,
    pub album: Option<String>,
    pub folder_count: usize,
    pub notice: Option<String>,
    pub busy: bool,
    pub tree: Entity<TreeState>,
    pub table: Entity<TableState<MusicTable>>,
    pub search: Entity<InputState>,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    raw: Vec<Option<Arc<Track>>>,
    pub prefs: ViewPrefs,
    pub discs: Entity<TableState<Discs>>,
    pub disc: Option<DiscId>,
    pub count: usize,
    pub duration: Option<f64>,
    covers: std::collections::HashMap<PathBuf, crate::artwork::CoverFile>,
    equivalent_sheets: std::collections::HashMap<PathBuf, PathBuf>,
    scanner: Scanner,
    task: Option<Task<()>>,
    selected: Option<Arc<Track>>,
    paths: Vec<PathBuf>,
    generation: u64,
    query: String,
}
pub fn music_home() -> PathBuf {
    crate::places::known_path(crate::places::Known::Music)
        .or_else(|| crate::places::known_path(crate::places::Known::Home))
        .unwrap_or_else(|| PathBuf::from("."))
}
impl Browser {
    pub fn new(
        player: Entity<Player>,
        columns: Vec<LayoutColumn>,
        prefs: ViewPrefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let browser = cx.weak_entity();
        let table = cx.new(|cx| {
            TableState::new(
                MusicTable::new(player.clone(), columns, browser),
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
        Self {
            path: PathBuf::new(),
            album: None,
            tree,
            table,
            search,
            raw: vec![],
            prefs,
            discs,
            disc: None,
            count: 0,
            duration: None,
            covers: Default::default(),
            equivalent_sheets: Default::default(),
            scanner: Scanner::new(),
            task: None,
            selected: None,
            paths: vec![],
            generation: Default::default(),
            query: String::new(),
            back: vec![],
            forward: vec![],
            notice: None,
            busy: false,
            folder_count: 0,
        }
    }
    pub fn revision(&self) -> u64 {
        self.generation
    }
    pub fn listing_discs(&self) -> bool {
        self.prefs.albums && self.disc.is_none()
    }
    pub fn show_disc(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.discs.read(cx).delegate().items.get(ix) {
            self.disc = Some(d.id.clone());
            self.filter(cx);
            self.table.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }
    pub fn view(&mut self, albums: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.albums = albums;
        cx.emit(ViewChanged);
        self.disc = None;
        self.filter(cx);
        if albums {
            self.discs.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.table.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
    fn filter(&mut self, cx: &mut App) {
        let rows: TrackList = self
            .raw
            .iter()
            .flatten()
            .filter(|t| {
                self.disc.as_ref().is_none_or(|d| d.contains(t))
                    && (self.query.is_empty() || crate::collection::matches(t, &self.query))
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
            self.discs.update(cx, |table, cx| {
                let selected = table
                    .selected_row()
                    .and_then(|i| table.delegate().items.get(i))
                    .map(|d| d.id.clone());
                table.delegate_mut().items = items;
                if let Some(ix) =
                    selected.and_then(|id| table.delegate().items.iter().position(|d| d.id == id))
                {
                    table.set_selected_row(ix, cx);
                } else {
                    table.clear_selection(cx);
                }
                cx.notify();
            });
        }
        self.table.update(cx, |t, cx| {
            if let Some(selected) = t
                .selected_row()
                .and_then(|i| t.delegate().rows.get(i))
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
            t.delegate_mut().empty = if self.busy {
                "Opening folder…".into()
            } else if !self.query.is_empty() {
                "No matching tracks".into()
            } else if self.notice.is_some() && self.raw.is_empty() && self.folder_count == 0 {
                SharedString::default()
            } else if self.folder_count > 0 && !self.prefs.recursive {
                "This folder holds folders. Choose one in the sidebar.".into()
            } else {
                "No supported recordings in this folder".into()
            };
            t.delegate_mut().set_rows(rows);
            if let Some(ix) =
                selected.and_then(|item| t.delegate().rows.iter().position(|r| r.same_item(item)))
            {
                t.set_selected_row(ix, cx);
            } else {
                t.clear_selection(cx);
            }
            cx.notify();
        });
    }
    /// Opens a folder and records the current one in the Back history.
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
        if !crate::places::same_path(&self.path, &path) && !self.path.as_os_str().is_empty() {
            self.back.push(std::mem::take(&mut self.path));
            if self.back.len() > HISTORY {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.load(path, reveal, window, cx);
    }
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.back.pop() {
            self.forward.push(std::mem::take(&mut self.path));
            self.load(p, None, window, cx);
        }
    }
    pub fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.forward.pop() {
            self.back.push(std::mem::take(&mut self.path));
            self.load(p, None, window, cx);
        }
    }
    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = self.path.parent().map(Path::to_owned) {
            self.open(parent, window, cx);
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
        let path = self.path.clone();
        self.load(path, None, window, cx);
    }
    pub fn load(
        &mut self,
        path: PathBuf,
        reveal: Option<Reveal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_folder = crate::places::same_path(&self.path, &path);
        if !same_folder {
            self.selected = None;
            self.table.update(cx, |t, cx| {
                t.delegate_mut().clear_marks();
                t.clear_selection(cx);
            });
        }
        self.generation += 1;
        cx.emit(ViewChanged);
        let revision = self.generation;
        let timing = std::env::var_os("DISQRO_TRACE_DISCOVERY").map(|_| std::time::Instant::now());
        self.task.take();
        let events = self.scanner.open(path.clone(), self.prefs.recursive);
        self.path = path;
        self.busy = true;
        if !same_folder {
            self.disc = None;
        }
        self.raw.clear();
        self.covers.clear();
        self.equivalent_sheets.clear();
        self.notice = None;
        self.set_tree(&[], None, cx);
        self.folder_count = 0;
        if !same_folder || reveal.is_some() {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            self.query.clear();
        }
        self.filter(cx);
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let mut reveal = reveal;
            let mut first = true;
            while let Ok(batch) = events.recv().await {
                if this
                    .update(cx, |b, cx| {
                        if b.generation != revision {
                            return;
                        }
                        if let Some(root) = batch.root {
                            b.folder_count = root.dirs.len();
                            b.set_tree(&root.dirs, root.siblings.as_deref(), cx);
                        }
                        b.covers.extend(batch.covers);
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
                        b.filter(cx);
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
                            && b.reveal(request, cx)
                        {
                            reveal = None;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |b, cx| {
                if b.generation == revision {
                    b.busy = false;
                    if let Some(start) = timing {
                        eprintln!(
                            "Discovery {revision} finished {:?} ({} tracks)",
                            start.elapsed(),
                            b.raw.iter().flatten().count()
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
    fn reveal(&mut self, request: &Reveal, cx: &mut App) -> bool {
        let item = self
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
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
        if self.prefs.albums {
            self.disc = Some(DiscId::of(&item));
            self.filter(cx);
        }
        self.table.update(cx, |t, cx| {
            if let Some(i) = t.delegate().rows.iter().position(|t| t.same_item(&item)) {
                t.set_selected_row(i, cx);
                t.scroll_to_row(i, cx);
                if matches!(request, Reveal::Play(_)) {
                    t.delegate().play(i, cx);
                }
            }
        });
        true
    }
    /// The tree shows the parent folder with the current folder among its
    /// siblings, opened to show its own subfolders. Album discs and
    /// neighbouring albums are one click away.
    fn set_tree(&mut self, dirs: &[PathBuf], siblings: Option<&[PathBuf]>, cx: &mut App) {
        self.paths.clear();
        fn item(paths: &mut Vec<PathBuf>, p: &Path) -> TreeItem {
            let id = paths.len();
            paths.push(p.to_owned());
            TreeItem::new(id.to_string(), library::display_name(p))
        }
        let children =
            |paths: &mut Vec<PathBuf>| dirs.iter().map(|p| item(paths, p)).collect::<Vec<_>>();
        let current_id;
        let root = match (self.path.parent(), siblings) {
            (Some(parent), Some(siblings)) => {
                let mut root = item(&mut self.paths, parent).expanded(true);
                let mut found = None;
                let mut items = vec![];
                for s in siblings {
                    let mut it = item(&mut self.paths, s);
                    if crate::places::same_path(s, &self.path) {
                        found = Some(it.id.clone());
                        let kids = children(&mut self.paths);
                        it = it.children(kids).expanded(true);
                    }
                    items.push(it);
                }
                if found.is_none() {
                    let mut it = item(&mut self.paths, &self.path.clone());
                    found = Some(it.id.clone());
                    let kids = children(&mut self.paths);
                    it = it.children(kids).expanded(true);
                    items.push(it);
                }
                current_id = found;
                root = root.children(items);
                root
            }
            _ => {
                let mut it = item(&mut self.paths, &self.path.clone());
                current_id = Some(it.id.clone());
                let kids = children(&mut self.paths);
                it = it.children(kids).expanded(true);
                it
            }
        };
        self.tree.update(cx, |t, cx| {
            t.set_items(vec![root], cx);
            if let Some(id) = &current_id
                && let Some(ix) = t.index_of(id)
            {
                t.set_selected_index(Some(ix), cx);
                t.scroll_to_item(ix, ScrollStrategy::Center);
            }
        });
    }
    pub fn tree_path(&self, id: &str) -> Option<PathBuf> {
        self.paths.get(id.parse::<usize>().ok()?).cloned()
    }
}

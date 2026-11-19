use crate::{
    library::{self, Track},
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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub struct Browser {
    pub path: PathBuf,
    pub album: Option<String>,
    pub folder_count: usize,
    pub notice: Option<String>,
    pub busy: bool,
    pub tree: Entity<TreeState>,
    pub table: Entity<TableState<MusicTable>>,
    pub search: Entity<InputState>,
    pub history: Vec<PathBuf>,
    raw: Arc<[Track]>,
    paths: Vec<PathBuf>,
    generation: Arc<AtomicU64>,
    query: String,
}
pub fn music_home() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_else(|| ".".into()));
    let music = home.join("Music");
    if music.is_dir() { music } else { home }
}
impl Browser {
    pub fn new(
        player: Entity<Player>,
        columns: Vec<LayoutColumn>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let table = cx.new(|cx| {
            TableState::new(MusicTable::new(player.clone(), columns), window, cx)
                .col_selectable(false)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find in folder"));
        cx.subscribe(&player, |b: &mut Self, _, ev, cx| {
            b.table.update(cx, |t, cx| match ev {
                PlayerEvent::TrackChanged => cx.notify(),
                PlayerEvent::PlaySelection => t.delegate().play(t.selected_row().unwrap_or(0), cx),
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
            raw: Arc::default(),
            paths: vec![],
            generation: Default::default(),
            query: String::new(),
            history: vec![],
            notice: None,
            busy: false,
            folder_count: 0,
        }
    }
    fn filter(&self, cx: &mut App) {
        let rows = if self.query.is_empty() {
            self.raw.clone()
        } else {
            self.raw
                .iter()
                .filter(|t| {
                    [&t.title, &t.artist, &t.album]
                        .iter()
                        .any(|s| s.to_lowercase().contains(&self.query))
                })
                .cloned()
                .collect()
        };
        self.table.update(cx, |t, cx| {
            let selected = t
                .selected_row()
                .and_then(|i| t.delegate().rows.get(i))
                .cloned();
            let can_start = !rows.is_empty();
            t.delegate().player.update(cx, |p, cx| {
                if p.can_start != can_start {
                    p.can_start = can_start;
                    cx.notify();
                }
            });
            t.delegate_mut().set_rows(rows);
            if let Some(ix) =
                selected.and_then(|item| t.delegate().rows.iter().position(|r| r.same_item(&item)))
            {
                t.set_selected_row(ix, cx);
            } else {
                t.clear_selection(cx);
            }
            cx.notify();
        });
    }
    pub fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path && !self.path.as_os_str().is_empty() {
            self.history.push(self.path.clone());
        }
        self.load(path, None, window, cx);
    }
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.history.pop() {
            self.load(p, None, window, cx);
        }
    }
    pub fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let _ = this.update_in(cx, |b, w, cx| b.open(path, w, cx));
            }
        })
        .detach();
    }
    pub fn load(
        &mut self,
        path: PathBuf,
        autoplay: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let revision = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let generation = self.generation.clone();
        self.album = None;
        self.path = path.clone();
        self.busy = true;
        self.notice = None;
        self.raw = Arc::default();
        self.folder_count = 0;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.query.clear();
        self.filter(cx);
        self.set_tree(&[], cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let dir = path.clone();
            let result = cx
                .background_spawn(async move { library::browse(&dir) })
                .await;
            if generation.load(Ordering::Relaxed) != revision {
                return;
            }
            let folder = match result {
                Ok(f) => f,
                Err(e) => {
                    let _ = this.update(cx, |b, cx| {
                        b.busy = false;
                        b.notice = Some(format!("{e:#}"));
                        cx.notify();
                    });
                    return;
                }
            };
            let tracks = folder.tracks;
            let _ = this.update(cx, |b, cx| {
                b.set_album(&tracks);
                b.raw = tracks.clone();
                b.busy = false;
                b.folder_count = folder.dirs.len();
                b.notice = folder.notices.into_iter().next();
                b.set_tree(&folder.dirs, cx);
                b.filter(cx);
                if let Some(path) = &autoplay {
                    let t = b.table.read(cx).delegate();
                    if let Some(i) = t.rows.iter().position(|t| {
                        &t.source == path || t.disc.as_ref().is_some_and(|d| &d.sheet == path)
                    }) {
                        b.table.update(cx, |t, cx| t.delegate().play(i, cx));
                    } else {
                        b.notice = Some("This file could not be opened as a recording.".into());
                    }
                }
                cx.notify();
            });
            let cancel = generation.clone();
            let enriched = cx
                .background_spawn(async move {
                    let mut rows = tracks.to_vec();
                    for t in &mut rows {
                        if cancel.load(Ordering::Relaxed) != revision {
                            return None;
                        }
                        if let Some(d) = &t.disc {
                            if d.end.is_none() {
                                let start = d.start;
                                let mut source = Track::file(t.source.clone());
                                source.read_tags();
                                t.duration =
                                    source.duration.map(|v| (v - start as f64 / 75.).max(0.));
                            }
                        } else {
                            t.read_tags();
                        }
                    }
                    Some(Arc::<[Track]>::from(rows))
                })
                .await;
            if let Some(rows) = enriched
                && generation.load(Ordering::Relaxed) == revision
            {
                let _ = this.update(cx, |b, cx| {
                    b.set_album(&rows);
                    b.raw = rows;
                    b.filter(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn set_album(&mut self, rows: &[Track]) {
        self.album = rows
            .first()
            .filter(|t| !t.album.is_empty() && rows.iter().all(|r| r.album == t.album))
            .map(|t| t.album.clone());
    }
    fn set_tree(&mut self, dirs: &[PathBuf], cx: &mut App) {
        self.paths.clear();
        fn item(paths: &mut Vec<PathBuf>, p: &Path, label: String) -> TreeItem {
            let id = paths.len();
            paths.push(p.to_owned());
            TreeItem::new(id.to_string(), label)
        }
        let children = dirs
            .iter()
            .map(|p| item(&mut self.paths, p, library::folder_name(p)))
            .collect::<Vec<_>>();
        let current = item(
            &mut self.paths,
            &self.path,
            library::folder_name(&self.path),
        )
        .children(children)
        .expanded(true);
        self.tree.update(cx, |t, cx| t.set_items(vec![current], cx));
    }
    pub fn tree_path(&self, id: &str) -> Option<PathBuf> {
        self.paths.get(id.parse::<usize>().ok()?).cloned()
    }
}

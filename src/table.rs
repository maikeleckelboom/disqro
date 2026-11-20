//! The track table shared by folder listings, collections, Liked Tracks and
//! playlists. Columns, selection, menus and the like and rating cells are the
//! same everywhere; only where rows come from differs.
use crate::{
    audio::Command,
    catalog::{Catalog, Targets},
    library::{DiscTrack, Track, TrackList, clock},
    player::Player,
    store::{self, Mark, PlaylistId, Row, Scope, Ticket},
    theme::materials,
    workspace::{ClearRating, RateFive, RateFour, RateOne, RateThree, RateTwo, ToggleLike},
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable, h_flex,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        rating::Rating,
        table::{Column, ColumnSort, TableDelegate, TableState},
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};

pub const ROW_HEIGHT: f32 = 28.;
const CELL_X: f32 = 10.;
/// Playing from a library view queues at most this many tracks around the
/// chosen one, so a six-figure collection never becomes a six-figure queue.
const PLAY_LIMIT: usize = 5000;
/// Library rows kept in memory beyond the visible range.
const ROW_CACHE: usize = 4096;
const PREFETCH: usize = 64;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Field {
    Number,
    Title,
    Artist,
    Album,
    Liked,
    Rating,
    Length,
    Format,
    Disc,
    Year,
    Path,
}
impl Field {
    /// Shown until the listener chooses otherwise.
    const DEFAULT: [Self; 7] = [
        Self::Number,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Liked,
        Self::Rating,
        Self::Length,
    ];
    const ALL: [Self; 11] = [
        Self::Number,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Liked,
        Self::Rating,
        Self::Length,
        Self::Format,
        Self::Disc,
        Self::Year,
        Self::Path,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Liked => "Liked",
            Self::Rating => "Rating",
            Self::Length => "Time",
            Self::Format => "Format",
            Self::Disc => "Disc",
            Self::Year => "Year",
            Self::Path => "File Path",
        }
    }
    fn menu_label(self) -> &'static str {
        match self {
            Self::Number => "Track Number",
            Self::Length => "Duration",
            f => f.label(),
        }
    }
    fn width(self) -> f32 {
        match self {
            Self::Number => 40.,
            Self::Title => 200.,
            Self::Artist => 135.,
            Self::Album => 148.,
            Self::Liked => 34.,
            Self::Rating => 100.,
            Self::Length => 60.,
            Self::Format | Self::Year | Self::Disc => 64.,
            Self::Path => 330.,
        }
    }
    fn min_width(self) -> f32 {
        match self {
            Self::Number => 36.,
            Self::Liked => 30.,
            Self::Rating => 96.,
            Self::Length | Self::Format => 48.,
            _ => 72.,
        }
    }
    fn right_aligned(self) -> bool {
        matches!(self, Self::Number | Self::Length)
    }
    fn value(self, t: &Track, mark: Mark) -> String {
        match self {
            Self::Number => t
                .display_number()
                .map(|n| n.to_string())
                .unwrap_or_default(),
            Self::Title => t.title.clone(),
            Self::Artist => t.artist.clone(),
            Self::Album => t.album.clone(),
            Self::Liked => if mark.liked { "Liked" } else { "" }.into(),
            Self::Rating => match mark.rating {
                0 => String::new(),
                1 => "1 star".into(),
                n => format!("{n} stars"),
            },
            Self::Length => t.duration.map(clock).unwrap_or_default(),
            Self::Format => t.format(),
            Self::Disc => t.disc_number.map(|n| n.to_string()).unwrap_or_default(),
            Self::Year => t.year.map(|n| n.to_string()).unwrap_or_default(),
            Self::Path => t.source.display().to_string(),
        }
    }
    fn order(self) -> store::Order {
        use store::Order as O;
        match self {
            Self::Number => O::Number,
            Self::Title => O::Title,
            Self::Artist => O::Artist,
            Self::Album => O::Album,
            Self::Liked => O::Liked,
            Self::Rating => O::Rating,
            Self::Length => O::Length,
            Self::Format => O::Format,
            Self::Disc => O::Disc,
            Self::Year => O::Year,
            Self::Path => O::Path,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutColumn {
    field: Field,
    width: f32,
}
/// Adds the Liked and Rating columns to a layout saved before they existed.
pub fn add_personal_columns(columns: &mut Vec<LayoutColumn>) {
    if columns.is_empty()
        || columns
            .iter()
            .any(|c| matches!(c.field, Field::Liked | Field::Rating))
    {
        return;
    }
    let at = columns
        .iter()
        .position(|c| c.field == Field::Length)
        .unwrap_or(columns.len());
    for field in [Field::Rating, Field::Liked] {
        columns.insert(
            at,
            LayoutColumn {
                field,
                width: field.width(),
            },
        );
    }
}

/// A row's identity for selection: a physical file or authored segment in a
/// folder, or a library view key. Tags and positions are deliberately
/// absent, so a refresh or sort keeps the same selection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    Track(PathBuf, Option<DiscTrack>),
    Key(i64),
}
fn key(t: &Track) -> RowKey {
    RowKey::Track(t.source.clone(), t.disc.clone())
}

/// The rows of a library view: every key, and the rows near the visible
/// range, fetched page by page from the library worker.
struct Paged {
    scope: Scope,
    search: String,
    keys: Arc<[i64]>,
    cache: HashMap<i64, Row>,
    loaded: bool,
}

enum Rows {
    Folder {
        original: TrackList,
        sorted: TrackList,
    },
    Library(Paged),
}
impl Rows {
    fn len(&self) -> usize {
        match self {
            Self::Folder { sorted, .. } => sorted.len(),
            Self::Library(p) => p.keys.len(),
        }
    }
    fn key(&self, ix: usize) -> Option<RowKey> {
        match self {
            Self::Folder { sorted, .. } => sorted.get(ix).map(|t| key(t)),
            Self::Library(p) => p.keys.get(ix).map(|k| RowKey::Key(*k)),
        }
    }
    fn position(&self, k: &RowKey) -> Option<usize> {
        match (self, k) {
            (Self::Folder { sorted, .. }, RowKey::Track(source, disc)) => sorted
                .iter()
                .position(|t| &t.source == source && &t.disc == disc),
            (Self::Library(p), RowKey::Key(k)) => p.keys.iter().position(|x| x == k),
            _ => None,
        }
    }
    fn row(&self, ix: usize) -> Option<&Row> {
        match self {
            Self::Library(p) => p.cache.get(p.keys.get(ix)?),
            Self::Folder { .. } => None,
        }
    }
    fn track(&self, ix: usize) -> Option<&Arc<Track>> {
        match self {
            Self::Folder { sorted, .. } => sorted.get(ix),
            Self::Library(_) => self.row(ix).map(|r| &r.track),
        }
    }
}

#[derive(Default)]
struct Selection {
    marked: HashSet<RowKey>,
    anchor: Option<RowKey>,
    cursor: Option<RowKey>,
    ordered: std::cell::OnceCell<Arc<Targets>>,
}
impl Selection {
    fn modify(
        &mut self,
        rows: &Rows,
        primary: Option<usize>,
        at: usize,
        range: bool,
        additive: bool,
    ) {
        self.ordered.take();
        if self.marked.is_empty()
            && let Some(k) = primary.and_then(|i| rows.key(i))
        {
            self.marked.insert(k.clone());
            self.anchor = Some(k);
        }
        let Some(clicked) = rows.key(at) else { return };
        self.cursor = Some(clicked.clone());
        if range {
            let from = self
                .anchor
                .as_ref()
                .and_then(|k| rows.position(k))
                .unwrap_or(at);
            if !additive {
                self.marked.clear();
            }
            self.marked
                .extend((from.min(at)..=from.max(at)).filter_map(|i| rows.key(i)));
            self.anchor.get_or_insert(clicked);
        } else {
            if !self.marked.remove(&clicked) {
                self.marked.insert(clicked.clone());
            }
            self.anchor = Some(clicked);
        }
    }
}

/// Tracks dragged from a table: onto a sidebar playlist, or within a playlist.
#[derive(Clone)]
pub struct DraggedTracks {
    pub targets: Arc<Targets>,
    label: SharedString,
}
impl Render for DraggedTracks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = materials(cx);
        div()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(5.))
            .bg(m.overlay)
            .border_1()
            .border_color(m.control_line)
            .text_size(px(13.))
            .text_color(m.text)
            .child(self.label.clone())
    }
}

pub struct MusicTable {
    rows: Rows,
    pub columns: Vec<LayoutColumn>,
    sort: Option<(Field, ColumnSort)>,
    selection: Selection,
    pub player: Entity<Player>,
    pub catalog: Entity<Catalog>,
    browser: WeakEntity<crate::browser::Browser>,
    /// What an empty table says: loading, no match, or only subfolders.
    pub empty: SharedString,
    keys_ticket: Ticket,
    page_ticket: Ticket,
    visible: Range<usize>,
    /// Bumps when the table switches views; late results are dropped.
    view: u64,
}

/// Sends the playable tracks of `targets` to the player once they are known.
pub fn queue_targets(
    catalog: &Entity<Catalog>,
    player: &Entity<Player>,
    targets: Targets,
    command: impl FnOnce(TrackList) -> Command + 'static,
    cx: &mut App,
) {
    let tracks = catalog.read(cx).tracks(targets, cx);
    let catalog = catalog.clone();
    let player = player.clone();
    cx.spawn(async move |cx| match tracks.await {
        Ok(tracks) if !tracks.is_empty() => {
            player.update(cx, |p, cx| p.command(command(tracks), cx))
        }
        result => catalog.update(cx, |_, cx| {
            cx.emit(crate::catalog::CatalogEvent::Notice(
                result.err().map_or_else(
                    || "No available tracks in this selection.".into(),
                    |e| format!("{e:#}"),
                ),
            ))
        }),
    })
    .detach();
}

impl MusicTable {
    pub fn new(
        player: Entity<Player>,
        catalog: Entity<Catalog>,
        saved: Vec<LayoutColumn>,
        browser: WeakEntity<crate::browser::Browser>,
    ) -> Self {
        let valid = saved.iter().any(|c| c.field == Field::Title)
            && saved.iter().enumerate().all(|(i, c)| {
                (24. ..=1200.).contains(&c.width) && saved[..i].iter().all(|d| d.field != c.field)
            });
        Self {
            player,
            catalog,
            browser,
            rows: Rows::Folder {
                original: Arc::default(),
                sorted: Arc::default(),
            },
            columns: if valid {
                saved
            } else {
                Self::default_columns()
            },
            sort: None,
            selection: Default::default(),
            empty: SharedString::default(),
            keys_ticket: Ticket::default(),
            page_ticket: Ticket::default(),
            visible: 0..0,
            view: 0,
        }
    }
    fn default_columns() -> Vec<LayoutColumn> {
        Field::DEFAULT
            .into_iter()
            .map(|field| LayoutColumn {
                field,
                width: field.width(),
            })
            .collect()
    }
    /// Shows or hides a column. Title always stays.
    fn toggle_field(&mut self, field: Field, cx: &mut Context<TableState<Self>>) {
        if let Some(i) = self.columns.iter().position(|c| c.field == field) {
            if field != Field::Title {
                self.columns.remove(i);
                if self.sort.is_some_and(|(f, _)| f == field) {
                    self.sort = None;
                    match self.rows {
                        Rows::Folder { .. } => self.apply_sort(cx),
                        Rows::Library(_) => self.query(cx),
                    }
                }
            }
            return;
        }
        let rank = |f: Field| Field::ALL.iter().position(|a| *a == f).unwrap();
        let at = self
            .columns
            .iter()
            .rposition(|c| rank(c.field) < rank(field))
            .map_or(0, |i| i + 1);
        self.columns.insert(
            at,
            LayoutColumn {
                field,
                width: field.width(),
            },
        );
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.len() == 0
    }
    pub fn track(&self, ix: usize) -> Option<&Arc<Track>> {
        self.rows.track(ix)
    }
    pub fn is_loading(&self) -> bool {
        matches!(&self.rows, Rows::Library(p) if !p.loaded)
    }
    /// The playlist shown in its own order, which drag and drop may change.
    fn reorderable(&self) -> Option<PlaylistId> {
        match &self.rows {
            Rows::Library(Paged {
                scope: Scope::Playlist(id),
                search,
                ..
            }) if search.is_empty() && self.sort.is_none() => Some(*id),
            _ => None,
        }
    }
    fn mark(&self, ix: usize, cx: &App) -> Mark {
        let catalog = self.catalog.read(cx);
        match &self.rows {
            Rows::Library(_) => self
                .rows
                .row(ix)
                .map_or_else(Mark::default, |r| catalog.mark(r.id)),
            Rows::Folder { sorted, .. } => sorted
                .get(ix)
                .map_or_else(Mark::default, |t| catalog.mark_of(t)),
        }
    }

    pub fn set_rows(&mut self, rows: TrackList, cx: &App) {
        if !matches!(self.rows, Rows::Folder { .. }) {
            self.keys_ticket.next();
            self.page_ticket.next();
            self.view += 1;
            self.clear_marks();
        }
        self.selection.ordered.take();
        self.rows = Rows::Folder {
            original: rows,
            sorted: Arc::default(),
        };
        self.apply_sort(cx);
    }
    /// Shows a library view, or refreshes it when it is already shown.
    pub fn show_library(
        &mut self,
        scope: Scope,
        search: String,
        cx: &mut Context<TableState<Self>>,
    ) {
        let same = matches!(&self.rows, Rows::Library(p) if p.scope == scope);
        if same {
            if let Rows::Library(p) = &mut self.rows {
                p.search = search;
            }
        } else {
            self.view += 1;
            self.clear_marks();
            self.sort = None;
            self.rows = Rows::Library(Paged {
                scope,
                search,
                keys: Arc::default(),
                cache: HashMap::new(),
                loaded: false,
            });
        }
        self.query(cx);
    }
    /// Queries the library view again, keeping the rows on screen until the
    /// new listing arrives.
    pub fn query(&mut self, cx: &mut Context<TableState<Self>>) {
        let Rows::Library(p) = &self.rows else { return };
        let Some(store) = self.catalog.read(cx).handle() else {
            return;
        };
        let q = store::Query {
            scope: p.scope,
            search: p.search.clone(),
            order: self
                .sort
                .map(|(f, d)| (f.order(), d == ColumnSort::Descending)),
        };
        let generation = self.keys_ticket.next();
        let view = self.view;
        let listing = store.query(&self.keys_ticket, generation, move |s| s.listing(&q));
        let browser = self.browser.clone();
        cx.spawn(async move |this, cx| {
            let result = listing.await;
            let applied = this.update(cx, |t, cx| {
                if t.delegate().view != view || !t.delegate().keys_ticket.is_current(generation) {
                    return None;
                }
                let listing = match result {
                    Ok(listing) => listing,
                    Err(e) if store::is_cancelled(&e) => return None,
                    Err(e) => {
                        t.delegate_mut().empty = format!("{e:#}").into();
                        store::Listing::default()
                    }
                };
                let selected = t.selected_row().and_then(|i| t.delegate().rows.key(i));
                let count = listing.keys.len();
                t.delegate_mut().selection.ordered.take();
                if let Rows::Library(p) = &mut t.delegate_mut().rows {
                    p.keys = listing.keys.into();
                    p.loaded = true;
                    p.cache.clear();
                }
                match selected.and_then(|k| t.delegate().rows.position(&k)) {
                    Some(ix) => t.set_selected_row(ix, cx),
                    None => t.clear_selection(cx),
                }
                t.delegate_mut().page_ticket.next();
                t.delegate_mut().request_rows(true, cx);
                cx.notify();
                Some((count, listing.duration))
            });
            if let Ok(Some((count, duration))) = applied {
                let _ = browser.update(cx, |b, cx| b.library_listed(count, duration, cx));
            }
        })
        .detach();
    }
    /// Fetches the rows around the visible range that are not in memory.
    /// `stale` refetches rows already shown, after the library changed.
    fn request_rows(&mut self, stale: bool, cx: &mut Context<TableState<Self>>) {
        let Rows::Library(p) = &mut self.rows else {
            return;
        };
        let start = self
            .visible
            .start
            .saturating_sub(PREFETCH)
            .min(p.keys.len());
        let end = (self.visible.end.max(start + 48) + PREFETCH).min(p.keys.len());
        let wanted: Vec<i64> = p.keys[start..end]
            .iter()
            .filter(|k| stale || !p.cache.contains_key(k))
            .copied()
            .collect();
        if wanted.is_empty() {
            return;
        }
        let generation = self.page_ticket.next();
        let view = self.view;
        let rows = self
            .catalog
            .read(cx)
            .rows(p.scope, wanted, &self.page_ticket, generation, cx);
        cx.spawn(async move |this, cx| {
            let Ok(rows) = rows.await else { return };
            let _ = this.update(cx, |t, cx| {
                let d = t.delegate_mut();
                if d.view != view || !d.page_ticket.is_current(generation) {
                    return;
                }
                let visible = d.visible.clone();
                if let Rows::Library(p) = &mut d.rows {
                    for r in rows {
                        p.cache.insert(r.key, r);
                    }
                    if p.cache.len() > ROW_CACHE {
                        let lo = visible.start.saturating_sub(ROW_CACHE / 4);
                        let hi = (visible.end + ROW_CACHE / 4).min(p.keys.len());
                        let keep: HashSet<i64> = p.keys[lo.min(hi)..hi].iter().copied().collect();
                        p.cache.retain(|k, _| keep.contains(k));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_sort(&mut self, cx: &App) {
        self.selection.ordered.take();
        let marks = |t: &Track| self.catalog.read(cx).mark_of(t);
        let Rows::Folder { original, sorted } = &mut self.rows else {
            return;
        };
        *sorted = original.clone();
        if let Some((f, order)) = self.sort {
            let rows = Arc::make_mut(sorted);
            match f {
                Field::Length => rows.sort_by(|a, b| {
                    a.duration
                        .partial_cmp(&b.duration)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }),
                Field::Disc => rows.sort_by_key(|t| t.disc_number),
                Field::Year => rows.sort_by_key(|t| t.year),
                Field::Number => rows.sort_by_key(|t| t.display_number()),
                Field::Rating => rows.sort_by_cached_key(|t| marks(t).rating),
                Field::Liked => rows.sort_by_cached_key(|t| marks(t).liked),
                _ => rows.sort_by_cached_key(|t| f.value(t, Mark::default()).to_lowercase()),
            }
            if order == ColumnSort::Descending {
                rows.reverse();
            }
        }
    }
    /// Header clicks cycle ascending, descending, and the view's own order.
    fn cycle_sort(&mut self, field: Field, cx: &mut Context<TableState<Self>>) {
        self.sort = match self.sort {
            Some((f, ColumnSort::Ascending)) if f == field => Some((field, ColumnSort::Descending)),
            Some((f, ColumnSort::Descending)) if f == field => None,
            _ => Some((field, ColumnSort::Ascending)),
        };
        match self.rows {
            Rows::Folder { .. } => self.apply_sort(cx),
            Rows::Library(_) => self.query(cx),
        }
    }
    /// Plays the view from row `ix`. A library view queues the tracks
    /// around it, skipping unavailable ones.
    pub fn play(&self, ix: usize, cx: &mut App) {
        match &self.rows {
            Rows::Folder { sorted, .. } => {
                if ix < sorted.len() {
                    self.player
                        .update(cx, |p, cx| p.command(Command::Play(sorted.clone(), ix), cx));
                }
            }
            Rows::Library(p) => {
                let Some(&chosen) = p.keys.get(ix) else {
                    return;
                };
                let start = ix.saturating_sub(PLAY_LIMIT / 2);
                let keys = p.keys[start..(start + PLAY_LIMIT).min(p.keys.len())].to_vec();
                let rows = self
                    .catalog
                    .read(cx)
                    .rows(p.scope, keys, &Ticket::default(), 0, cx);
                let player = self.player.clone();
                let catalog = self.catalog.clone();
                cx.spawn(async move |cx| {
                    let rows = match rows.await {
                        Ok(rows) => rows,
                        Err(e) => {
                            catalog.update(cx, |_, cx| {
                                cx.emit(crate::catalog::CatalogEvent::Notice(format!("{e:#}")))
                            });
                            return;
                        }
                    };
                    let mut start = None;
                    let mut tracks = Vec::with_capacity(rows.len());
                    for r in rows.into_iter().filter(|r| r.available || r.key == chosen) {
                        if r.key == chosen {
                            if !r.available {
                                catalog.update(cx, |_, cx| {
                                    cx.emit(crate::catalog::CatalogEvent::Notice(format!(
                                        "{} is unavailable.",
                                        r.track.title
                                    )))
                                });
                                return;
                            }
                            start = Some(tracks.len());
                        }
                        tracks.push(r.track);
                    }
                    if let Some(start) = start {
                        player.update(cx, |p, cx| {
                            p.command(Command::Play(tracks.into(), start), cx)
                        });
                    }
                })
                .detach();
            }
        }
    }
    pub fn resize(&mut self, widths: &[Pixels]) {
        for (c, w) in self.columns.iter_mut().zip(widths) {
            c.width = f32::from(*w);
        }
    }
    pub fn has_marks(&self) -> bool {
        !self.selection.marked.is_empty()
    }
    pub fn extend_selection(&mut self, primary: Option<usize>, direction: isize) -> Option<usize> {
        let last = self.rows.len().checked_sub(1)?;
        let current = self
            .selection
            .cursor
            .as_ref()
            .and_then(|k| self.rows.position(k))
            .or(primary);
        let next = current.map_or(0, |i| i.saturating_add_signed(direction).min(last));
        self.selection
            .modify(&self.rows, primary, next, true, false);
        Some(next)
    }

    pub fn move_selection(&self, selected: Option<usize>, down: bool, cx: &mut App) {
        let Some(playlist) = self.reorderable() else {
            return;
        };
        let Rows::Library(page) = &self.rows else {
            return;
        };
        let Targets::Keys(_, entries) = self.targets(None, selected) else {
            return;
        };
        let edge = if down {
            entries.last()
        } else {
            entries.first()
        };
        let Some(index) = edge.and_then(|id| page.keys.iter().position(|k| k == id)) else {
            return;
        };
        if (!down && index == 0) || (down && index + 1 >= page.keys.len()) {
            return;
        }
        let before = page
            .keys
            .get(if down { index + 2 } else { index - 1 })
            .copied();
        self.catalog
            .update(cx, |c, cx| c.move_entries(playlist, entries, before, cx));
    }
    pub fn clear_marks(&mut self) {
        self.selection = Default::default();
    }
    /// What an action applies to: the selection, unless `clicked` is a row
    /// outside it, which then stands alone.
    pub fn targets(&self, clicked: Option<usize>, selected: Option<usize>) -> Targets {
        let marked = &self.selection.marked;
        let lone = match clicked {
            Some(ix) => self
                .rows
                .key(ix)
                .filter(|k| !marked.contains(k))
                .map(|_| ix),
            None if marked.is_empty() => selected,
            None => None,
        };
        let indices: Vec<usize> = match lone {
            Some(ix) => vec![ix],
            None if clicked.is_none() && marked.is_empty() => vec![],
            None => (0..self.rows.len())
                .filter(|&i| self.rows.key(i).is_some_and(|k| marked.contains(&k)))
                .collect(),
        };
        match &self.rows {
            Rows::Folder { sorted, .. } => Targets::Tracks(
                indices
                    .iter()
                    .filter_map(|&i| sorted.get(i).cloned())
                    .collect(),
            ),
            Rows::Library(p) => Targets::Keys(
                p.scope,
                indices
                    .iter()
                    .filter_map(|&i| p.keys.get(i).copied())
                    .collect(),
            ),
        }
    }
    fn single_target(&self, row: usize) -> Targets {
        match &self.rows {
            Rows::Folder { sorted, .. } => {
                Targets::Tracks(sorted.get(row).cloned().into_iter().collect())
            }
            Rows::Library(p) => {
                Targets::Keys(p.scope, p.keys.get(row).copied().into_iter().collect())
            }
        }
    }
    /// Every row of the view, in display order.
    pub fn all_targets(&self) -> Targets {
        match &self.rows {
            Rows::Folder { sorted, .. } => Targets::Tracks(sorted.to_vec()),
            Rows::Library(p) => Targets::Keys(p.scope, p.keys.to_vec()),
        }
    }
    fn is_playing(&self, t: &Track, cx: &App) -> bool {
        self.player
            .read(cx)
            .status
            .track
            .as_ref()
            .is_some_and(|p| p.same_item(t))
    }
    /// A stable element id for the cells of row `ix`.
    fn cell_id(&self, ix: usize) -> u64 {
        use std::hash::BuildHasher;
        FIXED.hash_one(self.rows.key(ix))
    }
    fn render_like(
        &self,
        row: usize,
        mark: Mark,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let m = *materials(cx);
        let catalog = self.catalog.clone();
        let target = self.single_target(row);
        let liked = mark.liked;
        div()
            .id(("like", self.cell_id(row)))
            .role(Role::Button)
            .aria_label(if liked { "Unlike track" } else { "Like track" })
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .when(!liked, |d| {
                d.opacity(0.).group_hover("track-row", |s| s.opacity(1.))
            })
            .hover(|d| d.bg(m.hover))
            .child(
                if liked {
                    Icon::default().path(crate::assets::HEART_FILL)
                } else {
                    Icon::new(IconName::Heart)
                }
                .size(px(14.))
                .text_color(if liked { m.accent } else { m.text_faint }),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let target = target.clone();
                catalog.update(cx, |c, cx| c.set_liked(target, !liked, cx));
            })
            .into_any_element()
    }
    fn render_rating(
        &self,
        row: usize,
        mark: Mark,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let m = *materials(cx);
        let catalog = self.catalog.clone();
        let target = self.single_target(row);
        div()
            .id(("rating-cell", self.cell_id(row)))
            .text_color(m.text_faint)
            .when(mark.rating == 0, |d| {
                d.opacity(0.).group_hover("track-row", |s| s.opacity(1.))
            })
            // A click on a star rates this row only: it never selects,
            // plays or clears the selection.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                Rating::new(("rating", self.cell_id(row)))
                    .value(usize::from(mark.rating))
                    .xsmall()
                    .color(m.warning)
                    .on_click(move |stars, _, cx| {
                        let target = target.clone();
                        let stars = (*stars).min(5) as u8;
                        catalog.update(cx, |c, cx| c.rate(target, stars, cx));
                    }),
            )
            .into_any_element()
    }
}

/// Element ids must not change between frames, so the hasher is fixed.
const FIXED: std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher> =
    std::hash::BuildHasherDefault::new();

impl TableDelegate for MusicTable {
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn column(&self, ix: usize, _: &App) -> Column {
        let c = &self.columns[ix];
        let pad = if c.field == Field::Liked { 7. } else { CELL_X };
        let mut col = Column::new(c.field.label(), c.field.label())
            .width(c.width)
            .min_width(px(c.field.min_width()))
            .paddings(Edges {
                top: px(0.),
                bottom: px(0.),
                left: px(pad),
                right: px(pad),
            });
        if c.field.right_aligned() {
            col = col.text_right();
        }
        col
    }
    fn move_column(
        &mut self,
        ix: usize,
        to: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let c = self.columns.remove(ix);
        self.columns.insert(to, c);
    }
    fn visible_rows_changed(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if self.visible != range {
            self.visible = range;
            self.request_rows(false, cx);
        }
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let field = self.columns[col_ix].field;
        let m = materials(cx);
        let sorted = self.sort.filter(|(f, _)| *f == field).map(|(_, d)| d);
        h_flex()
            .id(("sort", col_ix))
            .size_full()
            .gap_1()
            .when(field.right_aligned(), |d| d.justify_end())
            .when(field == Field::Liked, |d| d.justify_center())
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if sorted.is_some() {
                m.text_muted
            } else {
                m.text_faint
            })
            .hover(|d| d.text_color(m.text_muted))
            .cursor_pointer()
            .map(|d| match field {
                Field::Liked if sorted.is_none() => {
                    d.child(Icon::new(IconName::Heart).size(px(12.)))
                }
                Field::Liked => d,
                _ => d.child(field.label()),
            })
            .when_some(sorted, |d, dir| {
                d.child(
                    Icon::new(if dir == ColumnSort::Ascending {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .size(px(12.)),
                )
            })
            .on_click(cx.listener(move |t, _, _, cx| {
                let selected = t.selected_row().and_then(|i| t.delegate().rows.key(i));
                t.delegate_mut().cycle_sort(field, cx);
                if let Some(ix) = selected.and_then(|k| t.delegate().rows.position(&k)) {
                    t.set_selected_row(ix, cx);
                }
                t.refresh(cx);
                cx.notify();
            }))
            .context_menu({
                let table = cx.entity().downgrade();
                let shown: Vec<Field> = self.columns.iter().map(|c| c.field).collect();
                move |mut menu, _, _| {
                    let edit = |field: Option<Field>| {
                        let table = table.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = table.update(cx, |t, cx| {
                                if let Some(field) = field {
                                    t.delegate_mut().toggle_field(field, cx);
                                } else {
                                    t.delegate_mut().columns = MusicTable::default_columns();
                                }
                                t.refresh(cx);
                                cx.notify();
                            });
                        }
                    };
                    menu = menu.label("Columns");
                    for f in Field::ALL {
                        menu = menu.item(
                            PopupMenuItem::new(f.menu_label())
                                .checked(shown.contains(&f))
                                .disabled(f == Field::Title)
                                .on_click(edit(Some(f))),
                        );
                    }
                    menu.separator()
                        .item(PopupMenuItem::new("Reset Columns").on_click(edit(None)))
                }
            })
    }
    fn cell_label(&self, row: usize, col: usize, cx: &App) -> Option<SharedString> {
        let t = self.rows.track(row)?;
        Some(self.columns[col].field.value(t, self.mark(row, cx)).into())
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let f = self.columns[col].field;
        let m = *materials(cx);
        let cell = h_flex()
            .size_full()
            .min_w_0()
            .when(f.right_aligned(), |d| d.justify_end());
        let Some(t) = self.rows.track(row).cloned() else {
            return cell.into_any_element();
        };
        let available = self.rows.row(row).is_none_or(|r| r.available);
        let mark = self.mark(row, cx);
        let playing = self.is_playing(&t, cx);
        let cell = cell.when(!available, |d| d.opacity(0.45));
        match f {
            Field::Number if playing => cell
                .child(
                    Icon::new(IconName::AudioLines)
                        .size(px(14.))
                        .text_color(m.accent),
                )
                .into_any_element(),
            Field::Number if !available => cell
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(13.))
                        .text_color(m.warning),
                )
                .into_any_element(),
            Field::Liked => cell
                .justify_center()
                .child(self.render_like(row, mark, cx))
                .into_any_element(),
            Field::Rating => cell
                .child(self.render_rating(row, mark, cx))
                .into_any_element(),
            Field::Title => cell
                .text_color(if playing { m.accent } else { m.text })
                .child(div().min_w_0().truncate().child(f.value(&t, mark)))
                .into_any_element(),
            _ => cell
                .text_color(match f {
                    Field::Number | Field::Format => m.text_faint,
                    _ => m.text_muted,
                })
                .when(f == Field::Format, |d| d.text_size(px(11.5)))
                .child(div().min_w_0().truncate().child(f.value(&t, mark)))
                .into_any_element(),
        }
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let m = *materials(cx);
        let marked = !self.selection.marked.is_empty()
            && self
                .rows
                .key(row)
                .is_some_and(|k| self.selection.marked.contains(&k));
        if row >= self.rows.len() {
            return div().id(("empty-track-row", row));
        }
        let dragged = if marked {
            self.selection
                .ordered
                .get_or_init(|| Arc::new(self.targets(None, None)))
                .clone()
        } else {
            Arc::new(self.single_target(row))
        };
        let label: SharedString = match (dragged.len(), self.rows.track(row)) {
            (1, Some(t)) => t.title.clone().into(),
            (n, _) => format!("{n} tracks").into(),
        };
        let reorder = self.reorderable().zip(self.rows.row(row).map(|r| r.key));
        div()
            .id(("track-row", row))
            .group("track-row")
            .when(marked, |d| d.bg(m.selected))
            .on_drag(
                DraggedTracks {
                    targets: dragged,
                    label,
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            .when_some(reorder, |d, (playlist,before)| {
                d.drag_over::<DraggedTracks>(move |s, d, _, _| {
                    if matches!(d.targets.as_ref(), Targets::Keys(Scope::Playlist(p), _) if p == &playlist)
                    {
                        s.border_t_2().border_color(m.accent)
                    } else {
                        s
                    }
                })
                .on_drop(cx.listener(move |t, d: &DraggedTracks, _, cx| {
                    if let Targets::Keys(Scope::Playlist(p), entries) = d.targets.as_ref()
                        && *p == playlist
                    {
                        let entries = entries.clone();
                        t.delegate()
                            .catalog
                            .clone()
                            .update(cx, |c, cx| c.move_entries(playlist, entries, Some(before), cx));
                    }
                }))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |table, event: &MouseDownEvent, window, cx| {
                    table.focus_handle(cx).focus(window,cx);
                    let additive = event.modifiers.secondary(cx);
                    if additive || event.modifiers.shift {
                        let primary = table.selected_row();
                        let delegate = table.delegate_mut();
                        delegate.selection.modify(
                            &delegate.rows,
                            primary,
                            row,
                            event.modifiers.shift,
                            additive,
                        );
                        table.clear_selection(cx);
                    }
                    cx.notify();
                }),
            )
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let m = materials(cx);
        gpui_kit::component::v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .text_size(px(13.))
            .text_color(m.text_faint)
            .when(!self.empty.is_empty() && !self.is_loading(), |d| {
                d.child(Icon::new(IconName::FolderOpen).size(px(26.)))
                    .child(self.empty.clone())
            })
    }
    fn context_menu(
        &mut self,
        ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(track) = self.rows.track(ix).cloned() else {
            return menu;
        };
        let targets = self.targets(Some(ix), None);
        let count = targets.len();
        let (player, catalog) = (self.player.clone(), self.catalog.clone());
        let send = {
            let (player, catalog) = (player.clone(), catalog.clone());
            move |targets: Targets, command: fn(TrackList) -> Command, cx: &mut App| {
                queue_targets(&catalog, &player, targets, command, cx)
            }
        };
        let plural = |one: &str, many: String| if count > 1 { many } else { one.into() };
        let liked = if count == 1 {
            self.mark(ix, cx).liked
        } else {
            catalog.read(cx).all_liked(&targets)
        };
        let rating = (count == 1).then(|| self.mark(ix, cx).rating);
        let playlists: Vec<(PlaylistId, SharedString)> = catalog
            .read(cx)
            .playlists
            .iter()
            .map(|p| (p.id, p.name.clone().into()))
            .collect();
        let in_playlist = match &targets {
            Targets::Keys(Scope::Playlist(p), keys) => Some((*p, keys.clone())),
            _ => None,
        };
        let rate_menu = {
            let (catalog, targets) = (catalog.clone(), targets.clone());
            PopupMenu::build(window, cx, move |mut menu, _, _| {
                let actions: [Box<dyn Action>; 5] = [
                    Box::new(RateOne),
                    Box::new(RateTwo),
                    Box::new(RateThree),
                    Box::new(RateFour),
                    Box::new(RateFive),
                ];
                for (stars, action) in (1..=5u8).zip(actions).rev() {
                    let (catalog, targets) = (catalog.clone(), targets.clone());
                    menu = menu.item(
                        PopupMenuItem::new(if stars == 1 {
                            "1 Star".to_string()
                        } else {
                            format!("{stars} Stars")
                        })
                        .checked(rating == Some(stars))
                        .action(action)
                        .on_click(move |_, _, cx| {
                            catalog.update(cx, |c, cx| c.rate(targets.clone(), stars, cx))
                        }),
                    );
                }
                let (catalog, targets) = (catalog.clone(), targets.clone());
                menu.separator().item(
                    PopupMenuItem::new("Clear Rating")
                        .disabled(rating == Some(0))
                        .action(Box::new(ClearRating))
                        .on_click(move |_, _, cx| {
                            catalog.update(cx, |c, cx| c.rate(targets.clone(), 0, cx))
                        }),
                )
            })
        };
        let playlist_menu = {
            let (catalog, targets) = (catalog.clone(), targets.clone());
            PopupMenu::build(window, cx, move |mut menu, _, _| {
                let (c, t) = (catalog.clone(), targets.clone());
                menu = menu.item(
                    PopupMenuItem::new("New Playlist")
                        .icon(IconName::Plus)
                        .on_click(move |_, _, cx| {
                            c.update(cx, |c, cx| c.new_playlist(t.clone(), cx))
                        }),
                );
                if !playlists.is_empty() {
                    menu = menu.separator();
                }
                for (id, name) in &playlists {
                    let (c, t, id) = (catalog.clone(), targets.clone(), *id);
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        c.update(cx, |c, cx| c.add_to_playlist(id, t.clone(), cx))
                    }));
                }
                menu.max_h(px(360.)).scrollable(true)
            })
        };
        let browser = self.browser.clone();
        let (t1, t2, t3) = (targets.clone(), targets.clone(), targets.clone());
        let (s1, s2) = (send.clone(), send);
        let this = cx.entity().downgrade();
        let clicked = self.rows.key(ix);
        let view = self.view;
        menu.item(
            PopupMenuItem::new("Play")
                .icon(IconName::Play)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |t, cx| {
                        let d = t.delegate();
                        if d.view == view
                            && let Some(i) = clicked.as_ref().and_then(|k| d.rows.position(k))
                        {
                            d.play(i, cx);
                        }
                    });
                }),
        )
        .item(
            PopupMenuItem::new(plural("Play Next", format!("Play {count} Tracks Next")))
                .icon(IconName::ListStart)
                .on_click(move |_, _, cx| s1(t1.clone(), Command::PlayNext, cx)),
        )
        .item(
            PopupMenuItem::new(plural(
                "Add to Queue",
                format!("Add {count} Tracks to Queue"),
            ))
            .icon(IconName::ListEnd)
            .on_click(move |_, _, cx| s2(t2.clone(), Command::Enqueue, cx)),
        )
        .separator()
        .item({
            let catalog = catalog.clone();
            let toggle = count > 1 && in_playlist.is_some();
            PopupMenuItem::new(if toggle {
                "Toggle Like"
            } else if liked {
                "Unlike"
            } else {
                "Like"
            })
            .icon(if liked {
                IconName::HeartOff
            } else {
                IconName::Heart
            })
            .action(Box::new(ToggleLike))
            .on_click(move |_, _, cx| {
                catalog.update(cx, |c, cx| {
                    if toggle {
                        c.toggle_like(t3.clone(), cx)
                    } else {
                        c.set_liked(t3.clone(), !liked, cx)
                    }
                })
            })
        })
        .item(PopupMenuItem::submenu("Rate", rate_menu).icon(IconName::Star))
        .item(PopupMenuItem::submenu("Add to Playlist", playlist_menu).icon(IconName::ListPlus))
        .when_some(in_playlist, |menu, (playlist, entries)| {
            let catalog = catalog.clone();
            menu.item(
                PopupMenuItem::new(plural(
                    "Remove from Playlist",
                    format!("Remove {count} from Playlist"),
                ))
                .icon(IconName::X)
                .action(Box::new(crate::workspace::RemoveFromPlaylist))
                .on_click(move |_, _, cx| {
                    catalog.update(cx, |c, cx| c.remove_entries(playlist, entries.clone(), cx))
                }),
            )
        })
        .separator()
        .item(
            PopupMenuItem::new("Reveal in Folder")
                .icon(IconName::FolderOpen)
                .on_click(move |_, window, cx| {
                    let original = track.disc.as_ref().map_or(&track.source, |d| &d.sheet);
                    if let Some(folder) = original.parent() {
                        let _ = browser.update(cx, |b, cx| {
                            b.open_revealing(
                                folder.to_owned(),
                                Some(crate::browser::Reveal::Track(track.clone())),
                                window,
                                cx,
                            )
                        });
                    }
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, Rows, Selection, Track, key};
    #[test]
    fn additive_selection_survives_reorder_and_shift_uses_logical_anchor() {
        let mut rows: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|p| Arc::new(Track::file((*p).into())))
            .collect();
        let folder = |rows: &Vec<Arc<Track>>| Rows::Folder {
            original: Arc::default(),
            sorted: rows.clone().into(),
        };
        let mut selection = Selection::default();
        selection.modify(&folder(&rows), Some(0), 2, false, true);
        assert_eq!(selection.marked.len(), 2);
        rows.reverse();
        selection.modify(&folder(&rows), None, 3, true, false);
        assert_eq!(selection.marked.len(), 3);
        assert!(!selection.marked.contains(&key(&rows[0])));
        selection.modify(&folder(&rows), None, 1, false, true);
        assert_eq!(selection.marked.len(), 2);
    }
}

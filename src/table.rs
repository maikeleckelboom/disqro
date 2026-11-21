//! The track table shared by folder listings, collections, Liked Tracks and
//! playlists. Columns, selection, menus and the like and rating cells are the
//! same everywhere; only where rows come from differs.
mod layout;
mod playback;
mod transfer;
use crate::{
    audio::Command,
    catalog::{Catalog, Targets},
    collection::{Disc, DiscId, Page},
    library::{DiscTrack, Track, TrackList, clock},
    player::Player,
    store::{self, Mark, PlaylistId, Row, Scope, Ticket},
    theme::materials,
    workspace::{
        ActivateTrackLike, ClearRating, RateFive, RateFour, RateOne, RateThree, RateTwo, ToggleLike,
    },
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable, VirtualListScrollHandle,
        button::Button,
        h_flex,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        rating::Rating,
        table::{Column, ColumnSort, TableDelegate, TableState},
    },
    prelude::*,
    *,
};
pub use layout::{ContextKind, Layouts, LegacyColumn};
use layout::{Layout, Pin};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};
pub use transfer::{DraggedTracks, playlist_target};

/// The compact row, which holds a default 24-pixel cover.
const ROW_HEIGHT: f32 = 28.;
const CELL_X: f32 = 10.;
/// Cover thumbnails fill the column between its padding, from compact to
/// large. Rows grow to keep a larger cover whole.
const COVER_X: f32 = 8.;
const THUMBNAIL: f32 = 24.;
const LARGE_THUMBNAIL: f32 = 96.;
const COVER_Y: f32 = 2.;
/// Library rows kept in memory beyond the visible range.
const ROW_CACHE: usize = 4096;
const PREFETCH: usize = 64;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Field {
    Number,
    /// The track's artwork: an image column, compact by default and
    /// resizable up to a large cover, that never sorts.
    Cover,
    Title,
    Artist,
    Album,
    Rating,
    Length,
    Format,
    Disc,
    Year,
    Path,
    #[serde(alias = "Liked")]
    Favorite,
}
impl Field {
    const ALL: [Self; 12] = [
        Self::Number,
        Self::Cover,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Rating,
        Self::Length,
        Self::Format,
        Self::Disc,
        Self::Year,
        Self::Path,
        Self::Favorite,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Cover => "Cover",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Rating => "Rating",
            Self::Length => "Time",
            Self::Format => "Format",
            Self::Disc => "Disc",
            Self::Year => "Year",
            Self::Path => "File Path",
            Self::Favorite => "Favorite",
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
            Self::Cover => THUMBNAIL + 2. * COVER_X,
            Self::Number => 40.,
            Self::Favorite => 88.,
            Self::Title => 200.,
            Self::Artist => 135.,
            Self::Album => 148.,
            Self::Rating => 100.,
            Self::Length => 60.,
            Self::Format | Self::Year | Self::Disc => 64.,
            Self::Path => 330.,
        }
    }
    fn min_width(self) -> f32 {
        match self {
            Self::Cover => self.width(),
            Self::Number => 36.,
            Self::Title => 142.,
            Self::Year | Self::Disc => 48.,
            Self::Rating => 96.,
            Self::Length | Self::Format => 48.,
            _ => 72.,
        }
    }
    fn max_width(self) -> f32 {
        match self {
            Self::Cover => LARGE_THUMBNAIL + 2. * COVER_X,
            _ => 1200.,
        }
    }
    /// The horizontal padding inside each cell and heading of the column.
    fn padding(self) -> f32 {
        match self {
            Self::Cover => COVER_X,
            _ => CELL_X,
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
            Self::Cover | Self::Favorite => String::new(),
            Self::Title => t.title.clone(),
            Self::Artist => t.artist.clone(),
            Self::Album => t.album.clone(),
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
    fn order(self) -> Option<store::Order> {
        use store::Order as O;
        Some(match self {
            Self::Cover | Self::Favorite => return None,
            Self::Number => O::Number,
            Self::Title => O::Title,
            Self::Artist => O::Artist,
            Self::Album => O::Album,
            Self::Rating => O::Rating,
            Self::Length => O::Length,
            Self::Format => O::Format,
            Self::Disc => O::Disc,
            Self::Year => O::Year,
            Self::Path => O::Path,
        })
    }
    fn sortable(self) -> bool {
        self.order().is_some()
    }
}
/// A row's identity for selection: a physical file or authored segment in a
/// folder, or a library view key. Tags and positions are deliberately
/// absent, so a refresh or sort keeps the same selection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    Disc(DiscId),
    /// A release in the Albums / CDs list, by its first disc.
    Release(DiscId),
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
    grouped: bool,
}

enum Contents {
    Folder {
        original: TrackList,
        sorted: TrackList,
    },
    Library(Paged),
}
/// Section offsets are in the displayed table; track keys and pages stay dense.
#[derive(Clone, Default)]
pub struct Sections(Arc<[(usize, Arc<Disc>)]>);
impl Sections {
    pub fn new(groups: impl IntoIterator<Item = (Arc<Disc>, usize)>) -> Self {
        let mut row = 0;
        Self(
            groups
                .into_iter()
                .filter(|(_, count)| *count > 0)
                .map(|(disc, count)| {
                    let header = row;
                    row += 1 + count;
                    (header, disc)
                })
                .collect(),
        )
    }
    fn header(&self, row: usize) -> Option<&Arc<Disc>> {
        self.0
            .binary_search_by_key(&row, |(ix, _)| *ix)
            .ok()
            .map(|i| &self.0[i].1)
    }
    fn preceding(&self, row: usize) -> usize {
        self.0.partition_point(|(ix, _)| *ix < row)
    }
    fn data_index(&self, row: usize) -> Option<usize> {
        let count = self.preceding(row);
        self.0
            .get(count)
            .is_none_or(|(ix, _)| *ix != row)
            .then(|| row - count)
    }
    fn display_index(&self, index: usize) -> usize {
        let mut lo = 0;
        let mut hi = self.0.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.0[mid].0 - mid <= index {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        index + lo
    }
}
struct Rows {
    data: Contents,
    sections: Sections,
}
impl Rows {
    fn len(&self) -> usize {
        self.sections.0.len()
            + match &self.data {
                Contents::Folder { sorted, .. } => sorted.len(),
                Contents::Library(p) => p.keys.len(),
            }
    }
    fn key(&self, row: usize) -> Option<RowKey> {
        if let Some(disc) = self.sections.header(row) {
            return Some(RowKey::Disc(disc.id.clone()));
        }
        let ix = self.sections.data_index(row)?;
        match &self.data {
            Contents::Folder { sorted, .. } => sorted.get(ix).map(|t| key(t)),
            Contents::Library(p) => p.keys.get(ix).map(|k| RowKey::Key(*k)),
        }
    }
    fn position(&self, k: &RowKey) -> Option<usize> {
        if let RowKey::Disc(id) = k {
            return self
                .sections
                .0
                .iter()
                .find(|(_, d)| d.id == *id)
                .map(|(i, _)| *i);
        }
        let ix = match (&self.data, k) {
            (Contents::Folder { sorted, .. }, RowKey::Track(source, disc)) => sorted
                .iter()
                .position(|t| &t.source == source && &t.disc == disc),
            (Contents::Library(p), RowKey::Key(k)) => p.keys.iter().position(|x| x == k),
            _ => None,
        }?;
        Some(self.sections.display_index(ix))
    }
    fn row(&self, row: usize) -> Option<&Row> {
        match &self.data {
            Contents::Library(p) => p.cache.get(p.keys.get(self.sections.data_index(row)?)?),
            Contents::Folder { .. } => None,
        }
    }
    /// Range selection visits tracks; ordinary arrow navigation can still focus a CD header.
    fn adjacent_track(&self, current: Option<usize>, direction: isize) -> Option<usize> {
        let last = self.len().checked_sub(1)?;
        let first = self.sections.display_index(0);
        let step = |i: usize| i.saturating_add_signed(direction).clamp(first, last);
        let next = current.map_or(first, step);
        Some(if self.sections.header(next).is_some() {
            step(next)
        } else {
            next
        })
    }
    fn track(&self, row: usize) -> Option<&Arc<Track>> {
        match &self.data {
            Contents::Folder { sorted, .. } => sorted.get(self.sections.data_index(row)?),
            Contents::Library(_) => self.row(row).map(|r| &r.track),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Selection {
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
            && let Some(k) = primary
                .and_then(|i| rows.key(i))
                .filter(|k| !matches!(k, RowKey::Disc(_)))
        {
            self.marked.insert(k.clone());
            self.anchor = Some(k);
        }
        let Some(clicked) = rows.key(at).filter(|k| !matches!(k, RowKey::Disc(_))) else {
            return;
        };
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
            self.marked.extend(
                (from.min(at)..=from.max(at))
                    .filter_map(|i| rows.key(i))
                    .filter(|k| !matches!(k, RowKey::Disc(_))),
            );
            self.anchor.get_or_insert(clicked);
        } else {
            if !self.marked.remove(&clicked) {
                self.marked.insert(clicked.clone());
            }
            self.anchor = Some(clicked);
        }
    }
}

/// Grab room on each side of a column boundary in the track table's
/// heading. Both halves sit in the cells' padding.
const EDGE: f32 = 6.;
fn scroll_tail(scrolling: &Range<usize>, count: usize) -> f32 {
    if !scrolling.is_empty() && scrolling.end < count {
        2. * EDGE
    } else {
        0.
    }
}

/// Resolve a header's half of a divider in viewport coordinates. At a pin
/// seam both sides address the fixed column. A clipped edge has no handle.
fn resize_owner(
    widths: &[f32],
    scrolling: &Range<usize>,
    viewport: f32,
    offset: f32,
    host: usize,
    leading: bool,
) -> Option<usize> {
    let boundary = if leading { host } else { host + 1 };
    if host >= scrolling.end {
        return (boundary < widths.len()).then_some(boundary);
    }
    let left: f32 = widths[..scrolling.start].iter().sum();
    let right = viewport - widths[scrolling.end..].iter().sum::<f32>();
    let mut x = widths[..boundary].iter().sum::<f32>();
    if scrolling.contains(&host) {
        x += offset;
        if x < left - 0.5 || x > right + 0.5 {
            return None;
        }
        if scrolling.start > 0 && (x - left).abs() <= 0.5 {
            return Some(scrolling.start - 1);
        }
    }
    if scrolling.end < widths.len() && (x - right).abs() <= 0.5 {
        return Some(scrolling.end);
    }
    boundary.checked_sub(1)
}

/// A column boundary dragged in the track table's heading.
#[derive(Clone)]
struct ColumnEdge;
impl Render for ColumnEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
/// The column being resized: where the press began, and its shown width then.
#[derive(Clone, Copy)]
struct Resize {
    ix: usize,
    x: Pixels,
    width: f32,
    grow_left: bool,
    end_gap: Option<f32>,
}

pub struct MusicTable {
    rows: Rows,
    folder_sections: Sections,
    pub programs: Arc<[Arc<Disc>]>,
    pub folder_cd: bool,
    pub layouts: Layouts,
    context: ContextKind,
    viewport: f32,
    horizontal_scroll: Option<VirtualListScrollHandle>,
    /// The painted cover edge and the row height that keeps it whole. They
    /// follow the Cover column live, including while its boundary is dragged.
    cover: f32,
    row_height: f32,
    /// The cover edge of the saved layout, which chooses the decoded size.
    /// It holds still while a column boundary is dragged.
    settled_cover: f32,
    resizing: Option<Resize>,
    pub artwork_revision: u64,
    pub folder_covers: Arc<HashMap<PathBuf, crate::artwork::CoverFile>>,
    pub sort: Option<(Field, ColumnSort)>,
    selection: Selection,
    pub player: Entity<Player>,
    pub catalog: Entity<Catalog>,
    browser: WeakEntity<crate::browser::Browser>,
    /// What an empty table says: loading, no match, or only subfolders.
    pub empty: SharedString,
    /// An empty folder with subfolders offers to include them.
    pub offer_subfolders: bool,
    /// An empty library offers to add a music folder.
    pub offer_library: bool,
    /// Folder sources whose audio header could not be read.
    pub unreadable: Arc<HashSet<PathBuf>>,
    keys_ticket: Ticket,
    page_ticket: Ticket,
    visible: Range<usize>,
    /// Bumps when the table switches views; late results are dropped.
    view: u64,
}

/// Restores the selection after the rows changed under it. The view stays
/// where the listener left it: GPUI Kit scrolls to a newly selected row,
/// which would pull the view back to the selection on every update.
pub fn reselect<D: TableDelegate>(
    t: &mut TableState<D>,
    ix: Option<usize>,
    cx: &mut Context<TableState<D>>,
) {
    match ix {
        Some(ix) if t.selected_row() == Some(ix) => {}
        Some(ix) => {
            t.set_selected_row(ix, cx);
            t.vertical_scroll_handle
                .0
                .borrow_mut()
                .deferred_scroll_to_item = None;
        }
        None if t.selected_row().is_some() => t.clear_selection(cx),
        None => {}
    }
}
/// Replaces the rows of a folder view while it is being discovered. Once
/// the listener has scrolled, the row at the top of the view stays there
/// when rows are sorted in or removed above it.
pub fn set_rows_in_place(
    t: &mut TableState<MusicTable>,
    rows: TrackList,
    sections: Sections,
    cx: &mut Context<TableState<MusicTable>>,
) {
    let top = top_anchor(t);
    t.delegate_mut().set_rows(rows, sections, cx);
    restore_anchor(t, top);
}
type Anchor = (usize, Point<Pixels>, Option<RowKey>);
fn top_anchor(t: &TableState<MusicTable>) -> Anchor {
    let offset = t.vertical_scroll_handle.0.borrow().base_handle.offset();
    let first = (-offset.y / px(t.delegate().row_height)).max(0.) as usize;
    (
        first,
        offset,
        (offset.y < px(0.))
            .then(|| t.delegate().rows.key(first))
            .flatten(),
    )
}
fn restore_anchor(t: &mut TableState<MusicTable>, (first, offset, anchor): Anchor) {
    let height = px(t.delegate().row_height);
    if let Some(now) = anchor.and_then(|k| t.delegate().rows.position(&k))
        && now != first
    {
        let y = offset.y - height * (now as f32 - first as f32);
        t.vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(offset.x, y));
    }
}
/// Starts a newly opened view at its first row.
pub fn scroll_to_top<D: TableDelegate>(t: &mut TableState<D>) {
    let mut scroll = t.vertical_scroll_handle.0.borrow_mut();
    scroll.deferred_scroll_to_item = None;
    scroll.base_handle.set_offset(point(px(0.), px(0.)));
}

/// The menu of a CD: open or play it, and its release when it has one.
pub fn disc_menu(
    menu: PopupMenu,
    disc: &Arc<Disc>,
    browser: WeakEntity<crate::browser::Browser>,
    table: WeakEntity<TableState<MusicTable>>,
    cx: &App,
) -> PopupMenu {
    let (d, b, t) = (disc.clone(), browser.clone(), table.clone());
    let page = Page::Disc(disc.id.clone());
    let menu = menu
        .item(PopupMenuItem::new("Open CD").on_click(move |_, w, cx| {
            let _ = b.update(cx, |b, cx| b.open_disc(&d, w, cx));
        }))
        .item(
            PopupMenuItem::new("Play CD")
                .icon(IconName::Play)
                .on_click(move |_, _, cx| {
                    let _ = t.update(cx, |t, cx| t.delegate().play_page(&page, cx));
                }),
        );
    match &disc.release {
        Some(m) => release_items(
            menu.separator(),
            m.key.clone(),
            m.title.clone(),
            browser,
            table,
            cx,
        ),
        None => menu,
    }
}
/// Open Release and Play Release, for a disc that belongs to one. The
/// release page itself offers only Play Release.
pub fn release_items(
    menu: PopupMenu,
    key: DiscId,
    title: String,
    browser: WeakEntity<crate::browser::Browser>,
    table: WeakEntity<TableState<MusicTable>>,
    cx: &App,
) -> PopupMenu {
    let page = Page::Release(key.clone());
    let shown = browser
        .upgrade()
        .is_some_and(|b| b.read(cx).shows_release(&key));
    menu.when(!shown, |menu| {
        menu.item(
            PopupMenuItem::new("Open Release").on_click(move |_, w, cx| {
                let _ = browser.update(cx, |b, cx| b.open_release(&key, title.clone(), w, cx));
            }),
        )
    })
    .item(
        PopupMenuItem::new("Play Release")
            .icon(IconName::Play)
            .on_click(move |_, _, cx| {
                let _ = table.update(cx, |t, cx| t.delegate().play_page(&page, cx));
            }),
    )
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
        layouts: Layouts,
        browser: WeakEntity<crate::browser::Browser>,
    ) -> Self {
        Self {
            player,
            catalog,
            browser,
            rows: Rows {
                sections: Sections::default(),
                data: Contents::Folder {
                    original: Arc::default(),
                    sorted: Arc::default(),
                },
            },
            programs: Arc::default(),
            folder_cd: false,
            folder_sections: Sections::default(),
            layouts,
            context: ContextKind::Mixed,
            viewport: 0.,
            horizontal_scroll: None,
            cover: THUMBNAIL,
            row_height: ROW_HEIGHT,
            settled_cover: THUMBNAIL,
            resizing: None,
            artwork_revision: 0,
            folder_covers: Default::default(),
            sort: None,
            selection: Default::default(),
            empty: SharedString::default(),
            offer_subfolders: false,
            offer_library: false,
            unreadable: Arc::default(),
            keys_ticket: Ticket::default(),
            page_ticket: Ticket::default(),
            visible: 0..0,
            view: 0,
        }
    }
    fn layout(&self) -> &Layout {
        self.layouts.get(self.context)
    }
    fn layout_mut(&mut self) -> &mut Layout {
        self.layouts.get_mut(self.context)
    }
    pub fn set_context(
        table: &mut TableState<Self>,
        context: ContextKind,
        cx: &mut Context<TableState<Self>>,
    ) {
        if table.delegate().context != context {
            table.delegate_mut().context = context;
            table
                .horizontal_scroll_handle
                .set_offset(point(px(0.), px(0.)));
            table.refresh(cx);
        }
    }
    /// Sizes covers and rows from the Cover column and returns the row
    /// height. The row at the top of the view stays there when it changes.
    pub fn sync_rows(table: &mut TableState<Self>) -> f32 {
        let d = table.delegate();
        let column = d
            .layout()
            .columns
            .iter()
            .position(|c| c.field == Field::Cover);
        let settled = column.map(|ix| d.layout().widths(d.viewport)[ix]);
        let live = column
            .and_then(|ix| table.col_width(ix))
            .map(f32::from)
            .or(settled);
        let edge = |width: f32| {
            (width - 2. * COVER_X)
                .clamp(THUMBNAIL, LARGE_THUMBNAIL)
                .round()
        };
        let cover = live.map_or(THUMBNAIL, edge);
        let row = (cover + 2. * COVER_Y).max(ROW_HEIGHT);
        if row != d.row_height {
            let scroll = table.vertical_scroll_handle.0.borrow();
            let offset = scroll.base_handle.offset();
            let y = offset.y * (row / d.row_height);
            scroll.base_handle.set_offset(point(offset.x, y));
        }
        let d = table.delegate_mut();
        d.cover = cover;
        d.row_height = row;
        if d.resizing.is_none() {
            d.settled_cover = settled.map_or(THUMBNAIL, edge);
        }
        row
    }
    /// Follows a dragged column boundary, updating the table on every move.
    /// The first move fixes the other columns at their shown widths, so only
    /// this boundary moves and the table scrolls rather than squeezing.
    fn drag_edge(table: &mut TableState<Self>, x: Pixels, cx: &mut Context<TableState<Self>>) {
        let Some(r) = table.delegate().resizing else {
            return;
        };
        if table.delegate().layout().adaptive {
            let shown: Vec<f32> = (0..table.delegate().layout().columns.len())
                .map_while(|ix| table.col_width(ix).map(f32::from))
                .collect();
            table.delegate_mut().layout_mut().freeze(&shown);
        }
        let Some(column) = table.delegate_mut().layout_mut().columns.get_mut(r.ix) else {
            return;
        };
        let field = column.field;
        let delta = if r.grow_left { r.x - x } else { x - r.x };
        let width = (r.width + f32::from(delta))
            .clamp(field.min_width(), field.max_width())
            .round();
        if column.width != width {
            column.width = width;
            table.refresh(cx);
            if let Some(gap) = r.end_gap {
                let d = table.delegate();
                let widths = d.layout().widths(d.viewport);
                let x = px((d.viewport - widths.iter().sum::<f32>() - gap).min(0.));
                table.horizontal_scroll_handle.set_offset(point(x, px(0.)));
            }
            cx.notify();
        }
    }
    fn end_resize(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        if table.delegate_mut().resizing.take().is_some() {
            table.delegate().save_layout(cx);
            cx.notify();
        }
    }
    pub fn fit(table: &mut TableState<Self>, width: f32, cx: &mut Context<TableState<Self>>) {
        if table.delegate().viewport != width {
            let scroll = table.horizontal_scroll_handle.clone();
            table.delegate_mut().horizontal_scroll = Some(scroll);
            table.delegate_mut().viewport = width;
            table.refresh(cx);
            cx.notify();
        }
    }
    fn save_layout(&self, cx: &mut App) {
        let _ = self
            .browser
            .update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
    }
    /// Shows or hides a column. Title always stays.
    fn toggle_field(&mut self, field: Field, cx: &mut Context<TableState<Self>>) {
        self.layout_mut().toggle(field);
        self.clear_hidden_sort(cx);
    }

    fn clear_hidden_sort(&mut self, cx: &mut Context<TableState<Self>>) {
        if self
            .sort
            .is_some_and(|(field, _)| !self.layout().has(field))
        {
            self.sort = None;
            match self.rows.data {
                Contents::Folder { .. } => self.apply_sort(cx),
                Contents::Library(_) => self.query(cx),
            }
        }
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
        matches!(&self.rows.data, Contents::Library(p) if !p.loaded)
    }
    /// The playlist shown in its own order, which drag and drop may change.
    fn reorderable(&self) -> Option<PlaylistId> {
        match &self.rows.data {
            Contents::Library(Paged {
                scope: Scope::Playlist(id),
                search,
                ..
            }) if search.is_empty() && self.sort.is_none() => Some(*id),
            _ => None,
        }
    }
    fn mark(&self, ix: usize, cx: &App) -> Mark {
        let catalog = self.catalog.read(cx);
        match &self.rows.data {
            Contents::Library(_) => self
                .rows
                .row(ix)
                .map_or_else(Mark::default, |r| catalog.mark(r.id)),
            Contents::Folder { .. } => self
                .rows
                .track(ix)
                .map_or_else(Mark::default, |t| catalog.mark_of(t)),
        }
    }

    pub fn set_rows(&mut self, rows: TrackList, sections: Sections, cx: &App) {
        if !matches!(self.rows.data, Contents::Folder { .. }) {
            self.keys_ticket.next();
            self.page_ticket.next();
            self.view += 1;
            self.clear_marks();
        }
        self.selection.ordered.take();
        self.folder_sections = sections;
        self.rows.data = Contents::Folder {
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
        grouped: bool,
        cx: &mut Context<TableState<Self>>,
    ) {
        let same = matches!(&self.rows.data, Contents::Library(p) if p.scope == scope);
        if same {
            if let Contents::Library(p) = &mut self.rows.data {
                p.search = search;
                p.grouped = grouped;
            }
        } else {
            self.view += 1;
            self.clear_marks();
            self.sort = None;
            self.rows.sections = Sections::default();
            self.rows.data = Contents::Library(Paged {
                scope,
                search,
                keys: Arc::default(),
                cache: HashMap::new(),
                loaded: false,
                grouped,
            });
        }
        self.query(cx);
    }
    /// Queries the library view again, keeping the rows on screen until the
    /// new listing arrives.
    pub fn query(&mut self, cx: &mut Context<TableState<Self>>) {
        let Contents::Library(p) = &self.rows.data else {
            return;
        };
        let Some(store) = self.catalog.read(cx).handle() else {
            return;
        };
        let q = store::Query {
            scope: p.scope,
            search: p.search.clone(),
            order: self
                .sort
                .and_then(|(f, d)| Some((f.order()?, d == ColumnSort::Descending))),
        };
        let generation = self.keys_ticket.next();
        let view = self.view;
        let grouped = p.grouped && self.sort.is_none();
        let listing = store.query(&self.keys_ticket, generation, move |s| {
            let (listing, albums) = s.table_listing(&q, grouped)?;
            let sections = Sections::new(albums.into_iter().map(|a| {
                let count = a.count;
                (Arc::new(Disc::from_album(a)), count)
            }));
            Ok((listing, sections))
        });
        let browser = self.browser.clone();
        cx.spawn(async move |this, cx| {
            let result = listing.await;
            let applied = this.update(cx, |t, cx| {
                if t.delegate().view != view || !t.delegate().keys_ticket.is_current(generation) {
                    return None;
                }
                let (listing, sections) = match result {
                    Ok(listing) => listing,
                    Err(e) if store::is_cancelled(&e) => return None,
                    Err(e) => {
                        t.delegate_mut().empty = format!("{e:#}").into();
                        Default::default()
                    }
                };
                let selected = t.selected_row().and_then(|i| t.delegate().rows.key(i));
                let anchor = top_anchor(t);
                let count = listing.keys.len();
                t.delegate_mut().artwork_revision = generation;
                t.delegate_mut().rows.sections = sections;
                t.delegate_mut().selection.ordered.take();
                if let Contents::Library(p) = &mut t.delegate_mut().rows.data {
                    p.keys = listing.keys.into();
                    p.loaded = true;
                    p.cache.clear();
                }
                let ix = selected.and_then(|k| t.delegate().rows.position(&k));
                reselect(t, ix, cx);
                t.delegate_mut().page_ticket.next();
                restore_anchor(t, anchor);
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
        let Contents::Library(p) = &mut self.rows.data else {
            return;
        };
        let start = (self.visible.start - self.rows.sections.preceding(self.visible.start))
            .saturating_sub(PREFETCH)
            .min(p.keys.len());
        let end = ((self.visible.end - self.rows.sections.preceding(self.visible.end))
            .max(start + 48)
            + PREFETCH)
            .min(p.keys.len());
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
                let visible = d.visible.start - d.rows.sections.preceding(d.visible.start)
                    ..d.visible.end - d.rows.sections.preceding(d.visible.end);
                if let Contents::Library(p) = &mut d.rows.data {
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
        let Contents::Folder { original, sorted } = &mut self.rows.data else {
            return;
        };
        self.rows.sections = if self.sort.is_none() {
            self.folder_sections.clone()
        } else {
            Sections::default()
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
                _ => rows.sort_by_cached_key(|t| f.value(t, Mark::default()).to_lowercase()),
            }
            if order == ColumnSort::Descending {
                rows.reverse();
            }
        }
    }
    /// Header clicks cycle ascending, descending, and the view's own order.
    fn cycle_sort(&mut self, field: Field, cx: &mut Context<TableState<Self>>) {
        if !field.sortable() {
            return;
        }
        self.sort = match self.sort {
            Some((f, ColumnSort::Ascending)) if f == field => Some((field, ColumnSort::Descending)),
            Some((f, ColumnSort::Descending)) if f == field => None,
            _ => Some((field, ColumnSort::Ascending)),
        };
        match self.rows.data {
            Contents::Folder { .. } => self.apply_sort(cx),
            Contents::Library(_) => self.query(cx),
        }
    }
    pub fn header(&self, ix: usize) -> Option<&Arc<Disc>> {
        self.rows.sections.header(ix)
    }
    pub fn selected_key(&self, ix: usize) -> Option<RowKey> {
        self.rows.key(ix)
    }
    pub fn position(&self, k: &RowKey) -> Option<usize> {
        self.rows.position(k)
    }
    pub fn first_track(&self) -> usize {
        self.rows.sections.display_index(0)
    }

    pub fn has_marks(&self) -> bool {
        !self.selection.marked.is_empty()
    }
    pub fn extend_selection(&mut self, primary: Option<usize>, direction: isize) -> Option<usize> {
        let current = self
            .selection
            .cursor
            .as_ref()
            .and_then(|k| self.rows.position(k))
            .or(primary);
        let next = self.rows.adjacent_track(current, direction)?;
        self.selection
            .modify(&self.rows, primary, next, true, false);
        Some(next)
    }

    pub fn move_selection(&self, selected: Option<usize>, down: bool, cx: &mut App) {
        let Some(playlist) = self.reorderable() else {
            return;
        };
        let Contents::Library(page) = &self.rows.data else {
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
    pub fn marked_selection(&self) -> Option<Selection> {
        self.has_marks().then(|| self.selection.clone())
    }
    pub fn restore_marks(&mut self, mut selection: Selection) {
        selection.ordered.take();
        self.selection = selection;
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
        let indices: Vec<_> = indices
            .into_iter()
            .filter_map(|i| self.rows.sections.data_index(i))
            .collect();
        match &self.rows.data {
            Contents::Folder { sorted, .. } => Targets::Tracks(
                indices
                    .iter()
                    .filter_map(|&i| sorted.get(i).cloned())
                    .collect(),
            ),
            Contents::Library(p) => Targets::Keys(
                p.scope,
                indices
                    .iter()
                    .filter_map(|&i| p.keys.get(i).copied())
                    .collect(),
            ),
        }
    }
    fn single_target(&self, row: usize) -> Targets {
        let Some(row) = self.rows.sections.data_index(row) else {
            return Targets::Tracks(Vec::new());
        };
        match &self.rows.data {
            Contents::Folder { sorted, .. } => {
                Targets::Tracks(sorted.get(row).cloned().into_iter().collect())
            }
            Contents::Library(p) => {
                Targets::Keys(p.scope, p.keys.get(row).copied().into_iter().collect())
            }
        }
    }
    /// Every row of the view, in display order.
    pub fn all_targets(&self) -> Targets {
        match &self.rows.data {
            Contents::Folder { sorted, .. } => Targets::Tracks(sorted.to_vec()),
            Contents::Library(p) => Targets::Keys(p.scope, p.keys.to_vec()),
        }
    }
    fn is_playing(&self, t: &Track, cx: &App) -> bool {
        let status = &self.player.read(cx).status;
        status.playing && status.track.as_ref().is_some_and(|p| p.same_item(t))
    }
    /// A stable element id for the cells of row `ix`.
    fn cell_id(&self, ix: usize) -> u64 {
        use std::hash::BuildHasher;
        FIXED.hash_one(self.rows.key(ix))
    }
    /// A painted Cover cell. A missing or pending image keeps a faint tile
    /// in the same place, so the column stays aligned without repeating an
    /// icon on every row. The texture is decoded for the settled size, so
    /// dragging the column boundary scales it without decoding again.
    fn render_cover(
        &self,
        row: usize,
        t: &Track,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let folder = t.disc.as_ref().and_then(|d| d.sheet.parent());
        let fallback = match &self.rows.data {
            Contents::Library(_) => self.rows.row(row).and_then(|r| r.cover.as_ref()).cloned(),
            Contents::Folder { .. } => self
                .folder_covers
                .get(
                    folder
                        .or_else(|| t.source.parent())
                        .unwrap_or(std::path::Path::new("")),
                )
                .map(|cover| cover.path().to_owned()),
        };
        let image = crate::artwork::TrackCover {
            source: t.source.clone(),
            authored_folder: folder
                .filter(|f| Some(*f) != t.source.parent())
                .map(ToOwned::to_owned),
            disc_cover: fallback,
            listing: self.artwork_revision,
        }
        .image(self.settled_cover, window, cx);
        div()
            .size(px(self.cover))
            .flex_shrink_0()
            .rounded(px(2.))
            .overflow_hidden()
            .map(|d| match image {
                Some(image) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
                // Fainter than row hover, so even large empty tiles recede.
                None => d.bg(materials(cx).hover.opacity(0.4)),
            })
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
        let toggle = move |cx: &mut App| {
            catalog.update(cx, |c, cx| c.set_liked(target.clone(), !liked, cx));
        };
        let toggle_from_key = toggle.clone();
        div()
            .id(("like", self.cell_id(row)))
            .key_context("TrackLike")
            .role(Role::Button)
            .focusable()
            .tab_stop(true)
            .aria_toggled(if liked {
                gpui_kit::accesskit::Toggled::True
            } else {
                gpui_kit::accesskit::Toggled::False
            })
            .aria_label(if liked { "Unlike track" } else { "Like track" })
            .flex_shrink_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .when(!liked, |d| {
                d.opacity(0.).group_hover("track-row", |s| s.opacity(1.))
            })
            .focus_visible(|s| s.opacity(1.).bg(m.hover))
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
            .on_mouse_down(MouseButton::Left, |_, w, cx| {
                w.prevent_default();
                cx.stop_propagation();
            })
            .on_action(move |_: &ActivateTrackLike, _, cx| {
                cx.stop_propagation();
                toggle_from_key(cx);
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                toggle(cx);
            })
            .into_any_element()
    }
    /// Each half of a resize grip sits inside its host header. Right-fixed
    /// columns grow from their left edge, toward the scrolling area.
    fn render_edge(
        &self,
        ix: usize,
        host: usize,
        leading: bool,
        grow_left: bool,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let m = *materials(cx);
        let pad = -px(self.layout().columns[host].field.padding());
        let active = self.resizing.is_some_and(|r| r.ix == ix);
        div()
            .id((if leading { "edge-leading" } else { "edge" }, host))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(EDGE))
            .occlude()
            .cursor_col_resize()
            .map(|d| {
                if leading {
                    d.left(pad).border_l_1()
                } else {
                    d.right(pad).border_r_1()
                }
            })
            .border_color(if active && ix == host {
                m.text_faint
            } else {
                transparent_black()
            })
            .when(!active, |d| d.hover(|d| d.border_color(m.control_line)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |t, e: &MouseDownEvent, _, _| {
                    let d = t.delegate();
                    let widths = d.layout().widths(d.viewport);
                    let scrolling = d.layout().scrolling(&widths, d.viewport);
                    // Keep the last visible scrollable edge in view, including
                    // when it is flush with the first right pin before the drag.
                    let offset = f32::from(t.horizontal_scroll_handle.offset().x);
                    let gap = d.viewport - widths.iter().sum::<f32>() - offset;
                    let end_gap =
                        (scrolling.end < widths.len() && ix + 1 >= scrolling.end && gap >= -1.)
                            .then(|| gap.clamp(0., scroll_tail(&scrolling, widths.len())));
                    let width = t.col_width(ix).map(f32::from);
                    t.delegate_mut().resizing = width.map(|width| Resize {
                        ix,
                        x: e.position.x,
                        width,
                        grow_left,
                        end_gap,
                    });
                }),
            )
            .on_drag(ColumnEdge, |edge, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| edge.clone())
            })
            .on_drag_move(cx.listener(|t, e: &DragMoveEvent<ColumnEdge>, _, cx| {
                Self::drag_edge(t, e.event.position.x, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|t, _, _, cx| Self::end_resize(t, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|t, _, _, cx| Self::end_resize(t, cx)),
            )
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
        self.layout().columns.len()
    }
    fn column(&self, ix: usize, _: &App) -> Column {
        let c = &self.layout().columns[ix];
        let pad = c.field.padding();
        let widths = self.layout().widths(self.viewport);
        let scrolling = self.layout().scrolling(&widths, self.viewport);
        let mut col = Column::new(c.field.label(), c.field.label())
            .width(widths[ix])
            .min_width(px(c.field.min_width()))
            .max_width(px(c.field.max_width()))
            .paddings(Edges {
                top: px(0.),
                bottom: px(0.),
                left: px(pad),
                right: px(pad),
            });
        if ix < scrolling.start {
            col = col.fixed_left();
        } else if ix >= scrolling.end {
            col = col.fixed_right();
        }
        if c.field.right_aligned() {
            col = col.text_right();
        }
        col
    }
    fn move_column(
        &mut self,
        ix: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if !self.layout_mut().move_column(ix, to) {
            // Kit moves its own groups after calling the delegate. Undo a drop
            // across a pin temporarily released in a narrow pane.
            cx.defer_in(window, |table, _, cx| table.refresh(cx));
        }
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
        let field = self.layout().columns[col_ix].field;
        let m = *materials(cx);
        let sorted = self.sort.filter(|(f, _)| *f == field).map(|(_, d)| d);
        let layout = self.layout();
        let pin = layout.columns[col_ix].pin;
        let widths = layout.widths(self.viewport);
        let scrolling = layout.scrolling(&widths, self.viewport);
        let offset = self
            .horizontal_scroll
            .as_ref()
            .map_or(0., |s| f32::from(s.offset().x));
        let grip = |leading, cx: &mut Context<TableState<Self>>| {
            resize_owner(&widths, &scrolling, self.viewport, offset, col_ix, leading)
                .map(|owner| self.render_edge(owner, col_ix, leading, owner >= scrolling.end, cx))
        };
        let leading = grip(true, cx);
        let trailing = grip(false, cx);
        h_flex()
            .id(("sort", col_ix))
            .size_full()
            .gap_1()
            .when(field.right_aligned(), |d| d.justify_end())
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if sorted.is_some() {
                m.text_muted
            } else {
                m.text_faint
            })
            .whitespace_nowrap()
            .child(field.label())
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
            .children(trailing)
            .children(leading)
            .when(field.sortable(), |d| {
                d.hover(|d| d.text_color(m.text_muted))
                    .cursor_pointer()
                    .on_click(cx.listener(move |t, _, _, cx| {
                        let selected = t.selected_row().and_then(|i| t.delegate().rows.key(i));
                        t.delegate_mut().cycle_sort(field, cx);
                        if let Some(ix) = selected.and_then(|k| t.delegate().rows.position(&k)) {
                            t.set_selected_row(ix, cx);
                        }
                        t.refresh(cx);
                        cx.notify();
                    }))
            })
            .context_menu({
                let table = cx.entity().downgrade();
                let shown: Vec<Field> = self.layout().columns.iter().map(|c| c.field).collect();
                move |mut menu, _, _| {
                    enum Edit {
                        Pin(Pin),
                        Toggle(Field),
                        Reset,
                    }
                    let edit = |action: Edit| {
                        let table = table.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = table.update(cx, |t, cx| {
                                let d = t.delegate_mut();
                                match action {
                                    Edit::Pin(pin) => d.layout_mut().set_pin(field, pin),
                                    Edit::Toggle(field) => d.toggle_field(field, cx),
                                    Edit::Reset => {
                                        *d.layout_mut() = Layout::new(d.context);
                                        d.clear_hidden_sort(cx);
                                    }
                                }
                                t.refresh(cx);
                                t.delegate().save_layout(cx);
                                cx.notify();
                            });
                        }
                    };
                    for (side, label) in [
                        (Pin::Left, "Pin to left"),
                        (Pin::Right, "Pin to right"),
                        (Pin::None, "Unpin"),
                    ] {
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(pin == side)
                                .on_click(edit(Edit::Pin(side))),
                        );
                    }
                    menu = menu.separator().label("Columns");
                    for f in Field::ALL {
                        menu = menu.item(
                            PopupMenuItem::new(f.menu_label())
                                .checked(shown.contains(&f))
                                .disabled(f == Field::Title)
                                .on_click(edit(Edit::Toggle(f))),
                        );
                    }
                    menu.separator()
                        .item(PopupMenuItem::new("Reset Columns").on_click(edit(Edit::Reset)))
                }
            })
    }
    fn cell_label(&self, row: usize, col: usize, cx: &App) -> Option<SharedString> {
        if let Some(d) = self.header(row) {
            return (col == 0).then(|| d.title.clone().into());
        }
        let t = self.rows.track(row)?;
        let field = self.layout().columns[col].field;
        (!matches!(field, Field::Cover | Field::Favorite))
            .then(|| field.value(t, self.mark(row, cx)).into())
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Leave just enough travel to expose the last scrolling divider
        // separately from the first right pin. The space has no hit target.
        let layout = self.layout();
        let widths = layout.widths(self.viewport);
        let range = layout.scrolling(&widths, self.viewport);
        div()
            .flex_shrink_0()
            .w(px(scroll_tail(&range, widths.len())))
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let f = self.layout().columns[col].field;
        let m = *materials(cx);
        let cell = h_flex()
            .size_full()
            .min_w_0()
            .when(f.right_aligned(), |d| d.justify_end());
        let Some(t) = self.rows.track(row).cloned() else {
            return cell.into_any_element();
        };
        let unreadable = matches!(self.rows.data, Contents::Folder { .. })
            && !self.unreadable.is_empty()
            && self.unreadable.contains(&t.source);
        let available = self.rows.row(row).is_none_or(|r| r.available) && !unreadable;
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
                    div()
                        .id(("unavailable", self.cell_id(row)))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(13.))
                                .text_color(m.warning),
                        )
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if unreadable {
                                "This file could not be read as audio"
                            } else {
                                "This recording is unavailable"
                            })
                            .build(w, cx)
                        }),
                )
                .into_any_element(),
            Field::Rating => cell
                .child(self.render_rating(row, mark, cx))
                .into_any_element(),
            Field::Cover => cell
                .justify_center()
                .child(self.render_cover(row, &t, window, cx))
                .into_any_element(),
            Field::Favorite => cell
                .justify_center()
                .child(self.render_like(row, mark, cx))
                .into_any_element(),
            Field::Title => cell
                .text_color(if playing { m.accent } else { m.text })
                .child(
                    div()
                        .id(("track-title", self.cell_id(row)))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(t.title.clone())
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(t.title.clone()).build(w, cx)
                        }),
                )
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
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let m = *materials(cx);
        if let Some(d) = self.header(row) {
            let disc = d.clone();
            return div()
                .id(("cd-section", row))
                .overflow_hidden()
                .bg(m.raised)
                .cursor_pointer()
                .child(d.section(self.row_height, window, cx))
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "{}  ·  {}",
                        disc.title,
                        disc.folder.display()
                    ))
                    .build(w, cx)
                });
        }
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
                DraggedTracks::new(dragged, label),
                |d, _, _, cx| d.start(cx),
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
                    } else {
                        d.copy_to(playlist, &t.delegate().catalog, cx);
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
        let browser = self.browser.clone();
        let icon = match &self.rows.data {
            Contents::Library(Paged {
                scope: Scope::Playlist(_),
                ..
            }) => IconName::Music2,
            _ => IconName::FolderOpen,
        };
        gpui_kit::component::v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .px(px(24.))
            .when(!self.empty.is_empty() && !self.is_loading(), |d| {
                d.child(Icon::new(icon).size(px(24.)).text_color(m.text_faint))
                    .child(
                        div()
                            .id("empty-table-message")
                            .w_full()
                            .max_w(px(320.))
                            .text_center()
                            .text_size(px(13.))
                            .text_color(m.text_muted)
                            .role(Role::Label)
                            .aria_value(self.empty.clone())
                            .child(self.empty.clone()),
                    )
            })
            .when(self.offer_subfolders, |d| {
                d.child(
                    Button::new("include-subfolders")
                        .outline()
                        .small()
                        .label("Include Subfolders")
                        .on_click(move |_, w, cx| {
                            let _ = browser.update(cx, |b, cx| b.set_subfolders(true, w, cx));
                        }),
                )
            })
            .when(self.offer_library, |d| {
                d.child(
                    Button::new("add-music-folder")
                        .outline()
                        .small()
                        .label("Add Music Folder…")
                        .on_click(|_, w, cx| {
                            w.dispatch_action(Box::new(crate::workspace::AddMusicFolder), cx)
                        }),
                )
            })
    }
    fn context_menu(
        &mut self,
        ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if let Some(disc) = self.header(ix).cloned() {
            return disc_menu(menu, &disc, self.browser.clone(), cx.weak_entity(), cx);
        }
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
    use super::{Arc, Contents, Rows, Sections, Selection, Track, key};
    #[test]
    fn section_indices_map_only_tracks_into_actions() {
        use crate::collection::Discs;
        let mut files = Vec::new();
        for (folder, number) in [("CD1", 2), ("CD2", 1), ("CD1", 1)] {
            let mut t = Track::file(std::path::Path::new(folder).join(format!("{number}.wav")));
            t.album = "Same title".into();
            t.number = Some(number);
            files.push(Arc::new(t));
        }
        let discs = Discs::group(&files, &Default::default(), std::path::Path::new(""));
        assert_eq!(discs.len(), 2);
        assert_eq!(discs[0].tracks[0], files[2]);
        let sections = Sections::new(discs.iter().map(|d| (d.clone(), d.count)));
        assert_eq!(
            (0..5).map(|i| sections.data_index(i)).collect::<Vec<_>>(),
            [None, Some(0), Some(1), None, Some(2)]
        );
        assert_eq!(
            (0..3)
                .map(|i| sections.display_index(i))
                .collect::<Vec<_>>(),
            [1, 2, 4]
        );
        let rows = Rows {
            sections,
            data: Contents::Folder {
                original: Arc::default(),
                sorted: discs
                    .iter()
                    .flat_map(|d| d.tracks.iter().cloned())
                    .collect(),
            },
        };
        assert_eq!(rows.adjacent_track(None, 1), Some(1));
        assert_eq!(rows.adjacent_track(Some(1), -1), Some(1));
        assert_eq!(rows.adjacent_track(Some(2), 1), Some(4));
        assert_eq!(rows.adjacent_track(Some(4), -1), Some(2));
    }
    #[test]
    fn additive_selection_survives_reorder_and_shift_uses_logical_anchor() {
        let mut rows: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|p| Arc::new(Track::file((*p).into())))
            .collect();
        let folder = |rows: &Vec<Arc<Track>>| Rows {
            sections: Sections::default(),
            data: Contents::Folder {
                original: Arc::default(),
                sorted: rows.clone().into(),
            },
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

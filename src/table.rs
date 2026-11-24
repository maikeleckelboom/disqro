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
pub(crate) use playback::{ScanningFolder, play_library};
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
pub(crate) const COVER_X: f32 = 8.;
const THUMBNAIL: f32 = 24.;
const LARGE_THUMBNAIL: f32 = 96.;
const COVER_Y: f32 = 2.;
pub(crate) const COVER_WIDTH: f32 = THUMBNAIL + 2. * COVER_X;
pub(crate) const COVER_MAX_WIDTH: f32 = LARGE_THUMBNAIL + 2. * COVER_X;
/// The like control is a 14-pixel glyph; its column needs no more than a
/// comfortable hit target. The heading draws the same glyph.
pub(crate) const FAVORITE_WIDTH: f32 = 36.;

pub(crate) fn cover_edge(width: f32) -> f32 {
    (width - 2. * COVER_X)
        .clamp(THUMBNAIL, LARGE_THUMBNAIL)
        .round()
}

/// Keep the same top row when a different thumbnail size changes the row pitch.
pub(crate) fn cover_row_height<D: TableDelegate>(
    table: &TableState<D>,
    previous: f32,
    edge: f32,
) -> f32 {
    let height = |edge: f32| (edge + 2. * COVER_Y).max(ROW_HEIGHT);
    let (old, new) = (height(previous), height(edge));
    if old != new {
        let scroll = table.vertical_scroll_handle.0.borrow();
        let offset = scroll.base_handle.offset();
        scroll
            .base_handle
            .set_offset(point(offset.x, offset.y * (new / old)));
    }
    new
}

pub(crate) fn playing_icon(cx: &App) -> Icon {
    Icon::new(IconName::AudioLines)
        .size(px(14.))
        .text_color(materials(cx).accent)
}

/// A cover thumbnail, or the one fallback every artwork slot shares: a quiet
/// tile with a disc glyph. A slot is never left blank, whether the release has
/// no artwork or its image is still loading.
pub(crate) fn cover_tile(edge: f32, image: Option<Arc<RenderImage>>, cx: &App) -> Div {
    let m = materials(cx);
    div()
        .size(px(edge))
        .flex_shrink_0()
        .rounded(px(if edge > 40. { 4. } else { 2. }))
        .overflow_hidden()
        .map(|d| match image {
            Some(image) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
            None => d
                .bg(m.control)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Disc3)
                        .size(px((edge * 0.5).clamp(12., 40.).round()))
                        .text_color(m.text_faint),
                ),
        })
}
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
    Plays,
    LastPlayed,
    ListeningTime,
    #[serde(alias = "Liked")]
    Favorite,
}
impl Field {
    const ALL: [Self; 15] = [
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
        Self::Plays,
        Self::LastPlayed,
        Self::ListeningTime,
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
            Self::Plays => "Plays",
            Self::LastPlayed => "Last Played",
            Self::ListeningTime => "Listening Time",
        }
    }
    fn heading(self) -> &'static str {
        match self {
            Self::Cover | Self::Favorite => "",
            f => f.label(),
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
            Self::Cover => COVER_WIDTH,
            Self::Number => 40.,
            Self::Favorite => FAVORITE_WIDTH,
            Self::Title => 200.,
            Self::Artist => 135.,
            Self::Album => 148.,
            Self::Rating => 100.,
            Self::Length => 60.,
            Self::Format | Self::Year | Self::Disc => 64.,
            Self::Path => 330.,
            Self::Plays => 64.,
            Self::LastPlayed => 152.,
            Self::ListeningTime => 120.,
        }
    }
    fn min_width(self) -> f32 {
        match self {
            Self::Cover => self.width(),
            Self::Favorite => FAVORITE_WIDTH,
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
            Self::Cover => COVER_MAX_WIDTH,
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
        matches!(
            self,
            Self::Number | Self::Length | Self::Plays | Self::ListeningTime
        )
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
            Self::Plays | Self::LastPlayed | Self::ListeningTime => {
                unreachable!("listening field is formatted from totals")
            }
        }
    }
    fn listening(self) -> bool {
        matches!(self, Self::Plays | Self::LastPlayed | Self::ListeningTime)
    }
    fn display(self, t: &Track, mark: Mark, catalog: &Catalog) -> String {
        use crate::listening::{duration, local_date};
        match self {
            Self::Plays => catalog.listening_of(t).plays.to_string(),
            Self::LastPlayed => catalog
                .listening_of(t)
                .last_played
                .map_or_else(|| "Never".into(), |at| local_date(at, "%Y-%m-%d %H:%M")),
            Self::ListeningTime => duration(catalog.listening_of(t).credited_us),
            _ => self.value(t, mark),
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
            Self::Plays => O::Plays,
            Self::LastPlayed => O::LastPlayed,
            Self::ListeningTime => O::ListeningTime,
        })
    }
    fn sortable(self) -> bool {
        self.order().is_some()
    }
}
/// What an empty table says when its filter matched nothing.
pub const NO_MATCHES: &str = "No matching tracks";

/// A row's identity for selection: a physical file or authored segment in a
/// folder, or a library view key. Tags and positions are deliberately
/// absent, so a refresh or sort keeps the same selection.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum RowKey {
    Disc(DiscId),
    /// A release in the Albums / CDs list, by its first disc.
    Release(DiscId),
    Link(crate::collection::ShelfLink),
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
    playlist_tracks: Vec<i64>,
    requested_child: Option<Scope>,
    child_row: Option<(Scope, i64)>,
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
pub struct Sections(Arc<[Section]>);
struct Section {
    header: usize,
    end: usize,
    disc: Arc<Disc>,
}
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
                    Section {
                        header,
                        end: row,
                        disc,
                    }
                })
                .collect(),
        )
    }
    fn header(&self, row: usize) -> Option<&Arc<Disc>> {
        let section = self.0.get(self.preceding(row))?;
        (section.header == row).then_some(&section.disc)
    }
    fn parent(&self, row: usize) -> Option<&Disc> {
        let section = self.0.get(self.preceding(row).checked_sub(1)?)?;
        (row < section.end).then_some(&section.disc)
    }
    fn preceding(&self, row: usize) -> usize {
        self.0.partition_point(|section| section.header < row)
    }
    fn data_index(&self, row: usize) -> Option<usize> {
        let count = self.preceding(row);
        self.0
            .get(count)
            .is_none_or(|section| section.header != row)
            .then(|| row - count)
    }
    fn display_index(&self, index: usize) -> usize {
        let mut lo = 0;
        let mut hi = self.0.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.0[mid].header - mid <= index {
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
                .find(|section| section.disc.id == *id)
                .map(|section| section.header);
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
    fn recording_key(&self, track: &Track, library_id: Option<i64>) -> Option<RowKey> {
        match &self.data {
            Contents::Folder { sorted, .. } => {
                sorted.iter().find(|t| t.same_item(track)).map(|t| key(t))
            }
            Contents::Library(p) => {
                let id = library_id.or(track.library_id).or_else(|| {
                    p.cache
                        .values()
                        .find(|r| r.track.same_item(track))
                        .map(|r| r.id)
                })?;
                let identities = if p.scope.keys_are_entries() {
                    &p.playlist_tracks[..]
                } else {
                    &p.keys
                };
                let index = identities.iter().position(|&k| k == id)?;
                Some(RowKey::Key(p.keys[index]))
            }
        }
    }

    fn playlist_row_matches(
        &self,
        row: usize,
        occurrence: crate::queue::PlaylistOccurrence,
    ) -> Option<bool> {
        let Contents::Library(p) = &self.data else {
            return None;
        };
        let Scope::Playlist(playlist) = p.scope else {
            return None;
        };
        Some(
            playlist == occurrence.playlist
                && self.sections.data_index(row).and_then(|i| p.keys.get(i))
                    == Some(&occurrence.entry),
        )
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

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Selection {
    marked: Arc<HashSet<RowKey>>,
    anchor: Option<RowKey>,
    cursor: Option<RowKey>,
    #[serde(skip)]
    ordered: std::cell::OnceCell<Arc<Targets>>,
}
impl Selection {
    pub fn len(&self) -> usize {
        self.marked.len()
    }

    fn modify(
        &mut self,
        rows: &Rows,
        primary: Option<usize>,
        at: usize,
        range: bool,
        additive: bool,
    ) {
        self.ordered.take();
        let marked = Arc::make_mut(&mut self.marked);
        if marked.is_empty()
            && let Some(k) = primary
                .and_then(|i| rows.key(i))
                .filter(|k| !matches!(k, RowKey::Disc(_)))
        {
            marked.insert(k.clone());
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
                marked.clear();
            }
            marked.extend(
                (from.min(at)..=from.max(at))
                    .filter_map(|i| rows.key(i))
                    .filter(|k| !matches!(k, RowKey::Disc(_))),
            );
            self.anchor.get_or_insert(clicked);
        } else {
            if !marked.remove(&clicked) {
                marked.insert(clicked.clone());
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
    pub folder_page: Option<Page>,
    pub scanning_folder: Option<ScanningFolder>,
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
    primary: Option<RowKey>,
    focus: Option<FocusHandle>,
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
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Viewport {
    pub(crate) anchor: Option<RowKey>,
    within_row: f32,
    scroll: f32,
    horizontal: f32,
}
impl Viewport {
    pub fn legacy(scroll: f32) -> Self {
        Self {
            scroll,
            ..Self::default()
        }
    }

    pub fn capture<D: TableDelegate>(
        table: &TableState<D>,
        height: f32,
        key: impl FnOnce(usize) -> Option<RowKey>,
    ) -> Self {
        let offset = table.vertical_scroll_handle.0.borrow().base_handle.offset();
        let scroll = (-f32::from(offset.y)).max(0.);
        let first = (scroll / height) as usize;
        Self {
            anchor: (scroll > 0.).then(|| key(first)).flatten(),
            within_row: scroll - first as f32 * height,
            scroll,
            horizontal: (-f32::from(table.horizontal_scroll_handle.offset().x)).max(0.),
        }
    }

    pub fn restore<D: TableDelegate>(
        &self,
        table: &TableState<D>,
        height: f32,
        position: impl FnOnce(&RowKey) -> Option<usize>,
    ) {
        let y = self.offset_for(height, position);
        // UniformList clamps against the destination's content during prepaint.
        let mut scroll = table.vertical_scroll_handle.0.borrow_mut();
        scroll.deferred_scroll_to_item = None;
        scroll.base_handle.set_offset(point(px(0.), px(-y.max(0.))));
        table
            .horizontal_scroll_handle
            .set_offset(point(px(-self.horizontal.max(0.)), px(0.)));
    }

    pub fn is_at_start(&self) -> bool {
        self.scroll == 0. && self.horizontal == 0.
    }
    fn offset_for(&self, height: f32, position: impl FnOnce(&RowKey) -> Option<usize>) -> f32 {
        self.anchor
            .as_ref()
            .and_then(position)
            .map_or(self.scroll, |row| {
                row as f32 * height + self.within_row.clamp(0., height - 0.01)
            })
    }
}
fn top_anchor(t: &TableState<MusicTable>) -> Viewport {
    Viewport::capture(t, t.delegate().row_height, |row| t.delegate().rows.key(row))
}
fn restore_anchor(t: &TableState<MusicTable>, viewport: Viewport) {
    viewport.restore(t, t.delegate().row_height, |key| t.delegate().position(key));
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
            disc.root,
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
    root: Option<crate::store::RootId>,
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
                let _ =
                    browser.update(cx, |b, cx| b.open_release(&key, title.clone(), root, w, cx));
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
    command: impl FnOnce(crate::queue::Selection) -> Command + 'static,
    cx: &mut App,
) {
    let selection = catalog.read(cx).queue_selection(targets, cx);
    let catalog = catalog.clone();
    let player = player.clone();
    cx.spawn(async move |cx| match selection.await {
        Ok(selection) if !selection.tracks.is_empty() => {
            player.update(cx, |p, cx| p.command(command(selection), cx))
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

fn sort_folder(
    sorted: &mut TrackList,
    sort: Option<(Field, ColumnSort)>,
    marks: impl Fn(&Track) -> Mark,
    listening: impl Fn(&Track) -> crate::listening::Totals,
) {
    if let Some((f, order)) = sort {
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
            Field::Plays => rows.sort_by_cached_key(|t| listening(t).plays),
            Field::LastPlayed => rows.sort_by_cached_key(|t| listening(t).last_played),
            Field::ListeningTime => rows.sort_by_cached_key(|t| listening(t).credited_us),
            _ => rows.sort_by_cached_key(|t| f.value(t, Mark::default()).to_lowercase()),
        }
        if order == ColumnSort::Descending {
            rows.reverse();
        }
    }
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
            folder_page: None,
            scanning_folder: None,
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
            primary: None,
            focus: None,
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
        let cover = live.map_or(THUMBNAIL, cover_edge);
        let row = cover_row_height(table, d.cover, cover);
        let d = table.delegate_mut();
        d.cover = cover;
        d.row_height = row;
        if d.resizing.is_none() {
            d.settled_cover = settled.map_or(THUMBNAIL, cover_edge);
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
            let d = table.delegate();
            let shown = d.layout().widths(d.viewport);
            table.delegate_mut().layout_mut().settle(&shown);
            table.refresh(cx);
            table.delegate().save_layout(cx);
            cx.notify();
        }
    }
    /// Lays the columns out for the table's width; `true` when it changed.
    pub fn fit(
        table: &mut TableState<Self>,
        width: f32,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        if table.delegate().viewport == width {
            return false;
        }
        let scroll = table.horizontal_scroll_handle.clone();
        table.delegate_mut().horizontal_scroll = Some(scroll);
        table.delegate_mut().viewport = width;
        table.refresh(cx);
        cx.notify();
        true
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
        requested_child: Option<Scope>,
        cx: &mut Context<TableState<Self>>,
    ) {
        let same = matches!(&self.rows.data, Contents::Library(p) if p.scope == scope);
        if same {
            if let Contents::Library(p) = &mut self.rows.data {
                p.search = search;
                p.grouped = grouped;
                p.requested_child = requested_child;
            }
        } else {
            self.view += 1;
            self.clear_marks();
            self.rows.sections = Sections::default();
            self.rows.data = Contents::Library(Paged {
                scope,
                search,
                keys: Arc::default(),
                playlist_tracks: Vec::new(),
                requested_child,
                child_row: None,
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
        let Contents::Library(p) = &mut self.rows.data else {
            return;
        };
        let Some(store) = self.catalog.read(cx).handle() else {
            return;
        };
        p.loaded = false;
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
        let child = p.requested_child;
        let listing = store.query(&self.keys_ticket, generation, move |s| {
            let (listing, albums) = s.table_listing(&q, grouped, child)?;
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
                    p.playlist_tracks = listing.playlist_tracks;
                    p.child_row = listing.child_row;
                    p.requested_child = None;
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
        sort_folder(sorted, self.sort, marks, |t| {
            self.catalog.read(cx).listening_of(t)
        });
    }
    pub fn listening_changed(
        t: &mut TableState<Self>,
        order: bool,
        cx: &mut Context<TableState<Self>>,
    ) {
        let table = t.delegate();
        let sorted = table.sort.is_some_and(|(field, _)| {
            field == Field::ListeningTime || (order && field.listening())
        });
        match &table.rows.data {
            Contents::Library(p) if sorted || (order && p.scope == Scope::RecentlyPlayed) => {
                t.delegate_mut().query(cx);
            }
            Contents::Folder { .. } if sorted => {
                let selected = t.selected_row().and_then(|i| table.rows.key(i));
                t.delegate_mut().apply_sort(cx);
                if let Some(row) = selected.and_then(|key| t.delegate().position(&key)) {
                    t.set_selected_row(row, cx);
                }
                cx.notify();
            }
            _ if table.layout().columns.iter().any(|c| c.field.listening()) => cx.notify(),
            _ => {}
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
    pub fn row_height(&self) -> f32 {
        self.row_height
    }
    pub fn bind_focus(
        table: &mut TableState<Self>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let handle = table.focus_handle(cx);
        cx.on_focus(&handle, window, |_, _, cx| cx.notify())
            .detach();
        cx.on_blur(&handle, window, |_, _, cx| cx.notify()).detach();
        table.delegate_mut().focus = Some(handle);
    }

    pub fn set_primary(&mut self, primary: Option<RowKey>) {
        self.primary = primary;
    }

    pub fn cursor_row(&self, primary: Option<usize>) -> Option<usize> {
        self.selection
            .cursor
            .as_ref()
            .and_then(|key| self.position(key))
            .or(primary)
    }
    pub fn position(&self, k: &RowKey) -> Option<usize> {
        self.rows.position(k)
    }
    pub fn first_track(&self) -> usize {
        self.rows.sections.display_index(0)
    }
    pub(crate) fn child_position(&self, key: &RowKey, id: &DiscId) -> Option<usize> {
        if matches!(key, RowKey::Release(_))
            && let Some(section) = self
                .rows
                .sections
                .0
                .iter()
                .find(|s| s.disc.release.as_ref().is_some_and(|m| m.key.same(id)))
        {
            return Some(section.header);
        }
        match (&self.rows.data, id) {
            (Contents::Library(p), DiscId::Library(id)) => {
                let child = if matches!(key, RowKey::Release(_)) {
                    Scope::Release(*id)
                } else {
                    Scope::Album(*id)
                };
                let (_, row) = p.child_row.filter(|(scope, _)| *scope == child)?;
                self.rows.position(&RowKey::Key(row))
            }
            (Contents::Folder { sorted, .. }, _) => {
                let disc = if matches!(key, RowKey::Release(_)) {
                    self.programs
                        .iter()
                        .filter(|d| d.release.as_ref().is_some_and(|m| m.key.same(id)))
                        .find_map(|d| sorted.iter().position(|t| d.id.contains(t)))
                } else {
                    sorted.iter().position(|t| id.contains(t))
                }?;
                Some(self.rows.sections.display_index(disc))
            }
            _ => None,
        }
    }

    pub fn extend_selection(&mut self, primary: Option<usize>, direction: isize) -> Option<usize> {
        let current = self.cursor_row(primary);
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
        self.has_selection_cursor().then(|| Selection {
            marked: self.selection.marked.clone(),
            anchor: self.selection.anchor.clone(),
            cursor: self.selection.cursor.clone(),
            ordered: Default::default(),
        })
    }
    pub fn has_selection_cursor(&self) -> bool {
        self.selection.cursor.is_some() || !self.selection.marked.is_empty()
    }
    pub fn restore_marks(&mut self, mut selection: Selection, filtered: bool) {
        if !filtered && !selection.marked.is_empty() {
            let found: HashSet<_> = (0..self.rows.len())
                .filter_map(|i| self.rows.key(i))
                .filter(|key| selection.marked.contains(key))
                .collect();
            if found.len() != selection.marked.len() {
                selection.marked = Arc::new(found);
            }
        }
        if !filtered {
            selection.anchor = selection.anchor.filter(|key| self.position(key).is_some());
            selection.cursor = selection
                .cursor
                .filter(|key| self.position(key).is_some())
                .or_else(|| selection.anchor.clone())
                .or_else(|| {
                    (0..self.rows.len())
                        .filter_map(|row| self.rows.key(row))
                        .find(|key| selection.marked.contains(key))
                });
            selection.anchor = selection.anchor.or_else(|| selection.cursor.clone());
        }
        self.primary = None;
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
    pub(crate) fn disc_playing(&self, disc: &Disc, cx: &App) -> bool {
        let status = &self.player.read(cx).status;
        let Some(track) = status.track.as_ref().filter(|_| status.playing) else {
            return false;
        };
        match disc.id {
            DiscId::Library(id) => self.catalog.read(cx).playing_album(track) == Some(id),
            _ => disc.id.contains(track) && disc.tracks.iter().any(|t| t.same_item(track)),
        }
    }

    pub(crate) fn contains_playlist_entry(
        &self,
        occurrence: crate::queue::PlaylistOccurrence,
        track_id: Option<i64>,
    ) -> bool {
        let Contents::Library(p) = &self.rows.data else {
            return false;
        };
        p.scope == Scope::Playlist(occurrence.playlist)
            && p.keys
                .iter()
                .position(|&key| key == occurrence.entry)
                .is_some_and(|index| p.playlist_tracks.get(index).copied() == track_id)
    }

    fn is_playing(&self, row: usize, t: &Track, cx: &App) -> bool {
        let player = self.player.read(cx);
        player.status.playing
            && player.status.track.as_ref().is_some_and(|p| p.same_item(t))
            && player
                .playing_entry()
                .and_then(|entry| entry.playlist_occurrence)
                .and_then(|occurrence| self.rows.playlist_row_matches(row, occurrence))
                .unwrap_or(true)
    }
    /// A stable element id for the cells of row `ix`.
    fn cell_id(&self, ix: usize) -> u64 {
        stable_element_key(self.rows.key(ix))
    }
    /// Flat rows use the regular placeholder. In a section, only a track
    /// image that adds to the header's artwork needs a tile of its own.
    fn render_cover(
        &self,
        row: usize,
        t: &Track,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Div> {
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
        let cover = crate::artwork::TrackCover {
            source: t.source.clone(),
            authored_folder: folder
                .filter(|f| Some(*f) != t.source.parent())
                .map(ToOwned::to_owned),
            disc_cover: fallback,
            listing: self.artwork_revision,
        }
        .image(self.settled_cover, window, cx);
        if let Some(disc) = self.rows.sections.parent(row) {
            let (file, image) = cover?;
            if disc
                .section_cover(self.settled_cover, self.artwork_revision, window, cx)
                .is_some_and(|(shared, _)| shared == file)
            {
                return None;
            }
            return Some(cover_tile(self.cover, Some(image), cx));
        }
        Some(cover_tile(self.cover, cover.map(|(_, image)| image), cx))
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
            .border_1()
            .border_color(transparent_black())
            .focus_visible(|s| s.opacity(1.).border_color(m.focus_line()))
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
                    t.delegate_mut().layout_mut().holding = width.map(|_| ix);
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

pub(crate) fn stable_element_key(key: impl std::hash::Hash) -> u64 {
    use std::hash::BuildHasher;
    FIXED.hash_one(key)
}

pub(crate) fn sort_heading(
    col_ix: usize,
    label: &'static str,
    right_aligned: bool,
    sorted: Option<ColumnSort>,
    cx: &App,
) -> Stateful<Div> {
    let m = *materials(cx);
    h_flex()
        .id(("sort", col_ix))
        .size_full()
        .gap_1()
        .when(right_aligned, |d| d.justify_end())
        .text_size(px(crate::theme::size::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if sorted.is_some() {
            m.text_muted
        } else {
            m.text_faint
        })
        .whitespace_nowrap()
        .child(label)
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
}

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
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if self.visible != range {
            self.visible = range;
            self.request_rows(false, cx);
            let browser = self.browser.clone();
            cx.defer_in(window, move |_, _, cx| {
                let _ = browser.update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
            });
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
        // Artwork explains itself. Its heading keeps the name for assistive
        // technology and the column menu, without printing it.
        sort_heading(col_ix, field.heading(), field.right_aligned(), sorted, cx)
            .aria_label(field.label())
            .when(field == Field::Favorite, |d| {
                d.justify_center().child(
                    Icon::new(IconName::Heart)
                        .size(px(13.))
                        .text_color(m.text_faint),
                )
            })
            .children(trailing)
            .children(leading)
            .when(field.sortable(), |d| {
                d.hover(|d| d.text_color(m.text_muted))
                    .cursor_pointer()
                    .on_click(cx.listener(move |t, _, w, cx| {
                        let selected = t.selected_row().and_then(|i| t.delegate().rows.key(i));
                        let viewport = top_anchor(t);
                        t.delegate_mut().cycle_sort(field, cx);
                        let ix = selected.and_then(|k| t.delegate().position(&k));
                        reselect(t, ix, cx);
                        restore_anchor(t, viewport);
                        t.refresh(cx);
                        cx.notify();
                        let browser = t.delegate().browser.clone();
                        cx.defer_in(w, move |_, _, cx| {
                            let _ =
                                browser.update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
                        });
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
            return (col == 0).then(|| {
                if self.disc_playing(d, cx) {
                    format!("{} · Now playing", d.title).into()
                } else {
                    d.title.clone().into()
                }
            });
        }
        let t = self.rows.track(row)?;
        let field = self.layout().columns[col].field;
        if field == Field::Number && self.is_playing(row, t, cx) {
            return Some(format!("Now playing · {}", field.value(t, self.mark(row, cx))).into());
        }
        if field == Field::Title && !self.layout().has(Field::Number) && self.is_playing(row, t, cx)
        {
            return Some(format!("{} · Now playing", t.title).into());
        }
        (!matches!(field, Field::Cover | Field::Favorite)).then(|| {
            field
                .display(t, self.mark(row, cx), self.catalog.read(cx))
                .into()
        })
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
        let playing = self.is_playing(row, &t, cx);
        let cell = cell.when(!available, |d| d.opacity(0.45));
        match f {
            Field::Number if playing => cell.child(playing_icon(cx)).into_any_element(),
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
                .children(self.render_cover(row, &t, window, cx))
                .into_any_element(),
            Field::Favorite => cell
                .justify_center()
                .child(self.render_like(row, mark, cx))
                .into_any_element(),
            Field::Title => cell
                .pl(px(self.layout().content_inset(
                    col,
                    self.rows.sections.parent(row).is_some(),
                )))
                .when(playing && !self.layout().has(Field::Number), |d| {
                    d.gap(px(5.)).child(playing_icon(cx))
                })
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
                .when(f == Field::Format, |d| {
                    d.text_size(px(crate::theme::size::TEXT_SMALL))
                })
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(f.display(&t, mark, self.catalog.read(cx))),
                )
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
        let focused = self
            .focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window));
        let key = self.rows.key(row);
        let primary = self
            .primary
            .as_ref()
            .zip(key.as_ref())
            .is_some_and(|(a, b)| a == b);
        let cursor = self
            .selection
            .cursor
            .as_ref()
            .or(self.primary.as_ref())
            .zip(key.as_ref())
            .is_some_and(|(a, b)| a == b);
        let focus_indicator = || {
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(2.))
                .bg(m.accent.opacity(0.70))
        };
        if let Some(d) = self.header(row) {
            let disc = d.clone();
            let layout = self.layout();
            let lead = layout.header_lead(&layout.widths(self.viewport));
            return div()
                .id(("cd-section", row))
                .overflow_hidden()
                .bg(m.raised)
                .when(primary && !focused, |d| {
                    d.child(div().absolute().size_full().bg(m.canvas.opacity(0.55)))
                })
                .when(focused && cursor, |d| d.child(focus_indicator()))
                .cursor_pointer()
                .child(
                    d.section(
                        self.cover,
                        lead,
                        self.settled_cover,
                        self.artwork_revision,
                        window,
                        cx,
                    )
                    .when(self.disc_playing(d, cx), |d| d.child(playing_icon(cx))),
                )
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "{}  ·  {}",
                        disc.title,
                        disc.folder.display()
                    ))
                    .build(w, cx)
                });
        }
        let marked = key.is_some_and(|key| self.selection.marked.contains(&key));
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
            .when(marked, |d| d.bg(if focused { m.selected } else { m.canvas.blend(m.selected.opacity(0.48)) }))
            .when(primary && !focused, |d| d.child(div().absolute().size_full().bg(m.canvas.opacity(0.55))))
            .when(focused && cursor, |d| d.child(focus_indicator()))
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
                        delegate.set_primary(None);
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
        // The icon names what is empty: a filter that matched nothing, or
        // the kind of place being shown.
        let icon = match &self.rows.data {
            _ if self.empty.as_ref() == NO_MATCHES => Icon::new(IconName::ListFilter),
            Contents::Library(Paged {
                scope: Scope::Liked,
                ..
            }) => Icon::default().path(crate::assets::HEART_FILL),
            Contents::Library(_) => Icon::new(IconName::Music2),
            _ => Icon::new(IconName::FolderOpen),
        };
        gpui_kit::component::v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .px(px(24.))
            .when(!self.empty.is_empty() && !self.is_loading(), |d| {
                d.child(icon.size(px(24.)).text_color(m.text_faint)).child(
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
            move |targets: Targets,
                  command: fn(crate::queue::Selection) -> Command,
                  cx: &mut App| {
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
        let (track_table, track_key) = (this.clone(), clicked.clone());
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
            PopupMenuItem::new("Play Only This Track").on_click(move |_, _, cx| {
                let _ = track_table.update(cx, |t, cx| {
                    let d = t.delegate();
                    if d.view == view
                        && let Some(i) = track_key.as_ref().and_then(|k| d.rows.position(k))
                    {
                        d.play_track(i, cx);
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
    use super::{Arc, Contents, RowKey, Rows, Sections, Selection, Track, Viewport, key};
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
        for (row, disc) in [(1, &discs[0]), (2, &discs[0]), (4, &discs[1])] {
            assert_eq!(rows.sections.parent(row).unwrap().id, disc.id);
        }
        for (row, disc) in [(0, &discs[0]), (3, &discs[1])] {
            assert!(rows.sections.parent(row).is_none());
            assert_eq!(rows.key(row), Some(RowKey::Disc(disc.id.clone())));
            assert_eq!(rows.position(&RowKey::Disc(disc.id.clone())), Some(row));
            assert!(rows.track(row).is_none());
        }
    }
    #[test]
    fn filtering_preserves_chapter_keys_and_indents_only_visible_children() {
        use crate::collection::{Discs, matches};
        use crate::store::Term;
        use std::{fs, path::Path};
        let dir = tempfile::tempdir().unwrap();
        let sheet = dir.path().join("Set.cue");
        fs::write(
            &sheet,
            "FILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE One\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE Two\nINDEX 01 00:01:00",
        )
        .unwrap();
        let chapters: Vec<_> = crate::cue::read(&sheet)
            .unwrap()
            .into_iter()
            .map(Arc::new)
            .collect();
        let discs = Discs::group(&chapters, &Default::default(), dir.path());
        let disc = &discs[0];
        assert!(disc.cover.is_none());
        let standalone = Arc::new(Track::file(Path::new("Unattached.wav").into()));
        let project = |tracks: Vec<Arc<Track>>| {
            let sections = Sections::new([(disc.clone(), tracks.len())]);
            Rows {
                sections,
                data: Contents::Folder {
                    original: Arc::default(),
                    sorted: tracks.into_iter().chain([standalone.clone()]).collect(),
                },
            }
        };
        let all = project(chapters.clone());
        assert_eq!(
            all.recording_key(&chapters[1], None),
            Some(key(&chapters[1]))
        );
        let mut shifted = (*chapters[1]).clone();
        shifted.disc.as_mut().unwrap().start += 1;
        assert_eq!(all.recording_key(&shifted, None), None);
        assert_eq!(all.sections.parent(2).unwrap().id, disc.id);
        assert!(all.sections.parent(3).is_none());
        let mut selection = Selection::default();
        selection.modify(&all, None, 2, false, true);
        let selected = all.key(2).unwrap();
        let terms = Term::parse("title:two");
        let visible = project(
            chapters
                .into_iter()
                .filter(|t| matches(t, &terms, Default::default))
                .collect(),
        );
        assert_eq!(visible.key(0), all.key(0));
        assert_eq!(visible.position(&selected), Some(1));
        assert!(selection.marked.contains(&visible.key(1).unwrap()));
        assert_eq!(visible.track(1).unwrap().display_number(), Some(2));
        assert_eq!(visible.sections.parent(1).unwrap().id, disc.id);
        assert_eq!(visible.adjacent_track(Some(1), 1), Some(2));
        assert_eq!(visible.key(2), Some(key(&standalone)));
        assert!(visible.sections.parent(2).is_none());
        let no_matches = project(vec![]);
        assert_eq!(no_matches.len(), 1);
        assert_eq!(no_matches.key(0), Some(key(&standalone)));
        assert!(no_matches.sections.parent(0).is_none());
    }
    #[test]
    fn indexed_tagged_album_projects_the_same_track_keys_with_or_without_a_header() {
        use crate::store::{Order, Query, Scope, Store, index_root};
        use lofty::{
            config::WriteOptions,
            prelude::*,
            tag::{Tag, TagType},
        };
        let dir = tempfile::tempdir().unwrap();
        for (name, title, number) in [("a.wav", "Second", 2), ("b.wav", "First", 1)] {
            let path = dir.path().join(name);
            crate::audio::tests::fixture(&path, 44100, 441);
            let mut tag = Tag::new(TagType::Id3v2);
            tag.set_album("Scratch Beats".into());
            tag.set_title(title.into());
            tag.set_track(number);
            tag.save_to_path(path, WriteOptions::default()).unwrap();
        }
        let mut store = Store::memory();
        let root = store.add_root(dir.path()).unwrap();
        index_root(&mut store, root, None, &|| true, &mut |_| {}).unwrap();
        let project = |search: &str, order| {
            let scope = Scope::Library(Some(root));
            let query = Query {
                scope,
                search: search.into(),
                order,
            };
            let (listing, albums) = store.table_listing(&query, order.is_none(), None).unwrap();
            let sections = Sections::new(albums.into_iter().map(|a| {
                let count = a.count;
                (Arc::new(super::Disc::from_album(a)), count)
            }));
            let cache = store
                .rows(scope, &listing.keys)
                .unwrap()
                .into_iter()
                .map(|row| (row.key, row))
                .collect();
            Rows {
                sections,
                data: Contents::Library(super::Paged {
                    scope,
                    search: query.search,
                    keys: listing.keys.into(),
                    playlist_tracks: listing.playlist_tracks,
                    requested_child: None,
                    child_row: listing.child_row,
                    cache,
                    loaded: true,
                    grouped: order.is_none(),
                }),
            }
        };
        let grouped = project("", None);
        assert_eq!(grouped.track(1).unwrap().title, "First");
        assert_eq!(grouped.track(2).unwrap().title, "Second");
        assert_eq!(grouped.sections.parent(2).unwrap().title, "Scratch Beats");
        let filtered = project("title:second", None);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered.key(0), grouped.key(0));
        assert_eq!(filtered.key(1), grouped.key(2));
        assert!(filtered.sections.parent(1).unwrap().cover.is_none());
        let sorted = project("", Some((Order::Title, false)));
        assert_eq!(sorted.key(1), grouped.key(2));
        assert!(sorted.sections.parent(1).is_none());
    }
    #[test]
    fn playing_recording_in_an_unhydrated_playlist_uses_its_first_displayed_occurrence() {
        let rows = Rows {
            sections: Sections::default(),
            data: Contents::Library(super::Paged {
                scope: crate::store::Scope::Playlist(3),
                search: String::new(),
                keys: [11, 12, 13].into(),
                playlist_tracks: vec![12, 41, 41],
                requested_child: None,
                child_row: None,
                cache: Default::default(),
                loaded: true,
                grouped: false,
            }),
        };
        let track = Track::file("recording.wav".into());
        assert_eq!(rows.recording_key(&track, Some(41)), Some(RowKey::Key(12)));
        assert_eq!(rows.recording_key(&track, Some(12)), Some(RowKey::Key(11)));
        assert_eq!(rows.recording_key(&track, Some(13)), None);
        let occurrence = crate::queue::PlaylistOccurrence {
            playlist: 3,
            entry: 13,
        };
        assert_eq!(rows.playlist_row_matches(1, occurrence), Some(false));
        assert_eq!(rows.playlist_row_matches(2, occurrence), Some(true));
        let from_another_playlist = crate::queue::PlaylistOccurrence {
            playlist: 4,
            ..occurrence
        };
        assert_eq!(
            rows.playlist_row_matches(2, from_another_playlist),
            Some(false)
        );
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
        let saved = serde_json::to_vec(&selection).unwrap();
        let mut selection: Selection = serde_json::from_slice(&saved).unwrap();
        rows.reverse();
        selection.modify(&folder(&rows), None, 3, true, false);
        assert_eq!(selection.marked.len(), 3);
        assert!(!selection.marked.contains(&key(&rows[0])));
        selection.modify(&folder(&rows), None, 1, false, true);
        assert_eq!(selection.marked.len(), 2);
    }

    #[test]
    fn viewport_relocates_its_logical_row_and_keeps_the_pixel_remainder() {
        let viewport = Viewport {
            anchor: Some(RowKey::Key(617)),
            within_row: 12.5,
            scroll: 100.5,
            horizontal: 30.,
        };
        let viewport: Viewport =
            serde_json::from_value(serde_json::to_value(viewport).unwrap()).unwrap();
        assert_eq!(
            viewport.offset_for(28., |key| {
                assert_eq!(*key, RowKey::Key(617));
                Some(21)
            }),
            600.5
        );
        assert_eq!(viewport.offset_for(28., |_| None), 100.5);
        assert_eq!(viewport.horizontal, 30.);
    }
}

//! A disc has an authored CUE program, or a tagged album in one folder.
//! Two identically titled rips in different folders remain distinct.
use crate::{
    artwork::{CoverFile, CoverThumbnail, TrackCover},
    library::{self, Track},
    store::{AlbumId, AlbumRow, Mark, PlaylistId, RootId, Term},
    table::{
        ColumnInteraction, ContextKind, Field, Layout, SHELF_FIELDS, TableColumns, Viewport,
        reselect,
    },
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, h_flex,
        table::{Column, ColumnSort, TableDelegate, TableState},
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Tracks,
    Albums,
    Tree,
}

impl From<bool> for ViewMode {
    fn from(albums: bool) -> Self {
        if albums { Self::Albums } else { Self::Tracks }
    }
}

pub(crate) fn read_view_mode<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ViewMode, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SavedMode {
        Current(ViewMode),
        Legacy(bool),
    }
    Ok(match SavedMode::deserialize(d) {
        Ok(SavedMode::Current(mode)) => mode,
        Ok(SavedMode::Legacy(albums)) => albums.into(),
        Err(_) => ViewMode::default(),
    })
}

/// How collections are presented and which physical folders include descendants.
/// Earlier versions' global recursive switch is ignored.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPrefs {
    #[serde(alias = "albums", deserialize_with = "read_view_mode")]
    pub mode: ViewMode,
    pub local_collections: Vec<RootId>,
    pub subfolders: Vec<SubfolderRule>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubfolderRule {
    pub path: PathBuf,
    pub include: bool,
}
/// Remembered folder choices beyond this many forget the oldest.
const RULES: usize = 256;
impl ViewPrefs {
    pub fn collection_includes_subfolders(&self, root: RootId) -> bool {
        !self.local_collections.contains(&root)
    }

    pub fn set_collection_subfolders(&mut self, root: RootId, include: bool) {
        self.local_collections.retain(|&id| id != root);
        if !include {
            self.local_collections.push(root);
            let excess = self.local_collections.len().saturating_sub(RULES);
            self.local_collections.drain(..excess);
        }
    }

    /// Whether `folder` is browsed with its subfolders: the choice made for
    /// it or its nearest ancestor, or not.
    pub fn includes_subfolders(&self, folder: &Path) -> bool {
        self.subfolder_rule(folder).is_some_and(|r| r.include)
    }
    pub fn subfolder_rule(&self, folder: &Path) -> Option<&SubfolderRule> {
        self.subfolders
            .iter()
            .filter(|r| library::within(folder, &r.path))
            .max_by_key(|r| r.path.as_os_str().len())
    }
    /// Records a choice for `folder` and everything beneath it, replacing
    /// earlier choices made inside it.
    pub fn set_subfolders(&mut self, folder: &Path, include: bool) {
        self.subfolders
            .retain(|r| !library::within(&r.path, folder));
        self.subfolders.push(SubfolderRule {
            path: folder.to_owned(),
            include,
        });
        let excess = self.subfolders.len().saturating_sub(RULES);
        self.subfolders.drain(..excess);
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DiscId {
    Cue(PathBuf),
    Album(PathBuf, String, Option<u32>),
    /// A disc of a registered collection.
    Library(AlbumId),
}
impl DiscId {
    pub fn of(t: &Track) -> Self {
        match &t.disc {
            Some(d) => Self::Cue(d.sheet.clone()),
            None => Self::Album(
                t.source.parent().unwrap_or(Path::new("")).into(),
                t.album.clone(),
                t.disc_number,
            ),
        }
    }
    pub fn contains(&self, t: &Track) -> bool {
        match (self, &t.disc) {
            (Self::Cue(p), Some(d)) => p == &d.sheet,
            (Self::Album(p, album, n), None) => {
                t.source.parent() == Some(p) && album == &t.album && *n == t.disc_number
            }
            _ => false,
        }
    }
    pub fn album(&self) -> Option<AlbumId> {
        match self {
            Self::Library(id) => Some(*id),
            _ => None,
        }
    }
    pub fn folder(&self) -> Option<&Path> {
        match self {
            Self::Cue(sheet) => sheet.parent(),
            Self::Album(folder, ..) => Some(folder),
            Self::Library(_) => None,
        }
    }
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Cue(a), Self::Cue(b)) => crate::places::same_path(a, b),
            (Self::Album(a, title, number), Self::Album(b, other_title, other_number)) => {
                title == other_title && number == other_number && crate::places::same_path(a, b)
            }
            _ => self == other,
        }
    }
}

/// A disc's place in a release of several discs.
#[derive(Clone, Debug, PartialEq)]
pub struct Membership {
    /// The release's first disc, which identifies it.
    pub key: DiscId,
    pub title: String,
    pub number: u32,
    /// What this disc adds to the release title, such as `Filthy Kicks`.
    pub subtitle: String,
}

/// A page within a collection: one CD, or every CD of a release.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Page {
    Disc(DiscId),
    /// Identified by its first disc.
    Release(DiscId),
}
pub struct Disc {
    pub tracks: Vec<Arc<Track>>,
    pub id: DiscId,
    pub root: Option<RootId>,
    pub title: String,
    pub folder: PathBuf,
    pub cover: Option<CoverFile>,
    cue_image: Option<PathBuf>,
    year: Option<u32>,
    number: Option<u32>,
    format: String,
    pub count: usize,
    duration: f64,
    complete: bool,
    pub release: Option<Arc<Membership>>,
}

/// The discs of the release whose first disc is `key`, in disc order.
pub fn release_discs(discs: &[Arc<Disc>], key: &DiscId) -> Vec<Arc<Disc>> {
    let mut found: Vec<_> = discs
        .iter()
        .filter(|d| d.release.as_ref().is_some_and(|m| m.key.same(key)))
        .cloned()
        .collect();
    found.sort_by_key(|d| d.release.as_ref().map(|m| m.number));
    found
}

/// Which programs supply rows for this physical folder projection.
pub fn folder_discs(programs: &[Arc<Disc>], page: Option<&Page>, grouped: bool) -> Vec<Arc<Disc>> {
    match page {
        Some(Page::Disc(id)) => programs.iter().filter(|d| d.id.same(id)).cloned().collect(),
        Some(Page::Release(key)) => release_discs(programs, key),
        None if grouped || is_single_disc(programs) => programs.to_vec(),
        None => vec![],
    }
}

pub fn is_single_disc(programs: &[Arc<Disc>]) -> bool {
    programs.len() == 1
        && programs[0]
            .tracks
            .first()
            .is_some_and(|t| t.has_disc_context())
}

/// A row in a browser directory.
#[derive(Clone)]
pub enum Shelf {
    Disc(Arc<Disc>),
    Release(Arc<Release>),
    Link(ShelfLink, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShelfLink {
    Collection(RootId),
    Playlist(PlaylistId),
}
impl ShelfLink {
    pub fn location(self) -> crate::browser::Location {
        use crate::browser::Location;
        match self {
            Self::Collection(id) => Location::Library(Some(id)),
            Self::Playlist(id) => Location::Playlist(id),
        }
    }
    fn icon(self) -> IconName {
        match self {
            Self::Collection(_) => IconName::Library,
            Self::Playlist(_) => IconName::ListMusic,
        }
    }
}
impl Shelf {
    pub fn key(&self) -> crate::table::RowKey {
        match self {
            Self::Disc(d) => crate::table::RowKey::Disc(d.id.clone()),
            Self::Release(r) => crate::table::RowKey::Release(r.key.clone()),
            Self::Link(link, _) => crate::table::RowKey::Link(*link),
        }
    }
}

/// The discs of one release as the Releases list shows them.
pub struct Release {
    pub key: DiscId,
    pub title: String,
    pub discs: Vec<Arc<Disc>>,
    folder: PathBuf,
    cover: Option<crate::artwork::CoverFile>,
    year: Option<u32>,
    format: String,
    count: usize,
    duration: f64,
    complete: bool,
}

pub struct Discs {
    pub rows: Vec<Shelf>,
    original: Arc<[Shelf]>,
    pub(crate) shelf_layout: Layout,
    directory_layout: Layout,
    columns: ColumnInteraction,
    pub(crate) sort: Option<(Field, ColumnSort)>,
    pub directory_label: Option<&'static str>,
    pub empty: SharedString,
    /// Where the commands of the rows' menus go.
    pub browser: Option<WeakEntity<crate::browser::Browser>>,
    pub tracks: Option<WeakEntity<TableState<crate::table::MusicTable>>>,
}
impl Discs {
    pub fn new(
        browser: WeakEntity<crate::browser::Browser>,
        tracks: WeakEntity<TableState<crate::table::MusicTable>>,
        shelf_layout: Layout,
    ) -> Self {
        Self {
            browser: Some(browser),
            tracks: Some(tracks),
            rows: Vec::new(),
            original: Arc::default(),
            shelf_layout,
            directory_layout: Layout::for_fields(&[Field::Title]),
            columns: ColumnInteraction::default(),
            sort: None,
            directory_label: None,
            empty: SharedString::default(),
        }
    }

    pub(crate) fn row_height(&self) -> f32 {
        if self.directory_label.is_some() {
            crate::table::ROW_HEIGHT
        } else {
            crate::table::artwork_row_height(SHELF_COVER)
        }
    }

    pub(crate) fn set_rows(&mut self, rows: Vec<Shelf>) {
        self.original = rows.into();
        self.project();
    }

    fn project(&mut self) {
        self.rows.clear();
        let Some((field, direction)) = self.sort() else {
            self.rows.extend(self.original.iter().cloned());
            return;
        };
        ordered_shelf(&self.original, field, direction, &mut self.rows);
    }

    fn column_field(&self, col: usize) -> Field {
        self.layout().fields().nth(col).unwrap_or(Field::Title)
    }

    fn update_order(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        let row_height = table.delegate().row_height();
        let key = |index| table.delegate().rows.get(index).map(Shelf::key);
        let selected = table.selected_row().and_then(key);
        let viewport = Viewport::capture(table, row_height, key);
        table.delegate_mut().project();
        let position = |key: &_| {
            table
                .delegate()
                .rows
                .iter()
                .position(|row| row.key() == *key)
        };
        let row = selected.as_ref().and_then(position);
        reselect(table, row, cx);
        viewport.restore(table, row_height, |key| {
            table
                .delegate()
                .rows
                .iter()
                .position(|row| row.key() == *key)
        });
    }

    fn row_playing(&self, row: &Shelf, cx: &App) -> bool {
        let Some(table) = self.tracks.as_ref().and_then(WeakEntity::upgrade) else {
            return false;
        };
        let playing = |disc: &Arc<Disc>| table.read(cx).delegate().disc_playing(disc, cx);
        match row {
            Shelf::Disc(disc) => playing(disc),
            Shelf::Release(release) => release.discs.iter().any(playing),
            Shelf::Link(..) => false,
        }
    }
    /// Top-level albums and releases. Each release owns its numbered discs.
    pub fn shelve(discs: Vec<Arc<Disc>>) -> Vec<Shelf> {
        let order = crate::release::ordered(
            discs
                .iter()
                .map(|d| d.release.as_ref().map(|m| (&m.key, m.number))),
        );
        let mut rows = Vec::with_capacity(discs.len());
        for item in order {
            let indices = match item {
                crate::release::Ordered::Disc(index) => {
                    rows.push(Shelf::Disc(discs[index].clone()));
                    continue;
                }
                crate::release::Ordered::Release(indices) => indices,
            };
            let found: Vec<_> = indices
                .into_iter()
                .map(|index| discs[index].clone())
                .collect();
            let first = &found[0];
            let m = first.release.as_ref().expect("release member");
            rows.push(Shelf::Release(Arc::new(Release {
                key: m.key.clone(),
                title: m.title.clone(),
                folder: first
                    .folder
                    .ancestors()
                    .find(|dir| found.iter().all(|d| d.folder.starts_with(dir)))
                    .unwrap_or(Path::new(""))
                    .to_owned(),
                cover: found.iter().find_map(|d| d.cover.clone()),
                year: first
                    .year
                    .filter(|y| found.iter().all(|disc| disc.year == Some(*y))),
                format: if found.iter().all(|d| d.format == first.format) {
                    first.format.clone()
                } else {
                    "Mixed".into()
                },
                count: found.iter().map(|d| d.count).sum(),
                duration: found.iter().map(|d| d.duration).sum(),
                complete: found.iter().all(|d| d.complete),
                discs: found.clone(),
            })));
            rows.extend(found.into_iter().map(Shelf::Disc));
        }
        rows
    }
    pub fn group<'a>(
        rows: impl IntoIterator<Item = &'a Arc<Track>>,
        covers: &HashMap<PathBuf, crate::artwork::CoverFile>,
        root: &Path,
    ) -> Vec<Arc<Disc>> {
        let mut positions = HashMap::new();
        let mut items: Vec<Disc> = Vec::new();
        let mut folders: Vec<PathBuf> = Vec::new();
        let mut last: Option<usize> = None;
        for t in rows {
            // A disc's tracks arrive together, so most rows belong to the
            // disc before them and need no key of their own.
            if let Some(i) = last.filter(|&i| items[i].id.contains(t)) {
                items[i].add(t);
                continue;
            }
            let id = DiscId::of(t);
            let i = *positions.entry(id.clone()).or_insert_with(|| {
                let folder = t
                    .disc
                    .as_ref()
                    .map_or(&t.source, |d| &d.sheet)
                    .parent()
                    .unwrap_or(Path::new(""));
                let title = if !t.album.is_empty() {
                    t.album.clone()
                } else if let Some(d) = &t.disc {
                    library::name(&d.sheet)
                } else {
                    library::folder_name(folder)
                };
                let i = items.len();
                folders.push(folder.to_owned());
                items.push(Disc {
                    tracks: Vec::new(),
                    id,
                    root: None,
                    title,
                    folder: folder.strip_prefix(root).unwrap_or(folder).into(),
                    cover: covers.get(folder).cloned(),
                    cue_image: None,
                    year: t.year,
                    number: t.disc_number,
                    format: t.format(),
                    count: 0,
                    duration: 0.,
                    complete: true,
                    release: None,
                });
                i
            });
            items[i].add(t);
            last = Some(i);
        }
        let candidates: Vec<_> = items
            .iter()
            .zip(&folders)
            .map(|(d, folder)| crate::release::Candidate {
                folder,
                title: &d.title,
                number: d.number,
            })
            .collect();
        let releases = crate::release::releases(&candidates);
        drop(candidates);
        for g in releases {
            let key = items[g.discs[0]].id.clone();
            for (n, &i) in g.discs.iter().enumerate() {
                items[i].release = Some(Arc::new(Membership {
                    key: key.clone(),
                    title: g.title.clone(),
                    number: n as u32 + 1,
                    subtitle: g.subtitles[n].clone(),
                }));
            }
        }
        // Discs keep the order their first tracks were found in, which is
        // folder order. Discs found later are added below the ones on screen
        // rather than among them, and arriving tags never reorder the list.
        items
            .into_iter()
            .map(|mut disc| {
                if matches!(disc.id, DiscId::Album(..)) {
                    disc.tracks.sort_by_key(|t| t.number.unwrap_or(u32::MAX));
                } else if let Some(first) = disc.tracks.first()
                    && disc.tracks.iter().all(|t| t.source == first.source)
                {
                    disc.cue_image = Some(first.source.clone());
                }
                Arc::new(disc)
            })
            .collect()
    }
}
/// Reconcile releases from indexed candidates belonging to this branch.
pub(crate) fn tree_releases(discs: &mut [Arc<Disc>]) {
    let candidates = discs
        .iter()
        .map(|d| crate::release::Candidate {
            folder: &d.folder,
            title: &d.title,
            number: d.number,
        })
        .collect::<Vec<_>>();
    let releases = crate::release::releases(&candidates);
    for release in releases {
        let key = discs[release.discs[0]].id.clone();
        for (i, (at, subtitle)) in release.discs.into_iter().zip(release.subtitles).enumerate() {
            Arc::get_mut(&mut discs[at])
                .expect("fresh projection")
                .release = Some(Arc::new(Membership {
                key: key.clone(),
                title: release.title.clone(),
                number: i as u32 + 1,
                subtitle,
            }));
        }
    }
}

impl Disc {
    fn add(&mut self, t: &Arc<Track>) {
        self.tracks.push(t.clone());
        self.count += 1;
        self.duration += t.duration.unwrap_or(0.);
        self.complete &= t.duration.is_some();
        if self.format != "Mixed"
            && !t
                .source
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(&self.format))
        {
            self.format = "Mixed".into();
        }
    }
    /// A disc of a collection view. Reads the cover's file stamp, so it is
    /// built off the interface thread.
    pub fn from_album(a: AlbumRow) -> Self {
        Self {
            tracks: Vec::new(),
            id: DiscId::Library(a.id),
            root: a.root,
            title: a.title,
            folder: a.folder,
            cover: a.cover.and_then(CoverFile::new),
            cue_image: a.cue_image,
            year: a.year,
            number: a.disc_number,
            format: a.format,
            count: a.count,
            duration: a.duration,
            complete: a.complete,
            release: a.release.map(|r| {
                Arc::new(Membership {
                    key: DiscId::Library(r.first),
                    title: r.title,
                    number: r.number,
                    subtitle: r.subtitle,
                })
            }),
        }
    }
    /// How a disc is named beneath its release: `CD 2`, and its subtitle.
    pub fn member_label(&self) -> Option<(String, &str)> {
        let m = self.release.as_ref()?;
        Some((format!("CD {}", m.number), m.subtitle.as_str()))
    }
    /// Artwork owned by the group: a shared CUE image or the group's sidecar.
    pub(crate) fn section_cover(
        &self,
        settled_cover: f32,
        listing: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(CoverFile, Arc<RenderImage>)> {
        let size = section_cover_edge(settled_cover);
        if let Some(source) = &self.cue_image {
            return TrackCover {
                source: source.clone(),
                authored_folder: match &self.id {
                    DiscId::Cue(sheet) => sheet.parent().map(Path::to_owned),
                    _ => None,
                },
                disc_cover: self.cover.as_ref().map(|file| file.path().to_owned()),
                listing,
            }
            .image(size, window, cx);
        }
        let file = self.cover.as_ref()?;
        CoverThumbnail::get(file, size, window, cx)
            .flatten()
            .map(|image| (file.clone(), image))
    }

    /// The album or disc heading spans the visible table. Its artwork sits in
    /// the Cover column, above the tracks it describes, and its title lines
    /// up with theirs. Without a Cover column both lead the row.
    ///
    /// `lead` is the left edge and width of the Cover column and the left
    /// edge of Title; `cover_edge` is the size tracks draw covers at.
    pub fn section(
        &self,
        cover_edge: f32,
        lead: Option<(f32, f32, f32)>,
        settled_cover: f32,
        listing: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Div {
        let m = *materials(cx);
        let cover = self
            .section_cover(settled_cover, listing, window, cx)
            .map(|(_, image)| image);
        let mut detail = Vec::new();
        if let Some(number) = self.release.as_ref().map(|m| m.number).or(self.number) {
            let normalized = self.title.replace(' ', "").to_lowercase();
            if !normalized.contains(&format!("cd{number}"))
                && !normalized.contains(&format!("disc{number}"))
            {
                detail.push(format!("Disc {number}"));
            }
        }
        if let Some(member) = &self.release
            && !member.subtitle.is_empty()
            && !self
                .title
                .to_lowercase()
                .contains(&member.subtitle.to_lowercase())
        {
            detail.push(member.subtitle.clone());
        }
        if let Some(year) = self.year {
            detail.push(year.to_string());
        }
        if self.count > 0 {
            detail.push(format!(
                "{} {}",
                self.count,
                if self.count == 1 { "track" } else { "tracks" }
            ));
        }
        if self.complete && self.duration > 0. {
            detail.push(crate::library::clock(self.duration));
        }
        detail.push(self.format.clone());
        let (edge, tile_x, title_x) = match lead {
            Some((cover_x, cover_w, title_x)) => {
                let edge = cover_edge.min(cover_w - 2. * crate::table::COVER_X);
                (
                    edge,
                    cover_x + ((cover_w - edge) / 2.).floor(),
                    title_x + 10.,
                )
            }
            None => {
                let edge = section_cover_edge(cover_edge);
                (edge, 10., 10. + edge + 10.)
            }
        };
        h_flex()
            .w_full()
            .h_full()
            .flex_shrink_0()
            .min_w_0()
            .relative()
            .items_center()
            .pl(px(title_x))
            .pr(px(10.))
            .gap(px(10.))
            .border_t_1()
            .border_color(m.line)
            .child(
                div()
                    .absolute()
                    .left(px(tile_x))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(crate::table::cover_tile(edge, cover, cx)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .truncate()
                    .text_ellipsis_middle()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .text_color(m.text)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_shrink_0()
                    .max_w(relative(0.45))
                    .truncate()
                    .text_size(px(crate::theme::size::TEXT_SMALL))
                    .text_color(m.text_faint)
                    .child(detail.join("  ·  ")),
            )
    }
    pub fn duration(&self) -> f64 {
        self.duration
    }
    pub(crate) fn label(&self, field: Field) -> String {
        let member = self.member_label();
        match field {
            Field::Title => match &member {
                Some((number, sub)) if !sub.is_empty() => format!("{number} · {sub}"),
                Some((number, _)) => number.clone(),
                None => self.title.clone(),
            },
            // A release names its year once, above its discs.
            Field::Year if member.is_none() => self.year.map(|v| v.to_string()).unwrap_or_default(),
            Field::Count => self.count.to_string(),
            Field::Length if self.complete => library::clock(self.duration),
            Field::Format => self.format.clone(),
            Field::Path if member.is_some() => self
                .folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Field::Path => self.folder.display().to_string(),
            _ => String::new(),
        }
    }
}
impl Release {
    pub(crate) fn label(&self, field: Field) -> String {
        match field {
            Field::Title => self.title.clone(),
            Field::Year => self.year.map(|v| v.to_string()).unwrap_or_default(),
            Field::Count => self.count.to_string(),
            Field::Length if self.complete => library::clock(self.duration),
            Field::Format => self.format.clone(),
            Field::Path => self.folder.display().to_string(),
            _ => String::new(),
        }
    }
}
impl Shelf {
    pub(crate) fn label(&self, field: Field) -> String {
        match self {
            Self::Disc(d) => d.label(field),
            Self::Release(r) => r.label(field),
            Self::Link(_, title) => title.clone(),
        }
    }
}
const SHELF_COVER: f32 = 32.;

fn ordered_shelf(source: &[Shelf], field: Field, direction: ColumnSort, output: &mut Vec<Shelf>) {
    let mut families = Vec::new();
    let mut start = 0;
    while let Some(parent) = source.get(start) {
        let end = match parent {
            Shelf::Release(release) => start + 1 + release.discs.len(),
            _ => start + 1,
        }
        .min(source.len());
        families.push(&source[start..end]);
        start = end;
    }
    families.sort_by_cached_key(|family| {
        let parent = &family[0];
        match field {
            Field::Year | Field::Count | Field::Length => {
                let (year, count, duration) = match parent {
                    Shelf::Disc(d) => (d.year, d.count, d.duration),
                    Shelf::Release(r) => (r.year, r.count, r.duration),
                    Shelf::Link(..) => (None, 0, 0.),
                };
                let number = match field {
                    Field::Year => u64::from(year.unwrap_or(0)),
                    Field::Count => count as u64,
                    _ => duration.max(0.).to_bits(),
                };
                (number, String::new())
            }
            _ => (0, parent.label(field).to_lowercase()),
        }
    });
    if direction == ColumnSort::Descending {
        families.reverse();
    }
    output.extend(families.into_iter().flatten().cloned());
}

impl TableColumns for Discs {
    fn layout(&self) -> &Layout {
        if self.directory_label.is_some() {
            &self.directory_layout
        } else {
            &self.shelf_layout
        }
    }
    fn layout_mut(&mut self) -> &mut Layout {
        if self.directory_label.is_some() {
            &mut self.directory_layout
        } else {
            &mut self.shelf_layout
        }
    }
    fn interaction(&self) -> &ColumnInteraction {
        &self.columns
    }
    fn interaction_mut(&mut self) -> &mut ColumnInteraction {
        &mut self.columns
    }
    fn browser(&self) -> Option<WeakEntity<crate::browser::Browser>> {
        self.browser.clone()
    }
    fn sort(&self) -> Option<(Field, ColumnSort)> {
        self.sort
            .filter(|(field, _)| self.shelf_layout.has(*field) && self.directory_label.is_none())
    }
    fn sort_rows(table: &mut TableState<Self>, field: Field, cx: &mut Context<TableState<Self>>) {
        let next = crate::table::next_sort(table.delegate().sort(), field);
        table.delegate_mut().sort = next;
        Self::update_order(table, cx);
    }
    fn clear_sort_if_hidden(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        let shelf = table.delegate();
        if shelf.sort.is_some() && shelf.sort().is_none() && shelf.directory_label.is_none() {
            table.delegate_mut().sort = None;
            Self::update_order(table, cx);
        }
    }
    fn default_layout(&self) -> Layout {
        if self.directory_label.is_some() {
            Layout::for_fields(&[Field::Title])
        } else {
            Layout::new(ContextKind::Shelf)
        }
    }
    fn available_fields(&self) -> &'static [Field] {
        if self.directory_label.is_some() {
            &[Field::Title]
        } else {
            SHELF_FIELDS
        }
    }
    fn sortable_field(&self, _: Field) -> bool {
        self.directory_label.is_none()
    }
    fn column_label(&self, field: Field) -> &'static str {
        match field {
            Field::Title => self.directory_label.unwrap_or("Album / Disc"),
            Field::Path => "Folder",
            _ => field.label(),
        }
    }
    fn column_heading(&self, field: Field) -> &'static str {
        self.column_label(field)
    }
}

impl TableDelegate for Discs {
    fn columns_count(&self, _: &App) -> usize {
        self.layout().fields().len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn column(&self, i: usize, _: &App) -> Column {
        self.table_column(i)
    }
    fn move_column(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.move_table_column(from, to, window, cx);
    }
    fn render_th(
        &mut self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.table_heading(ix, cx)
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.table_tail()
    }
    fn cell_label(&self, r: usize, c: usize, cx: &App) -> Option<SharedString> {
        let row = self.rows.get(r)?;
        let field = self.column_field(c);
        let label = row.label(field);
        Some(if field == Field::Title && self.row_playing(row, cx) {
            format!("{label} · Now playing").into()
        } else {
            label.into()
        })
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .items_center()
            .px(px(12.))
            .text_size(px(13.))
            .text_color(materials(cx).text_muted)
            .child(self.empty.clone())
    }
    fn visible_rows_changed(
        &mut self,
        _: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if let Some(browser) = self.browser.clone() {
            cx.defer_in(window, move |_, _, cx| {
                let _ = browser.update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
            });
        }
    }
    fn render_td(
        &mut self,
        r: usize,
        c: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let m = *materials(cx);
        let row = &self.rows[r];
        let field = self.column_field(c);
        let cell = h_flex()
            .size_full()
            .min_w_0()
            .gap(px(10.))
            .text_size(px(13.))
            .when(field.right_aligned(), |d| d.justify_end())
            .text_color(if field == Field::Title {
                m.text
            } else {
                m.text_muted
            });
        let (cover, member, discs) = match row {
            Shelf::Disc(d) => (d.cover.as_ref(), d.member_label(), None),
            Shelf::Release(rel) => (rel.cover.as_ref(), None, Some(rel.discs.len())),
            Shelf::Link(link, title) => {
                return cell
                    .child(
                        Icon::new(link.icon())
                            .size(px(16.))
                            .text_color(m.text_faint),
                    )
                    .child(div().truncate().child(title.clone()));
            }
        };
        if field != Field::Title {
            return cell.child(
                div()
                    .truncate()
                    .text_ellipsis_middle()
                    .child(row.label(field)),
            );
        }
        // A disc of a release sits beneath the release's cover, without one
        // of its own.
        if let Some((number, subtitle)) = member {
            return cell
                .pl(px(42.))
                .when(self.row_playing(row, cx), |d| {
                    d.child(crate::table::playing_icon(cx))
                })
                .child(div().flex_shrink_0().text_color(m.text_muted).child(number))
                .when(!subtitle.is_empty(), |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ellipsis_middle()
                            .text_color(m.text_faint)
                            .child(subtitle.to_owned()),
                    )
                });
        }
        let cover = cover.and_then(|file| {
            crate::artwork::CoverThumbnail::get(file, SHELF_COVER, window, cx).flatten()
        });
        cell.child(
            div()
                .size(px(SHELF_COVER))
                .flex_shrink_0()
                .rounded(px(2.))
                .overflow_hidden()
                .bg(m.control)
                .flex()
                .items_center()
                .justify_center()
                .map(|d| match cover {
                    Some(path) => d.child(img(path).size_full().object_fit(ObjectFit::Cover)),
                    None => d.child(
                        Icon::new(IconName::Disc3)
                            .size(px(16.))
                            .text_color(m.text_faint),
                    ),
                }),
        )
        .child({
            let label = row.label(Field::Title);
            div()
                .id(("disc-title", crate::table::stable_element_key(row.key())))
                .min_w_0()
                .truncate()
                .text_ellipsis_middle()
                .child(label.clone())
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(w, cx)
                })
        })
        .when(self.row_playing(row, cx), |d| {
            d.child(crate::table::playing_icon(cx))
        })
        .when_some(discs, |d, n| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.5))
                    .text_color(m.text_faint)
                    .child(format!("{n} CDs")),
            )
        })
    }
    fn context_menu(
        &mut self,
        ix: usize,
        menu: gpui_kit::component::menu::PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::component::menu::PopupMenu {
        let (Some(browser), Some(tracks), Some(row)) =
            (self.browser.clone(), self.tracks.clone(), self.rows.get(ix))
        else {
            return menu;
        };
        match row {
            Shelf::Disc(d) => crate::table::disc_menu(menu, d, browser, tracks, cx),
            Shelf::Release(r) => crate::table::release_items(
                menu,
                r.key.clone(),
                r.title.clone(),
                r.discs.first().and_then(|d| d.root),
                browser,
                tracks,
                cx,
            ),
            Shelf::Link(link, _) => {
                let location = link.location();
                menu.item(
                    gpui_kit::component::menu::PopupMenuItem::new("Open").on_click(
                        move |_, window, cx| {
                            let _ =
                                browser.update(cx, |b, cx| b.show(location.clone(), window, cx));
                        },
                    ),
                )
            }
        }
    }
}

fn section_cover_edge(edge: f32) -> f32 {
    (edge - 4.).clamp(20., 36.)
}

/// Whether a folder track matches every search term. The same terms filter
/// library views in SQL; `mark` is only consulted for `rating:` and `liked:`.
pub fn matches(t: &Track, terms: &[Term], mark: impl Fn() -> Mark) -> bool {
    terms.iter().all(|term| term.matches(t, &mark))
}

#[cfg(test)]
mod tests {
    use super::ViewPrefs;
    use std::path::Path;
    #[test]
    fn subfolder_choices_hold_inside_a_folder_only() {
        let mut prefs = ViewPrefs::default();
        let archive = Path::new(r"C:\Music\Archive");
        let disc = Path::new(r"C:\Music\Archive\1995\CD1");
        assert!(!prefs.includes_subfolders(archive));
        prefs.set_subfolders(archive, true);
        assert!(prefs.includes_subfolders(archive));
        assert!(prefs.includes_subfolders(Path::new(r"c:\music\archive\\")));
        assert!(prefs.includes_subfolders(disc));
        assert!(!prefs.includes_subfolders(Path::new(r"C:\Music")));
        assert!(!prefs.includes_subfolders(Path::new(r"C:\Music\Archive 2")));
        // An exception inside the collection, then the collection again.
        prefs.set_subfolders(disc, false);
        assert!(!prefs.includes_subfolders(disc));
        assert!(prefs.includes_subfolders(Path::new(r"C:\Music\Archive\1995")));
        prefs.set_subfolders(archive, true);
        assert!(prefs.includes_subfolders(disc));
        assert_eq!(prefs.subfolders.len(), 1);
        // Turning a folder back to itself leaves no rule behind.
        prefs.set_subfolders(archive, false);
        assert_eq!(prefs.subfolders.len(), 1);
        assert!(!prefs.includes_subfolders(disc));
    }
    #[test]
    fn earlier_global_subfolder_switch_is_not_carried_over() {
        let prefs: ViewPrefs = serde_json::from_str(r#"{"recursive":true,"albums":true}"#).unwrap();
        assert_eq!(prefs.mode, super::ViewMode::Albums);
        assert!(!prefs.includes_subfolders(Path::new(r"C:\Users\Me\Music")));
        let again: ViewPrefs =
            serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(again.mode, super::ViewMode::Albums);
        assert!(again.subfolders.is_empty());
    }
}

#[cfg(test)]
mod release_tests {
    use super::{Discs, Shelf};
    use crate::library::Track;
    use std::{path::Path, sync::Arc};

    fn track(path: &str, album: &str) -> Arc<Track> {
        let mut t = Track::file(Path::new(path).into());
        t.album = album.into();
        Arc::new(t)
    }

    #[test]
    fn overview_keeps_release_order_and_owns_both_discs() {
        // Discovery or a year typo can put the second disc first.
        let rows = [
            track(r"M:\Live\CD2\a.wav", "Live (CD2)"),
            track(r"M:\Single\a.wav", "Single"),
            track(r"M:\Live\CD1\a.wav", "Live (CD1)"),
        ];
        let discs = Discs::group(&rows, &Default::default(), Path::new(r"M:\"));
        assert_eq!(discs.len(), 3);
        let shelf = Discs::shelve(discs);
        let labels: Vec<String> = shelf
            .iter()
            .map(|r| match r {
                Shelf::Release(r) => format!("release {} {}", r.title, r.discs.len()),
                Shelf::Link(_, title) => title.clone(),
                Shelf::Disc(d) => match d.member_label() {
                    Some((n, _)) => n,
                    None => d.title.clone(),
                },
            })
            .collect();
        assert_eq!(labels, ["Single", "release Live 2", "CD 1", "CD 2"]);
        let mut sorted = Vec::new();
        super::ordered_shelf(
            &shelf,
            crate::table::Field::Title,
            gpui_kit::component::table::ColumnSort::Ascending,
            &mut sorted,
        );
        assert_eq!(
            sorted.iter().map(Shelf::key).collect::<Vec<_>>(),
            [1, 2, 3, 0].map(|i| shelf[i].key())
        );
        let discs = shelf.iter().filter(|r| matches!(r, Shelf::Disc(_))).count();
        assert_eq!(discs, 3);
    }
}

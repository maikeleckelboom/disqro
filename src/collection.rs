//! A disc has an authored CUE program, or a tagged album in one folder.
//! Two identically titled rips in different folders remain distinct.
use crate::{
    library::{self, Track},
    store::{AlbumId, AlbumRow, Mark, Term},
    theme::materials,
};
use gpui_kit::{
    component::{
        h_flex,
        table::{Column, TableDelegate, TableState},
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Copy, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPrefs {
    pub recursive: bool,
    pub albums: bool,
}
impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            recursive: true,
            albums: false,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
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
}

pub struct Disc {
    pub id: DiscId,
    pub title: String,
    folder: PathBuf,
    pub cover: Option<crate::artwork::CoverFile>,
    year: Option<u32>,
    format: String,
    pub count: usize,
    duration: f64,
    complete: bool,
}
#[derive(Default)]
pub struct Discs {
    pub items: Vec<Disc>,
}
impl Discs {
    pub fn group(
        rows: &[Arc<Track>],
        covers: &HashMap<PathBuf, crate::artwork::CoverFile>,
        root: &Path,
    ) -> Vec<Disc> {
        let mut positions = HashMap::new();
        let mut items = Vec::new();
        for t in rows {
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
                items.push(Disc {
                    id,
                    title,
                    folder: folder.strip_prefix(root).unwrap_or(folder).into(),
                    cover: covers.get(folder).cloned(),
                    year: t.year,
                    format: t.format(),
                    count: 0,
                    duration: 0.,
                    complete: true,
                });
                i
            });
            let disc = &mut items[i];
            disc.count += 1;
            disc.duration += t.duration.unwrap_or(0.);
            disc.complete &= t.duration.is_some();
            if disc.format != "Mixed"
                && !t
                    .source
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(&disc.format))
            {
                disc.format = "Mixed".into();
            }
        }
        items.sort_by(|a, b| {
            a.year
                .unwrap_or(u32::MAX)
                .cmp(&b.year.unwrap_or(u32::MAX))
                .then_with(|| a.folder.cmp(&b.folder))
                .then_with(|| a.title.cmp(&b.title))
        });
        items
    }
}
impl Disc {
    /// A disc of a collection view. Reads the cover's file stamp, so it is
    /// built off the interface thread.
    pub fn from_album(a: AlbumRow) -> Self {
        Self {
            id: DiscId::Library(a.id),
            title: a.title,
            folder: a.folder,
            cover: a.cover.and_then(crate::artwork::CoverFile::new),
            year: a.year,
            format: a.format,
            count: a.count,
            duration: a.duration,
            complete: a.complete,
        }
    }
    pub fn duration(&self) -> f64 {
        self.duration
    }
    fn label(&self, col: usize) -> String {
        match col {
            0 => self.title.clone(),
            1 => self.year.map(|v| v.to_string()).unwrap_or_default(),
            2 => self.count.to_string(),
            3 if self.complete => library::clock(self.duration),
            4 => self.format.clone(),
            5 => self.folder.display().to_string(),
            _ => String::new(),
        }
    }
}
impl TableDelegate for Discs {
    fn columns_count(&self, _: &App) -> usize {
        6
    }
    fn rows_count(&self, _: &App) -> usize {
        self.items.len()
    }
    fn column(&self, i: usize, _: &App) -> Column {
        let (label, w) = [
            ("Album / Disc", 340.),
            ("Year", 56.),
            ("Tracks", 60.),
            ("Time", 76.),
            ("Format", 68.),
            ("Folder", 300.),
        ][i];
        Column::new(label, label).width(w)
    }
    fn cell_label(&self, r: usize, c: usize, _: &App) -> Option<SharedString> {
        Some(self.items.get(r)?.label(c).into())
    }
    fn render_td(
        &mut self,
        r: usize,
        c: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let d = &self.items[r];
        let m = *materials(cx);
        let cover = d.cover.as_ref().filter(|_| c == 0).and_then(|file| {
            window
                .use_asset::<crate::artwork::CoverThumbnail>(file, cx)
                .flatten()
        });
        h_flex()
            .size_full()
            .min_w_0()
            .gap(px(10.))
            .text_size(px(13.))
            .text_color(if c == 0 { m.text } else { m.text_muted })
            .when(c == 0, |row| {
                row.child(
                    div()
                        .size(px(32.))
                        .flex_shrink_0()
                        .rounded(px(2.))
                        .overflow_hidden()
                        .when_some(cover, |d, path| {
                            d.child(img(path).size_full().object_fit(ObjectFit::Cover))
                        }),
                )
            })
            .child(div().truncate().child(d.label(c)))
    }
}

/// Whether a folder track matches every search term. The same terms filter
/// library views in SQL; `mark` is only consulted for `rating:` and `liked:`.
pub fn matches(t: &Track, terms: &[Term], mark: impl Fn() -> Mark) -> bool {
    terms.iter().all(|term| term.matches(t, &mark))
}

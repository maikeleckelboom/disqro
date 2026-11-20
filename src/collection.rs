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

/// How collections are presented, and which folders are browsed as
/// collections.
///
/// A folder normally shows the recordings it holds. Including subfolders
/// turns it into a collection view of everything beneath it. The choice is
/// remembered for that folder and holds inside it, but never for its parent
/// or its neighbours. The global `recursive` switch of earlier versions is
/// ignored when read, since it was on for every folder by default.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPrefs {
    /// Collections show Albums / CDs rather than All Tracks.
    pub albums: bool,
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
    /// Whether `folder` is browsed with its subfolders: the choice made for
    /// it or its nearest ancestor, or not.
    pub fn includes_subfolders(&self, folder: &Path) -> bool {
        self.subfolders
            .iter()
            .filter(|r| library::within(folder, &r.path))
            .max_by_key(|r| r.path.as_os_str().len())
            .is_some_and(|r| r.include)
    }
    /// Records a choice for `folder` and everything beneath it, replacing
    /// earlier choices made inside it.
    pub fn set_subfolders(&mut self, folder: &Path, include: bool) {
        self.subfolders
            .retain(|r| !library::within(&r.path, folder));
        if self.includes_subfolders(folder) != include {
            self.subfolders.push(SubfolderRule {
                path: folder.to_owned(),
                include,
            });
        }
        let excess = self.subfolders.len().saturating_sub(RULES);
        self.subfolders.drain(..excess);
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    pub tracks: Vec<Arc<Track>>,
    pub id: DiscId,
    pub title: String,
    pub folder: PathBuf,
    pub cover: Option<crate::artwork::CoverFile>,
    year: Option<u32>,
    number: Option<u32>,
    format: String,
    pub count: usize,
    duration: f64,
    complete: bool,
}
#[derive(Default)]
pub struct Discs {
    pub items: Vec<Arc<Disc>>,
}
impl Discs {
    pub fn group<'a>(
        rows: impl IntoIterator<Item = &'a Arc<Track>>,
        covers: &HashMap<PathBuf, crate::artwork::CoverFile>,
        root: &Path,
    ) -> Vec<Arc<Disc>> {
        let mut positions = HashMap::new();
        let mut items: Vec<Disc> = Vec::new();
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
                items.push(Disc {
                    tracks: Vec::new(),
                    id,
                    title,
                    folder: folder.strip_prefix(root).unwrap_or(folder).into(),
                    cover: covers.get(folder).cloned(),
                    year: t.year,
                    number: t.disc_number,
                    format: t.format(),
                    count: 0,
                    duration: 0.,
                    complete: true,
                });
                i
            });
            items[i].add(t);
            last = Some(i);
        }
        // Discs keep the order their first tracks were found in, which is
        // folder order. Discs found later are added below the ones on screen
        // rather than among them, and arriving tags never reorder the list.
        items
            .into_iter()
            .map(|mut disc| {
                if matches!(disc.id, DiscId::Album(..)) {
                    disc.tracks.sort_by_key(|t| t.number.unwrap_or(u32::MAX));
                }
                Arc::new(disc)
            })
            .collect()
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
            title: a.title,
            folder: a.folder,
            cover: a.cover.and_then(crate::artwork::CoverFile::new),
            year: a.year,
            number: a.disc_number,
            format: a.format,
            count: a.count,
            duration: a.duration,
            complete: a.complete,
        }
    }
    pub fn section(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *materials(cx);
        let cover = self.cover.as_ref().and_then(|file| {
            window
                .use_asset::<crate::artwork::CoverThumbnail>(file, cx)
                .flatten()
        });
        let mut detail = Vec::new();
        if let Some(year) = self.year {
            detail.push(year.to_string());
        }
        detail.push(self.format.clone());
        if let Some(number) = self.number {
            let normalized = self.title.replace(' ', "").to_lowercase();
            if !normalized.contains(&format!("cd{number}"))
                && !normalized.contains(&format!("disc{number}"))
            {
                detail.insert(0, format!("CD {number}"));
            }
        }
        h_flex()
            .w_full()
            .h_full()
            .flex_shrink_0()
            .min_w_0()
            .gap(px(8.))
            .px(px(9.))
            .border_t_1()
            .border_color(m.control_line)
            .child(
                div()
                    .size(px(22.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .rounded(px(2.))
                    .when_some(cover, |d, cover| {
                        d.child(img(cover).size_full().object_fit(ObjectFit::Cover))
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(12.5))
                    .text_color(m.text)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(m.text_faint)
                    .child(detail.join("  ·  ")),
            )
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
        assert!(prefs.subfolders.is_empty());
    }
    #[test]
    fn earlier_global_subfolder_switch_is_not_carried_over() {
        let prefs: ViewPrefs = serde_json::from_str(r#"{"recursive":true,"albums":true}"#).unwrap();
        assert!(prefs.albums);
        assert!(!prefs.includes_subfolders(Path::new(r"C:\Users\Me\Music")));
        let again: ViewPrefs =
            serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert!(again.albums && again.subfolders.is_empty());
    }
}

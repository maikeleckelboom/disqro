//! A disc has an authored CUE program, or a tagged album in one folder.
//! Two identically titled rips in different folders remain distinct.
mod albums;
use crate::{
    library::{self, Track},
    store::{AlbumId, AlbumRow, Mark, Term},
    theme::materials,
};
pub use albums::AlbumView;
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, h_flex,
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
    pub title: String,
    pub folder: PathBuf,
    pub cover: Option<crate::artwork::CoverFile>,
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
        .filter(|d| d.release.as_ref().is_some_and(|m| &m.key == key))
        .cloned()
        .collect();
    found.sort_by_key(|d| d.release.as_ref().map(|m| m.number));
    found
}

/// One standalone album or one multi-disc release in the Albums / CDs overview.
#[derive(Clone)]
pub enum Shelf {
    Disc(Arc<Disc>),
    Release(Arc<Release>),
}
impl Shelf {
    pub fn key(&self) -> crate::table::RowKey {
        match self {
            Self::Disc(d) => crate::table::RowKey::Disc(d.id.clone()),
            Self::Release(r) => crate::table::RowKey::Release(r.key.clone()),
        }
    }
}

/// The discs of one release as the Albums / CDs list shows them.
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

#[derive(Default)]
pub struct Discs {
    pub rows: Vec<Shelf>,
    parents: Vec<Shelf>,
    pub view: AlbumView,
    viewport: f32,
    pub loading: bool,
    /// Where the commands of the rows' menus go.
    pub browser: Option<WeakEntity<crate::browser::Browser>>,
    pub tracks: Option<WeakEntity<TableState<crate::table::MusicTable>>>,
}
impl Discs {
    pub fn new(
        browser: WeakEntity<crate::browser::Browser>,
        tracks: WeakEntity<TableState<crate::table::MusicTable>>,
    ) -> Self {
        Self {
            browser: Some(browser),
            tracks: Some(tracks),
            ..Default::default()
        }
    }
    /// Top-level albums and releases. Each release owns its numbered discs.
    pub fn shelve(discs: Vec<Arc<Disc>>) -> Vec<Shelf> {
        let mut members: HashMap<DiscId, Vec<Arc<Disc>>> = HashMap::new();
        for d in &discs {
            if let Some(m) = &d.release {
                members.entry(m.key.clone()).or_default().push(d.clone());
            }
        }
        for found in members.values_mut() {
            found.sort_by_key(|d| d.release.as_ref().map(|m| m.number));
        }
        let mut rows = Vec::with_capacity(discs.len());
        for d in discs {
            let Some(m) = &d.release else {
                rows.push(Shelf::Disc(d));
                continue;
            };
            if members
                .get(&m.key)
                .is_none_or(|found| !Arc::ptr_eq(&found[0], &d))
            {
                continue;
            }
            let Some(found) = members.remove(&m.key) else {
                continue;
            };
            let first = &found[0];
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
                discs: found,
            })));
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
                    title,
                    folder: folder.strip_prefix(root).unwrap_or(folder).into(),
                    cover: covers.get(folder).cloned(),
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
    /// The header of a CD within a track table. Its cover follows the row
    /// height, which grows with the table's Cover column.
    pub fn section(&self, row_height: f32, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *materials(cx);
        let size = row_height - 6.;
        let cover = self
            .cover
            .as_ref()
            .and_then(|file| crate::artwork::CoverThumbnail::get(file, size, window, cx).flatten());
        let mut detail = Vec::new();
        if let Some(year) = self.year {
            detail.push(year.to_string());
        }
        detail.push(self.format.clone());
        if let Some(number) = self.release.as_ref().map(|m| m.number).or(self.number) {
            let normalized = self.title.replace(' ', "").to_lowercase();
            if !normalized.contains(&format!("cd{number}"))
                && !normalized.contains(&format!("disc{number}"))
            {
                detail.insert(0, format!("CD {number}"));
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
                    .size(px(size))
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
                    .text_ellipsis_middle()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(12.5))
                    .text_color(m.text)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_ellipsis_middle()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(m.text_faint)
                    .child(detail.join("  ·  ")),
            )
    }
    pub fn duration(&self) -> f64 {
        self.duration
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
        let mut table = Discs {
            parents: Discs::shelve(discs),
            ..Default::default()
        };
        table.project();
        assert_eq!(table.rows.len(), 2);
        let Shelf::Release(release) = &table.rows[1] else {
            panic!("missing release")
        };
        assert!(matches!(&table.rows[0], Shelf::Disc(d) if d.title == "Single"));
        assert_eq!(release.title, "Live");
        let titles: Vec<_> = release.discs.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, ["Live (CD1)", "Live (CD2)"]);
    }
}

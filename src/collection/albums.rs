//! The Albums / CDs overview shows one row per release or standalone album.
use super::*;
use crate::table::{reselect, sort_heading};
use gpui_kit::component::{
    menu::{ContextMenuExt, PopupMenuItem},
    table::ColumnSort,
    tooltip::Tooltip,
    v_flex,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq, Serialize, Deserialize)]
enum Field {
    Title,
    Year,
    Tracks,
    Time,
    Format,
    Folder,
}
impl Field {
    const ALL: [Self; 6] = [
        Self::Title,
        Self::Year,
        Self::Tracks,
        Self::Time,
        Self::Format,
        Self::Folder,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Title => "Album / Disc",
            Self::Year => "Year",
            Self::Tracks => "Tracks",
            Self::Time => "Time",
            Self::Format => "Format",
            Self::Folder => "Folder",
        }
    }
    fn width(self) -> f32 {
        match self {
            Self::Title => 280.,
            Self::Year => 60.,
            Self::Tracks => 62.,
            Self::Time => 72.,
            Self::Format => 70.,
            Self::Folder => 240.,
        }
    }
    fn numeric(self) -> bool {
        matches!(self, Self::Year | Self::Tracks | Self::Time)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumView {
    columns: Vec<Field>,
    widths: BTreeMap<Field, f32>,
    sort: Option<(Field, bool)>, // true for descending
}
impl Default for AlbumView {
    fn default() -> Self {
        Self {
            columns: Field::ALL[..5].to_vec(),
            widths: Default::default(),
            sort: None,
        }
    }
}
impl AlbumView {
    pub fn validate(mut self) -> Self {
        let mut seen = BTreeSet::new();
        self.columns.retain(|field| seen.insert(*field));
        if !seen.contains(&Field::Title) {
            self.columns.insert(0, Field::Title);
        }
        self.widths
            .retain(|_, w| w.is_finite() && (40. ..=1200.).contains(w));
        self
    }
    fn width(&self, field: Field, viewport: f32) -> f32 {
        if let Some(width) = self.widths.get(&field) {
            return *width;
        }
        if field == Field::Title && viewport > 0. {
            let remaining: f32 = self
                .columns
                .iter()
                .filter(|f| **f != field)
                .map(|f| self.widths.get(f).copied().unwrap_or_else(|| f.width()))
                .sum();
            return (viewport - remaining - 2.).clamp(178., 1200.);
        }
        field.width()
    }
}
impl Disc {
    fn label(&self, col: Field) -> String {
        match col {
            Field::Title => self.title.clone(),
            Field::Year => self.year.map(|v| v.to_string()).unwrap_or_default(),
            Field::Tracks => self.count.to_string(),
            Field::Time if self.complete => library::clock(self.duration),
            Field::Format => self.format.clone(),
            Field::Folder => self.folder.display().to_string(),
            _ => String::new(),
        }
    }
}
impl Release {
    fn label(&self, col: Field) -> String {
        match col {
            Field::Title => self.title.clone(),
            Field::Year => self.year.map(|v| v.to_string()).unwrap_or_default(),
            Field::Tracks => self.count.to_string(),
            Field::Time if self.complete => library::clock(self.duration),
            Field::Format => self.format.clone(),
            Field::Folder => self.folder.display().to_string(),
            _ => String::new(),
        }
    }
}
impl Shelf {
    fn label(&self, col: Field) -> String {
        match self {
            Self::Disc(d) => d.label(col),
            Self::Release(r) => r.label(col),
        }
    }

    fn sort_value(&self, field: Field) -> Option<SortValue> {
        let (title, year, count, time, complete, format, folder) = match self {
            Self::Disc(d) => (
                &d.title, d.year, d.count, d.duration, d.complete, &d.format, &d.folder,
            ),
            Self::Release(r) => (
                &r.title, r.year, r.count, r.duration, r.complete, &r.format, &r.folder,
            ),
        };
        match field {
            Field::Title => Some(SortValue::Text(title.to_lowercase())),
            Field::Year => year.map(|v| SortValue::Number(v as u64)),
            Field::Tracks => Some(SortValue::Number(count as u64)),
            Field::Time => complete.then(|| SortValue::Number((time * 1000.).round() as u64)),
            Field::Format => (!format.is_empty()).then(|| SortValue::Text(format.to_lowercase())),
            Field::Folder => (!folder.as_os_str().is_empty())
                .then(|| SortValue::Text(folder.to_string_lossy().to_lowercase())),
        }
    }
}
#[derive(Eq, PartialEq, Ord, PartialOrd)]
enum SortValue {
    Number(u64),
    Text(String),
}

impl Discs {
    pub(super) fn project(&mut self) {
        self.rows.clone_from(&self.parents);
        if let Some((field, descending)) = self.view.sort {
            if descending {
                self.rows.sort_by_cached_key(|r| {
                    let v = r.sort_value(field);
                    (v.is_none(), std::cmp::Reverse(v))
                });
            } else {
                self.rows.sort_by_cached_key(|r| {
                    let v = r.sort_value(field);
                    (v.is_none(), v)
                });
            }
        }
    }

    pub fn position(&self, key: &crate::table::RowKey) -> Option<usize> {
        use crate::table::RowKey;
        self.rows.iter().position(|r| match (r, key) {
            (Shelf::Disc(d), RowKey::Disc(id)) => d.id == *id,
            (Shelf::Release(r), RowKey::Release(id)) => r.key == *id,
            // Earlier workspaces could bookmark a CD inside an expanded release.
            (Shelf::Release(r), RowKey::Disc(id)) => r.discs.iter().any(|d| d.id == *id),
            _ => false,
        })
    }

    fn edit_rows(
        t: &mut TableState<Self>,
        cx: &mut Context<TableState<Self>>,
        edit: impl FnOnce(&mut Self),
    ) {
        let selected = t
            .selected_row()
            .and_then(|i| t.delegate().rows.get(i))
            .map(Shelf::key);
        let d = t.delegate_mut();
        edit(d);
        d.project();
        let index = selected.and_then(|key| d.position(&key));
        reselect(t, index, cx);
        cx.notify();
    }
    pub fn set_items(
        t: &mut TableState<Self>,
        items: Vec<Arc<Disc>>,
        cx: &mut Context<TableState<Self>>,
    ) {
        Self::edit_rows(t, cx, |d| {
            d.parents = Self::shelve(items);
            d.loading = false;
        });
    }
    /// Sizes the columns to `width`. Returns whether it changed, since the
    /// headings that were already drawn this frame need another frame.
    pub fn fit(t: &mut TableState<Self>, width: f32, cx: &mut Context<TableState<Self>>) -> bool {
        if t.delegate().viewport == width {
            return false;
        }
        t.delegate_mut().viewport = width;
        t.refresh(cx);
        cx.notify();
        true
    }
    pub fn resized(&mut self, widths: &[Pixels]) {
        if self.view.columns.len() == widths.len() {
            for (&field, &width) in self.view.columns.iter().zip(widths) {
                self.view.widths.insert(field, f32::from(width));
            }
        }
    }
    fn changed(&self, cx: &mut App) {
        if let Some(browser) = &self.browser {
            let _ = browser.update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
        }
    }
}
impl TableDelegate for Discs {
    fn columns_count(&self, _: &App) -> usize {
        self.view.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn column(&self, i: usize, _: &App) -> Column {
        let field = self.view.columns[i];
        let col = Column::new(field.label(), field.label())
            .width(self.view.width(field, self.viewport))
            .min_width(px(if field == Field::Title { 150. } else { 48. }))
            .paddings(Edges {
                top: px(0.),
                bottom: px(0.),
                left: px(10.),
                right: px(10.),
            });
        if field.numeric() {
            col.text_right()
        } else {
            col
        }
    }
    fn move_column(
        &mut self,
        ix: usize,
        to: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let field = self.view.columns.remove(ix);
        self.view.columns.insert(to, field);
    }
    fn render_th(
        &mut self,
        i: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let field = self.view.columns[i];
        let direction = self
            .view
            .sort
            .filter(|(f, _)| *f == field)
            .map(|(_, desc)| {
                if desc {
                    ColumnSort::Descending
                } else {
                    ColumnSort::Ascending
                }
            });
        let table = cx.entity().downgrade();
        let visible = self.view.columns.clone();
        sort_heading(i, field.label(), field.numeric(), direction, cx)
            .cursor_pointer()
            .on_click(cx.listener(move |t, _, _, cx| {
                Self::edit_rows(t, cx, |d| {
                    d.view.sort = match d.view.sort {
                        Some((f, false)) if f == field => Some((field, true)),
                        Some((f, true)) if f == field => None,
                        _ => Some((field, false)),
                    }
                });
                t.delegate().changed(cx);
            }))
            .context_menu(move |mut menu, _, _| {
                menu = menu.label("Columns");
                for f in Field::ALL {
                    let table = table.clone();
                    menu = menu.item(
                        PopupMenuItem::new(f.label())
                            .checked(visible.contains(&f))
                            .disabled(f == Field::Title)
                            .on_click(move |_, _, cx| {
                                let _ = table.update(cx, |t, cx| {
                                    Self::edit_rows(t, cx, |d| {
                                        if let Some(at) =
                                            d.view.columns.iter().position(|field| *field == f)
                                        {
                                            d.view.columns.remove(at);
                                            if d.view.sort.is_some_and(|(field, _)| field == f) {
                                                d.view.sort = None;
                                            }
                                        } else {
                                            d.view.columns.push(f);
                                        }
                                    });
                                    t.refresh(cx);
                                    t.delegate().changed(cx);
                                });
                            }),
                    );
                }
                let table = table.clone();
                menu.separator()
                    .item(
                        PopupMenuItem::new("Reset Columns").on_click(move |_, _, cx| {
                            let _ = table.update(cx, |t, cx| {
                                Self::edit_rows(t, cx, |d| {
                                    d.view.columns = AlbumView::default().columns;
                                    d.view.widths.clear();
                                    d.view.sort = None;
                                });
                                t.refresh(cx);
                                t.delegate().changed(cx);
                            });
                        }),
                    )
            })
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .text_color(materials(cx).text_muted)
            .text_size(px(13.))
            .child(if self.loading {
                "Loading albums…"
            } else {
                "No albums or CDs"
            })
    }
    fn cell_label(&self, r: usize, c: usize, _: &App) -> Option<SharedString> {
        Some(self.rows.get(r)?.label(self.view.columns[c]).into())
    }
    fn render_td(
        &mut self,
        r: usize,
        c: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let m = *materials(cx);
        let field = self.view.columns[c];
        let row = &self.rows[r];
        let cell = h_flex()
            .size_full()
            .min_w_0()
            .gap(px(6.))
            .text_size(px(13.))
            .text_color(if field == Field::Title {
                m.text
            } else {
                m.text_muted
            });
        let (cover, discs) = match row {
            Shelf::Disc(d) => (d.cover.as_ref(), None),
            Shelf::Release(rel) => (rel.cover.as_ref(), Some(rel.discs.len())),
        };
        if field != Field::Title {
            let label = row.label(field);
            return cell.when(field.numeric(), |d| d.justify_end()).child(
                div()
                    .id((
                        "album-cell",
                        crate::table::stable_element_key((row.key(), field as usize)),
                    ))
                    .truncate()
                    .child(label.clone())
                    .tooltip(move |w, cx| Tooltip::new(label.clone()).build(w, cx)),
            );
        }
        let cover = cover
            .and_then(|file| crate::artwork::CoverThumbnail::get(file, 32., window, cx).flatten());
        cell.child(
            div()
                .size(px(32.))
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
                .child(label.clone())
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(w, cx)
                })
        })
        .when_some(discs, |d, n| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(crate::theme::size::TEXT_SMALL))
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
                browser,
                tracks,
                cx,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AlbumView, Discs, Field, Shelf, Track};
    use std::{path::Path, sync::Arc};

    #[test]
    fn sorting_and_legacy_preferences_keep_one_row_per_release() {
        let tracks = [
            ("Single", "Solo"),
            ("Live/CD2", "Live (CD2)"),
            ("Live/CD1", "Live (CD1)"),
        ]
        .map(|(folder, name)| {
            let mut t = Track::file(Path::new("M:/").join(folder).join("track.wav"));
            t.album = name.into();
            Arc::new(t)
        });
        let mut table = Discs {
            parents: Discs::shelve(Discs::group(&tracks, &Default::default(), Path::new("M:/"))),
            ..Default::default()
        };
        let release = table
            .parents
            .iter()
            .find_map(|r| match r {
                Shelf::Release(r) => Some(r),
                _ => None,
            })
            .unwrap();
        let old_child = crate::table::RowKey::Disc(release.discs[1].id.clone());
        let release_id = release.key.clone();
        table.view.sort = Some((Field::Tracks, true));
        table.view.columns = vec![Field::Tracks, Field::Title];
        table.view.widths.insert(Field::Title, 301.);
        let mut saved = serde_json::to_value(&table.view).unwrap();
        saved["expanded"] = serde_json::json!([release_id]);
        table.view = serde_json::from_value::<AlbumView>(saved)
            .unwrap()
            .validate();
        table.project();
        let names = |t: &Discs| {
            t.rows
                .iter()
                .map(|r| r.label(Field::Title))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&table), ["Live", "Solo"]);
        assert_eq!(table.view.columns, [Field::Tracks, Field::Title]);
        assert_eq!(table.view.width(Field::Title, 700.), 301.);
        table.view.sort = Some((Field::Tracks, false));
        table.project();
        assert_eq!(table.position(&old_child), Some(1));
        assert_eq!(names(&table), ["Solo", "Live"]);
        assert!(
            table
                .rows
                .iter()
                .all(|row| row.label(Field::Year).is_empty())
        );
    }
}

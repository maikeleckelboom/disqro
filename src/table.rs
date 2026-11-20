use crate::{
    audio::Command,
    library::{Track, clock},
    player::Player,
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, h_flex,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        table::{Column, ColumnSort, TableDelegate, TableState},
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const ROW_HEIGHT: f32 = 28.;
const CELL_X: f32 = 10.;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Field {
    Number,
    Title,
    Artist,
    Album,
    Length,
    Format,
    Disc,
    Year,
    Path,
}
impl Field {
    /// Shown until the listener chooses otherwise.
    const DEFAULT: [Self; 5] = [
        Self::Number,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Length,
    ];
    const ALL: [Self; 9] = [
        Self::Number,
        Self::Title,
        Self::Artist,
        Self::Album,
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
            Self::Title => 280.,
            Self::Artist => 160.,
            Self::Album => 200.,
            Self::Length => 60.,
            Self::Format | Self::Year | Self::Disc => 64.,
            Self::Path => 330.,
        }
    }
    fn min_width(self) -> f32 {
        match self {
            Self::Number => 36.,
            Self::Length | Self::Format => 48.,
            _ => 72.,
        }
    }
    fn right_aligned(self) -> bool {
        matches!(self, Self::Number | Self::Length)
    }
    fn value(self, t: &Track) -> String {
        match self {
            Self::Number => t
                .display_number()
                .map(|n| n.to_string())
                .unwrap_or_default(),
            Self::Title => t.title.clone(),
            Self::Artist => t.artist.clone(),
            Self::Album => t.album.clone(),
            Self::Length => t.duration.map(clock).unwrap_or_default(),
            Self::Format => t.format(),
            Self::Disc => t.disc_number.map(|n| n.to_string()).unwrap_or_default(),
            Self::Year => t.year.map(|n| n.to_string()).unwrap_or_default(),
            Self::Path => t.source.display().to_string(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutColumn {
    field: Field,
    width: f32,
}
/// A physical file, or an authored segment of one. Tags and row positions
/// are deliberately absent so a refresh or sort keeps the same selection.
type TrackKey = (std::path::PathBuf, Option<crate::library::DiscTrack>);
fn key(t: &Track) -> TrackKey {
    (t.source.clone(), t.disc.clone())
}
#[derive(Default)]
struct Selection {
    marked: std::collections::HashSet<TrackKey>,
    anchor: Option<TrackKey>,
}
impl Selection {
    fn modify(
        &mut self,
        rows: &[Arc<Track>],
        primary: Option<usize>,
        at: usize,
        range: bool,
        additive: bool,
    ) {
        if self.marked.is_empty()
            && let Some(t) = primary.and_then(|i| rows.get(i))
        {
            self.marked.insert(key(t));
            self.anchor = Some(key(t));
        }
        let clicked = key(&rows[at]);
        if range {
            let from = self
                .anchor
                .as_ref()
                .and_then(|id| rows.iter().position(|t| t.source == id.0 && t.disc == id.1))
                .unwrap_or(at);
            if !additive {
                self.marked.clear();
            }
            self.marked
                .extend(rows[from.min(at)..=from.max(at)].iter().map(|t| key(t)));
            self.anchor.get_or_insert(clicked);
        } else {
            if !self.marked.remove(&clicked) {
                self.marked.insert(clicked.clone());
            }
            self.anchor = Some(clicked);
        }
    }
}
pub struct MusicTable {
    pub rows: crate::library::TrackList,
    original: crate::library::TrackList,
    pub columns: Vec<LayoutColumn>,
    sort: Option<(Field, ColumnSort)>,
    selection: Selection,
    pub player: Entity<Player>,
    browser: WeakEntity<crate::browser::Browser>,
    /// What an empty table says: loading, no match, or only subfolders.
    pub empty: SharedString,
}
impl MusicTable {
    pub fn new(
        player: Entity<Player>,
        saved: Vec<LayoutColumn>,
        browser: WeakEntity<crate::browser::Browser>,
    ) -> Self {
        let valid = saved.iter().any(|c| c.field == Field::Title)
            && saved.iter().enumerate().all(|(i, c)| {
                (24. ..=1200.).contains(&c.width) && saved[..i].iter().all(|d| d.field != c.field)
            });
        Self {
            player,
            browser,
            rows: Arc::default(),
            original: Arc::default(),
            columns: if valid {
                saved
            } else {
                Self::default_columns()
            },
            sort: None,
            selection: Default::default(),
            empty: SharedString::default(),
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
    fn toggle_field(&mut self, field: Field) {
        if let Some(i) = self.columns.iter().position(|c| c.field == field) {
            if field != Field::Title {
                self.columns.remove(i);
                if self.sort.is_some_and(|(f, _)| f == field) {
                    self.sort = None;
                    self.apply_sort();
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
    pub fn set_rows(&mut self, rows: crate::library::TrackList) {
        self.original = rows;
        self.apply_sort();
    }
    fn apply_sort(&mut self) {
        self.rows = self.original.clone();
        if let Some((f, order)) = self.sort {
            let rows = Arc::make_mut(&mut self.rows);
            match f {
                Field::Length => rows.sort_by(|a, b| {
                    a.duration
                        .partial_cmp(&b.duration)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }),
                Field::Disc => rows.sort_by_key(|t| t.disc_number),
                Field::Year => rows.sort_by_key(|t| t.year),
                Field::Number => rows.sort_by_key(|t| t.display_number()),
                _ => rows.sort_by_cached_key(|t| f.value(t).to_lowercase()),
            }
            if order == ColumnSort::Descending {
                rows.reverse();
            }
        }
    }
    /// Header clicks cycle ascending, descending, and the folder order.
    fn cycle_sort(&mut self, field: Field) {
        self.sort = match self.sort {
            Some((f, ColumnSort::Ascending)) if f == field => Some((field, ColumnSort::Descending)),
            Some((f, ColumnSort::Descending)) if f == field => None,
            _ => Some((field, ColumnSort::Ascending)),
        };
        self.apply_sort();
    }
    pub fn play(&self, ix: usize, cx: &mut App) {
        if ix < self.rows.len() {
            self.player.update(cx, |p, cx| {
                p.command(Command::Play(self.rows.clone(), ix), cx)
            });
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
    pub fn clear_marks(&mut self) {
        self.selection = Default::default();
    }
    fn chosen(&self, ix: usize) -> crate::library::TrackList {
        if self.selection.marked.is_empty() || !self.selection.marked.contains(&key(&self.rows[ix]))
        {
            return vec![self.rows[ix].clone()].into();
        }
        self.rows
            .iter()
            .filter(|t| self.selection.marked.contains(&key(t)))
            .cloned()
            .collect()
    }
    fn is_playing(&self, t: &Track, cx: &App) -> bool {
        self.player
            .read(cx)
            .status
            .track
            .as_ref()
            .is_some_and(|p| p.same_item(t))
    }
}
impl TableDelegate for MusicTable {
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn column(&self, ix: usize, _: &App) -> Column {
        let c = &self.columns[ix];
        let mut col = Column::new(c.field.label(), c.field.label())
            .width(c.width)
            .min_width(px(c.field.min_width()))
            .paddings(Edges {
                top: px(0.),
                bottom: px(0.),
                left: px(CELL_X),
                right: px(CELL_X),
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
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if sorted.is_some() {
                m.text_muted
            } else {
                m.text_faint
            })
            .hover(|d| d.text_color(m.text_muted))
            .cursor_pointer()
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
            .on_click(cx.listener(move |t, _, _, cx| {
                let selected = t
                    .selected_row()
                    .and_then(|i| t.delegate().rows.get(i))
                    .cloned();
                t.delegate_mut().cycle_sort(field);
                if let Some(ix) = selected
                    .and_then(|item| t.delegate().rows.iter().position(|r| r.same_item(&item)))
                {
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
                                    t.delegate_mut().toggle_field(field);
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
    fn cell_label(&self, row: usize, col: usize, _: &App) -> Option<SharedString> {
        Some(self.columns[col].field.value(self.rows.get(row)?).into())
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let f = self.columns[col].field;
        let t = &self.rows[row];
        let m = *materials(cx);
        let playing = self.is_playing(t, cx);
        let cell = h_flex()
            .size_full()
            .min_w_0()
            .when(f.right_aligned(), |d| d.justify_end());
        match f {
            Field::Number if playing => cell
                .child(
                    Icon::new(IconName::AudioLines)
                        .size(px(14.))
                        .text_color(m.accent),
                )
                .into_any_element(),
            Field::Title => cell
                .text_color(if playing { m.accent } else { m.text })
                .child(div().min_w_0().truncate().child(f.value(t)))
                .into_any_element(),
            _ => cell
                .text_color(match f {
                    Field::Number | Field::Format => m.text_faint,
                    _ => m.text_muted,
                })
                .when(f == Field::Format, |d| d.text_size(px(11.5)))
                .child(div().min_w_0().truncate().child(f.value(t)))
                .into_any_element(),
        }
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let marked = !self.selection.marked.is_empty()
            && self.selection.marked.contains(&key(&self.rows[row]));
        div()
            .id(("track-row", row))
            .when(marked, |d| d.bg(materials(cx).selected))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |table, event: &MouseDownEvent, _, cx| {
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
                    } else {
                        table.delegate_mut().clear_marks();
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
            .when(!self.empty.is_empty(), |d| {
                d.child(Icon::new(IconName::FolderOpen).size(px(26.)))
                    .child(self.empty.clone())
            })
    }
    fn context_menu(
        &mut self,
        ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(track) = self.rows.get(ix).cloned() else {
            return menu;
        };
        let rows = self.rows.clone();
        let player = self.player.clone();
        let send = move |cmd: Command, cx: &mut App| player.update(cx, |p, cx| p.command(cmd, cx));
        let (s1, s2, s3) = (send.clone(), send.clone(), send);
        let chosen = self.chosen(ix);
        let next_label = if chosen.len() > 1 {
            format!("Play {} Tracks Next", chosen.len())
        } else {
            "Play Next".into()
        };
        let add_label = if chosen.len() > 1 {
            format!("Add {} Tracks to Queue", chosen.len())
        } else {
            "Add to Queue".into()
        };
        let upcoming = chosen.clone();
        let browser = self.browser.clone();
        menu.item(
            PopupMenuItem::new("Play")
                .icon(IconName::Play)
                .on_click(move |_, _, cx| s1(Command::Play(rows.clone(), ix), cx)),
        )
        .item(
            PopupMenuItem::new(next_label)
                .icon(IconName::ListStart)
                .on_click(move |_, _, cx| s2(Command::PlayNext(upcoming.clone()), cx)),
        )
        .item(
            PopupMenuItem::new(add_label)
                .icon(IconName::ListEnd)
                .on_click(move |_, _, cx| s3(Command::Enqueue(chosen.clone()), cx)),
        )
        .separator()
        .item(
            PopupMenuItem::new("Open Containing Folder")
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
    use super::{Arc, Selection, Track, key};
    #[test]
    fn additive_selection_survives_reorder_and_shift_uses_logical_anchor() {
        let mut rows: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|p| Arc::new(Track::file((*p).into())))
            .collect();
        let mut selection = Selection::default();
        selection.modify(&rows, Some(0), 2, false, true);
        assert_eq!(selection.marked.len(), 2);
        rows.reverse();
        selection.modify(&rows, None, 3, true, false);
        assert_eq!(selection.marked.len(), 3);
        assert!(!selection.marked.contains(&key(&rows[0])));
        selection.modify(&rows, None, 1, false, true);
        assert_eq!(selection.marked.len(), 2);
    }
}

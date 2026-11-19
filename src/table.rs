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
    const ALL: [Self; 6] = [
        Self::Number,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Length,
        Self::Format,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Length => "Time",
            Self::Format => "Format",
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
            Self::Format => 64.,
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
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutColumn {
    field: Field,
    width: f32,
}
pub struct MusicTable {
    pub rows: Arc<[Track]>,
    original: Arc<[Track]>,
    pub columns: Vec<LayoutColumn>,
    sort: Option<(Field, ColumnSort)>,
    pub player: Entity<Player>,
    /// What an empty table says: loading, no match, or only subfolders.
    pub empty: SharedString,
}
impl MusicTable {
    pub fn new(player: Entity<Player>, saved: Vec<LayoutColumn>) -> Self {
        let valid = saved.iter().any(|c| c.field == Field::Title)
            && saved.iter().enumerate().all(|(i, c)| {
                (24. ..=1200.).contains(&c.width) && saved[..i].iter().all(|d| d.field != c.field)
            });
        Self {
            player,
            rows: Arc::default(),
            original: Arc::default(),
            columns: if valid {
                saved
            } else {
                Self::default_columns()
            },
            sort: None,
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
    pub fn set_rows(&mut self, rows: Arc<[Track]>) {
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
                t.delegate_mut().cycle_sort(field);
                t.refresh(cx);
                cx.notify();
            }))
            .context_menu({
                let table = cx.entity().downgrade();
                let shown: Vec<Field> = self.columns.iter().map(|c| c.field).collect();
                move |mut menu, _, _| {
                    let edit = |f: fn(&mut MusicTable)| {
                        let table = table.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = table.update(cx, |t, cx| {
                                f(t.delegate_mut());
                                t.refresh(cx);
                                cx.notify();
                            });
                        }
                    };
                    menu = menu.label("Columns");
                    for f in Field::ALL {
                        let toggle: fn(&mut MusicTable) = match f {
                            Field::Number => |t| t.toggle_field(Field::Number),
                            Field::Title => |t| t.toggle_field(Field::Title),
                            Field::Artist => |t| t.toggle_field(Field::Artist),
                            Field::Album => |t| t.toggle_field(Field::Album),
                            Field::Length => |t| t.toggle_field(Field::Length),
                            Field::Format => |t| t.toggle_field(Field::Format),
                        };
                        menu = menu.item(
                            PopupMenuItem::new(f.menu_label())
                                .checked(shown.contains(&f))
                                .disabled(f == Field::Title)
                                .on_click(edit(toggle)),
                        );
                    }
                    menu.separator().item(
                        PopupMenuItem::new("Reset Columns")
                            .on_click(edit(|t| t.columns = MusicTable::default_columns())),
                    )
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
        let (t1, t2) = (track.clone(), track);
        menu.item(
            PopupMenuItem::new("Play")
                .icon(IconName::Play)
                .on_click(move |_, _, cx| s1(Command::Play(rows.clone(), ix), cx)),
        )
        .item(
            PopupMenuItem::new("Play Next")
                .icon(IconName::ListStart)
                .on_click(move |_, _, cx| s2(Command::PlayNext(vec![t1.clone()]), cx)),
        )
        .item(
            PopupMenuItem::new("Add to Queue")
                .icon(IconName::ListEnd)
                .on_click(move |_, _, cx| s3(Command::Enqueue(vec![t2.clone()]), cx)),
        )
    }
}

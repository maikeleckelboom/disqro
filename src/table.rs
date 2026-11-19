use crate::{
    assets::FileIcon,
    audio::Command,
    library::{Track, clock},
    player::Player,
};
use gpui_kit::{
    component::{
        ActiveTheme, Icon, IconName, Sizable, h_flex,
        menu::{PopupMenu, PopupMenuItem},
        table::{Column, ColumnSort, TableDelegate, TableState},
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Field {
    Title,
    Artist,
    Album,
    Length,
    Format,
}
impl Field {
    const ALL: [Self; 5] = [
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Length,
        Self::Format,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Length => "Time",
            Self::Format => "Format",
        }
    }
    fn width(self) -> f32 {
        match self {
            Self::Title => 300.,
            Self::Artist => 164.,
            Self::Album => 176.,
            Self::Length => 72.,
            Self::Format => 78.,
        }
    }
    fn value(self, t: &Track) -> String {
        match self {
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
}
impl MusicTable {
    pub fn new(player: Entity<Player>, saved: Vec<LayoutColumn>) -> Self {
        let valid = saved.len() == Field::ALL.len()
            && Field::ALL.iter().all(|f| {
                saved
                    .iter()
                    .any(|c| c.field == *f && (48. ..=1200.).contains(&c.width))
            });
        Self {
            player,
            rows: Arc::default(),
            original: Arc::default(),
            columns: if valid {
                saved
            } else {
                Field::ALL
                    .into_iter()
                    .map(|field| LayoutColumn {
                        field,
                        width: field.width(),
                    })
                    .collect()
            },
            sort: None,
        }
    }
    pub fn set_rows(&mut self, rows: Arc<[Track]>) {
        self.original = rows;
        self.apply_sort();
    }
    fn apply_sort(&mut self) {
        self.rows = self.original.clone();
        if let Some((f, order)) = self.sort {
            let rows = Arc::make_mut(&mut self.rows);
            if f == Field::Length {
                rows.sort_by(|a, b| {
                    a.duration
                        .partial_cmp(&b.duration)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            } else {
                rows.sort_by_cached_key(|t| f.value(t).to_lowercase());
            }
            if order == ColumnSort::Descending {
                rows.reverse();
            }
        }
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
            .min_width(px(48.))
            .sortable();
        if c.field == Field::Length {
            col = col.text_right();
        }
        if let Some((field, dir)) = self.sort
            && field == c.field
        {
            col.sort = Some(dir);
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
    fn perform_sort(
        &mut self,
        ix: usize,
        dir: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = (dir != ColumnSort::Default).then_some((self.columns[ix].field, dir));
        self.apply_sort();
        cx.notify();
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
        let content = div().min_w_0().truncate().child(f.value(t));
        if f == Field::Title {
            let playing = self
                .player
                .read(cx)
                .status
                .track
                .as_ref()
                .is_some_and(|p| p.same_item(t));
            let ico = if t.disc.is_some() {
                FileIcon::Chapter
            } else {
                FileIcon::Recording
            };
            h_flex()
                .gap_2()
                .min_w_0()
                .when(playing, |row| row.text_color(rgb(0x85d9cf)))
                .child(if playing {
                    Icon::new(IconName::Play).small().text_color(rgb(0x85d9cf))
                } else {
                    ico.icon().small().text_color(cx.theme().muted_foreground)
                })
                .child(content)
                .into_any_element()
        } else {
            content
                .text_color(cx.theme().muted_foreground)
                .into_any_element()
        }
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn context_menu(
        &mut self,
        ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let rows = self.rows.clone();
        let player = self.player.clone();
        menu.item(
            PopupMenuItem::new("Play from here")
                .icon(IconName::Play)
                .on_click(move |_, _, cx| {
                    player.update(cx, |p, cx| p.command(Command::Play(rows.clone(), ix), cx));
                }),
        )
    }
}

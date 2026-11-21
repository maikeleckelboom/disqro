//! The Queue panel: Now Playing, Up Next, and what played before.
use crate::{
    audio::{Command, TransportState},
    browser::Destination,
    library::clock,
    player::{Player, PlayerEvent},
    queue::{Entry, Queue, Repeat},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, h_flex,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        v_flex,
    },
    prelude::*,
    *,
};
use std::sync::Arc;

gpui_kit::actions!(
    queue,
    [
        PlaySelectedEntry,
        RemoveSelectedEntry,
        MoveEntryUp,
        MoveEntryDown
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", PlaySelectedEntry, Some("Queue")),
        KeyBinding::new("delete", RemoveSelectedEntry, Some("Queue")),
        KeyBinding::new("alt-up", MoveEntryUp, Some("Queue")),
        KeyBinding::new("alt-down", MoveEntryDown, Some("Queue")),
    ]);
}

const ROW: f32 = 30.;

#[derive(Clone)]
struct DraggedEntry {
    id: u64,
    title: SharedString,
}
impl Render for DraggedEntry {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = materials(cx);
        div()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(5.))
            .bg(m.overlay)
            .border_1()
            .border_color(m.control_line)
            .text_size(px(13.))
            .text_color(m.text)
            .child(self.title.clone())
    }
}

#[derive(Clone, Copy)]
enum Row {
    Header(Section),
    Entry(usize),
    Empty,
}
#[derive(Clone, Copy, PartialEq)]
enum Section {
    History,
    NowPlaying,
    UpNext,
}

pub struct QueueView {
    player: Entity<Player>,
    focus: FocusHandle,
    selected: Option<u64>,
    show_history: bool,
    scroll: UniformListScrollHandle,
}

impl QueueView {
    pub fn new(player: Entity<Player>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&player, |_, _, e, cx| {
            if matches!(
                e,
                PlayerEvent::QueueChanged | PlayerEvent::TrackChanged | PlayerEvent::StateChanged
            ) {
                cx.notify();
            }
        })
        .detach();
        Self {
            player,
            focus: cx.focus_handle(),
            selected: None,
            show_history: false,
            scroll: UniformListScrollHandle::new(),
        }
    }
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
    fn queue(&self, cx: &App) -> Arc<Queue> {
        self.player.read(cx).queue().clone()
    }
    fn send(&self, c: Command, cx: &mut App) {
        self.player.update(cx, |p, cx| p.command(c, cx));
    }
    fn rows(&self, q: &Queue) -> Vec<Row> {
        let mut rows = vec![];
        let history = q.history().len();
        if history > 0 {
            rows.push(Row::Header(Section::History));
            if self.show_history {
                rows.extend((0..history).map(Row::Entry));
            }
        }
        if let Some(c) = q.current {
            rows.push(Row::Header(Section::NowPlaying));
            rows.push(Row::Entry(c));
        }
        let start = q.current.map_or(0, |c| c + 1);
        if start < q.entries.len() {
            rows.push(Row::Header(Section::UpNext));
            rows.extend((start..q.entries.len()).map(Row::Entry));
        } else {
            rows.push(Row::Empty);
        }
        rows
    }
    pub fn summary(&self, cx: &App) -> String {
        let q = self.queue(cx);
        let upcoming = q.upcoming();
        if upcoming.is_empty() {
            return String::new();
        }
        let total: f64 = upcoming.iter().filter_map(|e| e.track.duration).sum();
        if total > 0. {
            format!("{} up next · {}", upcoming.len(), clock(total))
        } else {
            format!("{} up next", upcoming.len())
        }
    }
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        let q = self.queue(cx);
        let Some(id) = self.selected else { return };
        let Some(ix) = q.index_of(id) else { return };
        let to = if delta < 0 {
            ix.saturating_sub(1)
        } else {
            (ix + 2).min(q.entries.len())
        };
        self.send(Command::Move(id, to), cx);
    }
    pub fn panel_menu(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        let q = self.queue(cx);
        let p = self.player.clone();
        let item = |label: &'static str, checked: bool, c: fn() -> Command| {
            let p = p.clone();
            PopupMenuItem::new(label)
                .checked(checked)
                .on_click(move |_, _, cx| p.update(cx, |p, cx| p.command(c(), cx)))
        };
        let shuffle = q.shuffle;
        menu.item(item(
            "Shuffle",
            shuffle,
            if shuffle {
                || Command::Shuffle(false)
            } else {
                || Command::Shuffle(true)
            },
        ))
        .separator()
        .item(item("Repeat Off", q.repeat == Repeat::Off, || {
            Command::Repeat(Repeat::Off)
        }))
        .item(item("Repeat All", q.repeat == Repeat::All, || {
            Command::Repeat(Repeat::All)
        }))
        .item(item("Repeat One", q.repeat == Repeat::One, || {
            Command::Repeat(Repeat::One)
        }))
        .separator()
        .item(
            item("Clear Up Next", false, || Command::ClearUpcoming)
                .disabled(q.upcoming().is_empty()),
        )
        .item(
            item("Clear History", false, || Command::ClearHistory).disabled(q.history().is_empty()),
        )
    }
    fn entry_menu(
        this: WeakEntity<Self>,
        entry: Entry,
        is_current: bool,
        next_slot: usize,
        menu: PopupMenu,
    ) -> PopupMenu {
        let id = entry.id;
        let act = |c: Command| {
            let this = this.clone();
            let c = std::cell::Cell::new(Some(c));
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                if let Some(c) = c.take() {
                    let _ = this.update(cx, |s, cx| s.send(c, cx));
                }
            }
        };
        let (destination, label) = if entry.disc_context.allows(&entry.track) {
            (Destination::Disc, "Show Track on CD")
        } else {
            (Destination::Collection, "Show in Collection")
        };
        let play = act(Command::Jump(id));
        let next = act(Command::Move(id, next_slot));
        let remove = act(Command::Remove(vec![id]));
        menu.when(!is_current, |m| {
            m.item(
                PopupMenuItem::new("Play")
                    .icon(IconName::Play)
                    .on_click(play),
            )
            .item(
                PopupMenuItem::new("Play Next")
                    .icon(IconName::ListStart)
                    .on_click(next),
            )
        })
        .item(
            PopupMenuItem::new("Remove from Queue")
                .icon(IconName::X)
                .on_click(remove),
        )
        .separator()
        .item(
            PopupMenuItem::new(label)
                .icon(IconName::FolderOpen)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |s, cx| {
                        s.player.update(cx, |_, cx| {
                            cx.emit(PlayerEvent::Locate(entry.track.clone(), destination))
                        })
                    });
                }),
        )
    }
    fn render_row(&self, row: Row, q: &Queue, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        match row {
            Row::Empty => div()
                .h(px(ROW))
                .px(px(16.))
                .flex()
                .items_center()
                .text_size(px(12.5))
                .text_color(m.text_faint)
                .child("Nothing up next")
                .into_any_element(),
            Row::Header(section) => {
                let (label, count) = match section {
                    Section::History => ("History", Some(q.history().len())),
                    Section::NowPlaying => (
                        match self.player.read(cx).status.playback_state() {
                            TransportState::Playing => "Now Playing",
                            TransportState::Paused => "Paused",
                            TransportState::Finished => "Finished",
                            TransportState::Stopped => "Stopped",
                        },
                        None,
                    ),
                    Section::UpNext => ("Up Next", None),
                };
                h_flex()
                    .id(("section", section as usize))
                    .w_full()
                    .h(px(ROW))
                    .px(px(16.))
                    .pt(px(8.))
                    .gap(px(6.))
                    .items_center()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(m.text_faint)
                    .when(section == Section::History, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.text_color(m.text_muted))
                            .child(
                                Icon::new(if self.show_history {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.)),
                            )
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.show_history = !s.show_history;
                                cx.notify();
                            }))
                    })
                    .child(label)
                    .when_some(count, |d, n| d.child(format!("{n}")))
                    .into_any_element()
            }
            Row::Entry(ix) => {
                let entry = q.entries[ix].clone();
                let id = entry.id;
                let is_current = q.current == Some(ix);
                let is_history = q.current.is_some_and(|c| ix < c);
                let selected = self.selected == Some(id);
                let t = &entry.track;
                let title: SharedString = t.title.clone().into();
                let artist = t.artist.clone();
                let duration = t.duration.map(clock).unwrap_or_default();
                let status = &self.player.read(cx).status;
                let current_icon = match status.playback_state() {
                    TransportState::Playing => IconName::AudioLines,
                    TransportState::Paused => IconName::Pause,
                    _ => IconName::Square,
                };
                let next_slot = q.current.map_or(0, |c| c + 1);
                let this = cx.weak_entity();
                let menu_entry = entry.clone();
                let row = h_flex()
                    .id(("entry", id))
                    .w_full()
                    .h(px(ROW))
                    .px(px(10.))
                    .gap(px(10.))
                    .rounded(px(5.))
                    .items_center()
                    .text_size(px(13.))
                    .when(selected, |d| d.bg(m.selected))
                    .when(!selected, |d| d.hover(|d| d.bg(m.hover)))
                    .when(is_history, |d| d.opacity(0.55))
                    .child(
                        div()
                            .w(px(16.))
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .when(is_current, |d| {
                                d.child(Icon::new(current_icon).size(px(14.)).text_color(m.accent))
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(8.))
                            .child(
                                // The title keeps most of the row; the artist takes the rest.
                                div()
                                    .flex_shrink_0()
                                    .max_w(relative(0.72))
                                    .truncate()
                                    .text_color(if is_current { m.accent } else { m.text })
                                    .child(title.clone()),
                            )
                            .when(!artist.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(m.text_faint)
                                        .child(artist),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .text_color(m.text_faint)
                            .child(duration),
                    )
                    .on_click(cx.listener(move |s, e: &ClickEvent, w, cx| {
                        s.selected = Some(id);
                        s.focus.focus(w, cx);
                        if e.click_count() >= 2 {
                            s.send(Command::Jump(id), cx);
                        }
                        cx.notify();
                    }))
                    .on_drag(
                        DraggedEntry {
                            id,
                            title: title.clone(),
                        },
                        |d, _, _, cx| cx.new(|_| d.clone()),
                    )
                    .drag_over::<DraggedEntry>(move |d, _, _, _| {
                        d.border_t_2().border_color(m.accent)
                    })
                    .on_drop(cx.listener(move |s, d: &DraggedEntry, _, cx| {
                        s.send(Command::Move(d.id, ix), cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        Self::entry_menu(
                            this.clone(),
                            menu_entry.clone(),
                            is_current,
                            next_slot,
                            menu,
                        )
                    });
                div().w_full().px(px(6.)).child(row).into_any_element()
            }
        }
    }
}

impl Render for QueueView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let q = self.queue(cx);
        let rows = self.rows(&q);
        let view = cx.entity();
        let q2 = q.clone();
        let end_drop = cx.listener(|s, d: &DraggedEntry, _, cx| {
            let end = s.queue(cx).entries.len();
            s.send(Command::Move(d.id, end), cx);
        });
        let body = if q.entries.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .px(px(24.))
                .text_center()
                .child(
                    Icon::new(IconName::ListMusic)
                        .size(px(26.))
                        .text_color(m.text_faint),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(m.text_muted)
                        .child("The queue is empty"),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list("queue-rows", rows.len(), move |range, _, cx| {
                        view.update(cx, |v, cx| {
                            range
                                .map(|i| v.render_row(rows[i], &q2, cx))
                                .collect::<Vec<_>>()
                        })
                    })
                    .size_full()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        v_flex()
            .id("queue")
            .key_context("Queue")
            .track_focus(&self.focus)
            .size_full()
            .bg(m.chrome)
            .on_action(cx.listener(|s, _: &PlaySelectedEntry, _, cx| {
                if let Some(id) = s.selected {
                    s.send(Command::Jump(id), cx);
                }
            }))
            .on_action(cx.listener(|s, _: &RemoveSelectedEntry, _, cx| {
                if let Some(id) = s.selected {
                    let q = s.queue(cx);
                    // Keep a selection so Delete can be pressed repeatedly.
                    s.selected = q
                        .index_of(id)
                        .and_then(|i| {
                            q.entries
                                .get(i + 1)
                                .or_else(|| q.entries.get(i.wrapping_sub(1)))
                        })
                        .map(|e| e.id);
                    s.send(Command::Remove(vec![id]), cx);
                }
            }))
            .on_action(cx.listener(|s, _: &MoveEntryUp, _, cx| s.move_selected(-1, cx)))
            .on_action(cx.listener(|s, _: &MoveEntryDown, _, cx| s.move_selected(1, cx)))
            .child(body)
            .child(
                div()
                    .id("queue-end")
                    .h(px(18.))
                    .flex_shrink_0()
                    .drag_over::<DraggedEntry>(move |d, _, _, _| {
                        d.border_t_2().border_color(m.accent)
                    })
                    .on_drop(end_drop),
            )
    }
}

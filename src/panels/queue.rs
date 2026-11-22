//! Queue and History: two panels over the one playback queue.
//!
//! **Queue** shows the current entry, marked by its state rather than a
//! heading, and what plays after it. **History** shows what played before,
//! most recent first. Each panel keeps its own selection and scroll
//! position; switching, splitting or closing them never changes the queue.
use super::{Pane, PaneContent, PanelType, Services};
use crate::{
    audio::{Command, TransportState},
    browser::Destination,
    library::clock,
    player::{Player, PlayerEvent},
    queue::{Entry, Queue, Repeat},
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, h_flex,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        v_flex,
    },
    prelude::*,
    *,
};
use std::{rc::Rc, sync::Arc};

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

pub static QUEUE: PanelType = PanelType {
    key: "disqro.queue",
    title: "Queue",
    icon: IconName::ListMusic,
    instances: super::Instances::Single,
    home: super::Home::Trailing,
    build: |services, _, _, cx| Rc::new(cx.new(|cx| Entries::new(Mode::Queue, services, cx))),
};

pub static HISTORY: PanelType = PanelType {
    key: "disqro.history",
    title: "History",
    icon: IconName::Clock,
    instances: super::Instances::Single,
    home: super::Home::Trailing,
    build: |services, _, _, cx| Rc::new(cx.new(|cx| Entries::new(Mode::History, services, cx))),
};

const ROW: f32 = 30.;

#[derive(Clone)]
struct DraggedEntry {
    id: u64,
    title: SharedString,
}
impl Render for DraggedEntry {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::nav::chip(self.title.clone(), cx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Queue,
    History,
}

impl Mode {
    fn indices(self, q: &Queue) -> std::ops::Range<usize> {
        let current = q.history().len();
        match self {
            Self::Queue => current..q.entries.len(),
            Self::History => 0..current,
        }
    }
}

/// The queue indices a panel lists, in the order it lists them.
pub fn rows(mode: Mode, q: &Queue) -> Vec<usize> {
    let indices = mode.indices(q);
    match mode {
        // Before anything plays, everything is still to come.
        Mode::Queue => indices.collect(),
        Mode::History => indices.rev().collect(),
    }
}

pub struct Entries {
    presented: bool,
    mode: Mode,
    player: Entity<Player>,
    focus: FocusHandle,
    selected: Option<u64>,
    scroll: UniformListScrollHandle,
}

impl Entries {
    fn new(mode: Mode, services: &Services, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&services.player, |s: &mut Self, player, e, cx| {
            if matches!(
                e,
                PlayerEvent::QueueChanged | PlayerEvent::TrackChanged | PlayerEvent::StateChanged
            ) {
                // A seek or queue edit can move the selected entry into the
                // other view. Its old pane must no longer act on that entry.
                let q = player.read(cx).queue();
                s.selected = s.selected.filter(|id| {
                    q.index_of(*id)
                        .is_some_and(|ix| s.mode.indices(q).contains(&ix))
                });
                if s.presented {
                    cx.notify();
                }
            }
        })
        .detach();
        Self {
            presented: false,
            mode,
            player: services.player.clone(),
            focus: cx.focus_handle(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        }
    }
    fn queue(&self, cx: &App) -> Arc<Queue> {
        self.player.read(cx).queue().clone()
    }
    fn send(&self, c: Command, cx: &mut App) {
        self.player.update(cx, |p, cx| p.command(c, cx));
    }
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.mode != Mode::Queue {
            return;
        }
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
    fn panel_menu(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        let q = self.queue(cx);
        let p = self.player.clone();
        let item = |label: &'static str, checked: bool, c: fn() -> Command| {
            let p = p.clone();
            PopupMenuItem::new(label)
                .checked(checked)
                .on_click(move |_, _, cx| p.update(cx, |p, cx| p.command(c(), cx)))
        };
        if self.mode == Mode::History {
            return menu.item(
                item("Clear History", false, || Command::ClearHistory)
                    .disabled(q.history().is_empty()),
            );
        }
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
    }
    fn entry_menu(
        this: WeakEntity<Self>,
        entry: Entry,
        mode: Mode,
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
            PopupMenuItem::new(match mode {
                Mode::Queue => "Remove from Queue",
                Mode::History => "Remove from History",
            })
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
    fn render_row(&self, ix: usize, q: &Queue, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let entry = q.entries[ix].clone();
        let id = entry.id;
        let is_current = q.current == Some(ix);
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
        let mode = self.mode;
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
            .when(mode == Mode::Queue, |row| {
                row.on_drag(
                    DraggedEntry {
                        id,
                        title: title.clone(),
                    },
                    |d, _, _, cx| cx.new(|_| d.clone()),
                )
                .drag_over::<DraggedEntry>(move |d, _, _, _| d.border_t_2().border_color(m.accent))
                .on_drop(cx.listener(move |s, d: &DraggedEntry, _, cx| {
                    s.send(Command::Move(d.id, ix), cx);
                }))
            })
            .context_menu(move |menu, _, _| {
                Self::entry_menu(
                    this.clone(),
                    menu_entry.clone(),
                    mode,
                    is_current,
                    next_slot,
                    menu,
                )
            });
        div().w_full().px(px(6.)).child(row).into_any_element()
    }
}

impl Render for Entries {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let q = self.queue(cx);
        let order = rows(self.mode, &q);
        let view = cx.entity();
        let listed = q.clone();
        let end_drop = cx.listener(|s, d: &DraggedEntry, _, cx| {
            let end = s.queue(cx).entries.len();
            s.send(Command::Move(d.id, end), cx);
        });
        let body = if order.is_empty() {
            let (icon, text) = match self.mode {
                Mode::Queue => (IconName::ListMusic, "The queue is empty"),
                Mode::History => (IconName::Clock, "Nothing has played yet"),
            };
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .px(px(24.))
                .text_center()
                .child(Icon::new(icon).size(px(26.)).text_color(m.text_faint))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(m.text_muted)
                        .child(text),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .pt(px(4.))
                .child(
                    uniform_list("queue-rows", order.len(), move |range, _, cx| {
                        view.update(cx, |v, cx| {
                            range
                                .map(|i| v.render_row(order[i], &listed, cx))
                                .collect::<Vec<_>>()
                        })
                    })
                    .size_full()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let panel = cx.entity();
        v_flex()
            .id("entries")
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
                let Some(id) = s.selected else { return };
                let q = s.queue(cx);
                // Keep a selection so Delete can be pressed repeatedly.
                let listed = rows(s.mode, &q);
                let ids: Vec<u64> = listed.iter().map(|&i| q.entries[i].id).collect();
                s.selected = ids.iter().position(|&e| e == id).and_then(|i| {
                    ids.get(i + 1)
                        .or_else(|| i.checked_sub(1).and_then(|i| ids.get(i)))
                        .copied()
                });
                s.send(Command::Remove(vec![id]), cx);
            }))
            .on_action(cx.listener(|s, _: &MoveEntryUp, _, cx| s.move_selected(-1, cx)))
            .on_action(cx.listener(|s, _: &MoveEntryDown, _, cx| s.move_selected(1, cx)))
            .child(body)
            .when(self.mode == Mode::Queue, |d| {
                d.child(
                    div()
                        .id("queue-end")
                        .h(px(18.))
                        .flex_shrink_0()
                        .drag_over::<DraggedEntry>(move |d, _, _, _| {
                            d.border_t_2().border_color(m.accent)
                        })
                        .on_drop(end_drop),
                )
            })
            .context_menu(move |menu, _, cx| panel.read(cx).panel_menu(menu, cx))
    }
}

impl PaneContent for Entity<Entries> {
    fn set_presented(&self, presented: bool, cx: &mut App) {
        self.update(cx, |s, cx| {
            s.presented = presented;
            if presented {
                cx.notify();
            }
        });
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus.clone()
    }
    fn title(&self, cx: &App) -> SharedString {
        match self.read(cx).mode {
            Mode::Queue => QUEUE.title.into(),
            Mode::History => HISTORY.title.into(),
        }
    }
    fn watch(&self, _: &mut Context<Pane>) -> Vec<Subscription> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, rows};
    use crate::{library::Track, queue::Queue};

    fn queue(n: usize, current: Option<usize>) -> Queue {
        let tracks: Vec<Track> = (0..n)
            .map(|i| Track::file(std::path::PathBuf::from(format!("C:\\Music\\{i}.flac"))))
            .collect();
        let mut q = Queue::default();
        q.replace(&tracks, current.unwrap_or(0));
        q.current = current;
        q
    }

    #[test]
    fn queue_lists_the_current_entry_then_what_follows() {
        let q = queue(5, Some(2));
        assert_eq!(rows(Mode::Queue, &q), [2, 3, 4]);
        assert_eq!(rows(Mode::History, &q), [1, 0]);
    }

    #[test]
    fn before_anything_plays_everything_is_upcoming_and_history_is_empty() {
        let q = queue(3, None);
        assert_eq!(rows(Mode::Queue, &q), [0, 1, 2]);
        assert!(rows(Mode::History, &q).is_empty());
        assert!(rows(Mode::Queue, &Queue::default()).is_empty());
    }
}

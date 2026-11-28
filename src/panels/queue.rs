//! Queue and History: two panels over the one playback sequence.
//!
//! The active occurrence is Now Playing, owned by the transport and shown
//! by the player. **Queue** lists only what plays after it, and **History**
//! what played before it, most recent first. Neither lists the active
//! occurrence, so no queue edit can remove it. Each panel keeps its own
//! selection and scroll position; switching, splitting or closing them
//! never changes the sequence.
use super::{Pane, PaneContent, PanelType, Services};
use crate::{
    audio::Command,
    library::clock,
    player::{Player, PlayerEvent, reveal_items},
    queue::{Entry, Queue, Repeat},
    theme::{materials, size},
};
use gpui_kit::{
    assets::IconName,
    component::{
        ElementExt, Icon, h_flex,
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
        SelectPreviousEntry,
        SelectNextEntry,
        MoveEntryUp,
        MoveEntryDown
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", PlaySelectedEntry, Some("Queue")),
        KeyBinding::new("delete", RemoveSelectedEntry, Some("Queue")),
        KeyBinding::new("up", SelectPreviousEntry, Some("Queue")),
        KeyBinding::new("down", SelectNextEntry, Some("Queue")),
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
    title: "Queue History",
    icon: IconName::Clock,
    instances: super::Instances::Single,
    home: super::Home::Trailing,
    build: |services, _, _, cx| Rc::new(cx.new(|cx| Entries::new(Mode::History, services, cx))),
};

const ROW: f32 = size::ROW;

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
        match self {
            Self::Queue => q.upcoming_range(),
            Self::History => 0..q.history().len(),
        }
    }
}

/// Map a virtual row directly to its occurrence, including reverse history.
fn row_index(mode: Mode, indices: &std::ops::Range<usize>, row: usize) -> usize {
    match mode {
        Mode::Queue => indices.start + row,
        Mode::History => indices.end - 1 - row,
    }
}

pub struct Entries {
    presented: bool,
    mode: Mode,
    player: Entity<Player>,
    focus: FocusHandle,
    selected: Option<u64>,
    scroll: UniformListScrollHandle,
    /// Whether rows have room for an artist beside the title, as of the
    /// panel's last layout.
    roomy: Rc<std::cell::Cell<bool>>,
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
            roomy: Rc::new(std::cell::Cell::new(true)),
        }
    }
    fn queue(&self, cx: &App) -> Arc<Queue> {
        self.player.read(cx).queue().clone()
    }
    fn send(&self, c: Command, cx: &mut App) {
        self.player.update(cx, |p, cx| p.command(c, cx));
    }
    fn select_adjacent(&mut self, delta: isize, cx: &mut Context<Self>) {
        let q = self.queue(cx);
        let range = self.mode.indices(&q);
        if range.is_empty() {
            return;
        }
        let row = self
            .selected
            .and_then(|id| q.index_of(id))
            .filter(|ix| range.contains(ix))
            .map_or(0, |ix| {
                let row = match self.mode {
                    Mode::Queue => ix - range.start,
                    Mode::History => range.end - 1 - ix,
                };
                row.saturating_add_signed(delta).min(range.len() - 1)
            });
        self.selected = Some(q.entries[row_index(self.mode, &range, row)].id);
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        cx.notify();
    }
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.mode != Mode::Queue {
            return;
        }
        let q = self.queue(cx);
        let Some(id) = self.selected else { return };
        let Some(ix) = q.index_of(id) else { return };
        let to = if delta < 0 {
            let Some(to) = ix.checked_sub(1) else { return };
            to
        } else {
            ix + 2
        };
        let before = q.entries.get(to).map(|entry| entry.id);
        self.send(Command::MoveUpcoming(id, before), cx);
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
        menu.when_some(q.source.as_ref(), |menu, source| {
            menu.item(PopupMenuItem::label(format!("Started from {source}")))
                .separator()
        })
        .item(item(
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
        player: &Entity<Player>,
        entry: Entry,
        mode: Mode,
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
        let play = act(Command::Jump(id));
        let next = act(Command::ScheduleNext(id));
        let remove = act(Command::Remove(vec![id]));
        let menu = menu
            .item(
                PopupMenuItem::new("Play")
                    .icon(IconName::Play)
                    .on_click(play),
            )
            .item(
                PopupMenuItem::new("Play Next")
                    .icon(IconName::ListStart)
                    .on_click(next),
            )
            .item(
                PopupMenuItem::new(match mode {
                    Mode::Queue => "Remove from Queue",
                    Mode::History => "Remove from History",
                })
                .icon(IconName::X)
                .on_click(remove),
            )
            .separator();
        reveal_items(
            menu,
            player,
            &entry.track,
            entry.disc_context,
            entry.playlist_occurrence,
        )
    }
    fn render_row(&self, ix: usize, q: &Queue, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let entry = q.entries[ix].clone();
        let id = entry.id;
        let selected = self.selected == Some(id);
        let t = &entry.track;
        let title: SharedString = t.title.clone().into();
        let artist = Some(SharedString::from(t.artist.clone())).filter(|a| !a.is_empty());
        let duration = t.duration.map(clock).unwrap_or_default();
        let this = cx.weak_entity();
        let mode = self.mode;
        let menu_entry = entry.clone();
        let player = self.player.clone();
        let row = h_flex()
            .id(("entry", id))
            .role(Role::ListBoxOption)
            .aria_selected(selected)
            .aria_label(title.clone())
            .w_full()
            .h(px(ROW))
            .px(px(size::INSET))
            .gap(px(10.))
            .rounded(px(size::RADIUS))
            .items_center()
            .text_size(px(size::TEXT))
            .when(selected, |d| d.bg(m.selected))
            .when(!selected, |d| d.hover(|d| d.bg(m.hover)))
            .child({
                // A narrow panel shows whole titles rather than a sliver of
                // each artist.
                let artist = artist.filter(|_| self.roomy.get());
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div()
                            .when(artist.is_some(), |d| {
                                d.flex_shrink_0().max_w(relative(0.72))
                            })
                            .when(artist.is_none(), |d| d.flex_1().min_w_0())
                            .truncate()
                            .text_color(m.text)
                            .child(title.clone()),
                    )
                    .when_some(artist, |d, artist| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(size::TEXT_SMALL))
                                .text_color(m.text_muted)
                                .child(artist),
                        )
                    })
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(size::TEXT_SMALL))
                    .font_features(crate::theme::tabular())
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
                    s.send(Command::MoveUpcoming(d.id, Some(id)), cx);
                }))
            })
            .context_menu(move |menu, _, _| {
                Self::entry_menu(this.clone(), &player, menu_entry.clone(), mode, menu)
            });
        div().w_full().px(px(6.)).child(row).into_any_element()
    }
}

impl Render for Entries {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let q = self.queue(cx);
        let mode = self.mode;
        let order = mode.indices(&q);
        let view = cx.entity();
        let listed = q.clone();
        let end_drop = cx.listener(|s, d: &DraggedEntry, _, cx| {
            s.send(Command::MoveUpcoming(d.id, None), cx);
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
                        .text_size(px(crate::theme::size::TEXT))
                        .text_color(m.text_muted)
                        .child(text),
                )
                .into_any_element()
        } else {
            div()
                .id("entry-list")
                .flex_1()
                .min_h_0()
                .pt(px(4.))
                .role(Role::ListBox)
                .child(
                    uniform_list("queue-rows", order.len(), move |range, _, cx| {
                        view.update(cx, |v, cx| {
                            range
                                .map(|i| v.render_row(row_index(mode, &order, i), &listed, cx))
                                .collect::<Vec<_>>()
                        })
                    })
                    .size_full()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let panel = cx.entity();
        let (roomy, entity) = (self.roomy.clone(), cx.entity_id());
        v_flex()
            .id("entries")
            .key_context("Queue")
            .track_focus(&self.focus)
            .size_full()
            .bg(m.chrome)
            .on_prepaint(move |bounds, w, _| {
                // Below this width an artist would shrink to a bare ellipsis.
                let fits = bounds.size.width >= px(230.);
                if roomy.replace(fits) != fits {
                    w.on_next_frame(move |_, cx| cx.notify(entity));
                }
            })
            .on_action(cx.listener(|s, _: &PlaySelectedEntry, _, cx| {
                if let Some(id) = s.selected {
                    s.send(Command::Jump(id), cx);
                }
            }))
            .on_action(cx.listener(|s, _: &RemoveSelectedEntry, _, cx| {
                let Some(id) = s.selected else { return };
                let q = s.queue(cx);
                // Keep a selection so Delete can be pressed repeatedly.
                let indices = s.mode.indices(&q);
                s.selected = q.index_of(id).and_then(|ix| {
                    let row = match s.mode {
                        Mode::Queue => ix.checked_sub(indices.start)?,
                        Mode::History => indices.end.checked_sub(ix + 1)?,
                    };
                    let neighbor = if row + 1 < indices.len() {
                        row + 1
                    } else {
                        row.checked_sub(1)?
                    };
                    Some(q.entries[row_index(s.mode, &indices, neighbor)].id)
                });
                s.send(Command::Remove(vec![id]), cx);
            }))
            .on_action(cx.listener(|s, _: &MoveEntryUp, _, cx| s.move_selected(-1, cx)))
            .on_action(cx.listener(|s, _: &MoveEntryDown, _, cx| s.move_selected(1, cx)))
            .on_action(cx.listener(|s, _: &SelectPreviousEntry, _, cx| s.select_adjacent(-1, cx)))
            .on_action(cx.listener(|s, _: &SelectNextEntry, _, cx| s.select_adjacent(1, cx)))
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
    use super::{Mode, row_index};
    fn rows(mode: Mode, queue: &Queue) -> Vec<usize> {
        let indices = mode.indices(queue);
        (0..indices.len())
            .map(|i| row_index(mode, &indices, i))
            .collect()
    }
    use crate::{library::Track, queue::Queue};

    fn queue(n: usize, current: Option<usize>) -> Queue {
        let tracks: Vec<Track> = (0..n)
            .map(|i| Track::file(std::path::PathBuf::from(format!("C:\\Music\\{i}.flac"))))
            .collect();
        let mut q = Queue::default();
        q.replace(tracks.into_iter().collect(), current.unwrap_or(0));
        q.current = current;
        q
    }

    #[test]
    fn queue_lists_only_what_follows_the_active_occurrence() {
        let q = queue(5, Some(2));
        assert_eq!(rows(Mode::Queue, &q), [3, 4]);
        assert_eq!(rows(Mode::History, &q), [1, 0]);
        // A single playing entry leaves an empty, valid queue.
        let single = queue(1, Some(0));
        assert!(rows(Mode::Queue, &single).is_empty());
        assert!(rows(Mode::History, &single).is_empty());
    }

    #[test]
    fn upcoming_duplicates_of_the_active_recording_stay_listed() {
        let track = Track::file(std::path::PathBuf::from(r"C:\Music\same.flac"));
        let mut q = Queue::default();
        q.replace([track.clone(), track].into_iter().collect(), 0);
        assert_eq!(rows(Mode::Queue, &q), [1]);
        assert_ne!(q.entries[0].id, q.entries[1].id);
    }

    #[test]
    fn before_anything_plays_everything_is_upcoming_and_history_is_empty() {
        let q = queue(3, None);
        assert_eq!(rows(Mode::Queue, &q), [0, 1, 2]);
        assert!(rows(Mode::History, &q).is_empty());
        assert!(rows(Mode::Queue, &Queue::default()).is_empty());
    }
}

//! Native listening history and statistics panels.
use super::{PaneContent, PanelType, Router, Services};
use crate::{
    audio::Command,
    catalog::CatalogEvent,
    listening::{Period, duration, local_date, midnight},
    player::Player,
    store::{
        Handle, Ticket, TrackId,
        listening::{HISTORY_PAGE, HistoryKey, HistoryRow, Rank, Statistics, period_range},
    },
    theme::{materials, size},
};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Selectable, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu, PopupMenu, PopupMenuItem},
        table::{Column, DataTable, TableDelegate, TableEvent, TableState},
        tooltip::Tooltip,
        v_flex,
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::rc::Rc;

gpui_kit::actions!(listening_history, [PlayListen, RemoveListen]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", PlayListen, Some("ListeningHistory && DataTable")),
        KeyBinding::new("delete", RemoveListen, Some("ListeningHistory")),
    ]);
}

pub static HISTORY: PanelType = PanelType {
    key: "disqro.listening-history",
    title: "Listening History",
    icon: IconName::Clock,
    instances: super::Instances::Single,
    home: super::Home::Main,
    build: |services, record, window, cx| {
        Rc::new(cx.new(|cx| History::new(services, record.state(), window, cx)))
    },
};

pub static STATISTICS: PanelType = PanelType {
    key: "disqro.listening-statistics",
    title: "Listening Statistics",
    icon: IconName::AudioLines,
    instances: super::Instances::Single,
    home: super::Home::Main,
    build: |services, record, _, cx| Rc::new(cx.new(|cx| Stats::new(services, record.state(), cx))),
};

fn send(player: &Entity<Player>, command: Command, cx: &mut App) {
    player.update(cx, |player, cx| player.command(command, cx));
}

fn erase(player: &Entity<Player>, row: Option<i64>, cx: &mut App) {
    let (reply, result) = async_channel::bounded(1);
    send(player, Command::EraseListening(row, reply), cx);
    let player = player.clone();
    cx.spawn(async move |cx| {
        if let Ok(Err(error)) = result.recv().await {
            player.update(cx, |_, cx| {
                cx.emit(crate::player::PlayerEvent::Notice(error.to_string()))
            });
        }
    })
    .detach();
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct HistoryPrefs {
    oldest: bool,
    period: Option<Period>,
    from: String,
    through: String,
}

struct History {
    prefs: HistoryPrefs,
    store: Option<Handle>,
    player: Entity<Player>,
    table: Entity<TableState<HistoryTable>>,
    search: Entity<InputState>,
    from: Entity<InputState>,
    through: Entity<InputState>,
    ticket: Ticket,
    pages: Vec<Option<HistoryKey>>,
    more: bool,
    loading: bool,
    visible: bool,
    dirty: bool,
    error: Option<String>,
    dates_open: bool,
}

impl History {
    fn new(
        services: &Services,
        prefs: HistoryPrefs,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table = cx.new(|cx| {
            TableState::new(
                HistoryTable {
                    rows: Vec::new(),
                    player: services.player.clone(),
                    message: "Loading history…".into(),
                },
                w,
                cx,
            )
        });
        cx.subscribe(&table, |_, table, event, cx| {
            if let TableEvent::DoubleClickedRow(row) = *event {
                let d = table.read(cx).delegate();
                let player = d.player.clone();
                let track = d
                    .rows
                    .get(row)
                    .and_then(|r| r.track.as_ref())
                    .map(|r| r.track.clone());
                if let Some(track) = track {
                    send(
                        &player,
                        Command::Play(crate::library::TrackList::from([track]).into(), 0),
                        cx,
                    );
                }
            }
        })
        .detach();
        let search = cx.new(|cx| InputState::new(w, cx).placeholder("Search history"));
        let from = cx.new(|cx| {
            InputState::new(w, cx)
                .placeholder("YYYY-MM-DD")
                .default_value(prefs.from.clone())
        });
        let through = cx.new(|cx| {
            InputState::new(w, cx)
                .placeholder("YYYY-MM-DD")
                .default_value(prefs.through.clone())
        });
        cx.subscribe(&search, |s: &mut Self, _, e, cx| {
            if matches!(e, InputEvent::Change) {
                s.first(cx)
            }
        })
        .detach();
        for input in [&from, &through] {
            cx.subscribe(input, |s: &mut Self, _, e, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    s.custom(cx);
                }
            })
            .detach();
        }
        cx.subscribe(&services.catalog, |s: &mut Self, _, e, cx| {
            if matches!(
                e,
                CatalogEvent::Listening(_)
                    | CatalogEvent::ListeningRemoved
                    | CatalogEvent::Content
                    | CatalogEvent::Structure(_)
            ) {
                s.dirty = true;
                let removed = matches!(e, CatalogEvent::ListeningRemoved);
                if removed {
                    s.table.update(cx, |t, cx| {
                        t.delegate_mut().rows.clear();
                        t.clear_selection(cx);
                        cx.notify();
                    });
                }
                if s.visible && (removed || s.pages.len() == 1 && !s.loading) {
                    s.refresh(cx);
                }
            }
        })
        .detach();
        let dates_open =
            prefs.period.is_none() && (!prefs.from.is_empty() || !prefs.through.is_empty());
        Self {
            store: services.catalog.update(cx, |c, _| c.listening_handle()),
            player: services.player.clone(),
            prefs,
            table,
            search,
            from,
            through,
            ticket: Ticket::default(),
            pages: vec![None],
            more: false,
            loading: false,
            visible: false,
            dirty: true,
            error: None,
            dates_open,
        }
    }
    fn custom(&mut self, cx: &mut Context<Self>) {
        self.prefs.period = None;
        self.prefs.from = self.from.read(cx).value().trim().to_string();
        self.prefs.through = self.through.read(cx).value().trim().to_string();
        self.first(cx);
    }
    fn range(&self) -> anyhow::Result<(i64, i64)> {
        if let Some(period) = self.prefs.period {
            return Ok(period_range(period));
        }
        let date = |v: &str| -> anyhow::Result<Option<chrono::NaiveDate>> {
            if v.is_empty() {
                return Ok(None);
            }
            Ok(Some(
                chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d")
                    .map_err(|_| anyhow::anyhow!("Use YYYY-MM-DD for dates."))?,
            ))
        };
        let from = date(&self.prefs.from)?;
        let through = date(&self.prefs.through)?;
        anyhow::ensure!(
            from.zip(through).is_none_or(|(a, b)| a <= b),
            "End date must be on or after the start date."
        );
        Ok((
            from.map_or(i64::MIN, |day| midnight(&chrono::Local, day)),
            through
                .and_then(|day| day.succ_opt())
                .map_or(i64::MAX, |day| midnight(&chrono::Local, day)),
        ))
    }
    fn first(&mut self, cx: &mut Context<Self>) {
        self.pages.truncate(1);
        self.table.update(cx, |t, cx| {
            t.clear_selection(cx);
            t.scroll_to_row(0, cx);
        });
        self.refresh(cx);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let generation = self.ticket.next();
        self.loading = false;
        self.dirty = false;
        self.error = None;
        let range = match self.range() {
            Ok(range) => range,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let Some(store) = self.store.clone() else {
            self.error = Some("Listening history is unavailable.".into());
            cx.notify();
            return;
        };
        let search = self.search.read(cx).value().to_string();
        let filtered = !search.trim().is_empty() || range != (i64::MIN, i64::MAX);
        let cursor = *self.pages.last().unwrap();
        let oldest = self.prefs.oldest;
        self.loading = true;
        let result = store.query(&self.ticket, generation, move |s| {
            s.history_page(range, &search, oldest, cursor)
        });
        cx.spawn(async move |this, cx| {
            let result = result.await;
            let _ = this.update(cx, |s, cx| {
                if !s.ticket.is_current(generation) {
                    return;
                }
                s.loading = false;
                match result {
                    Ok(mut rows) => {
                        s.more = rows.len() > HISTORY_PAGE;
                        rows.truncate(HISTORY_PAGE);
                        if rows.is_empty() && s.pages.len() > 1 {
                            s.pages.pop();
                            s.refresh(cx);
                            return;
                        }
                        s.table.update(cx, |t, cx| {
                            let selected = t
                                .selected_row()
                                .and_then(|i| t.delegate().rows.get(i))
                                .map(|r| r.id);
                            let target =
                                selected.and_then(|id| rows.iter().position(|r| r.id == id));
                            let d = t.delegate_mut();
                            d.rows = rows;
                            d.message = if !filtered {
                                "No listening history yet."
                            } else {
                                "No listens match these filters."
                            }
                            .into();
                            if let Some(row) = target {
                                t.set_selected_row(row, cx);
                            } else {
                                t.clear_selection(cx);
                            }
                            cx.notify();
                        });
                    }
                    Err(error) if !crate::store::is_cancelled(&error) => {
                        s.error = Some(format!("{error:#}"))
                    }
                    Err(_) => {}
                }
                cx.notify();
                if s.dirty && s.visible && s.pages.len() == 1 {
                    s.refresh(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn delete(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        erase(&self.player, id, cx);
        self.first_after_delete(cx);
    }
    fn first_after_delete(&mut self, cx: &mut Context<Self>) {
        // A change notification follows the transaction and refreshes page 1.
        self.pages.truncate(1);
        self.table.update(cx, |t, cx| t.scroll_to_row(0, cx));
    }
}

impl Render for History {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let current = self.prefs.period.unwrap_or(Period::All);
        let toolbar = h_flex()
            .flex_wrap()
            .gap(px(5.))
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(m.line)
            .child(
                div().w(px(155.)).child(
                    Input::new(&self.search)
                        .aria_label("Search history")
                        .with_size(gpui_kit::component::Size::Small),
                ),
            )
            .children(Period::OPTIONS.into_iter().map(|period| {
                Button::new(period.label())
                    .ghost()
                    .small()
                    .label(period.label())
                    .selected(
                        current == period
                            && (self.prefs.period.is_some()
                                || self.prefs.from.is_empty() && self.prefs.through.is_empty()),
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.prefs.period = Some(period);
                        s.first(cx);
                    }))
            }))
            .child(
                Button::new("history-dates")
                    .ghost()
                    .small()
                    .label("Dates…")
                    .selected(self.dates_open)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.dates_open = !s.dates_open;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("history-sort")
                    .ghost()
                    .small()
                    .label(if self.prefs.oldest {
                        "Oldest first"
                    } else {
                        "Newest first"
                    })
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.prefs.oldest = !s.prefs.oldest;
                        s.first(cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("history-menu")
                    .ghost()
                    .small()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("History menu")
                    .tooltip("History menu")
                    .dropdown_menu({
                        let history = cx.weak_entity();
                        move |menu, _, _| {
                            let history = history.clone();
                            menu.item(PopupMenuItem::new("Clear History…").on_click(move |_, w, cx| {
                                let history = history.clone();
                                w.open_alert_dialog(cx, move |alert, _, _| {
                                    let history = history.clone();
                                    alert.title("Clear listening history?")
                                        .description("All listening entries, play counts and listening statistics will be deleted.")
                                        .show_cancel(true)
                                        .ok_text("Clear History")
                                        .on_ok(move |_, _, cx| {
                                            let _ = history.update(cx, |s, cx| s.delete(None, cx));
                                            true
                                        })
                                });
                            }))
                        }
                    }),
            );
        let dates = h_flex()
            .flex_wrap()
            .gap(px(5.))
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(m.line)
            .child(div().text_color(m.text_muted).child("From"))
            .child(
                div().w(px(112.)).child(
                    Input::new(&self.from)
                        .aria_label("From date")
                        .with_size(gpui_kit::component::Size::Small),
                ),
            )
            .child(div().text_color(m.text_muted).child("Through"))
            .child(
                div().w(px(112.)).child(
                    Input::new(&self.through)
                        .aria_label("Through date")
                        .with_size(gpui_kit::component::Size::Small),
                ),
            )
            .child(
                Button::new("history-apply")
                    .ghost()
                    .small()
                    .label("Apply dates")
                    .on_click(cx.listener(|s, _, _, cx| s.custom(cx))),
            );
        let footer = h_flex()
            .px_2()
            .py_1()
            .gap(px(4.))
            .border_t_1()
            .border_color(m.line)
            .child(
                Button::new("history-previous")
                    .ghost()
                    .small()
                    .label("Previous")
                    .disabled(self.pages.len() == 1 || self.loading)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.pages.pop();
                        s.table.update(cx, |t, cx| {
                            t.clear_selection(cx);
                            t.scroll_to_row(0, cx);
                        });
                        s.refresh(cx);
                    })),
            )
            .child(
                Button::new("history-next")
                    .ghost()
                    .small()
                    .label("Next")
                    .disabled(!self.more || self.loading)
                    .on_click(cx.listener(|s, _, _, cx| {
                        if let Some(key) =
                            s.table.read(cx).delegate().rows.last().map(HistoryRow::key)
                        {
                            s.pages.push(Some(key));
                            s.table.update(cx, |t, cx| {
                                t.clear_selection(cx);
                                t.scroll_to_row(0, cx);
                            });
                            s.refresh(cx);
                        }
                    })),
            );
        v_flex()
            .size_full()
            .min_w_0()
            .bg(m.canvas)
            .text_size(px(size::TEXT))
            .child(toolbar)
            .when(self.dates_open, |d| d.child(dates))
            .children(
                self.error
                    .as_ref()
                    .map(|e| div().px_3().py_1().text_color(m.warning).child(e.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .key_context("ListeningHistory")
                    .on_action(cx.listener(|s, _: &PlayListen, _, cx| {
                        let table = s.table.read(cx);
                        let track = table
                            .selected_row()
                            .and_then(|i| table.delegate().rows.get(i))
                            .and_then(|row| row.track.as_ref())
                            .map(|row| row.track.clone());
                        if let Some(track) = track {
                            send(
                                &s.player,
                                Command::Play(crate::library::TrackList::from([track]).into(), 0),
                                cx,
                            );
                        }
                    }))
                    .on_action(cx.listener(|s, _: &RemoveListen, _, cx| {
                        let table = s.table.read(cx);
                        if let Some(id) = table
                            .selected_row()
                            .and_then(|ix| table.delegate().rows.get(ix))
                            .map(|r| r.id)
                        {
                            erase(&s.player, Some(id), cx);
                        }
                    }))
                    .child(
                        DataTable::new(&self.table)
                            .accessibility_label("Listening occurrences")
                            .bordered(false)
                            .stripe(false)
                            .header_row_height(px(size::HEADER))
                            .with_size(gpui_kit::component::Size::Size(px(28.))),
                    ),
            )
            .child(footer)
    }
}

struct HistoryTable {
    rows: Vec<HistoryRow>,
    player: Entity<Player>,
    message: SharedString,
}
impl HistoryTable {
    fn label(&self, row: usize, col: usize) -> String {
        let r = &self.rows[row];
        match col {
            0 if r.track.is_none() => format!("{} · Unavailable", r.title),
            0 => r.title.clone(),
            1 => local_date(r.started_at, "%Y-%m-%d %H:%M"),
            2 => duration(r.credited_us),
            3 => {
                if r.qualified {
                    "1 play".into()
                } else {
                    "0 plays".into()
                }
            }
            4 => r.artist.clone(),
            _ => r.album.clone(),
        }
    }
}
impl TableDelegate for HistoryTable {
    fn columns_count(&self, _: &App) -> usize {
        6
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn column(&self, col: usize, _: &App) -> Column {
        let (name, width) = [
            ("Recording", 220.),
            ("Started", 145.),
            ("Listened", 79.),
            ("Counted", 78.),
            ("Artist", 145.),
            ("Album", 160.),
        ][col];
        Column::new(name, name).width(px(width))
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .text_color(materials(cx).text_muted)
            .child(
                div()
                    .id("history-empty")
                    .role(Role::Label)
                    .aria_label(self.message.clone())
                    .child(self.message.clone()),
            )
    }
    fn cell_label(&self, row: usize, col: usize, _: &App) -> Option<SharedString> {
        Some(self.label(row, col).into())
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let m = materials(cx);
        div()
            .min_w_0()
            .truncate()
            .text_color(if col == 0 { m.text } else { m.text_muted })
            .child(self.label(row, col))
    }
    fn context_menu(
        &mut self,
        row: usize,
        mut menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(row) = self.rows.get(row) else {
            return menu;
        };
        if let Some(track) = row.track.as_ref() {
            use crate::queue::Selection;
            for (label, command) in [
                (
                    "Play",
                    (|selection| Command::Play(selection, 0)) as fn(Selection) -> Command,
                ),
                ("Play Next", Command::PlayNext),
                ("Add to Queue", Command::Enqueue),
            ] {
                let track = track.track.clone();
                let player = self.player.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    send(
                        &player,
                        command(crate::library::TrackList::from([track.clone()]).into()),
                        cx,
                    );
                }));
            }
            menu = menu.separator();
        }
        let id = row.id;
        let player = self.player.clone();
        menu.item(
            PopupMenuItem::new("Remove Listen")
                .icon(IconName::Trash)
                .on_click(move |_, _, cx| {
                    erase(&player, Some(id), cx);
                }),
        )
    }
}

impl PaneContent for Entity<History> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).table.read(cx).focus_handle(cx)
    }
    fn title(&self, _: &App) -> SharedString {
        HISTORY.title.into()
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(&self.read(cx).prefs).unwrap_or_default()
    }
    fn set_presented(&self, visible: bool, cx: &mut App) {
        self.update(cx, |s, cx| {
            s.visible = visible;
            if !visible && s.loading {
                s.ticket.next();
                s.loading = false;
                s.dirty = true;
            }
            if visible && s.dirty && !s.loading {
                s.refresh(cx);
            }
        });
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct StatsPrefs {
    period: Period,
    by_time: bool,
}

struct Stats {
    prefs: StatsPrefs,
    store: Option<Handle>,
    router: Entity<Router>,
    ticket: Ticket,
    focus: FocusHandle,
    state: Option<(StatsPrefs, Statistics)>,
    error: Option<String>,
    visible: bool,
    busy: bool,
    dirty: bool,
    last_refresh: Option<std::time::Instant>,
    scheduled: bool,
}
impl Stats {
    fn new(services: &Services, prefs: StatsPrefs, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&services.catalog, |s: &mut Self, _, e, cx| match e {
            CatalogEvent::ListeningRemoved => {
                s.state = None;
                s.dirty = true;
                if s.visible {
                    s.refresh(cx);
                }
            }
            CatalogEvent::Listening(changes) if changes.iter().all(|c| !c.removed) => {
                s.invalidate(cx);
            }
            _ => {}
        })
        .detach();
        Self {
            prefs,
            store: services.catalog.update(cx, |c, _| c.statistics_handle()),
            router: services.router.clone(),
            ticket: Ticket::default(),
            focus: cx.focus_handle(),
            state: None,
            error: None,
            visible: false,
            busy: false,
            dirty: true,
            last_refresh: None,
            scheduled: false,
        }
    }
    fn select(&mut self, prefs: StatsPrefs, cx: &mut Context<Self>) {
        if prefs != self.prefs {
            self.prefs = prefs;
            self.state = None;
            self.refresh(cx);
        }
    }

    fn reveal_recording(&self, id: TrackId, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(browser) = self.router.read(cx).active() {
            browser.update(cx, |b, cx| b.reveal_recording(id, &self.router, w, cx));
        } else {
            w.push_notification("Open a Music Browser to show this recording.", cx);
        }
    }
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        if !self.visible || self.busy {
            return;
        }
        let delay = self.last_refresh.map_or(std::time::Duration::ZERO, |at| {
            std::time::Duration::from_secs(10).saturating_sub(at.elapsed())
        });
        if delay.is_zero() {
            self.refresh(cx);
        } else if !self.scheduled {
            self.scheduled = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |s, cx| {
                    s.scheduled = false;
                    if s.visible && s.dirty && !s.busy {
                        s.refresh(cx);
                    }
                });
            })
            .detach();
        }
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        let Some(store) = self.store.clone() else {
            self.error = Some("Listening statistics are unavailable.".into());
            cx.notify();
            return;
        };
        let generation = self.ticket.next();
        let prefs = self.prefs;
        let StatsPrefs { period, by_time } = prefs;
        self.error = None;
        self.busy = true;
        self.last_refresh = Some(std::time::Instant::now());
        let result = store.query(&self.ticket, generation, move |s| {
            s.listening_statistics(period, by_time)
        });
        cx.spawn(async move |this, cx| {
            let result = result.await;
            let _ = this.update(cx, |s, cx| {
                if !s.ticket.is_current(generation) {
                    return;
                }
                s.busy = false;
                match result {
                    Ok(stats) => {
                        s.state = Some((prefs, stats));
                        s.error = None;
                    }
                    Err(e) => s.error = Some(format!("{e:#}")),
                }
                cx.notify();
                if s.dirty {
                    s.invalidate(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for Stats {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let periods = h_flex()
            .gap(px(5.))
            .flex_wrap()
            .children(Period::OPTIONS.map(|period| {
                Button::new(period.label())
                    .ghost()
                    .small()
                    .label(period.label())
                    .selected(self.prefs.period == period)
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.select(StatsPrefs { period, ..s.prefs }, cx);
                    }))
            }));
        let rank_selector = h_flex()
            .flex_wrap()
            .gap(px(5.))
            .child(div().text_color(m.text_muted).child("Rank by"))
            .children(
                [(false, "Plays"), (true, "Listening Time")].map(|(by_time, name)| {
                    Button::new(name)
                        .ghost()
                        .small()
                        .label(name)
                        .selected(self.prefs.by_time == by_time)
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.select(StatsPrefs { by_time, ..s.prefs }, cx);
                        }))
                }),
            );
        let content = if let Some(error) = &self.error {
            div()
                .px_3()
                .py_2()
                .text_color(m.warning)
                .child(error.clone())
                .into_any_element()
        } else if let Some((prefs, stats)) = &self.state
            && *prefs == self.prefs
        {
            if stats.totals.credited_us == 0 && stats.totals.plays == 0 {
                div()
                    .id("statistics-empty")
                    .role(Role::Label)
                    .aria_label("No listening activity in this period.")
                    .px_3()
                    .py_2()
                    .text_color(m.text_muted)
                    .child("No listening activity in this period.")
                    .into_any_element()
            } else {
                let summary = h_flex().flex_wrap().gap(px(12.)).children(
                    [
                        ("Listening time", long_duration(stats.totals.credited_us)),
                        ("Qualified plays", grouped_count(stats.totals.plays)),
                        ("Recordings", grouped_count(stats.recordings)),
                        ("Artists", grouped_count(stats.artists)),
                        ("Releases", grouped_count(stats.releases)),
                    ]
                    .map(|(label, value)| {
                        v_flex()
                            .gap(px(2.))
                            .id(label)
                            .role(Role::Group)
                            .aria_label(format!("{label}, {value}"))
                            .child(div().text_size(px(18.)).child(value))
                            .child(div().text_color(m.text_muted).child(label))
                    }),
                );
                v_flex()
                    .gap(px(10.))
                    .p_3()
                    .child(summary)
                    .child(activity(&stats.days, cx))
                    .child(ranking("Recordings", &stats.tracks, prefs.by_time, cx))
                    .child(ranking("Artists", &stats.performers, prefs.by_time, cx))
                    .child(ranking("Releases", &stats.albums, prefs.by_time, cx))
                    .into_any_element()
            }
        } else {
            div()
                .px_3()
                .py_2()
                .text_color(m.text_muted)
                .child("Loading statistics…")
                .into_any_element()
        };
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .bg(m.canvas)
            .text_size(px(size::TEXT))
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap(px(5.))
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(m.line)
                    .child(periods)
                    .child(rank_selector),
            )
            .child(
                div()
                    .id("statistics-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        self.ticket.next();
    }
}

fn grouped_count(value: i64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.bytes().enumerate() {
        if i != 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(char::from(digit));
    }
    grouped
}

fn long_duration(us: i64) -> String {
    let minutes = us / 60_000_000;
    match minutes {
        0 => duration(us),
        1..60 => {
            let seconds = us / 1_000_000 % 60;
            if seconds == 0 {
                format!("{minutes} m")
            } else {
                format!("{minutes} m {seconds} s")
            }
        }
        _ => {
            let hours = grouped_count(minutes / 60);
            let minutes = minutes % 60;
            if minutes == 0 {
                format!("{hours} h")
            } else {
                format!("{hours} h {minutes} m")
            }
        }
    }
}

fn exact_duration(us: i64) -> String {
    let seconds = us / 1_000_000;
    let fraction = us % 1_000_000;
    let integral = format!(
        "{}h {:02}m {:02}",
        grouped_count(seconds / 3600),
        seconds / 60 % 60,
        seconds % 60
    );
    if fraction == 0 {
        format!("{integral}s")
    } else {
        let fraction = format!("{fraction:06}");
        format!("{integral}.{}s", fraction.trim_end_matches('0'))
    }
}

fn activity(days: &[(chrono::NaiveDate, i64)], cx: &App) -> impl IntoElement {
    let m = *materials(cx);
    let Some(&(first, _)) = days.first() else {
        return div();
    };
    let &(last, _) = days.last().unwrap();
    let first = first.min(last - chrono::Days::new(6));
    let span = (last - first).num_days() as usize + 1;
    let per_bar = [1, 7, 28, 91, 364]
        .into_iter()
        .find(|interval| span.div_ceil(*interval) <= 31)
        .unwrap_or_else(|| span.div_ceil(31).div_ceil(364) * 364);
    let mut bars = vec![0; span.div_ceil(per_bar)];
    for (day, time) in days {
        bars[(*day - first).num_days() as usize / per_bar] += time;
    }
    let peak = bars.iter().copied().max().unwrap_or(0);
    let unit = match peak {
        0..=59_999_999 => 1_000_000,
        60_000_000..=3_599_999_999 => 60_000_000,
        _ => 3_600_000_000,
    };
    let magnitude = ((peak / unit).max(1) as u64).ilog10();
    let step = unit * 10_i64.pow(magnitude);
    let scale = [1, 2, 5, 10]
        .into_iter()
        .map(|n| n * step)
        .find(|n| *n >= peak)
        .unwrap();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pt_2()
        .border_t_1()
        .border_color(m.line)
        .child(
            h_flex()
                .justify_between()
                .child(if per_bar == 1 {
                    "Daily listening".to_string()
                } else {
                    format!("Listening per {per_bar} days")
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(m.text_muted)
                        .child(long_duration(scale)),
                ),
        )
        .child(
            h_flex()
                .h(px(67.))
                .w_full()
                .items_end()
                .gap(px(if bars.len() > 14 { 1. } else { 3. }))
                .border_t_1()
                .border_b_1()
                .border_color(m.line)
                .children(bars.into_iter().enumerate().map(|(i, value)| {
                    let at = first + chrono::Days::new((i * per_bar) as u64);
                    let end = (at + chrono::Days::new(per_bar as u64 - 1)).min(last);
                    let label = if at == end {
                        at.format("%-d %b %Y").to_string()
                    } else {
                        format!("{} – {}", at.format("%-d %b %Y"), end.format("%-d %b %Y"))
                    };
                    div()
                        .id(("listening-bar", i))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_end()
                        .justify_center()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(24.))
                                .h(px(if value == 0 {
                                    0.
                                } else {
                                    (65. * value as f32 / scale as f32).max(1.)
                                }))
                                .bg(m.accent),
                        )
                        .tooltip(move |w, cx| {
                            Tooltip::new(format!("{label} · {}", exact_duration(value)))
                                .build(w, cx)
                        })
                })),
        )
        .child(
            h_flex()
                .justify_between()
                .gap(px(4.))
                .text_color(m.text_muted)
                .child(
                    div()
                        .truncate()
                        .child(first.format("%-d %b %Y").to_string()),
                )
                .when(last != first, |d| {
                    d.child(div().truncate().child(last.format("%-d %b %Y").to_string()))
                }),
        )
}

fn ranking(
    title: &'static str,
    ranks: &[Rank],
    by_time: bool,
    cx: &mut Context<Stats>,
) -> impl IntoElement {
    let m = *materials(cx);
    let show_actions = ranks.iter().any(|rank| rank.track.is_some());
    v_flex()
        .child(
            h_flex()
                .justify_between()
                .py_1()
                .border_b_1()
                .border_color(m.line)
                .child(title)
                .child(
                    div()
                        .text_color(m.text_muted)
                        .when(show_actions, |d| d.pr(px(32.)))
                        .child(if by_time { "Time" } else { "Plays" }),
                ),
        )
        .when(ranks.is_empty(), |d| {
            d.child(div().py_1().text_color(m.text_muted).child(if by_time {
                "No ranked listening time."
            } else {
                "No qualified plays."
            }))
        })
        .children(ranks.iter().enumerate().map(|(i, rank)| {
            let value = if by_time {
                long_duration(rank.credited_us)
            } else {
                grouped_count(rank.plays)
            };
            h_flex()
                .min_h(px(if rank.detail.is_empty() { 24. } else { 34. }))
                .gap(px(10.))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(rank.name.clone()))
                        .when(!rank.detail.is_empty(), |d| {
                            d.child(
                                div()
                                    .truncate()
                                    .text_color(m.text_muted)
                                    .child(rank.detail.clone()),
                            )
                        }),
                )
                .child(div().flex_shrink_0().text_right().child(value))
                .when_some(rank.track, |row, id| {
                    row.child(
                        Button::new(("reveal-ranked-recording", i))
                            .ghost()
                            .xsmall()
                            .w(px(22.))
                            .icon(IconName::FolderOpen)
                            .accessibility_label(format!("Show {} in Music Browser", rank.name))
                            .tooltip("Show in Music Browser")
                            .on_click(
                                cx.listener(move |s, _, w, cx| s.reveal_recording(id, w, cx)),
                            ),
                    )
                })
                .when(show_actions && rank.track.is_none(), |row| {
                    row.child(div().w(px(22.)).flex_shrink_0())
                })
        }))
}

impl PaneContent for Entity<Stats> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus.clone()
    }
    fn title(&self, _: &App) -> SharedString {
        STATISTICS.title.into()
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(self.read(cx).prefs).unwrap_or_default()
    }
    fn set_presented(&self, visible: bool, cx: &mut App) {
        self.update(cx, |s, cx| {
            s.visible = visible;
            if !visible && s.busy {
                s.ticket.next();
                s.busy = false;
                s.dirty = true;
            }
            if visible && s.dirty && !s.busy {
                s.refresh(cx);
            }
        });
    }
}

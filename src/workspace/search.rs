//! A transient library search session. Its only effects on the workspace
//! come from an explicit open or play action.
use super::*;
use crate::browser::Location;
use crate::store::{
    Ticket,
    search::{Hit, Matches, Target},
};
use gpui_kit::component::input::{Input, InputEvent, InputState};

const PREVIEW: usize = 5;
const PAGE: usize = 20;

pub(super) struct LibrarySearch {
    workspace: WeakEntity<Workspace>,
    catalog: Entity<Catalog>,
    input: Entity<InputState>,
    ticket: Ticket,
    rows: Matches,
    selected: usize,
    page: Option<usize>,
    status: String,
    scroll: ScrollHandle,
    bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}

impl Drop for LibrarySearch {
    fn drop(&mut self) {
        self.ticket.next();
    }
}

impl Workspace {
    pub(super) fn open_search(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.search_visible = true;
        self.search.update(cx, |s, cx| {
            s.input.update(cx, |i, cx| {
                i.focus(w, cx);
                i.select_all(w, cx);
            });
            s.query(true, cx);
        });
        cx.notify();
    }
    pub(super) fn open_filter(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let browser = self.ensure_browser(w, cx);
        self.reveal_active_browser(w, cx);
        self.hide_suggestions(cx);
        browser.update(cx, |b, cx| {
            b.begin_navigation(cx);
            b.set_filter_open(true, w, cx)
        });
        cx.notify();
    }
    pub(super) fn hide_suggestions(&mut self, cx: &mut Context<Self>) {
        self.search_visible = false;
        self.search.update(cx, |s, _| {
            s.ticket.next();
            s.page = None;
        });
        cx.notify();
    }
    /// Library search: always the whole library, so the field never changes
    /// meaning. Each browser filters its own rows in its own bar.
    pub(super) fn search_field(&self, w: &Window, cx: &Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let narrow = w.viewport_size().width < px(860.);
        let input = self.search.read(cx).input.clone();
        let focused = input.read(cx).focus_handle(cx).is_focused(w);
        let empty = input.read(cx).value().is_empty();
        let popup = self.search_visible && !self.search.read(cx).full_results() && !empty;
        h_flex()
            .id("shell-search")
            .occlude()
            .relative()
            .flex_shrink_0()
            .w(px(if narrow { 180. } else { 320. }))
            .h(px(crate::location::NAV_FIELD_HEIGHT))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(8.))
            .items_center()
            .rounded(px(crate::theme::size::RADIUS))
            .border_1()
            .map(|d| {
                if focused {
                    d.bg(m.canvas).border_color(m.accent.opacity(0.6))
                } else {
                    // A hairline at rest: findable without a filled block in
                    // the title bar.
                    d.border_color(m.line)
                        .hover(|d| d.bg(m.field()).border_color(m.control_line))
                }
            })
            .child(
                Icon::new(IconName::Search)
                    .size(px(14.))
                    .flex_shrink_0()
                    .text_color(m.text_faint),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&input)
                        .small()
                        .appearance(false)
                        .cleanable(true)
                        .aria_label("Search library"),
                ),
            )
            .when(empty && !focused && !narrow, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .pr(px(4.))
                        .text_size(px(crate::theme::size::LABEL))
                        .text_color(m.text_faint)
                        .child("Ctrl+K"),
                )
            })
            .capture_key_down(cx.listener(move |s, e: &KeyDownEvent, w, cx| {
                if e.keystroke.modifiers.modified() {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "escape" => {
                        s.hide_suggestions(cx);
                        if let Some(b) = s.browser(cx) {
                            b.read(cx).list_focus(cx).focus(w, cx);
                        }
                    }
                    key @ ("up" | "down") if s.search_visible => {
                        s.search.update(cx, |search, cx| {
                            if key == "up" {
                                search.selected = search.selected.saturating_sub(1);
                            } else {
                                search.selected = (search.selected + 1)
                                    .min(search.rows.hits.len().saturating_sub(1));
                            }
                            search.scroll.scroll_to_item(search.selected);
                            cx.notify();
                        })
                    }
                    _ => return,
                }
                w.prevent_default();
                cx.stop_propagation();
            }))
            .when(popup, |d| {
                d.child(
                    deferred(
                        div()
                            .absolute()
                            .top(px(crate::location::NAV_FIELD_HEIGHT + 4.))
                            .right(px(-1.))
                            .w(px(380.))
                            .child(self.search.clone()),
                    )
                    .with_priority(2),
                )
            })
            .on_mouse_down_out(cx.listener(|s, e: &MouseDownEvent, _, cx| {
                if !s
                    .search
                    .read(cx)
                    .bounds
                    .get()
                    .is_some_and(|b| s.search_visible && b.contains(&e.position))
                    && !s.search.read(cx).full_results()
                    && s.search_visible
                {
                    s.hide_suggestions(cx);
                }
            }))
            .into_any_element()
    }
}
impl LibrarySearch {
    pub(super) fn new(
        workspace: WeakEntity<Workspace>,
        catalog: Entity<Catalog>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(w, cx).placeholder("Search library"));
        cx.subscribe_in(&input, w, |s: &mut Self, _, event, w, cx| match event {
            InputEvent::Focus | InputEvent::Change => {
                let _ = s.workspace.update(cx, |ws, cx| {
                    ws.search_visible = true;
                    cx.notify();
                });
                s.page = None;
                s.query(false, cx);
            }
            InputEvent::PressEnter { .. } => s.open(s.selected, w, cx),
            _ => {}
        })
        .detach();
        cx.subscribe(&catalog, |s: &mut Self, _, event, cx| {
            if matches!(event, CatalogEvent::Changed | CatalogEvent::Content)
                && s.workspace
                    .upgrade()
                    .is_some_and(|w| w.read(cx).search_visible)
            {
                s.query(true, cx);
            }
        })
        .detach();
        Self {
            workspace,
            catalog,
            input,
            ticket: Ticket::default(),
            rows: Matches::default(),
            selected: 0,
            page: None,
            status: String::new(),
            scroll: ScrollHandle::new(),
            bounds: Default::default(),
        }
    }
    fn query(&mut self, preserve: bool, cx: &mut Context<Self>) {
        let generation = self.ticket.next();
        let text = self.input.read(cx).value().trim().to_owned();
        let selection = self
            .rows
            .hits
            .get(self.selected)
            .filter(|_| preserve)
            .map(|h| h.target.clone());
        self.rows = Matches::default();
        self.selected = 0;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.status.clear();
        cx.notify();
        if text.is_empty() {
            return;
        }
        let Some(store) = self.catalog.read(cx).handle() else {
            self.status = "Library unavailable".into();
            return;
        };
        self.status = "Searching…".into();
        let limit = self.page.map_or(PREVIEW, |_| PAGE);
        let offset = self.page.unwrap_or(0) * limit;
        let ticket = self.ticket.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(65))
                .await;
            if !ticket.is_current(generation) {
                return;
            }
            let result = store
                .query(&ticket, generation, move |s| {
                    s.search_library(&text, limit, offset)
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                if !ticket.is_current(generation) {
                    return;
                }
                match result {
                    Ok(mut rows) => {
                        if s.page.is_none() && rows.hits.len() > PREVIEW {
                            let mut groups = std::collections::HashSet::new();
                            let mut keep = vec![false; rows.hits.len()];
                            let mut available = PREVIEW;
                            for (hit, slot) in rows.hits.iter().zip(&mut keep) {
                                if available > 0 && groups.insert(hit.target.group()) {
                                    *slot = true;
                                    available -= 1;
                                }
                            }
                            for slot in keep.iter_mut().filter(|v| !**v).take(available) {
                                *slot = true;
                            }
                            rows.hits = rows
                                .hits
                                .into_iter()
                                .zip(keep)
                                .filter_map(|(h, keep)| keep.then_some(h))
                                .collect();
                            rows.more = true;
                        }
                        s.status = if rows.hits.is_empty() {
                            "No results".into()
                        } else {
                            String::new()
                        };
                        s.selected = rows
                            .hits
                            .iter()
                            .position(|h| Some(&h.target) == selection.as_ref())
                            .unwrap_or(0);
                        s.rows = rows;
                        s.scroll.scroll_to_item(s.selected);
                    }
                    Err(e) => s.status = format!("Library search failed · {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn full_results(&self) -> bool {
        self.page.is_some()
    }
    fn close(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.page = None;
        self.ticket.next();
        let _ = self.workspace.update(cx, |ws, cx| {
            ws.search_visible = false;
            if let Some(b) = ws.browser(cx) {
                b.read(cx).list_focus(cx).focus(w, cx);
            }
            cx.notify();
        });
    }
    fn open(&mut self, index: usize, w: &mut Window, cx: &mut Context<Self>) {
        let Some(Hit { target, title, .. }) = self.rows.hits.get(index).cloned() else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(browser) = workspace.read(cx).browser(cx) else {
            self.status = "Open a Music Browser to show this result".into();
            cx.notify();
            return;
        };
        self.ticket.next();
        self.page = None;
        workspace.update(cx, |ws, cx| {
            ws.search_visible = false;
            ws.reveal_active_browser(w, cx);
            cx.notify();
        });
        browser.update(cx, |b, cx| match target {
            Target::Track(id) => b.show_search_track(id, w, cx),
            Target::Artist(id) => b.show(Location::Artist { id, name: title }, w, cx),
            Target::Album(root, id) => b.show(
                Location::Album {
                    root: Some(root),
                    id,
                    title,
                },
                w,
                cx,
            ),
            Target::Release(root, id) => b.show(
                Location::Release {
                    root: Some(root),
                    id,
                    title,
                },
                w,
                cx,
            ),
            Target::Collection(id) => b.show(Location::Library(Some(id)), w, cx),
            Target::Playlist(id) => b.show(Location::Playlist(id), w, cx),
        });
        browser.read(cx).list_focus(cx).focus(w, cx);
    }
    fn row(&self, index: usize, hit: &Hit, cx: &Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let group = hit.target.group();
        let header = self.full_results()
            && (index == 0 || self.rows.hits[index - 1].target.group() != group);
        let icon = match hit.target {
            Target::Track(_) => IconName::Music,
            Target::Artist(_) => IconName::User,
            Target::Album(..) | Target::Release(..) => IconName::Disc3,
            Target::Playlist(_) => IconName::ListMusic,
            Target::Collection(_) => IconName::Library,
        };
        v_flex()
            .flex_shrink_0()
            .when(header, |d| {
                d.child(
                    div()
                        .id(("result-heading", index))
                        .role(Role::Heading)
                        .aria_label(group)
                        .px(px(8.))
                        .pt(px(8.))
                        .pb(px(3.))
                        .text_size(px(crate::theme::size::LABEL))
                        .text_color(m.text_faint)
                        .child(group),
                )
            })
            .child(
                h_flex()
                    .rounded(px(4.))
                    .items_center()
                    .when(self.selected == index, |d| d.bg(m.selected))
                    .child(
                        h_flex()
                            .id(("search-hit", index))
                            .role(Role::ListBoxOption)
                            .aria_label(hit.title.clone())
                            .aria_description(hit.detail.clone())
                            .aria_selected(self.selected == index)
                            .when(!hit.detail.is_empty(), |d| {
                                d.tooltip({
                                    let detail = hit.detail.clone();
                                    move |w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                                            .build(w, cx)
                                    }
                                })
                            })
                            .flex_1()
                            .min_w_0()
                            .px(px(8.))
                            .py(px(4.))
                            .gap(px(8.))
                            .cursor_pointer()
                            .hover(|d| d.bg(m.hover))
                            .on_click(cx.listener(move |s, _, w, cx| s.open(index, w, cx)))
                            .child(Icon::new(icon).size(px(15.)).text_color(m.text_muted))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(crate::theme::size::TEXT))
                                            .child(hit.title.clone()),
                                    )
                                    .when(!hit.context.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .truncate()
                                                .text_size(px(crate::theme::size::LABEL))
                                                .text_color(m.text_muted)
                                                .child(hit.context.clone()),
                                        )
                                    }),
                            ),
                    )
                    .when_some(
                        match hit.target {
                            Target::Track(id) => Some(id),
                            _ => None,
                        },
                        |d, id| {
                            d.child(
                                Button::new(("play-search-hit", index))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Play)
                                    .accessibility_label(format!("Play {}", hit.title))
                                    .tooltip("Play from search results")
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        if let Some(workspace) = s.workspace.upgrade() {
                                            let player = workspace.read(cx).services.player.clone();
                                            crate::table::play_library(
                                                &s.catalog,
                                                &player,
                                                crate::catalog::ListeningList::Search(
                                                    s.input.read(cx).value().to_string(),
                                                ),
                                                Some(id),
                                                Command::Play,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
}
impl Render for LibrarySearch {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let size = w.viewport_size();
        let rows = &self.rows.hits;
        let no_browser = self
            .workspace
            .upgrade()
            .is_some_and(|s| s.read(cx).browser(cx).is_none());
        let full = self.full_results();
        let query = self.input.read(cx).value().trim().to_string();
        let bounds = self.bounds.clone();
        v_flex()
            .id("library-results")
            .on_prepaint(move |b, _, _| bounds.set(Some(b)))
            .capture_key_down(cx.listener(|s, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    s.close(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .occlude()
            .when(full, |d| {
                d.size_full().px(px(18.)).pt(px(10.)).child(
                    h_flex()
                        .h(px(crate::theme::size::HEADER + 8.))
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("Library results for “{query}”")),
                        )
                        .child(
                            Button::new("close-results")
                                .ghost()
                                .small()
                                .label("Done")
                                .on_click(cx.listener(|s, _, w, cx| s.close(w, cx))),
                        ),
                )
            })
            .when(!full, |d| {
                d.w_full()
                    .max_h((size.height - px(70.)).min(px(315.)))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(m.control_line)
                    .bg(m.overlay)
                    .p(px(4.))
            })
            .when(!self.status.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(10.))
                        .text_size(px(crate::theme::size::LABEL))
                        .text_color(m.text_muted)
                        .child(self.status.clone()),
                )
            })
            .when(no_browser, |d| {
                d.child(
                    Button::new("search-open-browser")
                        .ghost()
                        .small()
                        .label("Open Music Browser")
                        .on_click(cx.listener(|s, _, w, cx| {
                            let _ = s.workspace.update(cx, |ws, cx| {
                                ws.ensure_browser(w, cx);
                            });
                            s.input.read(cx).focus_handle(cx).focus(w, cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                v_flex()
                    .id("search-results")
                    .role(Role::ListBox)
                    .aria_label("Library results")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows.iter().enumerate().map(|(i, h)| self.row(i, h, cx))),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap(px(6.))
                    // The way to every result is a footer, apart from the suggestions.
                    .when(!full && self.rows.more, |d| {
                        d.mt(px(4.)).pt(px(4.)).border_t_1().border_color(m.line)
                    })
                    .when(full, |d| d.py(px(8.)))
                    .when(self.page.is_some_and(|p| p > 0), |d| {
                        d.child(
                            Button::new("search-prev")
                                .small()
                                .ghost()
                                .label("Previous results")
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.page = s.page.map(|p| p.saturating_sub(1));
                                    s.query(false, cx);
                                })),
                        )
                    })
                    .when(self.rows.more, |d| {
                        d.child(
                            Button::new("search-more")
                                .small()
                                .ghost()
                                .label(if self.page.is_none() {
                                    "View all results"
                                } else {
                                    "Next results"
                                })
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.page = Some(s.page.map_or(0, |p| p + 1));
                                    s.query(false, cx);
                                })),
                        )
                    }),
            )
    }
}

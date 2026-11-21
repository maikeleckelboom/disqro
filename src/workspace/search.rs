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
    previous: Option<FocusHandle>,
    input: Entity<InputState>,
    ticket: Ticket,
    rows: Matches,
    selected: usize,
    page: Option<usize>,
    status: String,
    scroll: ScrollHandle,
}

impl Drop for LibrarySearch {
    fn drop(&mut self) {
        self.ticket.next();
    }
}

impl Workspace {
    pub(super) fn open_search(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search {
            search.read(cx).input.read(cx).focus_handle(cx).focus(w, cx);
            return;
        }
        let workspace = cx.weak_entity();
        let catalog = self.services.catalog.clone();
        self.search = Some(cx.new(|cx| LibrarySearch::new(workspace, catalog, w, cx)));
        cx.notify();
    }
}

impl LibrarySearch {
    fn new(
        workspace: WeakEntity<Workspace>,
        catalog: Entity<Catalog>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let previous = w.focused(cx);
        let input = cx.new(|cx| {
            InputState::new(w, cx).placeholder("Search tracks, artists, albums, playlists…")
        });
        cx.subscribe_in(&input, w, |s: &mut Self, _, event, w, cx| match event {
            InputEvent::Change => {
                s.page = None;
                s.query(false, cx);
            }
            InputEvent::PressEnter { .. } => s.open(s.selected, w, cx),
            _ => {}
        })
        .detach();
        cx.subscribe(&catalog, |s: &mut Self, _, event, cx| {
            if matches!(event, CatalogEvent::Changed | CatalogEvent::Content) {
                s.query(true, cx);
            }
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(w, cx);
        Self {
            workspace,
            catalog,
            previous,
            input,
            ticket: Ticket::default(),
            rows: Matches::default(),
            selected: 0,
            page: None,
            status: String::new(),
            scroll: ScrollHandle::new(),
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
                    Ok(rows) => {
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
    fn dismiss(&self, w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.update(cx, |s, cx| {
            s.search = None;
            cx.notify()
        });
        if let Some(previous) = &self.previous {
            previous.focus(w, cx);
        }
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
        self.dismiss(w, cx);
        workspace.update(cx, |s, cx| s.reveal_active_browser(w, cx));
        browser.update(cx, |b, cx| match target {
            Target::Track(id) => b.show_search_track(id, w, cx),
            Target::Artist(name) => {
                b.show(Location::Library(None), w, cx);
                b.view(false, w, cx);
                b.filter_open = true;
                b.restore_search(format!("artist:={name}"), w, cx);
            }
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
    }
    fn row(&self, index: usize, hit: &Hit, cx: &Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let group = hit.target.group();
        let header = index == 0 || self.rows.hits[index - 1].target.group() != group;
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
                        .text_size(px(11.))
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
                                            .text_size(px(13.))
                                            .child(hit.title.clone()),
                                    )
                                    .when(!hit.detail.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.))
                                                .text_color(m.text_muted)
                                                .child(hit.detail.clone()),
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
                                    .tooltip("Play track")
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        if let Some(workspace) = s.workspace.upgrade() {
                                            let player = workspace.read(cx).services.player.clone();
                                            crate::table::queue_targets(
                                                &s.catalog,
                                                &player,
                                                Targets::Keys(Scope::Library(None), vec![id]),
                                                |tracks| Command::Play(tracks, 0),
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
        div()
            .id("library-search-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui_kit::black().opacity(0.36))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(46.))
            .on_click(cx.listener(|s, _, w, cx| s.dismiss(w, cx)))
            .child(
                v_flex()
                    .id("library-search")
                    .occlude()
                    .role(Role::Dialog)
                    .aria_label("Search library")
                    .w((size.width - px(24.)).min(px(640.)))
                    .max_h(size.height - px(58.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(m.control_line)
                    .bg(m.overlay)
                    .p(px(7.))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(|s, e: &KeyDownEvent, w, cx| {
                        match e.keystroke.key.as_str() {
                            "escape" => s.dismiss(w, cx),
                            "up" | "down" if !s.rows.hits.is_empty() => {
                                let last = s.rows.hits.len() - 1;
                                s.selected = if e.keystroke.key == "up" {
                                    s.selected.saturating_sub(1)
                                } else {
                                    (s.selected + 1).min(last)
                                };
                                s.scroll.scroll_to_item(s.selected);
                                s.input.read(cx).focus_handle(cx).focus(w, cx);
                                cx.notify();
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                        w.prevent_default();
                    }))
                    .child(
                        Input::new(&self.input)
                            .small()
                            .prefix(Icon::new(IconName::Search).size(px(16.)))
                            .aria_label("Search library"),
                    )
                    .when(!self.status.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(8.))
                                .py(px(10.))
                                .text_size(px(12.))
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
                            .gap(px(6.))
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
                    ),
            )
    }
}

//! The Music Browser's compact location trail and keyboard-operated menus.
use super::ActivateLocationControl;
use crate::{
    browser::{BreadcrumbChoice, BreadcrumbChoices, Browser, Location, Route, ViewChanged},
    catalog::CatalogEvent,
    theme::{Materials, materials},
};
use gpui_kit::{
    assets::IconName,
    base::Popover,
    component::{Icon, ThemeStyled, h_flex, scroll::ScrollableElement, tooltip::Tooltip},
    prelude::*,
    *,
};
use std::sync::Arc;

gpui_kit::actions!(
    breadcrumb,
    [
        PreviousDestination,
        NextDestination,
        FirstDestination,
        LastDestination,
        ChooseDestination,
        LeaveMenuForward,
        LeaveMenuBackward
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", PreviousDestination, Some("BreadcrumbMenu")),
        KeyBinding::new("down", NextDestination, Some("BreadcrumbMenu")),
        KeyBinding::new("home", FirstDestination, Some("BreadcrumbMenu")),
        KeyBinding::new("end", LastDestination, Some("BreadcrumbMenu")),
        KeyBinding::new("enter", ChooseDestination, Some("BreadcrumbMenu")),
        KeyBinding::new("space", ChooseDestination, Some("BreadcrumbMenu")),
        KeyBinding::new("tab", LeaveMenuForward, Some("BreadcrumbMenu")),
        KeyBinding::new("shift-tab", LeaveMenuBackward, Some("BreadcrumbMenu")),
    ]);
}

const FONT_SIZE: f32 = crate::theme::size::TEXT;
const INSET: f32 = 3.;
const FIELD_CHROME: f32 = 2. * INSET + 2.;
const LABEL_PADDING: f32 = 4.;
const CHEVRON: f32 = 20.;
const OVERFLOW: f32 = 22.;
const CONTROL_HEIGHT: f32 = 26.;
const MENU_ROW: f32 = 30.;
const MENU_VISIBLE_ROWS: usize = 10;
const MENU_MIN_WIDTH: f32 = 156.;
const MENU_MAX_WIDTH: f32 = 340.;

#[derive(Clone)]
enum Source {
    Earlier(Vec<Route>),
    Related(Route),
}

impl Source {
    fn key(&self) -> Option<u64> {
        match self {
            Self::Earlier(_) => None,
            Self::Related(route) => Some(route.control_id()),
        }
    }

    fn element_id(&self, slot: &'static str) -> ElementId {
        match self.key() {
            Some(key) => (slot, key).into(),
            None => (ElementId::from(slot), "earlier").into(),
        }
    }
}

enum MenuItems {
    WaitingForView,
    Loading,
    Unavailable,
    Ready {
        rows: Arc<[BreadcrumbChoice]>,
        widest: SharedString,
    },
}

struct OpenMenu {
    source: Source,
    revision: u64,
    focus: MenuFocus,
    items: MenuItems,
    cursor: Option<usize>,
    scroll: UniformListScrollHandle,
    _blur: Subscription,
}

#[derive(Clone)]
struct MenuFocus {
    trigger: FocusHandle,
    surface: FocusHandle,
}

impl OpenMenu {
    fn set_items(&mut self, choices: Option<Vec<BreadcrumbChoice>>) {
        self.items = match choices {
            Some(choices) => {
                self.cursor = choices.iter().position(|choice| choice.current);
                if let Some(cursor) = self.cursor {
                    self.scroll.scroll_to_item(cursor, ScrollStrategy::Center);
                }
                let widest = choices
                    .iter()
                    .max_by_key(|choice| choice.label.len())
                    .map(|choice| choice.label.clone())
                    .unwrap_or_default();
                MenuItems::Ready {
                    rows: choices.into(),
                    widest,
                }
            }
            None => MenuItems::Unavailable,
        };
    }
}

pub(super) struct Breadcrumbs {
    browser: Entity<Browser>,
    width: Pixels,
    menu: Option<OpenMenu>,
    request: Option<Task<()>>,
    request_id: u64,
}

impl Breadcrumbs {
    pub(super) fn new(
        browser: Entity<Browser>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe_in(
            &browser,
            window,
            |trail, browser, _: &ViewChanged, w, cx| {
                if let Some(menu) = &trail.menu {
                    if menu.revision != browser.read(cx).navigation_revision() {
                        trail.close(false, w, cx);
                    } else if matches!(menu.items, MenuItems::WaitingForView) {
                        trail.load(cx);
                    }
                }
                cx.notify();
            },
        )
        .detach();
        cx.subscribe_in(&browser.read(cx).catalog.clone(), window, |trail, _, event, w, cx| {
            if let CatalogEvent::Structure(root) = event
                && trail.menu.as_ref().is_some_and(|menu| {
                    matches!(&menu.source, Source::Related(route)
                        if matches!(route.location,
                            Location::Album { root: scope, .. } | Location::Release { root: scope, .. }
                            if scope.is_none_or(|id| id == *root)))
                })
            {
                trail.close(true, w, cx);
            }
            if matches!(event, CatalogEvent::Changed | CatalogEvent::Notice(_)) {
                if trail.menu.as_ref().is_some_and(|menu| {
                    matches!(&menu.source, Source::Related(route)
                        if matches!(route.location, Location::Library(Some(_)) | Location::Playlist(_)))
                }) {
                    if matches!(event, CatalogEvent::Changed) {
                        trail.load(cx);
                    } else if let Some(menu) = &mut trail.menu
                        && matches!(menu.items, MenuItems::WaitingForView)
                    {
                        menu.set_items(None);
                    }
                }
                cx.notify();
            }
        })
        .detach();
        Self {
            browser,
            width: px(0.),
            menu: None,
            request: None,
            request_id: 0,
        }
    }

    fn open(
        &mut self,
        source: Source,
        focus: MenuFocus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blur = cx.on_blur(&focus.surface, window, |trail, w, cx| {
            if trail
                .menu
                .as_ref()
                .is_some_and(|menu| !menu.focus.surface.is_focused(w))
            {
                trail.close(false, w, cx);
            }
        });
        focus.surface.focus(window, cx);
        self.menu = Some(OpenMenu {
            source,
            revision: self.browser.read(cx).navigation_revision(),
            focus,
            items: MenuItems::Loading,
            cursor: None,
            scroll: UniformListScrollHandle::new(),
            _blur: blur,
        });
        self.load(cx);
    }

    fn close(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.request.take();
        if let Some(menu) = self.menu.take() {
            if menu.focus.surface.is_focused(window) {
                if restore {
                    menu.focus.trigger.focus(window, cx);
                } else {
                    self.browser.read(cx).list_focus(cx).focus(window, cx);
                }
            }
            cx.notify();
        }
    }

    pub(super) fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(false, window, cx);
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.request.take();
        let Some(menu) = &mut self.menu else { return };
        self.request_id += 1;
        let request_id = self.request_id;
        let browser = self.browser.read(cx);
        let choices = match &menu.source {
            Source::Related(route) => browser.breadcrumb_choices(route, cx),
            Source::Earlier(routes) => BreadcrumbChoices::Ready(
                routes
                    .iter()
                    .rev()
                    .map(|route| {
                        BreadcrumbChoice::new(route.clone(), browser.route_label(route, cx), false)
                    })
                    .collect(),
            ),
        };
        match choices {
            BreadcrumbChoices::Ready(rows) => menu.set_items(Some(rows)),
            BreadcrumbChoices::Unavailable => menu.set_items(None),
            BreadcrumbChoices::Waiting => menu.items = MenuItems::WaitingForView,
            BreadcrumbChoices::Enumerate(enumerate) => {
                menu.items = MenuItems::Loading;
                self.request = Some(cx.spawn(async move |trail, cx| {
                    let rows = cx.background_spawn(async move { enumerate() }).await;
                    let _ = trail.update(cx, |trail, cx| {
                        if let Some(menu) = &mut trail.menu
                            && trail.request_id == request_id
                            && menu.revision == trail.browser.read(cx).navigation_revision()
                        {
                            menu.set_items(rows);
                            cx.notify();
                        }
                    });
                }));
            }
        }
        cx.notify();
    }

    fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        self.close(false, window, cx);
        self.browser.update(cx, |browser, cx| {
            browser.open_breadcrumb(route, window, cx);
            browser.list_focus(cx).focus(window, cx);
        });
    }

    fn choose(
        &mut self,
        clicked: Option<(usize, &Route)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = &self.menu else { return };
        if menu.revision != self.browser.read(cx).navigation_revision() {
            self.close(false, window, cx);
            return;
        }
        let MenuItems::Ready { rows, .. } = &menu.items else {
            return;
        };
        let choice = match clicked {
            Some((index, route)) => rows.get(index).filter(|choice| choice.route.same(route)),
            None => menu.cursor.and_then(|index| rows.get(index)),
        };
        if let Some(choice) = choice {
            if choice.current {
                self.close(true, window, cx);
            } else {
                self.navigate(choice.route.clone(), window, cx);
            }
        }
    }

    fn move_cursor(&mut self, step: CursorStep, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.menu else { return };
        let MenuItems::Ready { rows, .. } = &menu.items else {
            return;
        };
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let next = match step {
            CursorStep::First => 0,
            CursorStep::Last => last,
            CursorStep::Next => menu
                .cursor
                .map_or(0, |ix| if ix == last { 0 } else { ix + 1 }),
            CursorStep::Previous => menu.cursor.and_then(|ix| ix.checked_sub(1)).unwrap_or(last),
        };
        menu.cursor = Some(next);
        menu.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn popup(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let Some(menu) = &self.menu else {
            return div().into_any_element();
        };
        let surface = |width, cx: &mut Context<Self>| {
            self.menu_surface(width, &menu.focus.surface, window, cx)
        };
        match &menu.items {
            MenuItems::Ready {
                rows: items,
                widest,
            } if !items.is_empty() => {
                let cursor = menu.cursor;
                let rows = items.clone();
                let trail = cx.weak_entity();
                let radio = matches!(menu.source, Source::Related(_));
                let width = (label_width(window, m, widest.clone(), false) + px(58.))
                    .clamp(px(MENU_MIN_WIDTH), px(MENU_MAX_WIDTH));
                let visible = MENU_VISIBLE_ROWS.min(
                    ((f32::from(window.viewport_size().height) - 20.) / (2. * MENU_ROW)).max(1.)
                        as usize,
                );
                let list = uniform_list("breadcrumb-choices", items.len(), move |range, _, _| {
                    range
                        .map(|index| {
                            menu_row(
                                index,
                                &rows[index],
                                cursor == Some(index),
                                radio,
                                trail.clone(),
                                m,
                            )
                        })
                        .collect()
                })
                .w_full()
                .h(px(MENU_ROW * items.len().min(visible) as f32))
                .track_scroll(&menu.scroll);
                let list = div().id("breadcrumb-list").w_full().child(list);
                surface(width, cx)
                    .child(if items.len() > visible {
                        list.vertical_scrollbar(&menu.scroll).into_any_element()
                    } else {
                        list.into_any_element()
                    })
                    .into_any_element()
            }
            items => {
                let message = match items {
                    MenuItems::WaitingForView | MenuItems::Loading => "Loading locations…",
                    MenuItems::Unavailable => "Locations unavailable",
                    MenuItems::Ready { .. } => "No available locations",
                };
                surface(px(MENU_MIN_WIDTH), cx)
                    .child(
                        h_flex()
                            .h(px(MENU_ROW))
                            .items_center()
                            .px(px(8.))
                            .text_color(m.text_muted)
                            .child(message),
                    )
                    .into_any_element()
            }
        }
    }

    fn menu_surface(
        &self,
        width: Pixels,
        focus: &FocusHandle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        h_flex()
            .id("breadcrumb-menu")
            .role(Role::Menu)
            .aria_label("Locations")
            .track_focus(focus)
            .key_context("BreadcrumbMenu")
            .w(width.min(window.viewport_size().width - px(16.)))
            .p(px(4.))
            .popover_style(cx)
            .text_size(px(FONT_SIZE))
            .on_action(cx.listener(|trail, _: &PreviousDestination, _, cx| {
                trail.move_cursor(CursorStep::Previous, cx)
            }))
            .on_action(cx.listener(|trail, _: &NextDestination, _, cx| {
                trail.move_cursor(CursorStep::Next, cx)
            }))
            .on_action(cx.listener(|trail, _: &FirstDestination, _, cx| {
                trail.move_cursor(CursorStep::First, cx)
            }))
            .on_action(cx.listener(|trail, _: &LastDestination, _, cx| {
                trail.move_cursor(CursorStep::Last, cx)
            }))
            .on_action(cx.listener(|trail, _: &ChooseDestination, w, cx| trail.choose(None, w, cx)))
            .on_action(cx.listener(|trail, _: &LeaveMenuForward, w, cx| {
                trail.close(true, w, cx);
                w.focus_next(cx);
            }))
            .on_action(cx.listener(|trail, _: &LeaveMenuBackward, w, cx| {
                trail.close(true, w, cx);
                w.focus_prev(cx);
            }))
    }

    fn trigger(
        &self,
        source: Source,
        label: SharedString,
        current: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Popover {
        let m = *materials(cx);
        let earlier = matches!(source, Source::Earlier(_));
        let width = px(if earlier { OVERFLOW } else { CHEVRON });
        let key = source.key();
        let open = self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.source.key() == key);
        let target = window
            .use_keyed_state(source.element_id("breadcrumb-focus"), cx, |_, cx| {
                MenuFocus {
                    trigger: cx.focus_handle().tab_stop(true),
                    surface: cx.focus_handle(),
                }
            })
            .read(cx)
            .clone();
        let focus = target.trigger.clone();
        let activate = (source.clone(), target.clone(), cx.weak_entity());
        let trail = cx.weak_entity();
        let trigger_id = source.element_id("breadcrumb-trigger");
        let content = cx.weak_entity();
        Popover::new(source.element_id("breadcrumb-popover"))
            .w(width)
            .flex_shrink_0()
            .offset(px(4.))
            .track_focus(&target.surface)
            .open(open)
            .on_open_change(move |opening, w, cx| {
                let _ = trail.update(cx, |trail, cx| {
                    if *opening {
                        trail.open(source.clone(), target.clone(), w, cx);
                    } else if trail
                        .menu
                        .as_ref()
                        .is_some_and(|menu| menu.source.key() == key)
                    {
                        trail.close(true, w, cx);
                    }
                });
            })
            .trigger_with(move |opened, _, _| {
                let icon = if earlier {
                    IconName::Ellipsis
                } else if opened || current {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                };
                let mouse_focus = focus.clone();
                control(trigger_id, m)
                    .w(width)
                    .justify_center()
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .aria_expanded(opened)
                    .track_focus(&focus)
                    .when(opened, |d| d.bg(m.control))
                    .on_mouse_down(MouseButton::Left, move |_, w, cx| mouse_focus.focus(w, cx))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_a11y_action(AccessibleAction::Click, move |_, w, cx| {
                        let (source, focus, trail) = &activate;
                        let _ = trail.update(cx, |trail, cx| {
                            if trail
                                .menu
                                .as_ref()
                                .is_some_and(|menu| menu.source.key() == key)
                            {
                                trail.close(true, w, cx);
                            } else {
                                focus.trigger.focus(w, cx);
                                trail.open(source.clone(), focus.clone(), w, cx);
                            }
                        });
                    })
                    .child(Icon::new(icon).size(px(12.)).text_color(if opened {
                        m.text
                    } else {
                        m.text_muted
                    }))
                    .when(!opened, |d| {
                        d.tooltip(move |w, cx| Tooltip::new(label.clone()).build(w, cx))
                    })
                    .into_any_element()
            })
            .content(move |_, w, cx| {
                content
                    .update(cx, |trail, cx| trail.popup(w, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
    }

    fn segment(
        &self,
        route: Route,
        width: Pixels,
        current: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let browser = self.browser.read(cx);
        let label: SharedString = browser.breadcrumb_label(&route, cx).into();
        let description: SharedString = browser.route_label(&route, cx).into();
        let menu_label = format!("Locations for {description}").into();
        let m = *materials(cx);
        let id = ("breadcrumb-label", route.control_id()).into();
        let label_width = (width - px(CHEVRON)).max(px(0.));
        let label_control = if current {
            h_flex()
                .id(id)
                .h(px(CONTROL_HEIGHT))
                .role(Role::Label)
                .aria_label(description.clone())
                .aria_description("Current location")
                .font_weight(FontWeight::MEDIUM)
                .text_color(m.text)
        } else {
            let target = route.clone();
            control(id, m)
                .role(Role::Button)
                .aria_label(description.clone())
                .text_color(m.text_muted)
                .tab_index(0)
                .key_context("LocationControl")
                .on_action(
                    cx.listener(move |trail, _: &ActivateLocationControl, w, cx| {
                        trail.navigate(target.clone(), w, cx);
                    }),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener({
                    let route = route.clone();
                    move |trail, _, w, cx| {
                        cx.stop_propagation();
                        trail.navigate(route.clone(), w, cx);
                    }
                }))
        };
        h_flex()
            .w(width)
            .flex_shrink_0()
            .child(
                label_control
                    .w(label_width)
                    .min_w_0()
                    .items_center()
                    .px(px(LABEL_PADDING))
                    .text_size(px(FONT_SIZE))
                    .overflow_hidden()
                    .child(div().min_w_0().truncate().child(label))
                    .tooltip(move |w, cx| Tooltip::new(description.clone()).build(w, cx)),
            )
            .child(self.trigger(Source::Related(route), menu_label, current, window, cx))
    }
}

fn control(id: ElementId, m: Materials) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(CONTROL_HEIGHT))
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(transparent_black())
        .hover(|d| d.bg(m.control).text_color(m.text))
        .focus_visible(|d| d.border_color(m.focus_line()).text_color(m.text))
        .active(|d| d.bg(m.control_hover))
        .cursor_pointer()
}

#[derive(Clone, Copy)]
enum CursorStep {
    First,
    Last,
    Next,
    Previous,
}

fn menu_row(
    index: usize,
    choice: &BreadcrumbChoice,
    cursor: bool,
    radio: bool,
    trail: WeakEntity<Breadcrumbs>,
    m: Materials,
) -> Stateful<Div> {
    let label = choice.label.clone();
    let route = choice.route.clone();
    let identity = choice.route.control_id();
    h_flex()
        .id(("breadcrumb-choice", identity))
        .role(if radio {
            Role::MenuItemRadio
        } else {
            Role::MenuItem
        })
        .aria_label(label.clone())
        .when(radio, |row| {
            row.aria_toggled(if choice.current {
                Toggled::True
            } else {
                Toggled::False
            })
        })
        .when(cursor, |row| row.aria_active_descendant().bg(m.selected))
        .w_full()
        .h(px(MENU_ROW))
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(4.))
        .items_center()
        .justify_between()
        .hover(|d| d.bg(m.hover))
        .cursor_pointer()
        .text_color(m.text)
        .child(div().min_w_0().truncate().child(label.clone()))
        .child(div().w(px(14.)).flex_shrink_0().when(choice.current, |d| {
            d.child(
                Icon::new(IconName::Check)
                    .size(px(13.))
                    .text_color(m.accent),
            )
        }))
        .on_click(move |_, w, cx| {
            cx.stop_propagation();
            let _ = trail.update(cx, |trail, cx| trail.choose(Some((index, &route)), w, cx));
        })
        .tooltip(move |w, cx| Tooltip::new(label.clone()).build(w, cx))
}

/// The toolbar reserves space for measured labels and a readable current destination.
pub(crate) struct TrailWidths {
    pub(crate) preferred: Pixels,
    pub(crate) minimum: Pixels,
}

impl TrailWidths {
    pub(crate) fn measure(browser: &Browser, window: &Window, cx: &App) -> Self {
        let routes = browser.ancestors();
        let widths = measure_trail(browser, &routes, window, cx);
        let total = px(FIELD_CHROME) + widths.iter().copied().sum::<Pixels>();
        let earlier = if widths.len() > 1 {
            px(OVERFLOW)
        } else {
            px(0.)
        };
        let leaf = widths.last().copied().unwrap_or_default() + earlier + px(FIELD_CHROME);
        let preferred = total.clamp(px(140.), px(240.));
        Self {
            preferred,
            minimum: leaf.clamp(px(84.), px(120.)).min(preferred),
        }
    }
}

fn measure_trail(browser: &Browser, routes: &[Route], window: &Window, cx: &App) -> Vec<Pixels> {
    routes
        .iter()
        .enumerate()
        .map(|(index, route)| {
            let label: SharedString = browser.breadcrumb_label(route, cx).into();
            label_width(window, *materials(cx), label, index + 1 == routes.len())
                + px(2. * LABEL_PADDING + CHEVRON + 2.)
        })
        .collect()
}

impl Breadcrumbs {
    pub(super) fn set_field_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        let width = (width - px(FIELD_CHROME)).max(px(0.));
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }
}

impl Render for Breadcrumbs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let browser = self.browser.read(cx);
        let routes = browser.ancestors();
        let last = routes.len() - 1;
        let widths = measure_trail(browser, &routes, window, cx);
        let (hidden, visible) = fit_trail(&widths, self.width);
        let still_visible = |menu: &OpenMenu| match &menu.source {
            Source::Earlier(earlier) => {
                earlier.len() == hidden
                    && earlier
                        .iter()
                        .zip(&routes[..hidden])
                        .all(|(a, b)| a.same(b))
            }
            Source::Related(target) => routes[hidden..]
                .iter()
                .any(|route| route.control_id() == target.control_id()),
        };
        // A competing trigger can take focus before this surface has painted
        // once. In that case GPUI has no previous focus path to deliver blur.
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| !menu.focus.surface.is_focused(window) || !still_visible(menu))
        {
            self.close(false, window, cx);
        }
        h_flex()
            .id("breadcrumbs")
            .role(Role::Group)
            .aria_label("Location")
            .h_full()
            .flex_1()
            .min_w_0()
            .px(px(INSET))
            .items_center()
            .overflow_hidden()
            .when(hidden > 0, |row| {
                row.child(self.trigger(
                    Source::Earlier(routes[..hidden].to_vec()),
                    "Earlier locations".into(),
                    false,
                    window,
                    cx,
                ))
            })
            .children(
                routes
                    .into_iter()
                    .enumerate()
                    .skip(hidden)
                    .zip(visible)
                    .map(|((index, route), width)| {
                        self.segment(route, width, index == last, window, cx)
                    }),
            )
    }
}

fn label_width(window: &Window, m: Materials, label: SharedString, current: bool) -> Pixels {
    let mut font = window.text_style().font();
    if current {
        font.weight = FontWeight::MEDIUM;
    }
    let len = label.len();
    window
        .text_system()
        .shape_line(
            label,
            px(FONT_SIZE),
            &[TextRun {
                len,
                font,
                color: m.text,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width
        .ceil()
}

/// Work from the leaf back. Full labels use their measured widths. When
/// space is tight, keep a floor for the parent before giving the leaf the
/// rest of the space. Chevron widths are included in each slot.
fn fit_trail(widths: &[Pixels], space: Pixels) -> (usize, Vec<Pixels>) {
    let last = widths.len() - 1;
    let mut total: Pixels = widths.iter().copied().sum();
    if total <= space {
        return (0, widths.to_vec());
    }
    let parent_floor = |width: Pixels| width.min(px(60.));
    let mut hidden = 0;
    while last - hidden > 1 && total > space {
        total -= widths[hidden];
        if hidden == 0 {
            total += px(OVERFLOW);
        }
        hidden += 1;
    }
    let mut budget = space - if hidden > 0 { px(OVERFLOW) } else { px(0.) };
    if last > hidden && budget < parent_floor(widths[last - 1]) + widths[last].min(px(94.)) {
        hidden = last;
        budget = space - px(OVERFLOW);
    }
    if widths[hidden..].iter().copied().sum::<Pixels>() <= budget {
        return (hidden, widths[hidden..].to_vec());
    }
    if hidden == last {
        return (hidden, vec![budget.max(px(0.))]);
    }
    let leaf = widths[last].min(budget - parent_floor(widths[last - 1]));
    (hidden, vec![budget - leaf, leaf])
}

#[cfg(test)]
mod tests {
    use super::fit_trail;
    use gpui_kit::px;

    #[test]
    fn breadcrumb_widths_keep_full_labels_then_the_nearest_parent() {
        let widths = [px(60.), px(64.), px(180.), px(52.)];
        assert_eq!(fit_trail(&widths, px(356.)), (0, widths.to_vec()));
        assert_eq!(fit_trail(&widths, px(320.)), (1, widths[1..].to_vec()));
        assert_eq!(fit_trail(&widths, px(140.)), (2, vec![px(66.), px(52.)]));
        assert_eq!(fit_trail(&widths, px(86.)), (3, vec![px(52.)]));
        assert_eq!(
            fit_trail(&[px(78.), px(86.)], px(146.)),
            (0, vec![px(60.), px(86.)])
        );
        assert_eq!(
            fit_trail(&[px(72.), px(210.), px(118.)], px(164.)),
            (2, vec![px(118.)])
        );
        assert_eq!(fit_trail(&[px(160.)], px(70.)), (0, vec![px(70.)]));
    }
}

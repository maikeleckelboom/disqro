//! Group-local tab strips and navigation above the panel they describe.
//! The dock owns the layout, panel dragging, tab order and selection.
mod tabs;
use super::Workspace;
use crate::{
    panels::{Home, Pane, browser::BrowserHeaderWidths, nav::DraggedFolder},
    table::DraggedTracks,
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    base::ResizeHandleContext,
    component::{
        ElementExt, Icon, Sizable,
        button::{Button, ButtonVariants},
        dock::{
            DockAreaRenderer, DockContext, DockSkin, DragPanel, DropIndicator, NodeId, PanelId,
            PanelState, TabGroupContext, TabGroupRenderer,
        },
        h_flex,
        input::Input,
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        tooltip::Tooltip,
    },
    prelude::*,
    *,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

/// How long an item dragged over a tab rests there before the tab opens.
const DWELL: Duration = Duration::from_millis(550);
const PREVIEW_HEIGHT: f32 = 24.;
pub const STRIP_HEIGHT: f32 = crate::theme::size::HEADER;
/// The text inset on either side of a tab label.
pub const TAB_X: f32 = 10.;
const TAB_ICON: f32 = 14.;
const TAB_GAP: f32 = 6.;
const TAB_CLOSE: f32 = 18.;
const TAB_END: f32 = 4.;

/// Shared panel commands and hover activation for all dock groups.
pub struct SkinState {
    pub workspace: WeakEntity<Workspace>,
    dwell: RefCell<Dwell<PanelId>>,
    routine_drag: RefCell<Option<(PanelId, WeakEntity<TabPreview>)>>,
    headers: RefCell<HashMap<NodeId, GroupHeader>>,
}

#[derive(Clone, Copy, Default)]
struct GroupHeader {
    width: Pixels,
    stacked: bool,
}

/// Which tab a dragged item rests over. Each arrival starts a new rest, and
/// only the rest still under way when its time is up opens the tab.
struct Dwell<T> {
    over: Option<(T, u64)>,
    epoch: u64,
}
impl<T> Default for Dwell<T> {
    fn default() -> Self {
        Self {
            over: None,
            epoch: 0,
        }
    }
}
impl<T: Copy + PartialEq> Dwell<T> {
    /// The item moved inside or outside `tab`. Returns the rest to time when
    /// it just arrived over a tab that is not shown.
    fn moved(&mut self, tab: T, inside: bool, shown: bool) -> Option<u64> {
        let current = self.over.map(|(t, _)| t);
        if !inside {
            if current == Some(tab) {
                self.over = None;
            }
            return None;
        }
        if current == Some(tab) || shown {
            return None;
        }
        self.epoch += 1;
        self.over = Some((tab, self.epoch));
        Some(self.epoch)
    }
    /// Whether the rest `epoch` over `tab` lasted; it ends either way.
    fn lasted(&mut self, tab: T, epoch: u64) -> bool {
        let lasted = self.over == Some((tab, epoch));
        if lasted {
            self.over = None;
        }
        lasted
    }
}
impl SkinState {
    pub fn new(workspace: WeakEntity<Workspace>) -> Rc<Self> {
        Rc::new(Self {
            workspace,
            routine_drag: Default::default(),
            dwell: RefCell::new(Dwell::default()),
            headers: RefCell::default(),
        })
    }
    pub fn can_open_beside(&self, node: NodeId) -> bool {
        // Keep both tables readable when opening a browser beside this group.
        self.headers
            .borrow()
            .get(&node)
            .is_some_and(|header| header.width >= px(2. * 320. + 4.))
    }
    pub fn retain_groups(&self, tree: &gpui_kit::component::dock::PaneTree) {
        self.headers
            .borrow_mut()
            .retain(|node, _| tree.find_node(*node).is_some());
    }
    /// An item dragged over `panel`'s tab: open the tab once it rests there.
    /// Hovering alone never changes anything but which tab is shown.
    fn hover(
        self: &Rc<Self>,
        group: &TabGroupContext,
        ix: usize,
        panel: PanelId,
        inside: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let shown = group.active_ix() == ix;
        let Some(epoch) = self.dwell.borrow_mut().moved(panel, inside, shown) else {
            return;
        };
        let (state, group) = (self.clone(), group.clone());
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(DWELL).await;
                let _ = cx.update(|window, cx| {
                    let lasted = state.dwell.borrow_mut().lasted(panel, epoch);
                    if lasted && cx.has_active_drag() {
                        group.select_tab(ix, window, cx);
                    }
                });
            })
            .detach();
    }
}

/// The renderer installed on the dock area.
pub struct Skin {
    inner: Rc<DockSkin>,
    state: Rc<SkinState>,
}
impl Skin {
    pub fn new(inner: Rc<DockSkin>, state: Rc<SkinState>) -> Self {
        Self { inner, state }
    }
}

impl DockAreaRenderer for Skin {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.inner.frame(window, cx).bg(materials(cx).canvas)
    }
    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.inner.center_frame(window, cx)
    }
    fn split_frame(
        &self,
        node: NodeId,
        axis: Axis,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.inner.split_frame(node, axis, window, cx)
    }
    fn render_split_handle(
        &self,
        handle: &ResizeHandleContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.render_split_handle(handle, window, cx)
    }
    fn closed_dock_extent(&self, placement: gpui_kit::component::dock::DockPlacement) -> Pixels {
        self.inner.closed_dock_extent(placement)
    }
    fn render_dock(
        &self,
        dock: &DockContext,
        content: AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.inner.render_dock(dock, content, window, cx)
    }
    /// A panel type this build does not know, from a layout a newer build
    /// wrote: kept, and saved back unchanged.
    fn build_placeholder(
        &self,
        state: &PanelState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn gpui_kit::component::dock::BasePanelView>> {
        self.inner.build_placeholder(state, window, cx)
    }
    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(Tabs {
            state: self.state.clone(),
            scroll: ScrollHandle::new(),
        })
    }
}

/// One tab group's appearance.
struct Tabs {
    state: Rc<SkinState>,
    scroll: ScrollHandle,
}

impl TabGroupRenderer for Tabs {
    fn frame(&self, _: &TabGroupContext, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        div().id("tab-group").bg(materials(cx).chrome)
    }
    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let tabs = group
            .panels()
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible(cx))
            .map(|(ix, _)| ix)
            .collect::<Vec<_>>();
        if tabs.is_empty() {
            return Empty.into_any_element();
        }
        let utility = tabs.iter().all(|&ix| is_utility(&group.panels()[ix], cx));
        let m = *materials(cx);
        let node = group.node();
        let pane = group.active_panel().and_then(pane_of);
        let browser = pane.as_ref().and_then(|p| super::browser_of(p, cx));
        let controls_widths = browser
            .as_ref()
            .map(|b| b.read(cx).header_widths(window, cx));
        let several = tabs.len() > 1;
        let editing = tabs.iter().copied().find(|&ix| {
            pane_of(&group.panels()[ix]).is_some_and(|p| p.read(cx).renaming.is_some())
        });
        let selected = if tabs.contains(&group.active_ix()) {
            group.active_ix()
        } else {
            tabs[0]
        };
        let shown = editing.unwrap_or(selected);
        let current = tabs::labels_width(group, &[shown], window, cx);
        let labels = tabs::TabLabels {
            current,
            all: if editing.is_some() || !several {
                current
            } else {
                tabs::labels_width(group, &tabs, window, cx)
            },
            several: several && editing.is_none(),
        };
        let previous = self.state.headers.borrow().get(&node).copied();
        let stacked = previous.is_some_and(|h| labels.stacked(h.width, controls_widths, h.stacked));
        let group = group.clone();
        let state = self.state.clone();
        let scroll = self.scroll.clone();
        container_query(move |size, window, cx| {
            let needs_stack = labels.stacked(size.width, controls_widths, stacked);
            state.headers.borrow_mut().insert(
                node,
                GroupHeader {
                    width: size.width,
                    stacked: needs_stack,
                },
            );
            // Container queries lay out children after their own height has
            // been assigned. Reflow the group once when crossing the boundary.
            if stacked != needs_stack
                && let Some(group) = pane.as_ref().and_then(|p| p.read(cx).group.clone())
            {
                window.on_next_frame(move |_, cx| {
                    let _ = group.update(cx, |_, cx| cx.notify());
                });
            }
            let widths = labels.fit(size.width, controls_widths, stacked);
            let overflow = widths.overflow;
            let compact_width =
                (widths.tabs < if overflow { current } else { labels.all }).then_some(widths.tabs);
            let controls = browser
                .as_ref()
                .zip(pane.as_ref())
                .zip(controls_widths)
                .map(|((browser, pane), metrics)| {
                    let p = pane.read(cx);
                    let content = p.content.clone();
                    let focus = p.controls_focus.clone();
                    let id = p.id;
                    let space = drop_space(&group, ("browser-space", node.as_u64()), cx);
                    let controls = browser.update(cx, |b, cx| {
                        b.header_controls(widths.controls, metrics, space, window, cx)
                    });
                    h_flex()
                        .id(("panel-controls", id.0))
                        .track_focus(&focus)
                        .capture_any_mouse_down(move |_, w, cx| content.focused(w, cx))
                        .flex_shrink_0()
                        .w(widths.controls)
                        .when(stacked, |d| d.bg(m.canvas))
                        .child(controls)
                });
            // Panel commands live on each tab's own context menu, where their
            // scope is the tab. The strip offers a switcher only while it is
            // too narrow to show every tab.
            let switcher = overflow.then(|| {
                let select = group.clone();
                Button::new("tab-switcher")
                    .ghost()
                    .xsmall()
                    .w(px(tabs::SWITCHER))
                    .icon(
                        Icon::new(IconName::ChevronDown)
                            .size(px(12.))
                            .text_color(m.text_muted),
                    )
                    .accessibility_label("Show another panel")
                    .tooltip("Show another panel")
                    .tab_stop(true)
                    .dropdown_menu(move |mut menu, _, cx| {
                        for (ix, panel) in select
                            .panels()
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| p.visible(cx))
                        {
                            let label = tab_label(panel, cx);
                            let select = select.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .checked(ix == selected)
                                    .on_click(move |_, w, cx| select.select_tab(ix, w, cx)),
                            );
                        }
                        menu
                    })
            });
            let strip = h_flex()
                .id(("tab-strip", node.as_u64()))
                .w(if stacked || controls.is_none() {
                    size.width
                } else {
                    size.width - widths.controls
                })
                .h(px(STRIP_HEIGHT))
                .flex_shrink_0()
                .items_center()
                .bg(if utility && !several {
                    m.chrome
                } else {
                    m.tab_well
                })
                .child(
                    h_flex()
                        .id(("tabs", node.as_u64()))
                        .h_full()
                        .min_w_0()
                        .w(widths.tabs)
                        .tab_group()
                        .role(Role::TabList)
                        .overflow_x_scroll()
                        .track_scroll(&scroll)
                        .children(
                            tabs.into_iter()
                                .filter(|&ix| (!overflow && editing.is_none()) || ix == shown)
                                .map(|ix| tab(&group, ix, &state, compact_width, cx)),
                        ),
                )
                .children(switcher)
                .when(stacked || controls.is_none(), |d| {
                    d.child(drop_space(&group, ("strip-space", node.as_u64()), cx))
                });
            h_flex()
                .w_full()
                .when(stacked, |d| d.flex_col())
                .bg(m.tab_well)
                .child(strip)
                .children(controls)
        })
        .h(px(STRIP_HEIGHT * if stacked { 2. } else { 1. }))
        .flex_shrink_0()
        .into_any_element()
    }
    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let body = div()
            .id("tab-content")
            .flex_1()
            .relative()
            .child(if window.is_a11y_active() {
                div().absolute().size_full().child(panel).into_any_element()
            } else {
                panel
                    .cached(StyleRefinement::default().absolute().size_full())
                    .into_any_element()
            });
        if let Some(panel) = group
            .active_panel()
            .filter(|p| group.is_locked() && is_utility(p, cx))
        {
            let target = panel.panel_id(cx);
            let state = self.state.clone();
            let overlay = canvas(
                |_, _, _| (),
                move |bounds, _, w, cx| {
                    let Some((source, _)) = state
                        .routine_drag
                        .borrow()
                        .as_ref()
                        .and_then(|(id, preview)| Some((*id, preview.upgrade()?)))
                    else {
                        return;
                    };
                    if !cx.has_active_drag() || !bounds.contains(&w.mouse_position()) {
                        return;
                    }
                    let Some(edge) = split_edge(bounds, w.mouse_position()) else {
                        return;
                    };
                    if !state.workspace.upgrade().is_some_and(|ws| {
                        ws.read(cx).routine_move(source, target, edge, cx).is_some()
                    }) {
                        return;
                    }
                    let half = bounds.size.height * 0.5;
                    let origin = if matches!(edge, TabDrop::Below) {
                        bounds.origin + point(px(0.), half)
                    } else {
                        bounds.origin
                    };
                    w.paint_quad(preview_quad(
                        Bounds::new(origin, size(bounds.size.width, half)),
                        materials(cx),
                    ));
                },
            )
            .absolute()
            .inset_0()
            .size_full();
            routine_target(body.child(overlay), target, false, &self.state).into_any_element()
        } else {
            body.into_any_element()
        }
    }
    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let m = materials(cx);
        let to = indicator.to();
        let inset = px(crate::theme::size::PREVIEW_INSET);
        Some(
            div()
                .absolute()
                .left(to.origin().x + inset)
                .top(to.origin().y + inset)
                .w((to.size().width - inset * 2.).max(px(0.)))
                .h((to.size().height - inset * 2.).max(px(0.)))
                .bg(m.preview_fill())
                .border_1()
                .border_color(m.preview_edge())
                .rounded(px(6.))
                .into_any_element(),
        )
    }
}

/// The region a dragged panel will occupy, in the same form as the dock's
/// own drop indicator.
fn preview_quad(region: Bounds<Pixels>, m: &crate::theme::Materials) -> PaintQuad {
    let inset = px(crate::theme::size::PREVIEW_INSET);
    let bounds = Bounds::new(
        region.origin + point(inset, inset),
        size(
            (region.size.width - inset * 2.).max(px(0.)),
            (region.size.height - inset * 2.).max(px(0.)),
        ),
    );
    quad(
        bounds,
        px(6.),
        m.preview_fill(),
        px(1.),
        m.preview_edge(),
        BorderStyle::Solid,
    )
}

/// The pane behind a dock handle; `None` for a placeholder.
pub fn pane_of(panel: &Arc<dyn gpui_kit::component::dock::BasePanelView>) -> Option<Entity<Pane>> {
    panel.as_any().downcast_ref::<Entity<Pane>>().cloned()
}

fn tab_label(panel: &Arc<dyn gpui_kit::component::dock::BasePanelView>, cx: &App) -> SharedString {
    pane_of(panel).map_or_else(
        || format!("Unavailable: {}", panel.panel_name(cx)).into(),
        |p| p.read(cx).label(cx),
    )
}

fn is_utility(panel: &Arc<dyn gpui_kit::component::dock::BasePanelView>, cx: &App) -> bool {
    pane_of(panel).is_some_and(|p| p.read(cx).ty.home != Home::Main)
}

/// One tab: icon, label and a close button that shows on the selected or
/// hovered tab. It selects on click, closes on middle-click, drags to another
/// group or edge, takes tabs dropped on it, and opens when a track or folder
/// rests over it while being dragged.
fn tab(
    group: &TabGroupContext,
    ix: usize,
    state: &Rc<SkinState>,
    compact_width: Option<Pixels>,
    cx: &mut App,
) -> AnyElement {
    let m = *materials(cx);
    let panel = group.panels()[ix].clone();
    let id = panel.panel_id(cx);
    let pane = pane_of(&panel);
    let utility = is_utility(&panel, cx);
    let active_browser = pane
        .as_ref()
        .and_then(|p| super::browser_of(p, cx))
        .is_some_and(|browser| browser.read(cx).active);
    let selected = group.active_panel().map(|p| p.panel_id(cx)) == Some(id);
    let (label, icon, renaming, accepts_folder) = match &pane {
        Some(pane) => {
            let p = pane.read(cx);
            (
                p.label(cx),
                p.icon(cx),
                p.renaming.clone(),
                p.content.accepts_folder(),
            )
        }
        None => (
            tab_label(&panel, cx),
            Icon::new(IconName::TriangleAlert),
            None,
            false,
        ),
    };
    let hover_group = SharedString::from(format!("tab-{}", id.as_u64()));
    let workspace = state.workspace.clone();
    let close = {
        let workspace = workspace.clone();
        let group = group.clone();
        move |window: &mut Window, cx: &mut App| {
            let handled = workspace
                .update(cx, |w, cx| w.close_panel(id, window, cx))
                .unwrap_or(false);
            if !handled {
                group.close(id, window, cx);
            }
        }
    };
    let close_click = close.clone();
    let editing = renaming.is_some();
    let compact = compact_width.is_some();
    // Keep at least three ems for the name when the strip compresses a tab.
    let fits_close = compact_width
        .is_none_or(|width| width >= tabs::tab_width(px(3. * crate::theme::size::TEXT), false));
    let label_element = match renaming {
        Some(input) => {
            let pane = pane.clone();
            div()
                .flex_1()
                .min_w_0()
                .key_context("LocationBar")
                .on_action(move |_: &crate::location::CancelEdit, w, cx| {
                    if let Some(pane) = &pane {
                        pane.update(cx, |p, cx| p.cancel_rename(w, cx));
                    }
                })
                .child(Input::new(&input).xsmall().aria_label("Tab name"))
                .into_any_element()
        }
        None => div()
            .flex_1()
            .min_w_0()
            .truncate()
            .child(label.clone())
            .into_any_element(),
    };
    let long = label.len() > 26;
    let select = group.clone();
    let drop_group = group.clone();
    let (folder_drop_pane, menu_pane, click_pane) = (pane.clone(), pane.clone(), pane.clone());
    let (hover_tracks, hover_folders) = (
        (state.clone(), group.clone()),
        (state.clone(), group.clone()),
    );
    let drag = group
        .is_draggable()
        .then(|| group.drag_panel(ix, cx))
        .flatten();
    let surface = if utility { m.chrome } else { m.canvas };
    let tab = h_flex()
        .id(("tab", id.as_u64()))
        .group(hover_group.clone())
        .occlude()
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(selected)
        .when(!editing, |d| tabs::keyboard_tab(d, group, ix, selected, cx))
        .relative()
        .h(px(STRIP_HEIGHT))
        .min_w(px(40.))
        .max_w(px(220.))
        .pl(px(TAB_X))
        .pr(px(if utility { TAB_X } else { TAB_END }))
        .gap(px(TAB_GAP))
        .flex_shrink_0()
        .rounded_t(px(crate::theme::size::TAB_RADIUS))
        // Reserved for the keyboard focus outline.
        .border_1()
        .border_color(transparent_black())
        .items_center()
        .text_size(px(crate::theme::size::TEXT))
        .when(compact, |d| d.min_w_0().flex_1())
        .when(editing, |d| d.flex_1().min_w_0())
        .text_color(if selected { m.text } else { m.text_muted })
        .when(selected, |d| d.bg(surface))
        .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
        .when(!utility, |d| {
            d.child(
                icon.size(px(TAB_ICON))
                    .flex_shrink_0()
                    .text_color(if active_browser {
                        m.accent
                    } else if selected {
                        m.text_muted
                    } else {
                        m.text_faint
                    }),
            )
        })
        .child(label_element)
        // Keep hover geometry stable. In overflow the tab menu and middle
        // click still close the panel when its label needs this space.
        .when(!utility && !editing && fits_close, |d| {
            d.child(
                div()
                    .id(("tab-close", id.as_u64()))
                    .size(px(TAB_CLOSE))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .hover(|d| d.bg(m.chrome.blend(m.control_hover)))
                    .invisible()
                    .group_hover(hover_group, |s| s.visible())
                    .child(
                        Icon::new(IconName::X)
                            .size(px(12.))
                            .text_color(m.text_faint),
                    )
                    .tooltip(|w, cx| Tooltip::new("Close").build(w, cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, w, cx| {
                        cx.stop_propagation();
                        close_click(w, cx)
                    }),
            )
        })
        .when(long || compact, |d| {
            let tip = label.clone();
            d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
        })
        .on_click(move |_, w, cx| {
            select.select_tab(ix, w, cx);
            // A tab chosen by the listener is the panel they are working in.
            if let Some(pane) = &click_pane {
                let content = pane.read(cx).content.clone();
                content.focused(w, cx);
            }
        })
        .on_mouse_down(MouseButton::Middle, move |_, w, cx| {
            cx.stop_propagation();
            close(w, cx)
        })
        .when_some(drag, |d, drag| {
            let preview = label.clone();
            let workspace = workspace.clone();
            let node = group.node();
            d.on_drag(drag, move |drag, offset, _, cx| {
                let _ = workspace.update(cx, |s, cx| s.start_panel_drag(id, node, ix, cx));
                drag.set_drag_offset(offset);
                drag.set_preview_size(size(px(140.), px(PREVIEW_HEIGHT)));
                let label = preview.clone();
                cx.new(|_| TabPreview { label })
            })
        })
        .when(group.is_droppable(), |d| {
            d.drag_over::<DragPanel>(move |d, _, _, _| d.border_l_2().border_color(m.accent))
                .on_drop(move |drag: &DragPanel, w, cx| {
                    drop_group.drop_panel(drag.clone(), Some(ix), true, w, cx)
                })
        })
        // Library-bound items: rest over a tab to open it, then drop inside.
        .on_drag_move::<DraggedTracks>(move |e, w, cx| {
            let inside = e.bounds.contains(&e.event.position);
            hover_tracks.0.hover(&hover_tracks.1, ix, id, inside, w, cx)
        })
        .on_drag_move::<DraggedFolder>(move |e, w, cx| {
            let inside = e.bounds.contains(&e.event.position);
            hover_folders
                .0
                .hover(&hover_folders.1, ix, id, inside, w, cx)
        })
        .when(accepts_folder, |d| {
            d.drag_over::<DraggedFolder>(move |d, _, _, _| {
                d.bg(m.accent.opacity(0.16)).text_color(m.text)
            })
            .on_drop(move |dragged: &DraggedFolder, _, cx| {
                if let Some(pane) = &folder_drop_pane {
                    let content = pane.read(cx).content.clone();
                    content.drop_folder(dragged.path.clone(), cx);
                }
            })
        });
    let tab = if group.is_locked() && !editing {
        let title = label.clone();
        let source_state = state.clone();
        routine_target(
            tab.on_drag(RoutineTab(id), move |_, _, _, cx| {
                let label = title.clone();
                let preview = cx.new(|_| TabPreview { label });
                source_state
                    .routine_drag
                    .replace(Some((id, preview.downgrade())));
                preview
            }),
            id,
            true,
            state,
        )
    } else {
        tab
    };
    tab.context_menu(move |menu, w, cx| match &menu_pane {
        Some(pane) => super::tab_menu(&workspace, pane, menu, w, cx),
        None => menu,
    })
    .into_any_element()
}

/// A locked-mode tab carries only its identity. Music data and unrestricted
/// dock drags have their own payloads and never reach this handler.
#[derive(Clone, Copy)]
struct RoutineTab(PanelId);

#[derive(Clone, Copy)]
enum TabDrop {
    Before,
    After,
    Above,
    Below,
}

fn split_edge(bounds: Bounds<Pixels>, point: Point<Pixels>) -> Option<TabDrop> {
    if point.y < bounds.top() + bounds.size.height * 0.30 {
        Some(TabDrop::Above)
    } else if point.y > bounds.bottom() - bounds.size.height * 0.30 {
        Some(TabDrop::Below)
    } else {
        None
    }
}

fn routine_target(
    view: Stateful<Div>,
    target: PanelId,
    strip: bool,
    state: &SkinState,
) -> Stateful<Div> {
    let bounds = Rc::new(Cell::new(Bounds::default()));
    let measured = bounds.clone();
    let direction = move |w: &Window| {
        let bounds: Bounds<Pixels> = bounds.get();
        let point = w.mouse_position();
        if strip {
            Some(if point.x < bounds.center().x {
                TabDrop::Before
            } else {
                TabDrop::After
            })
        } else {
            split_edge(bounds, point)
        }
    };
    let indicator = direction.clone();
    let ws = state.workspace.clone();
    let owner = ws.clone();
    view.on_prepaint(move |b, _, _| measured.set(b))
        .drag_over::<RoutineTab>(move |style, drag, w, cx| {
            let Some(edge) = indicator(w) else {
                return style;
            };
            if !ws
                .upgrade()
                .is_some_and(|ws| ws.read(cx).routine_move(drag.0, target, edge, cx).is_some())
            {
                return style;
            }
            // Between tabs an insertion line marks the slot. A split is shown
            // by the content's own region preview, painted by the overlay.
            let style = style.border_color(materials(cx).accent);
            match edge {
                TabDrop::Before => style.border_l_2(),
                TabDrop::After => style.border_r_2(),
                TabDrop::Above | TabDrop::Below => style,
            }
        })
        .on_drop(move |drag: &RoutineTab, w, cx| {
            if let Some(edge) = direction(w) {
                let _ = owner.update(cx, |ws, cx| {
                    if let Some(dest) = ws.routine_move(drag.0, target, edge, cx) {
                        ws.remember_layout(None, cx);
                        ws.dock.update(cx, |dock, cx| {
                            dock.move_panel(drag.0, dest, w, cx);
                            dock.select_panel(drag.0, w, cx);
                        });
                    }
                });
            }
        })
}

impl Workspace {
    fn routine_move(
        &self,
        from: PanelId,
        to: PanelId,
        edge: TabDrop,
        cx: &App,
    ) -> Option<super::InsertTarget> {
        use super::{InsertTarget, PaneRef};
        use gpui_kit::base::Placement;
        if !self.dock.read(cx).is_locked() {
            return None;
        }
        let tree = self.center(cx);
        let source = tree.find_panel_node(from)?;
        let node = tree.find_panel_node(to)?;
        let PaneRef::Tabs { panels, .. } = tree.find_node(node)?.kind() else {
            return None;
        };
        // The selected tab can be dragged onto its own group's content edge.
        // The other member stays in the original slot.
        let to = if from == to {
            if panels.len() != 2 || !matches!(edge, TabDrop::Above | TabDrop::Below) {
                return None;
            }
            *panels.iter().find(|p| **p != from)?
        } else {
            to
        };
        let home = self.panes.get(&from)?.pane.read(cx).ty.home;
        if self.panes.get(&to)?.pane.read(cx).ty.home != home {
            return None;
        }
        if home == Home::Main {
            if source != node
                || !panels.iter().all(|p| {
                    self.panes
                        .get(p)
                        .is_some_and(|p| super::browser_of(&p.pane, cx).is_some())
                })
            {
                return None;
            }
        } else if super::sidebar::arrangement(&tree, from, to).is_none() {
            return None;
        }
        match edge {
            TabDrop::Before | TabDrop::After => {
                let ix = panels
                    .iter()
                    .filter(|p| **p != from)
                    .position(|p| *p == to)?
                    + usize::from(matches!(edge, TabDrop::After));
                if panels.get(ix) == Some(&from) {
                    return None;
                }
                Some(InsertTarget::Tabs {
                    node,
                    ix: Some(ix),
                    activate: true,
                })
            }
            TabDrop::Above | TabDrop::Below if home != Home::Main => {
                if source != node {
                    let order: Vec<_> = tree.panels().filter(|p| *p == from || *p == to).collect();
                    if (order.first() == Some(&from)) == matches!(edge, TabDrop::Above) {
                        return None;
                    }
                }
                Some(InsertTarget::Split {
                    node,
                    placement: if matches!(edge, TabDrop::Above) {
                        Placement::Top
                    } else {
                        Placement::Bottom
                    },
                    size: None,
                })
            }
            _ => None,
        }
    }
}

/// The space after the tabs, where a dragged tab lands at the end.
fn drop_space(group: &TabGroupContext, id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let m = *materials(cx);
    let node = group.node();
    let count = group.panels().len();
    let group = group.clone();
    div()
        .id(id)
        .h_full()
        .flex_1()
        .min_w(px(crate::theme::size::TAB_DROP))
        .when(group.is_droppable(), |d| {
            d.drag_over::<DragPanel>(move |d, _, _, _| d.bg(m.accent.opacity(0.08)))
                .on_drop(move |drag: &DragPanel, w, cx| {
                    // A tab dropped past its own group's last tab moves to the end.
                    let ix = if drag.source() == node {
                        count.saturating_sub(1)
                    } else {
                        count
                    };
                    group.drop_panel(drag.clone(), Some(ix), true, w, cx);
                })
        })
}

/// What follows the pointer while a tab is dragged.
struct TabPreview {
    label: SharedString,
}
impl Render for TabPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = materials(cx);
        div()
            .h(px(PREVIEW_HEIGHT))
            .max_w(px(220.))
            .px(px(12.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(m.overlay)
            .border_1()
            .border_color(m.control_line)
            .text_size(px(crate::theme::size::TEXT))
            .text_color(m.text)
            .opacity(0.9)
            .child(div().truncate().child(self.label.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::Dwell;

    #[test]
    fn resting_over_a_hidden_tab_opens_it_once() {
        let mut dwell = Dwell::default();
        let rest = dwell.moved(1, true, false).expect("arriving starts a rest");
        // Moving within the tab does not restart it.
        assert_eq!(dwell.moved(1, true, false), None);
        assert!(dwell.lasted(1, rest));
        assert!(!dwell.lasted(1, rest), "a rest opens its tab only once");
    }

    #[test]
    fn leaving_or_moving_on_cancels_the_rest() {
        let mut dwell = Dwell::default();
        let first = dwell.moved(1, true, false).unwrap();
        dwell.moved(1, false, false);
        assert!(!dwell.lasted(1, first), "the item left the tab");

        let first = dwell.moved(1, true, false).unwrap();
        let second = dwell.moved(2, true, false).unwrap();
        assert!(!dwell.lasted(1, first), "the item moved to another tab");
        assert!(dwell.lasted(2, second));
    }

    #[test]
    fn the_shown_tab_needs_no_rest() {
        let mut dwell = Dwell::default();
        assert_eq!(dwell.moved(1, true, true), None);
        // Leaving a tab the item never rested on changes nothing.
        let rest = dwell.moved(2, true, false).unwrap();
        dwell.moved(1, false, false);
        assert!(dwell.lasted(2, rest));
    }
}

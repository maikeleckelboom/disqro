//! Group-local tab strips and navigation above the panel they describe.
//! The dock owns the layout, panel dragging, tab order and selection.
mod tabs;
use super::Workspace;
use crate::{
    panels::{Home, Pane, nav::DraggedFolder},
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
const PREVIEW_HEIGHT: f32 = 28.;
pub const STRIP_HEIGHT: f32 = crate::theme::size::HEADER;
/// The text inset on either side of a tab label.
pub const TAB_X: f32 = 12.;
const TAB_ICON: f32 = crate::theme::size::ICON_SMALL;
const TAB_GAP: f32 = 8.;
const TAB_CLOSE: f32 = 20.;
const TAB_END: f32 = 6.;

/// Shared panel commands and hover activation for all dock groups.
pub struct SkinState {
    pub workspace: WeakEntity<Workspace>,
    dwell: RefCell<Dwell<PanelId>>,
    routine_drag: RefCell<Option<(PanelId, WeakEntity<TabPreview>)>>,
    /// The width of each group's content, as last painted.
    widths: RefCell<HashMap<NodeId, Pixels>>,
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
            widths: RefCell::default(),
        })
    }
    pub fn can_open_beside(&self, node: NodeId) -> bool {
        // Keep both tables readable when opening a browser beside this group.
        self.widths
            .borrow()
            .get(&node)
            .is_some_and(|width| *width >= px(2. * 320. + 4.))
    }
    pub fn retain_groups(&self, tree: &gpui_kit::component::dock::PaneTree) {
        self.widths
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
        })
    }
}

/// One tab group's appearance.
struct Tabs {
    state: Rc<SkinState>,
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
        let editing = tabs.iter().copied().find(|&ix| {
            pane_of(&group.panels()[ix]).is_some_and(|p| p.read(cx).renaming.is_some())
        });
        let several = tabs.len() > 1;
        let tabs = shown_tabs(tabs, editing);
        if tabs.is_empty() {
            return Empty.into_any_element();
        }
        let utility = tabs.iter().all(|&ix| is_utility(&group.panels()[ix], cx));
        let m = *materials(cx);
        let node = group.node();
        let selected = if tabs.contains(&group.active_ix()) {
            group.active_ix()
        } else {
            tabs[0]
        };
        let shown = editing.unwrap_or(selected);
        let tabs: Vec<_> = tabs
            .into_iter()
            .map(|ix| (ix, tabs::label_width(group, ix, window, cx)))
            .collect();
        let group = group.clone();
        let state = self.state.clone();
        container_query(move |size, window, cx| {
            let snap = |width| window.pixel_snap(px(width));
            let gap = snap(tabs::SPACING);
            let labels = tabs::TabLabels::new(&tabs, gap);
            let widths = labels.fit(size.width, snap);
            let overflow = widths.overflow;
            let inline = !overflow && tabs.len() > 1;
            let visible: Vec<_> = tabs
                .iter()
                .filter(|&&(ix, _)| !overflow || ix == shown)
                .map(|&(ix, natural)| {
                    let width = if inline { natural } else { widths.tabs };
                    let compact_width = (width < natural).then_some(width);
                    (ix, tab(&group, ix, &state, width, compact_width, cx))
                })
                .collect();
            let bounds = Rc::new(RefCell::new(Vec::new()));
            let measured = bounds.clone();
            let indices: Vec<_> = visible.iter().map(|(ix, _)| *ix).collect();
            // Panel commands live on each tab's own context menu, where their
            // scope is the tab. The strip offers a switcher only while it is
            // too narrow to show every tab.
            let switcher = overflow.then(|| {
                let select = group.clone();
                Button::new("tab-switcher")
                    .ghost()
                    .xsmall()
                    .w(widths.switcher)
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
            h_flex()
                .id(("tab-strip", node.as_u64()))
                .relative()
                .w(size.width)
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
                        .on_children_prepainted(move |rects, _, _| {
                            *measured.borrow_mut() = indices.iter().copied().zip(rects).collect();
                        })
                        .id(("tabs", node.as_u64()))
                        .h_full()
                        .min_w_0()
                        .w(widths.tabs)
                        .flex_shrink_0()
                        .gap(gap)
                        .tab_group()
                        .role(Role::TabList)
                        .overflow_hidden()
                        .children(visible.into_iter().map(|(_, tab)| tab)),
                )
                .children(switcher)
                .child(tabs::drop_targets(
                    &group,
                    &state,
                    bounds,
                    widths.switcher,
                    cx,
                ))
        })
        .h(px(STRIP_HEIGHT))
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
        // Each group's width decides whether a browser can open beside it,
        // whether or not the group shows tabs.
        let (node, headers) = (group.node(), self.state.clone());
        let body = div()
            .id("tab-content")
            .flex_1()
            .relative()
            .on_prepaint(move |bounds, _, _| {
                headers.widths.borrow_mut().insert(node, bounds.size.width);
            })
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
            routine_split_target(body.child(overlay), target, &self.state).into_any_element()
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

/// The tabs a group's strip shows, of the visible panels in `visible`.
///
/// Tabs are ordinary workspace chrome: every group names its panels,
/// whether it holds one panel or several, and whether or not the layout is
/// being customized. While a tab is renamed, its strip shows only that tab.
fn shown_tabs(visible: Vec<usize>, renaming: Option<usize>) -> Vec<usize> {
    match renaming {
        Some(ix) => visible.into_iter().filter(|&v| v == ix).collect(),
        None => visible,
    }
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
    width: Pixels,
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
    // The panel the listener is working in; its selected tab carries a rule.
    let active_pane = pane.as_ref().is_some_and(|p| p.read(cx).is_active());
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
        .is_none_or(|width| width >= tabs::tab_width(px(3. * crate::theme::size::TEXT), false, px));
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
        .w(width)
        .min_w_0()
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
        .overflow_hidden()
        .text_color(if selected { m.text } else { m.text_muted })
        .when(selected, |d| d.bg(surface))
        .when(selected && active_pane, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(crate::theme::size::TAB_RADIUS))
                    .right(px(crate::theme::size::TAB_RADIUS))
                    .h(px(2.))
                    .rounded_b(px(1.))
                    .bg(m.accent),
            )
        })
        .hover(move |d| {
            if selected {
                d
            } else {
                d.bg(m.hover).text_color(m.text)
            }
        })
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
                    .rounded(px(crate::theme::size::RADIUS - 2.))
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
        tab.on_drag(RoutineTab(id), move |_, _, _, cx| {
            let label = title.clone();
            let preview = cx.new(|_| TabPreview { label });
            source_state
                .routine_drag
                .replace(Some((id, preview.downgrade())));
            preview
        })
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

fn routine_split_target(view: Stateful<Div>, target: PanelId, state: &SkinState) -> Stateful<Div> {
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let measured = bounds.clone();
    let ws = state.workspace.clone();
    view.on_prepaint(move |b, _, _| measured.set(b))
        .on_drop(move |drag: &RoutineTab, w, cx| {
            if let Some(edge) = split_edge(bounds.get(), w.mouse_position()) {
                let _ = ws.update(cx, |ws, cx| {
                    ws.drop_routine_tab(drag.0, target, edge, w, cx)
                });
            }
        })
}

impl Workspace {
    fn drop_routine_tab(
        &mut self,
        from: PanelId,
        to: PanelId,
        edge: TabDrop,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(dest) = self.routine_move(from, to, edge, cx) {
            self.remember_layout(None, cx);
            self.dock.update(cx, |dock, cx| {
                dock.move_panel(from, dest, window, cx);
                dock.select_panel(from, window, cx);
            });
        }
    }

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
    fn every_group_shows_its_tabs_whether_it_holds_one_panel_or_several() {
        assert_eq!(super::shown_tabs(vec![0], None), [0]);
        assert_eq!(super::shown_tabs(vec![0, 2, 3], None), [0, 2, 3]);
        assert!(super::shown_tabs(vec![], None).is_empty());
    }

    #[test]
    fn renaming_a_tab_shows_only_that_tab() {
        assert_eq!(super::shown_tabs(vec![0, 2, 3], Some(2)), [2]);
        assert_eq!(super::shown_tabs(vec![4], Some(4)), [4]);
    }

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

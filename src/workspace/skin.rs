//! Group-local tab strips and navigation above the panel they describe.
//! The dock owns the layout, panel dragging, tab order and selection.
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
        Icon, Sizable,
        button::{Button, ButtonVariants},
        dock::{
            DockAreaRenderer, DockContext, DockSkin, DragPanel, DropIndicator, NodeId, PanelId,
            PanelState, TabGroupContext, TabGroupRenderer,
        },
        h_flex,
        input::Input,
        menu::{ContextMenuExt, DropdownMenu},
        tooltip::Tooltip,
    },
    prelude::*,
    *,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, time::Duration};

/// How long an item dragged over a tab rests there before the tab opens.
const DWELL: Duration = Duration::from_millis(550);
pub const TAB_HEIGHT: f32 = 28.;
pub const STRIP_HEIGHT: f32 = 34.;

/// Shared panel commands and hover activation for all dock groups.
pub struct SkinState {
    pub workspace: WeakEntity<Workspace>,
    dwell: RefCell<Dwell<PanelId>>,
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
            dwell: RefCell::new(Dwell::default()),
            widths: RefCell::default(),
        })
    }
    pub fn can_open_beside(&self, node: NodeId) -> bool {
        // Room for navigation, search and a readable title in each half.
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
    fn render_tab_bar(&self, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> AnyElement {
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
        let menu = group.active_panel().and_then(pane_of).map(|pane| {
            let workspace = self.state.workspace.clone();
            Button::new("group-menu")
                .ghost()
                .xsmall()
                .icon(Icon::new(IconName::Ellipsis).text_color(m.text_faint))
                .accessibility_label("Panel menu")
                .tooltip("More")
                .tab_stop(false)
                .dropdown_menu(move |menu, w, cx| super::tab_menu(&workspace, &pane, menu, w, cx))
        });
        let active = group
            .active_panel()
            .and_then(pane_of)
            .map(|p| p.read(cx).content.clone());
        let has_address = active
            .as_ref()
            .is_some_and(|content| content.as_any().is::<crate::panels::browser::BrowserPane>());
        let single_browser = has_address && tabs.len() == 1;
        let height = if has_address && !single_browser {
            STRIP_HEIGHT * 2.
        } else {
            STRIP_HEIGHT
        };
        let group = group.clone();
        let state = self.state.clone();
        let scroll = self.scroll.clone();
        container_query(move |size, _, cx| {
            state.widths.borrow_mut().insert(node, size.width);
            let compact = utility && tabs.len() > 1 && size.width < px(196.);
            let mut address = active
                .as_ref()
                .and_then(|content| {
                    content
                        .as_any()
                        .downcast_ref::<crate::panels::browser::BrowserPane>()
                })
                .map(|browser| browser.address_bar(size.width, cx));
            let strip = h_flex()
                .id(("tab-strip", node.as_u64()))
                .w_full()
                .h(px(STRIP_HEIGHT))
                .flex_shrink_0()
                .when(!compact, |d| d.pr(px(4.)))
                .when(!utility, |d| d.pl(px(4.)).gap(px(2.)))
                .items_center()
                .bg(if utility { m.raised } else { m.chrome })
                .child(
                    h_flex()
                        .id(("tabs", node.as_u64()))
                        .min_w_0()
                        .when(!utility, |d| d.gap(px(2.)))
                        .overflow_x_scroll()
                        .track_scroll(&scroll)
                        .children(
                            tabs.into_iter()
                                .map(|ix| tab(&group, ix, &state, compact, cx)),
                        ),
                )
                .when(single_browser, |d| d.children(address.take()))
                .when(!single_browser, |d| {
                    d.child(
                        drop_space(&group, ("strip-space", node.as_u64()), cx)
                            .when(compact, |d| d.min_w_0()),
                    )
                })
                .children(menu);
            gpui_kit::component::v_flex()
                .size_full()
                .bg(m.chrome)
                .child(strip)
                .when_some(address, |d, bar| {
                    d.child(h_flex().w_full().h(px(STRIP_HEIGHT)).child(bar))
                })
        })
        .h(px(height))
        .flex_shrink_0()
        .into_any_element()
    }
    fn render_active_panel(
        &self,
        panel: AnyView,
        _: &TabGroupContext,
        window: &mut Window,
        _: &mut App,
    ) -> AnyElement {
        div()
            .id("tab-content")
            .flex_1()
            .relative()
            .child(if window.is_a11y_active() {
                div().absolute().size_full().child(panel).into_any_element()
            } else {
                panel
                    .cached(StyleRefinement::default().absolute().size_full())
                    .into_any_element()
            })
            .into_any_element()
    }
    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let m = materials(cx);
        let to = indicator.to();
        Some(
            div()
                .absolute()
                .left(to.origin().x)
                .top(to.origin().y)
                .w(to.size().width)
                .h(to.size().height)
                .bg(m.accent.opacity(0.10))
                .border_2()
                .border_color(m.accent.opacity(0.45))
                .rounded(px(6.))
                .into_any_element(),
        )
    }
}

/// The pane behind a dock handle; `None` for a placeholder.
pub fn pane_of(panel: &Arc<dyn gpui_kit::component::dock::BasePanelView>) -> Option<Entity<Pane>> {
    panel.as_any().downcast_ref::<Entity<Pane>>().cloned()
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
    compact: bool,
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
            SharedString::from(format!("Unavailable: {}", panel.panel_name(cx))),
            Icon::new(IconName::TriangleAlert),
            None,
            false,
        ),
    };
    // A lone browser already names its destination in the breadcrumb. Keep
    // the icon as its drag handle. An explicit custom name stays visible.
    let handle = group.panels().len() == 1
        && pane.as_ref().is_some_and(|p| {
            let p = p.read(cx);
            p.content
                .as_any()
                .is::<crate::panels::browser::BrowserPane>()
                && p.name.is_none()
                && renaming.is_none()
        });
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
    let label_element = match renaming {
        Some(input) => {
            let pane = pane.clone();
            div()
                .flex_1()
                .min_w(px(80.))
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
            .when(compact, |d| d.flex_none())
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
    h_flex()
        .id(("tab", id.as_u64()))
        .group(hover_group.clone())
        .occlude()
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(selected)
        .relative()
        .h(px(if utility { STRIP_HEIGHT } else { TAB_HEIGHT }))
        .min_w(px(48.))
        .max_w(px(220.))
        .px(px(8.))
        .gap(px(5.))
        .when(utility, |d| d.border_r_1().border_color(m.line))
        .when(!utility, |d| d.rounded(px(6.)))
        .items_center()
        .text_size(px(12.5))
        .when(compact, |d| {
            d.flex_col()
                .justify_center()
                .gap_0()
                .px(px(3.))
                .min_w(px(32.))
                .flex_shrink_0()
                .text_size(px(11.))
                .line_height(px(14.))
        })
        .text_color(if selected { m.text } else { m.text_muted })
        .when(handle, |d| {
            d.min_w(px(24.)).w(px(24.)).px_0().gap_0().justify_center()
        })
        .when(selected && !handle, |d| {
            d.bg(if utility { m.chrome } else { m.control })
        })
        .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
        .child(
            icon.size(px(14.))
                .flex_shrink_0()
                .text_color(if active_browser {
                    m.accent
                } else if selected {
                    m.text_muted
                } else {
                    m.text_faint
                }),
        )
        .when(!handle, |d| d.child(label_element))
        // The close button covers the end of the label while the pointer is
        // over the tab, so no tab gives up width to a button it is not showing.
        .when(!utility && !handle, |d| {
            d.child(
                div()
                    .id(("tab-close", id.as_u64()))
                    .absolute()
                    .top(px((TAB_HEIGHT - 18.) / 2.))
                    .right(px(4.))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(m.chrome.blend(if selected { m.control } else { m.hover }))
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
        .when(long || handle, |d| {
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
                drag.set_preview_size(size(px(140.), px(TAB_HEIGHT)));
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
        })
        .context_menu(move |menu, w, cx| match &menu_pane {
            Some(pane) => super::tab_menu(&workspace, pane, menu, w, cx),
            None => menu,
        })
        .into_any_element()
}

/// The space after the tabs, where a dragged tab lands at the end.
pub fn drop_space(group: &TabGroupContext, id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let m = *materials(cx);
    let node = group.node();
    let count = group.panels().len();
    let group = group.clone();
    div()
        .id(id)
        .h_full()
        .flex_1()
        .min_w(px(8.))
        .when(group.is_droppable(), |d| {
            d.drag_over::<DragPanel>(move |d, _, _, _| d.bg(m.accent.opacity(0.08)))
                .on_drop(move |drag: &DragPanel, w, cx| {
                    // A tab dropped past its own group's last tab moves to the end.
                    let ix = (drag.source() == node).then(|| count.saturating_sub(1));
                    group.drop_panel(drag.clone(), ix, true, w, cx);
                })
        })
}

/// The `+` that opens the panel picker, adding to `node` or the main group.
pub fn add_button(state: &Rc<SkinState>, node: Option<NodeId>, cx: &App) -> AnyElement {
    let m = materials(cx);
    let workspace = state.workspace.clone();
    Button::new(("add-panel", node.map_or(0, NodeId::as_u64)))
        .ghost()
        .xsmall()
        .icon(
            Icon::new(IconName::Plus)
                .size(px(14.))
                .text_color(m.text_faint),
        )
        .accessibility_label("Open a panel")
        .tooltip("Open a panel")
        .tab_stop(false)
        .dropdown_menu(move |menu, _, cx| super::picker_menu(&workspace, node, menu, cx))
        .into_any_element()
}

/// What follows the pointer while a tab is dragged.
struct TabPreview {
    label: SharedString,
}
impl Render for TabPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = materials(cx);
        div()
            .h(px(TAB_HEIGHT))
            .max_w(px(220.))
            .px(px(12.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(m.overlay)
            .border_1()
            .border_color(m.control_line)
            .text_size(px(12.5))
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

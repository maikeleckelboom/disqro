//! Local utility-pair commands. The dock owns pane lifetimes, geometry,
//! dividers and drag targets in both arrangements.
use super::*;
use gpui_kit::base::Placement;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum SidebarAction {
    Stack { below: PanelId, group: NodeId },
    Group { bottom: PanelId, top: NodeId },
}

impl SidebarAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Stack { .. } => "Stack Vertically",
            Self::Group { .. } => "Group as Tabs",
        }
    }

    fn movement(self) -> (PanelId, InsertTarget) {
        match self {
            Self::Stack { below, group } => (
                below,
                InsertTarget::Split {
                    node: group,
                    placement: Placement::Bottom,
                    size: None,
                },
            ),
            Self::Group { bottom, top } => (
                bottom,
                InsertTarget::Tabs {
                    node: top,
                    ix: None,
                    activate: false,
                },
            ),
        }
    }
}

/// Only offer a shortcut for a pair already sharing a group or vertical
/// divider. Other placements remain under the user's normal dock controls.
fn arrangement(tree: &PaneTree, selected: PanelId, peer: PanelId) -> Option<SidebarAction> {
    let group = tree.find_panel_node(selected)?;
    let node = tree.find_node(group)?;
    let PaneRef::Tabs { panels, .. } = node.kind() else {
        return None;
    };
    if panels.len() == 2 && panels.contains(&peer) {
        return Some(SidebarAction::Stack { below: peer, group });
    }
    if panels.len() != 1 {
        return None;
    }

    let mut action = None;
    tree.root().walk(&mut |node| {
        if let PaneRef::Split {
            axis: Axis::Vertical,
            children,
            ..
        } = node.kind()
        {
            for pair in children.windows(2) {
                if let (
                    PaneRef::Tabs { panels: [top], .. },
                    PaneRef::Tabs {
                        panels: [bottom], ..
                    },
                ) = (pair[0].kind(), pair[1].kind())
                    && ((*top == selected && *bottom == peer)
                        || (*top == peer && *bottom == selected))
                {
                    action = Some(SidebarAction::Group {
                        bottom: *bottom,
                        top: pair[0].id(),
                    });
                }
            }
        }
    });
    action
}

impl Workspace {
    pub(super) fn sidebar_action(&self, id: PanelId, cx: &App) -> Option<SidebarAction> {
        let home = self.panes.get(&id)?.pane.read(cx).ty.home;
        if home == Home::Main {
            return None;
        }
        let tree = self.center(cx);
        self.panes.iter().find_map(|(&peer, tracked)| {
            (peer != id && tracked.pane.read(cx).ty.home == home)
                .then(|| arrangement(&tree, id, peer))
                .flatten()
        })
    }

    pub(super) fn arrange_sidebar(
        &mut self,
        id: PanelId,
        action: SidebarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_action(id, cx) != Some(action) {
            return;
        }
        self.remember_layout(None, cx);
        let (panel, target) = action.movement();
        self.dock.update(cx, |dock, cx| {
            dock.move_panel(panel, target, window, cx);
            dock.select_panel(id, window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Axis, InsertTarget, PaneRef, PaneTree, PanelId, Placement, RootKind, arrangement, px,
    };

    #[test]
    fn stacking_in_either_order_and_regrouping_preserves_the_outer_slots() {
        let [browser, first, second] = [1, 2, 3].map(PanelId::from_u64);
        for (top, bottom) in [(first, second), (second, first)] {
            let mut tree = PaneTree::new(RootKind::Split);
            tree.split(tree.root().id(), browser, Placement::Right, Some(px(600.)));
            tree.split(
                tree.find_panel_node(browser).unwrap(),
                first,
                Placement::Left,
                Some(px(220.)),
            );
            let group = tree.find_panel_node(first).unwrap();
            tree.insert_panel(
                second,
                InsertTarget::Tabs {
                    node: group,
                    ix: None,
                    activate: true,
                },
            );
            let widths = |tree: &PaneTree| match tree.root().kind() {
                PaneRef::Split {
                    axis: Axis::Horizontal,
                    sizes,
                    ..
                } => sizes.to_vec(),
                _ => panic!("expected outer columns"),
            };
            let original = widths(&tree);
            let browser_node = tree.find_panel_node(browser);
            let (panel, target) = arrangement(&tree, top, bottom).unwrap().movement();
            tree.move_panel(panel, target);
            assert_eq!(tree.panels().collect::<Vec<_>>(), [top, bottom, browser]);
            assert_eq!(widths(&tree), original);
            assert_eq!(tree.find_panel_node(browser), browser_node);
            assert_eq!(arrangement(&tree, top, browser), None);
            let (panel, target) = arrangement(&tree, bottom, top).unwrap().movement();
            tree.move_panel(panel, target);
            assert_eq!(tree.find_panel_node(top), tree.find_panel_node(bottom));
            assert_eq!(widths(&tree), original);
        }
    }
}

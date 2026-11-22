//! One previous center layout. GPUI owns the geometry and restores the same
//! live panels, including their table selection, history and input entities.
use super::*;

pub(super) struct Checkpoint {
    layout: DockLayout,
    active: Option<Entity<Browser>>,
    closed: Vec<ClosedPanel>,
    pub closed_id: Option<InstanceId>,
    pub sidebar_visibility: Option<(bool, bool)>,
}

fn live_layout(node: &PaneNode, measured: &PanelState, dock: &DockArea, cx: &App) -> DockLayout {
    match node.kind() {
        PaneRef::Tabs { panels, active_ix } => panels
            .iter()
            .fold(DockLayout::tabs(), |tabs, id| {
                tabs.panel_view(dock.panel(*id).expect("registered dock panel").clone(), cx)
            })
            .active_index(active_ix),
        PaneRef::Split { axis, children, .. } => {
            let split = match axis {
                Axis::Horizontal => DockLayout::h_split(),
                Axis::Vertical => DockLayout::v_split(),
            };
            let sizes = measured.info.sizes().expect("measured dock split");
            children.iter().zip(&measured.children).zip(sizes).fold(
                split,
                |split, ((child, measured), size)| {
                    split.child(live_layout(child, measured, dock, cx), Some(*size))
                },
            )
        }
    }
}

impl Workspace {
    fn checkpoint(&self, closed_id: Option<InstanceId>, cx: &App) -> Checkpoint {
        let dock = self.dock.read(cx);
        let tree = self.center(cx);
        Checkpoint {
            layout: live_layout(tree.root(), &dock.dump(cx).center, dock, cx),
            active: self.browser(cx),
            closed: self.closed.clone(),
            closed_id,
            sidebar_visibility: None,
        }
    }

    pub(super) fn remember_layout(&mut self, closed_id: Option<InstanceId>, cx: &App) {
        self.drag_checkpoint = None;
        self.undo = Some(self.checkpoint(closed_id, cx));
    }

    pub(super) fn start_panel_drag(&mut self, panel: PanelId, node: NodeId, ix: usize, cx: &App) {
        self.drag_checkpoint = Some((panel, node, ix, self.checkpoint(None, cx)));
    }

    pub(super) fn finish_panel_drag(&mut self, cx: &App) {
        let Some((id, node, ix, _)) = &self.drag_checkpoint else {
            return;
        };
        let tree = self.center(cx);
        let unchanged = tree.find_node(*node).is_some_and(
            |n| matches!(n.kind(), PaneRef::Tabs { panels, .. } if panels.get(*ix) == Some(id)),
        );
        if !unchanged {
            self.undo = self.drag_checkpoint.take().map(|(_, _, _, before)| before);
        }
    }

    pub(super) fn undo_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_checkpoint = None;
        let Some(before) = self.undo.take() else {
            return;
        };
        self.dock
            .update(cx, |dock, cx| dock.set_center(before.layout, window, cx));
        if let Some((left, right)) = before.sidebar_visibility {
            self.left_hidden = left;
            self.right_hidden = right;
            self.sync_queue_button(cx);
        }
        self.closed = before.closed;
        self.services
            .router
            .update(cx, |r, cx| r.set_active(before.active.as_ref(), cx));
        self.layout_changed(window, cx);
    }
}

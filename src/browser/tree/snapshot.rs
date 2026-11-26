use super::*;
pub(super) use crate::ordered_result::{Snapshot, Writer};

impl Writer {
    pub(super) fn append_nodes(&self, page: Branch, field: Field) -> anyhow::Result<()> {
        for node in page.nodes() {
            self.append(&node.id, node, order_value(node, field), 0, 0)?;
        }
        Ok(())
    }
}

impl Snapshot {
    pub(super) fn page(self: &Arc<Self>, offset: usize) -> anyhow::Result<Branch> {
        self.page_with_limit(offset, PAGE)
    }

    pub(super) fn page_with_limit(
        self: &Arc<Self>,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Branch> {
        let mut nodes = self.rows::<Node>(offset, limit)?;
        for node in &mut nodes {
            // A queue persists only recording locators. A catalog result also owns
            // its database-local ID, which the node stores alongside the payload.
            if let (TreeId::IndexedTrack(id), Item::Track { track, .. }) =
                (&node.id, &mut node.item)
            {
                Arc::make_mut(track).library_id = Some(*id);
            }
        }
        let end = offset + nodes.len();
        let mut page = Branch::page(nodes, (end < self.len).then_some(Cursor::Offset(end)));
        page.snapshot = Some(self.clone());
        Ok(page)
    }
}

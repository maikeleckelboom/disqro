//! Resident page budgets and the layout descriptors retained on eviction.
use super::*;

impl Tree {
    /// Reattach the saved page boundaries before projecting the reloaded parent.
    pub(super) fn restore_children(&mut self, owner: &Option<TreeId>, index: usize) {
        let branch = self.branches.get_mut(owner).unwrap();
        let page = &mut branch.pages[index];
        if page.children.is_empty() {
            return;
        }
        let children = std::mem::take(&mut page.children);
        let deep = branch.deep;
        let present: HashSet<_> = page.nodes.iter().flatten().map(|n| n.id.clone()).collect();
        for (id, child) in children {
            if present.contains(&id) && self.is_open(&id, deep) {
                self.branches.insert(Some(id), child);
            }
        }
        self.reconcile();
    }

    pub(super) fn release_page(
        &mut self,
        owner: &Option<TreeId>,
        index: usize,
        lens: Option<&Lens>,
    ) {
        self.unload_page(owner, index, lens);
        self.reconcile();
    }

    /// Fold expanded descendants into the parent slot. Their cursors and
    /// row counts survive, so each child can reload its own viewport page.
    pub(super) fn unload_page(
        &mut self,
        owner: &Option<TreeId>,
        index: usize,
        lens: Option<&Lens>,
    ) {
        let branch = &self.branches[owner];
        let nodes = branch.pages[index].nodes.as_deref().expect("resident page");
        let rows = |filter: Option<&Lens>| {
            let mut view = Projection::new(self, filter);
            for node in nodes {
                view.node(
                    node,
                    owner,
                    0,
                    branch.deep,
                    filter.is_none_or(|l| l.matches(node)),
                );
            }
            view.len
        };
        let visible = rows(None);
        let matched = lens.map(|l| rows(Some(l)));
        let nodes = self.branches.get_mut(owner).unwrap().pages[index]
            .nodes
            .take()
            .unwrap();
        let mut children = Vec::new();
        for node in nodes.into_iter().filter(Node::expandable) {
            let id = node.id;
            let key = Some(id.clone());
            let Some(child) = self.branches.get(&key) else {
                continue;
            };
            // An empty folder and its loading message both occupy one row.
            if child.pages.iter().all(|p| p.len == 0)
                && child.more.is_none()
                && child.notice.is_none()
            {
                continue;
            }
            for i in 0..child.pages.len() {
                if self.branches[&key].pages[i].nodes.is_some() {
                    self.unload_page(&key, i, lens);
                }
            }
            let mut child = self.branches.remove(&key).unwrap();
            child.wanted = None;
            children.push((id, child));
        }
        let page = &mut self.branches.get_mut(owner).unwrap().pages[index];
        page.children = children;
        page.rows = visible;
        page.matched_rows = matched;
    }

    pub(super) fn settle(&mut self, id: &Option<TreeId>, keep: usize, lens: Option<&Lens>) {
        let over_branch = |tree: &Self| {
            tree.branches
                .get(id)
                .is_some_and(|b| b.loaded_pages() > RESIDENT_PAGES)
        };
        if !over_branch(self) && self.loaded() <= self.node_limit() {
            return;
        }
        let protected = self.selected_pages();
        while over_branch(self) {
            let victim = self.branches[id]
                .pages
                .iter()
                .enumerate()
                .filter(|(i, p)| p.nodes.is_some() && *i != keep && protected.get(id) != Some(i))
                .min_by_key(|(i, p)| (p.used, std::cmp::Reverse(i.abs_diff(keep))))
                .map(|(i, _)| i);
            let Some(i) = victim else { break };
            self.release_page(id, i, lens);
        }
        while self.loaded() > self.node_limit() {
            let victim = self
                .branches
                .iter()
                .flat_map(|(owner, branch)| {
                    branch
                        .pages
                        .iter()
                        .enumerate()
                        .map(move |(i, p)| (owner, i, p))
                })
                .filter(|(owner, i, p)| p.resident() > 0 && !(*owner == id && *i == keep))
                .min_by_key(|(owner, i, p)| (protected.get(*owner) == Some(i), p.used, *owner, *i))
                .map(|(owner, i, _)| (owner.clone(), i));
            let Some((owner, i)) = victim else { break };
            self.release_page(&owner, i, lens);
        }
    }

    /// Selection is reachable only while every page on its path is reachable.
    fn selected_pages(&self) -> HashMap<Option<TreeId>, usize> {
        let mut path = HashMap::new();
        let Some(selected) = &self.selected else {
            return path;
        };
        let Some((mut parent, page)) = self.branches.iter().find_map(|(parent, b)| {
            b.pages
                .iter()
                .position(|p| p.nodes.iter().flatten().any(|n| &n.id == selected))
                .map(|i| (parent.clone(), i))
        }) else {
            return path;
        };
        path.insert(parent.clone(), page);
        while let Some(id) = parent {
            let Some(owner) = self.links.get(&id) else {
                break;
            };
            let Some(page) = self.branches.get(owner).and_then(|b| {
                b.pages
                    .iter()
                    .position(|p| p.nodes.iter().flatten().any(|n| n.id == id))
            }) else {
                break;
            };
            if path.insert(owner.clone(), page).is_some() {
                break;
            }
            parent = owner.clone();
        }
        path
    }
}

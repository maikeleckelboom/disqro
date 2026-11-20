//! The folder tree: the open folder in the context of the place it belongs to.
//!
//! The tree is rooted at an anchor: the outermost Place or library
//! collection that contains the open folder, or its drive when none does.
//! Opening Music never shows the rest of the home folder. The anchor stays
//! while the listener moves within it, so rows do not shift under the
//! pointer; it changes only on leaving it or on choosing a Place.
//!
//! Entries are identified by path. Expansion is remembered per path, and
//! navigation expands the open folder and its ancestors without collapsing
//! anything. Subfolders are listed off the interface thread, together with
//! the subfolders of every visible entry so that expanders are accurate.
//! Opening a folder lists it afresh through discovery; Refresh lists the
//! visible folders inside it again.
use crate::library::{self, within};
use gpui_kit::{
    component::tree::{TreeEvent, TreeItem, TreeState},
    prelude::*,
    *,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

/// A path as Windows compares it.
fn key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The outermost of `candidates` containing `path`, else its drive or share.
fn anchor<'a>(candidates: impl Iterator<Item = &'a PathBuf>, path: &Path) -> PathBuf {
    candidates
        .filter(|c| within(path, c))
        .min_by_key(|c| c.as_os_str().len())
        .cloned()
        .or_else(|| path.ancestors().last().map(Path::to_owned))
        .unwrap_or_else(|| path.to_owned())
}

pub struct Folders {
    pub state: Entity<TreeState>,
    anchor: Option<PathBuf>,
    current: Option<PathBuf>,
    /// The open folder is highlighted while the browser shows a folder.
    highlight: bool,
    listed: HashMap<String, Arc<[PathBuf]>>,
    pending: HashSet<String>,
    expanded: HashSet<String>,
    ids: HashMap<SharedString, PathBuf>,
    /// Places and collections that can anchor the tree. Drives are not among
    /// them: they are the fallback.
    places: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    roots_known: bool,
    /// The open folder is kept in view until the folders above it are
    /// listed, since their listings can push it down.
    revealing: bool,
}

impl Folders {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| TreeState::new(cx));
        cx.subscribe(&state, |f: &mut Self, _, event, cx| {
            // The keyboard expands and collapses inside the tree itself.
            let (id, open) = match event {
                TreeEvent::Expanded(id) => (id, true),
                TreeEvent::Collapsed(id) => (id, false),
            };
            if let Some(path) = f.path(id) {
                f.set_expanded(&path, open, cx);
            }
        })
        .detach();
        Self {
            state,
            anchor: None,
            current: None,
            highlight: false,
            listed: HashMap::new(),
            pending: HashSet::new(),
            expanded: HashSet::new(),
            ids: HashMap::new(),
            places: vec![],
            roots: vec![],
            roots_known: false,
            revealing: false,
        }
    }

    /// The folder a tree entry stands for.
    pub fn path(&self, id: &SharedString) -> Option<PathBuf> {
        self.ids.get(id).cloned()
    }
    /// Nothing has been shown yet.
    pub fn is_empty(&self) -> bool {
        self.anchor.is_none()
    }

    /// Places and collections decide later anchors. The tree on screen stays
    /// as it is, except that the first list of collections, which arrives
    /// after startup, can lift a tree that fell back to its drive.
    pub fn set_places(&mut self, places: Vec<PathBuf>) {
        self.places = places;
    }
    pub fn set_roots(&mut self, roots: Vec<PathBuf>, cx: &mut Context<Self>) {
        let first = !self.roots_known;
        self.roots_known = true;
        self.roots = roots;
        if let (true, Some(current), Some(anchor)) = (first, &self.current, &self.anchor)
            && self.candidates().all(|c| !within(anchor, c))
        {
            let current = current.clone();
            self.anchor = Some(self.anchor_for(&current));
            self.reveal(&current);
            self.update(true, cx);
        }
    }
    fn candidates(&self) -> impl Iterator<Item = &PathBuf> {
        self.places.iter().chain(&self.roots)
    }
    fn anchor_for(&self, path: &Path) -> PathBuf {
        anchor(self.candidates(), path)
    }

    /// Shows `path` as the open folder. `place` anchors the tree at it, as
    /// choosing a Place does; otherwise the anchor stays while it contains
    /// `path`.
    pub fn show(&mut self, path: &Path, place: bool, cx: &mut Context<Self>) {
        if path.as_os_str().is_empty() {
            return;
        }
        let keep = !place
            && self
                .anchor
                .as_ref()
                .is_some_and(|anchor| within(path, anchor));
        if !keep {
            self.anchor = Some(if place {
                path.to_owned()
            } else {
                self.anchor_for(path)
            });
        }
        // Discovery lists the folder afresh and reports it through `listed`.
        self.current = Some(path.to_owned());
        self.highlight = true;
        self.revealing = true;
        self.reveal(path);
        self.update(true, cx);
    }
    /// Keeps the tree around the last folder while a library view is shown.
    pub fn set_highlight(&mut self, highlight: bool, cx: &mut Context<Self>) {
        if self.highlight != highlight {
            self.highlight = highlight;
            self.update(true, cx);
        }
    }
    /// The subfolders of `path` as discovery just listed them.
    pub fn listed(&mut self, path: &Path, dirs: &[PathBuf], cx: &mut Context<Self>) {
        let k = key(path);
        if self.listed.get(&k).is_some_and(|known| **known == *dirs) {
            return;
        }
        self.pending.remove(&k);
        self.listed.insert(k, dirs.into());
        self.update(false, cx);
    }
    /// Lists `path` and the folders shown inside it again. The tree keeps
    /// showing the earlier listings until the new ones arrive.
    pub fn refresh(&mut self, path: &Path, cx: &mut Context<Self>) {
        let visible = self.visible();
        let shown: HashSet<String> = visible.iter().map(|p| key(p)).collect();
        self.listed
            .retain(|k, _| shown.contains(k) || !within(Path::new(k), path));
        let todo = visible
            .into_iter()
            .filter(|p| within(p, path) && self.pending.insert(key(p)))
            .collect();
        self.list(todo, cx);
    }
    pub fn toggle(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if let Some(path) = self.path(id) {
            let open = !self.expanded.contains(&key(&path));
            self.set_expanded(&path, open, cx);
        }
    }
    fn set_expanded(&mut self, path: &Path, open: bool, cx: &mut Context<Self>) {
        let k = key(path);
        let changed = if open {
            self.expanded.insert(k)
        } else {
            self.expanded.remove(&k)
        };
        if changed {
            self.update(false, cx);
        }
    }
    /// Expands the anchor and every folder down to `path`, including it.
    fn reveal(&mut self, path: &Path) {
        if let Some(anchor) = &self.anchor {
            for p in path.ancestors().take_while(|p| within(p, anchor)) {
                self.expanded.insert(key(p));
            }
        }
    }

    /// The chain from the anchor to the open folder, outermost first.
    fn chain(&self) -> Vec<PathBuf> {
        let (Some(anchor), Some(current)) = (&self.anchor, &self.current) else {
            return vec![];
        };
        let mut chain: Vec<PathBuf> = current
            .ancestors()
            .take_while(|p| within(p, anchor))
            .map(Path::to_owned)
            .collect();
        chain.reverse();
        chain
    }
    /// What a folder shows beneath it. A folder on the way to the open one
    /// is shown even when listing passes over it, as for a dot-folder.
    fn children(&self, path: &Path, chain: &[PathBuf]) -> Vec<PathBuf> {
        let mut kids = self
            .listed
            .get(&key(path))
            .map(|k| k.to_vec())
            .unwrap_or_default();
        if let Some(next) = chain
            .iter()
            .position(|p| key(p) == key(path))
            .and_then(|i| chain.get(i + 1))
            && !kids.iter().any(|k| key(k) == key(next))
        {
            kids.push(next.clone());
            library::sort_by_name(&mut kids, library::folder_name);
        }
        kids
    }
    fn item(&mut self, path: &Path, chain: &[PathBuf], depth: usize) -> TreeItem {
        let id = self.id(path);
        let expanded = self.expanded.contains(&key(path));
        let kids = self.children(path, chain);
        // A collapsed entry carries its children only to show an expander;
        // their own subfolders are added when it opens.
        let children: Vec<TreeItem> = if expanded && depth < 64 {
            kids.iter()
                .map(|k| self.item(k, chain, depth + 1))
                .collect()
        } else {
            kids.iter()
                .map(|k| TreeItem::new(self.id(k), library::display_name(k)))
                .collect()
        };
        TreeItem::new(id, library::display_name(path))
            .children(children)
            .expanded(expanded)
    }
    fn id(&mut self, path: &Path) -> SharedString {
        let id: SharedString = path.to_string_lossy().into_owned().into();
        self.ids
            .entry(id.clone())
            .or_insert_with(|| path.to_owned());
        id
    }
    fn current_index(&self, cx: &App) -> Option<usize> {
        let current = self.current.as_ref()?;
        let id: SharedString = current.to_string_lossy().into_owned().into();
        self.state.read(cx).index_of(&id)
    }

    /// Rebuilds the entries from the listings, keeps the keyboard selection
    /// where it was unless `select_current`, and lists what became visible.
    fn update(&mut self, select_current: bool, cx: &mut Context<Self>) {
        let Some(anchor) = self.anchor.clone() else {
            return;
        };
        let chain = self.chain();
        let root = self.item(&anchor, &chain, 0);
        let highlight = self.highlight;
        let current: Option<SharedString> = self
            .current
            .as_ref()
            .map(|c| c.to_string_lossy().into_owned().into());
        self.state.update(cx, |t, cx| {
            let previous = t.selected_item().map(|i| i.id.clone());
            t.set_items(vec![root], cx);
            let wanted = if select_current || previous.is_none() {
                current.filter(|_| highlight)
            } else {
                previous
            };
            let ix = wanted.and_then(|id| t.index_of(&id));
            t.set_selected_index(ix, cx);
        });
        if self.revealing {
            if let Some(ix) = self.current_index(cx) {
                self.state.update(cx, |t, _| {
                    t.scroll_to_item(ix, ScrollStrategy::Nearest);
                });
            }
            self.revealing = chain.iter().any(|p| !self.listed.contains_key(&key(p)));
        }
        self.list_visible(cx);
        cx.notify();
    }

    /// The folders whose subfolders the tree needs: the anchor and every
    /// folder shown inside an expanded one.
    fn visible(&self) -> Vec<PathBuf> {
        let Some(anchor) = self.anchor.clone() else {
            return vec![];
        };
        let chain = self.chain();
        let mut out = vec![];
        let mut stack = vec![anchor];
        while let Some(p) = stack.pop() {
            let k = key(&p);
            if self.expanded.contains(&k) && self.listed.contains_key(&k) {
                stack.extend(self.children(&p, &chain));
            }
            out.push(p);
        }
        out
    }
    /// Lists every visible folder whose subfolders are not known yet.
    fn list_visible(&mut self, cx: &mut Context<Self>) {
        let todo = self
            .visible()
            .into_iter()
            .filter(|p| {
                let k = key(p);
                !self.listed.contains_key(&k) && self.pending.insert(k)
            })
            .collect();
        self.list(todo, cx);
    }
    /// Lists `todo` off the interface thread. A listing that discovery
    /// reported in the meantime is newer and wins.
    fn list(&mut self, todo: Vec<PathBuf>, cx: &mut Context<Self>) {
        if todo.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    todo.into_iter()
                        .map(|p| {
                            let dirs = library::subfolders(&p);
                            (p, dirs)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |f, cx| {
                for (p, dirs) in found {
                    let k = key(&p);
                    if f.pending.remove(&k) {
                        f.listed.insert(k, dirs.into());
                    }
                }
                f.update(false, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::anchor;
    use std::path::{Path, PathBuf};
    #[test]
    fn anchors_at_the_outermost_place_else_the_drive() {
        let places = [
            PathBuf::from(r"C:\Users\Me\Music"),
            PathBuf::from(r"C:\Users\Me\Music\Archive"),
            PathBuf::from(r"D:\Rips"),
        ];
        let at = |p: &str| anchor(places.iter(), Path::new(p));
        assert_eq!(at(r"C:\Users\Me\Music"), places[0]);
        assert_eq!(at(r"c:\users\me\music\Archive\CD1"), places[0]);
        assert_eq!(at(r"D:\Rips\Live"), places[2]);
        // A sibling that only shares a prefix is not inside the place.
        assert_eq!(at(r"C:\Users\Me\Musical"), PathBuf::from(r"C:\\"));
        assert_eq!(at(r"C:\Users\Me"), PathBuf::from(r"C:\\"));
        assert_eq!(at(r"\\nas\media\Rips"), PathBuf::from(r"\\nas\media\\"));
    }
}

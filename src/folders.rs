//! Locations: every Place is the root of its own folder hierarchy.
//!
//! A Place expands in place to show its folders, which are listed off the
//! interface thread as they become visible. The open folder is shown under
//! one root: the Place the listener chose, while the folder stays inside
//! it; otherwise the outermost Place other than a drive that holds it, then
//! its drive. A folder outside every Place is shown under a temporary root
//! for its drive or share until the listener returns to a Place.
//!
//! Entries are identified by their root and path, so a folder that two
//! Places reach has two entries, each opened and closed on its own.
//! Navigation expands the chain from the root down to the open folder and
//! never collapses anything. Expansion is remembered across restarts.
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

/// Remembered open folders beyond this many are not saved.
const REMEMBERED: usize = 512;

/// A path as Windows compares it.
fn key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}
/// Shorten only a complete prefix already named by the parent. A leading
/// catalog year moves to the end so the distinguishing title gets first space.
fn display_label(path: &Path) -> String {
    let full = library::display_name(path);
    if full.chars().count() < 32 {
        return full;
    }
    let Some(parent) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
    else {
        return full;
    };
    let (year, body) = full
        .split_once("] ")
        .filter(|(year, _)| {
            year.strip_prefix('[')
                .is_some_and(|n| n.len() == 4 && n.bytes().all(|c| c.is_ascii_digit()))
        })
        .map_or(("", full.as_str()), |(year, rest)| (&year[1..], rest));
    let mut prefix = parent;
    loop {
        if let Some(tail) = body.strip_prefix(prefix).and_then(|s| s.strip_prefix(' ')) {
            let title = tail.strip_prefix("- ").unwrap_or(tail).trim();
            if !title.is_empty() {
                return if year.is_empty() {
                    title.to_owned()
                } else {
                    format!("{title} [{year}]")
                };
            }
        }
        let Some((shorter, _)) = prefix.rsplit_once(" - ") else {
            break;
        };
        if !shorter.contains(" - ") {
            break;
        }
        prefix = shorter;
    }
    full
}

/// An entry: a folder as one root shows it.
fn entry_key(root: &Path, path: &Path) -> String {
    format!("{}\u{1}{}", key(root), key(path))
}

/// The root that shows `path`: the outermost place other than a drive that
/// holds it, else a drive that does, else `None`.
fn owner<'a>(
    places: impl Iterator<Item = &'a (PathBuf, bool)> + Clone,
    path: &Path,
) -> Option<&'a PathBuf> {
    let holds = |(p, _): &&(PathBuf, bool)| within(path, p);
    places
        .clone()
        .filter(|p| !p.1)
        .filter(holds)
        .min_by_key(|(p, _)| p.as_os_str().len())
        .or_else(|| {
            places
                .filter(holds)
                .max_by_key(|(p, _)| p.as_os_str().len())
        })
        .map(|(p, _)| p)
}

/// The listener opened or closed folders, so the sidebar state is saved.
pub struct ExpansionChanged;
impl EventEmitter<ExpansionChanged> for Folders {}

pub struct Folders {
    pub state: Entity<TreeState>,
    /// The Places in order, and whether each is a drive.
    places: Vec<(PathBuf, bool)>,
    /// A drive or share shown for an open folder outside every Place.
    temporary: Option<PathBuf>,
    /// The root that shows the open folder.
    shown_in: Option<PathBuf>,
    current: Option<PathBuf>,
    /// The open folder is highlighted while the browser shows a folder.
    highlight: bool,
    listed: HashMap<String, Arc<[PathBuf]>>,
    pending: HashSet<String>,
    expanded: HashSet<String>,
    ids: HashMap<SharedString, (PathBuf, PathBuf)>,
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
            if let Some((root, path)) = f.entry(id) {
                f.set_expanded(&root, &path, open, cx);
            }
        })
        .detach();
        Self {
            state,
            places: vec![],
            temporary: None,
            shown_in: None,
            current: None,
            highlight: false,
            listed: HashMap::new(),
            pending: HashSet::new(),
            expanded: HashSet::new(),
            ids: HashMap::new(),
            revealing: false,
        }
    }

    /// The root and folder a tree entry stands for.
    pub fn entry(&self, id: &SharedString) -> Option<(PathBuf, PathBuf)> {
        self.ids.get(id).cloned()
    }
    /// Nothing has been shown yet.
    pub fn is_empty(&self) -> bool {
        self.current.is_none()
    }
    /// The open folder and the root showing it, while it is highlighted.
    pub fn current(&self) -> Option<(&Path, &Path)> {
        if !self.highlight {
            return None;
        }
        self.shown_in.as_deref().zip(self.current.as_deref())
    }

    /// The Places, in order, and whether each is a drive.
    pub fn set_places(&mut self, places: Vec<(PathBuf, bool)>, cx: &mut Context<Self>) {
        if self.places == places {
            return;
        }
        self.places = places;
        if let Some(current) = self.current.clone() {
            let kept = self
                .shown_in
                .as_ref()
                .is_some_and(|r| self.places.iter().any(|(p, _)| key(p) == key(r)));
            if !kept || self.temporary.is_some() {
                self.place(&current, None);
                self.reveal(&current);
            }
        }
        self.update(false, cx);
    }
    pub fn expanded_state(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.expanded.iter().cloned().collect();
        keys.sort();
        keys.truncate(REMEMBERED);
        keys
    }
    pub fn restore_expanded(&mut self, keys: Vec<String>) {
        self.expanded.extend(keys);
    }

    /// Chooses the root showing `path`: `root` when it holds the folder, the
    /// root already showing the folder's surroundings, or its owner.
    fn place(&mut self, path: &Path, root: Option<&Path>) {
        let is_root = |r: &Path| self.places.iter().any(|(p, _)| key(p) == key(r));
        let chosen = root
            .filter(|r| within(path, r) && is_root(r))
            .map(Path::to_owned)
            .or_else(|| {
                self.shown_in
                    .clone()
                    .filter(|r| within(path, r) && is_root(r))
            })
            .or_else(|| owner(self.places.iter(), path).cloned());
        match chosen {
            Some(root) => {
                self.temporary = None;
                self.shown_in = Some(root);
            }
            None => {
                let drive = path
                    .ancestors()
                    .last()
                    .map(Path::to_owned)
                    .unwrap_or_else(|| path.to_owned());
                self.shown_in = Some(drive.clone());
                self.temporary = Some(drive);
            }
        }
    }

    /// Shows `path` as the open folder. `root` is the Place to show it in,
    /// as when the listener chose it there.
    pub fn show(&mut self, path: &Path, root: Option<&Path>, cx: &mut Context<Self>) {
        if path.as_os_str().is_empty() {
            return;
        }
        self.place(path, root);
        // Discovery lists the folder afresh and reports it through `listed`.
        self.current = Some(path.to_owned());
        self.highlight = true;
        self.revealing = true;
        if self.reveal(path) {
            cx.emit(ExpansionChanged);
        }
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
        let shown: HashSet<String> = visible.iter().map(|(_, p)| key(p)).collect();
        self.listed
            .retain(|k, _| shown.contains(k) || !within(Path::new(k), path));
        let todo = visible
            .into_iter()
            .filter(|(_, p)| within(p, path) && self.pending.insert(key(p)))
            .collect();
        self.list(todo, cx);
    }
    pub fn toggle(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if let Some((root, path)) = self.entry(id) {
            let open = !self.expanded.contains(&entry_key(&root, &path));
            self.set_expanded(&root, &path, open, cx);
        }
    }
    fn set_expanded(&mut self, root: &Path, path: &Path, open: bool, cx: &mut Context<Self>) {
        let k = entry_key(root, path);
        let changed = if open {
            self.expanded.insert(k)
        } else {
            self.expanded.remove(&k)
        };
        if changed {
            cx.emit(ExpansionChanged);
            self.update(false, cx);
        }
    }
    /// Expands the root showing `path` and every folder down to it,
    /// including it. Returns whether anything opened.
    fn reveal(&mut self, path: &Path) -> bool {
        let mut opened = false;
        if let Some(root) = self.shown_in.clone() {
            for p in path.ancestors().take_while(|p| within(p, &root)) {
                opened |= self.expanded.insert(entry_key(&root, p));
            }
        }
        opened
    }

    /// The roots shown, in order.
    fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.places.iter().map(|(p, _)| p.clone()).collect();
        roots.extend(self.temporary.clone());
        roots
    }
    /// The chain from the root showing the open folder down to it.
    fn chain(&self, root: &Path) -> Vec<PathBuf> {
        let (Some(shown), Some(current)) = (&self.shown_in, &self.current) else {
            return vec![];
        };
        if key(shown) != key(root) {
            return vec![];
        }
        let mut chain: Vec<PathBuf> = current
            .ancestors()
            .take_while(|p| within(p, root))
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
    fn item(&mut self, root: &Path, path: &Path, chain: &[PathBuf], depth: usize) -> TreeItem {
        let id = self.id(root, path);
        let expanded = self.expanded.contains(&entry_key(root, path));
        let kids = self.children(path, chain);
        // A collapsed entry carries its children only to show an expander;
        // their own subfolders are added when it opens.
        let children: Vec<TreeItem> = if expanded && depth < 64 {
            kids.iter()
                .map(|k| self.item(root, k, chain, depth + 1))
                .collect()
        } else {
            kids.iter()
                .map(|k| TreeItem::new(self.id(root, k), display_label(k)))
                .collect()
        };
        TreeItem::new(id, display_label(path))
            .children(children)
            .expanded(expanded)
    }
    fn id(&mut self, root: &Path, path: &Path) -> SharedString {
        let id: SharedString =
            format!("{}\u{1}{}", root.to_string_lossy(), path.to_string_lossy()).into();
        self.ids
            .entry(id.clone())
            .or_insert_with(|| (root.to_owned(), path.to_owned()));
        id
    }
    fn current_id(&self) -> Option<SharedString> {
        let (root, current) = self.current()?;
        Some(
            format!(
                "{}\u{1}{}",
                root.to_string_lossy(),
                current.to_string_lossy()
            )
            .into(),
        )
    }

    /// Rebuilds the entries from the listings, keeps the keyboard selection
    /// where it was unless `select_current`, and lists what became visible.
    fn update(&mut self, select_current: bool, cx: &mut Context<Self>) {
        let items: Vec<TreeItem> = self
            .roots()
            .iter()
            .map(|root| {
                let chain = self.chain(root);
                self.item(root, root, &chain, 0)
            })
            .collect();
        let current = self.current_id();
        let revealing = self.revealing;
        self.state.update(cx, |t, cx| {
            let previous = t.selected_item().map(|i| i.id.clone());
            t.set_items(items, cx);
            let wanted = if select_current || previous.is_none() {
                current.clone()
            } else {
                previous
            };
            let ix = wanted.and_then(|id| t.index_of(&id));
            t.set_selected_index(ix, cx);
            if revealing && let Some(ix) = current.and_then(|id| t.index_of(&id)) {
                t.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        });
        if self.revealing
            && let Some(root) = self.shown_in.clone()
        {
            let chain = self.chain(&root);
            self.revealing = chain.iter().any(|p| !self.listed.contains_key(&key(p)));
        }
        self.list_visible(cx);
        cx.notify();
    }

    /// The folders whose subfolders the tree needs: every root and every
    /// folder shown inside an expanded one, with the root showing it.
    fn visible(&self) -> Vec<(PathBuf, PathBuf)> {
        let mut out = vec![];
        for root in self.roots() {
            let chain = self.chain(&root);
            let mut stack = vec![root.clone()];
            while let Some(p) = stack.pop() {
                let k = key(&p);
                if self.expanded.contains(&entry_key(&root, &p)) && self.listed.contains_key(&k) {
                    stack.extend(self.children(&p, &chain));
                }
                out.push((root.clone(), p));
            }
        }
        out
    }
    /// Lists every visible folder whose subfolders are not known yet.
    fn list_visible(&mut self, cx: &mut Context<Self>) {
        let todo = self
            .visible()
            .into_iter()
            .filter(|(_, p)| {
                let k = key(p);
                !self.listed.contains_key(&k) && self.pending.insert(k)
            })
            .collect();
        self.list(todo, cx);
    }
    /// Lists `todo` off the interface thread, one task per root, so a
    /// sleeping network drive holds up only its own folders. A listing that
    /// discovery reported in the meantime is newer and wins.
    fn list(&mut self, todo: Vec<(PathBuf, PathBuf)>, cx: &mut Context<Self>) {
        let mut by_root: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for (root, path) in todo {
            by_root.entry(key(&root)).or_default().push(path);
        }
        for (_, paths) in by_root {
            cx.spawn(async move |this, cx| {
                let found = cx
                    .background_spawn(async move {
                        paths
                            .into_iter()
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
}

#[cfg(test)]
mod tests {
    use super::owner;
    use std::path::{Path, PathBuf};
    #[test]
    fn a_folder_shows_in_its_outermost_place_then_its_drive() {
        let places = [
            (PathBuf::from(r"C:\Users\Me\Music"), false),
            (PathBuf::from(r"C:\Users\Me\Music\Archive"), false),
            (PathBuf::from(r"C:\"), true),
            (PathBuf::from(r"D:\Rips"), false),
        ];
        let at = |p: &str| owner(places.iter(), Path::new(p)).cloned();
        assert_eq!(at(r"C:\Users\Me\Music"), Some(places[0].0.clone()));
        assert_eq!(
            at(r"c:\users\me\music\Archive\CD1"),
            Some(places[0].0.clone())
        );
        assert_eq!(at(r"D:\Rips\Live"), Some(places[3].0.clone()));
        // A sibling that only shares a prefix is not inside the place.
        assert_eq!(at(r"C:\Users\Me\Musical"), Some(PathBuf::from(r"C:\")));
        assert_eq!(at(r"E:\Other"), None);
        assert_eq!(at(r"\\nas\media\Rips"), None);
    }
}

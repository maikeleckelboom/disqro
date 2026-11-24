//! Destinations for the location trail. These use the same routes as history,
//! the collection shelves, and previous/next navigation.
use super::*;
use std::path::{Component, Prefix};

pub(crate) struct BreadcrumbChoice {
    pub route: Route,
    pub label: SharedString,
    pub current: bool,
}

impl BreadcrumbChoice {
    pub fn new(route: Route, label: impl Into<SharedString>, current: bool) -> Self {
        Self {
            route,
            label: label.into(),
            current,
        }
    }
}

pub(crate) enum BreadcrumbChoices {
    Ready(Vec<BreadcrumbChoice>),
    /// The browser or catalog is already loading the necessary structure.
    Waiting,
    Unavailable,
    Enumerate(Box<dyn FnOnce() -> Option<Vec<BreadcrumbChoice>> + Send>),
}

pub(super) fn folder_label(folder: &Path) -> String {
    match folder.components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        _ => match folder.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                    "\\\\{}\\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                ),
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    format!("{}:", letter as char)
                }
                _ => prefix.as_os_str().to_string_lossy().into_owned(),
            },
            _ => folder.display().to_string(),
        },
    }
}

impl Browser {
    pub(crate) fn breadcrumb_choices(&self, selected: &Route, cx: &App) -> BreadcrumbChoices {
        if selected.program().is_some() {
            return self.program_choices(selected);
        }
        if let Location::Folder(path) = &selected.location {
            let path = path.clone();
            return BreadcrumbChoices::Enumerate(Box::new(move || folder_choices(path)));
        }
        let catalog = self.catalog.read(cx);
        let choice = |location, label: String| {
            let current = location == selected.location;
            BreadcrumbChoice::new(Route::new(location, None), label, current)
        };
        let rows = match selected.location {
            Location::Library(Some(_)) | Location::Playlist(_) if !catalog.loaded => {
                return if catalog.handle().is_some() {
                    BreadcrumbChoices::Waiting
                } else {
                    BreadcrumbChoices::Unavailable
                };
            }
            Location::Library(Some(_)) => catalog
                .collections
                .iter()
                .map(|c| {
                    choice(
                        ShelfLink::Collection(c.root.id).location(),
                        c.root.name.clone(),
                    )
                })
                .collect(),
            Location::Playlist(_) => catalog
                .playlists
                .iter()
                .map(|p| choice(ShelfLink::Playlist(p.id).location(), p.name.clone()))
                .collect(),
            _ => [
                Location::Library(None),
                Location::Collections,
                Location::Playlists,
                Location::Liked,
            ]
            .into_iter()
            .map(|location| {
                let label = self.location_label(&location, cx);
                choice(location, label)
            })
            .collect(),
        };
        BreadcrumbChoices::Ready(rows)
    }
}

fn folder_choices(selected: PathBuf) -> Option<Vec<BreadcrumbChoice>> {
    if selected.as_os_str().is_empty() {
        return None;
    }
    let choice = |path: PathBuf, label: String, current| {
        BreadcrumbChoice::new(Route::new(Location::Folder(path), None), label, current)
    };
    if let Some(parent) = selected.parent().filter(|p| !p.as_os_str().is_empty()) {
        let mut folders = library::try_subfolders(parent).ok()?;
        // A hidden folder can still be opened by its address.
        if !folders
            .iter()
            .any(|p| crate::places::same_path(p, &selected))
            && selected.is_dir()
        {
            folders.push(selected.clone());
            library::sort_by_name(&mut folders, folder_label);
        }
        return Some(
            folders
                .into_iter()
                .map(|path| {
                    let label = folder_label(&path);
                    let current = crate::places::same_path(&path, &selected);
                    choice(path, label, current)
                })
                .collect(),
        );
    }
    let mut current_listed = false;
    let mut volumes: Vec<_> = crate::places::drives()
        .into_iter()
        .map(|(path, label)| {
            let current = crate::places::same_path(&path, &selected)
                || disk_letter(&path).is_some_and(|letter| disk_letter(&selected) == Some(letter));
            current_listed |= current;
            choice(path, label, current)
        })
        .collect();
    // UNC shares are addressable roots of their own. There is no server route.
    if !current_listed && disk_letter(&selected).is_none() {
        let label = folder_label(&selected);
        volumes.insert(0, choice(selected, label, true));
    }
    Some(volumes)
}

fn disk_letter(path: &Path) -> Option<u8> {
    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };
    match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Some(letter.to_ascii_uppercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{disk_letter, folder_choices, folder_label};
    use std::path::Path;

    #[test]
    fn drive_and_share_labels_keep_the_root_identity() {
        assert_eq!(folder_label(Path::new(r"C:\")), "C:");
        assert_eq!(folder_label(Path::new(r"\\?\C:\")), "C:");
        assert_eq!(
            folder_label(Path::new(r"\\?\UNC\nas\media\")),
            r"\\nas\media"
        );
        assert_eq!(folder_label(Path::new(r"\\nas\media")), r"\\nas\media");
        assert_eq!(folder_label(Path::new(r"\\nas\media\Rips")), "Rips");
        let root = Path::new(r"\\nas\media\");
        assert!(root.parent().is_none());
        assert_eq!(disk_letter(Path::new(r"\\?\C:\")), Some(b'C'));
    }

    #[test]
    fn folder_destinations_are_visible_siblings_in_natural_order() {
        let root = tempfile::tempdir().unwrap();
        for name in ["CD 10", "CD 2", "CD 1"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        std::fs::write(root.path().join("CD 3"), []).unwrap();
        let cd2 = root.path().join("CD 2");
        let choices = folder_choices(cd2.clone()).unwrap();
        assert_eq!(
            choices
                .iter()
                .map(|c| (c.label.as_ref(), c.current))
                .collect::<Vec<_>>(),
            [("CD 1", false), ("CD 2", true), ("CD 10", false)]
        );
        std::fs::remove_dir(&cd2).unwrap();
        let choices = folder_choices(cd2.clone()).unwrap();
        assert_eq!(choices.len(), 2);
        assert!(!choices.iter().any(|c| c.current));
        assert!(folder_choices(cd2.join("Removed")).is_none());
    }
}

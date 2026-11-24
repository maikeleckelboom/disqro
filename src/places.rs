//! Places: the user's own shortcuts in the sidebar.
//!
//! A Place only points at a folder. Adding, renaming, reordering or removing
//! one never touches the folder itself. Default places come from Windows
//! Known Folders and the drives present on this machine; the user may remove
//! them, and "Restore Missing Default Places" brings them back without
//! touching anything the user added.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Known {
    Music,
    Downloads,
    Desktop,
    Home,
}
impl Known {
    const DEFAULTS: [Self; 2] = [Self::Downloads, Self::Music];
    pub fn label(self) -> &'static str {
        match self {
            Self::Music => "Music",
            Self::Downloads => "Downloads",
            Self::Desktop => "Desktop",
            Self::Home => "Home",
        }
    }
}

/// Where a place came from. Defaults are identified by their origin so a
/// relocated Music folder or a renamed volume still counts as the same place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Known(Known),
    Drive,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub path: PathBuf,
    pub origin: Origin,
    /// A label the user chose. `None` shows the system name.
    #[serde(default)]
    pub label: Option<String>,
    /// The system name of a drive, refreshed at startup and not persisted.
    #[serde(skip)]
    pub system_label: Option<String>,
}
impl Place {
    pub fn custom(path: PathBuf) -> Self {
        Self {
            path,
            origin: Origin::Custom,
            label: None,
            system_label: None,
        }
    }
    pub fn name(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        match &self.origin {
            Origin::Known(k) => k.label().to_owned(),
            Origin::Drive => self
                .system_label
                .clone()
                .unwrap_or_else(|| drive_name(&self.path)),
            Origin::Custom => crate::library::folder_name(&self.path),
        }
    }
    fn default_key(&self) -> Option<DefaultKey> {
        match &self.origin {
            Origin::Known(k) => Some(DefaultKey::Known(*k)),
            Origin::Drive => Some(DefaultKey::Drive(self.path.clone())),
            Origin::Custom => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultKey {
    Known(Known),
    Drive(PathBuf),
}

/// The persisted form: the ordered list and the defaults the user removed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlacesState {
    pub items: Vec<Place>,
    #[serde(default)]
    pub dismissed: Vec<DefaultKey>,
}

pub struct Places {
    pub items: Vec<Place>,
    dismissed: Vec<DefaultKey>,
}

impl Places {
    /// Restores saved places, resolves known folders again, and appends any
    /// default the user has not removed, such as a newly attached drive.
    pub fn restore(saved: Option<PlacesState>) -> Self {
        let defaults = defaults();
        let Some(saved) = saved else {
            return Self {
                items: defaults,
                dismissed: vec![],
            };
        };
        let mut items: Vec<Place> = saved
            .items
            .into_iter()
            .filter_map(|mut p| {
                match &p.origin {
                    Origin::Known(k) => p.path = known_path(*k)?,
                    // A drive that is not attached right now returns when it is.
                    Origin::Drive => {
                        let current = defaults
                            .iter()
                            .find(|d| d.origin == Origin::Drive && d.path == p.path)?;
                        p.system_label = current.system_label.clone();
                    }
                    Origin::Custom => {}
                }
                Some(p)
            })
            .collect();
        for d in defaults {
            let key = d.default_key();
            if !saved.dismissed.contains(key.as_ref().unwrap())
                && !items.iter().any(|p| p.default_key() == key)
            {
                items.push(d);
            }
        }
        Self {
            items,
            dismissed: saved.dismissed,
        }
    }
    pub fn state(&self) -> PlacesState {
        PlacesState {
            items: self.items.clone(),
            dismissed: self.dismissed.clone(),
        }
    }
    pub fn contains(&self, path: &Path) -> bool {
        self.items.iter().any(|p| same_path(&p.path, path))
    }
    pub fn add(&mut self, path: PathBuf) -> bool {
        if self.contains(&path) {
            return false;
        }
        self.items.push(Place::custom(path));
        true
    }
    pub fn remove(&mut self, ix: usize) {
        if ix < self.items.len() {
            let place = self.items.remove(ix);
            if let Some(key) = place.default_key()
                && !self.dismissed.contains(&key)
            {
                self.dismissed.push(key);
            }
        }
    }
    pub fn rename(&mut self, ix: usize, label: &str) {
        if let Some(p) = self.items.get_mut(ix) {
            let label = label.trim();
            p.label = (!label.is_empty() && label != p.name_without_label()).then(|| label.into());
        }
    }
    pub fn move_item(&mut self, from: usize, to: usize) {
        if from < self.items.len() && to <= self.items.len() && from != to {
            let item = self.items.remove(from);
            let to = if to > from { to - 1 } else { to };
            self.items.insert(to.min(self.items.len()), item);
        }
    }
    /// Re-adds removed defaults at the end. Custom places, labels and order
    /// stay as they are.
    pub fn restore_missing(&mut self) -> usize {
        let mut added = 0;
        for d in defaults() {
            let key = d.default_key();
            self.dismissed
                .retain(|dismissed| Some(dismissed) != key.as_ref());
            if !self.items.iter().any(|p| p.default_key() == key) {
                self.items.push(d);
                added += 1;
            }
        }
        added
    }
    pub fn missing_defaults(&self) -> usize {
        defaults()
            .iter()
            .filter(|d| {
                !self
                    .items
                    .iter()
                    .any(|p| p.default_key() == d.default_key())
            })
            .count()
    }
    /// Replaces every place, label and the order with the defaults.
    pub fn reset(&mut self) {
        self.items = defaults();
        self.dismissed.clear();
    }
}
impl Place {
    fn name_without_label(&self) -> String {
        Self {
            label: None,
            ..self.clone()
        }
        .name()
    }
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn defaults() -> Vec<Place> {
    available_defaults(known_path, drives())
}

fn available_defaults(
    known: impl Fn(Known) -> Option<PathBuf>,
    drives: Vec<(PathBuf, String)>,
) -> Vec<Place> {
    let mut items: Vec<Place> = Known::DEFAULTS
        .iter()
        .filter_map(|k| {
            Some(Place {
                path: known(*k)?,
                origin: Origin::Known(*k),
                label: None,
                system_label: None,
            })
        })
        .collect();
    for (path, name) in drives {
        let first = same_path(&path, Path::new(r"C:\"));
        let drive = Place {
            path,
            origin: Origin::Drive,
            label: None,
            system_label: Some(name),
        };
        if first {
            items.insert(0, drive);
        } else {
            items.push(drive);
        }
    }
    items
}

fn drive_name(root: &Path) -> String {
    let letter = root.to_string_lossy().trim_end_matches('\\').to_owned();
    format!("Local Disk ({letter})")
}

#[cfg(windows)]
pub fn known_path(k: Known) -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Profile,
            KF_FLAG_DEFAULT, SHGetKnownFolderPath,
        },
    };
    let id = match k {
        Known::Music => &FOLDERID_Music,
        Known::Downloads => &FOLDERID_Downloads,
        Known::Desktop => &FOLDERID_Desktop,
        Known::Home => &FOLDERID_Profile,
    };
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = raw.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(raw.0 as _));
        path.filter(|p| p.is_dir())
    }
}
#[cfg(not(windows))]
pub fn known_path(k: Known) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let p = match k {
        Known::Music => home.join("Music"),
        Known::Downloads => home.join("Downloads"),
        Known::Desktop => home.join("Desktop"),
        Known::Home => home,
    };
    p.is_dir().then_some(p)
}

/// Fixed, removable, optical and network drives that are present now. Only
/// local fixed and removable volumes are asked for their name, since an
/// optical or disconnected network drive can stall for seconds.
#[cfg(windows)]
fn drives() -> Vec<(PathBuf, String)> {
    use windows::{
        Win32::{
            Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives},
            UI::Shell::{SHFILEINFOW, SHGFI_DISPLAYNAME, SHGetFileInfoW},
        },
        core::HSTRING,
    };
    const REMOVABLE: u32 = 2;
    const FIXED: u32 = 3;
    const REMOTE: u32 = 4;
    const CDROM: u32 = 5;
    let mask = unsafe { GetLogicalDrives() };
    let mut out = vec![];
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let h = HSTRING::from(root.as_str());
        let kind = unsafe { GetDriveTypeW(&h) };
        let short = format!("{letter}:");
        let name = match kind {
            FIXED | REMOVABLE => {
                let mut info = SHFILEINFOW::default();
                let found = unsafe {
                    SHGetFileInfoW(
                        &h,
                        Default::default(),
                        Some(&mut info),
                        std::mem::size_of::<SHFILEINFOW>() as u32,
                        SHGFI_DISPLAYNAME,
                    )
                };
                let name = info.szDisplayName;
                if found != 0 && name[0] != 0 {
                    String::from_utf16_lossy(name.split(|c| *c == 0).next().unwrap_or_default())
                } else if kind == REMOVABLE {
                    format!("Removable Disk ({short})")
                } else {
                    drive_name(Path::new(&root))
                }
            }
            REMOTE => format!("Network Drive ({short})"),
            CDROM => format!("CD Drive ({short})"),
            _ => continue,
        };
        out.push((PathBuf::from(root), name));
    }
    out
}
#[cfg(not(windows))]
fn drives() -> Vec<(PathBuf, String)> {
    vec![(PathBuf::from("/"), "Root".into())]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn custom(p: &str) -> Place {
        Place::custom(PathBuf::from(p))
    }
    #[test]
    fn available_defaults_start_with_c_downloads_and_music() {
        let known = |k: Known| Some(PathBuf::from(k.label()));
        let drives = vec![
            (PathBuf::from(r"D:\"), "Media".into()),
            (PathBuf::from(r"C:\"), "Windows (C:)".into()),
        ];
        let items = available_defaults(known, drives);
        assert_eq!(
            items.iter().map(Place::name).collect::<Vec<_>>(),
            ["Windows (C:)", "Downloads", "Music", "Media"]
        );
        let items = available_defaults(|k| (k == Known::Music).then(|| "Music".into()), vec![]);
        assert_eq!(items.iter().map(Place::name).collect::<Vec<_>>(), ["Music"]);
    }

    #[test]
    fn saved_order_labels_and_dismissals_survive_until_explicit_restoration() {
        let mut places = Places::restore(None);
        let fresh = places.items.clone();
        places.add("Custom fixture".into());
        places.move_item(fresh.len(), 0);
        places.rename(0, "Live recordings");
        if fresh.len() > 1 {
            places.move_item(fresh.len(), 1);
            places.rename(1, "Archive");
        }
        let removed = places.items.last().cloned().unwrap();
        places.remove(places.items.len() - 1);
        let saved = places.items.clone();
        let offline = DefaultKey::Drive(PathBuf::from(r"\\offline\media"));
        places.dismissed.push(offline.clone());
        let mut restored = Places::restore(Some(places.state()));
        assert_eq!(restored.items, saved);
        assert_eq!(restored.restore_missing(), 1);
        assert_eq!(&restored.items[..saved.len()], saved);
        assert_eq!(
            restored.items.last().unwrap().default_key(),
            removed.default_key()
        );
        assert_eq!(restored.restore_missing(), 0);
        assert!(restored.dismissed.contains(&offline));
        restored.reset();
        assert_eq!(restored.items, fresh);
        assert!(restored.dismissed.is_empty());
    }
    #[test]
    fn removing_and_restoring_defaults_keeps_custom_places() {
        let mut places = Places {
            items: vec![custom("X:\\Albums"), custom("Y:\\Live")],
            dismissed: vec![],
        };
        let defaults = defaults().len();
        places.restore_missing();
        assert_eq!(places.items.len(), 2 + defaults);
        assert_eq!(places.items[0].path, PathBuf::from("X:\\Albums"));
        if defaults > 0 {
            places.remove(2);
            assert_eq!(places.missing_defaults(), 1);
            // A dismissed default stays away across a restart.
            let again = Places::restore(Some(places.state()));
            assert_eq!(again.items.len(), 1 + defaults);
            places.restore_missing();
            assert_eq!(places.items.len(), 2 + defaults);
        }
        places.reset();
        assert!(places.items.iter().all(|p| p.origin != Origin::Custom));
    }
    #[test]
    fn moving_and_renaming() {
        let mut places = Places {
            items: vec![custom("A:\\1"), custom("A:\\2"), custom("A:\\3")],
            dismissed: vec![],
        };
        places.move_item(0, 3);
        assert_eq!(places.items[2].path, PathBuf::from("A:\\1"));
        places.move_item(2, 0);
        assert_eq!(places.items[0].path, PathBuf::from("A:\\1"));
        places.rename(1, "  Live sets ");
        assert_eq!(places.items[1].name(), "Live sets");
        places.rename(1, "");
        assert_eq!(places.items[1].name(), "2");
        assert!(!places.add(PathBuf::from("a:\\1\\")));
    }
}

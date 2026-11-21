use crate::{
    panels::{InstanceId, PanelRecord},
    places::PlacesState,
    queue::Queue,
    table::{Layouts, LegacyColumn},
};
use gpui_kit::component::dock::DockAreaState;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// How the close button behaves. The preference defaults to off,
/// so closing Disqro quits it unless the listener asks otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPrefs {
    pub close_to_tray: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SavedQueue {
    pub queue: Queue,
    pub position: f64,
}

/// Workspace preferences and panel placement beyond the dock geometry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceState {
    /// The destination for navigation from Library and Locations.
    pub active: Option<InstanceId>,
    /// The most recently closed instance of each type, in closing order.
    pub closed: Vec<ClosedPanel>,
    /// The height of the player along the bottom of the window.
    pub player_height: Option<f32>,
}

/// A closed panel: its saved state, and the panels it was grouped with, so
/// reopening it puts it back beside them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClosedPanel {
    pub key: String,
    pub record: PanelRecord,
    #[serde(default)]
    pub beside: Vec<InstanceId>,
}

/// Navigation from the fixed workspace, consumed when its geometry is replaced.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct LegacyNavigation {
    pub folder: Option<PathBuf>,
    pub location: Option<crate::browser::Location>,
    pub sidebar: LegacySidebar,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct LegacySidebar {
    pub expanded: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    #[serde(flatten, skip_serializing)]
    pub legacy_navigation: LegacyNavigation,
    pub volume: f32,
    pub full_cd: bool,
    /// The dock layout, with every panel instance's own state. A layout with
    /// another version is replaced by the default layout.
    pub layout: Option<DockAreaState>,
    pub workspace: WorkspaceState,
    #[serde(skip_serializing)]
    pub columns: Vec<LegacyColumn>,
    /// The column layouts a new Music Browser starts with.
    pub table_layouts: Option<Layouts>,
    pub places: Option<PlacesState>,
    pub queue: Option<SavedQueue>,
    pub window: WindowPrefs,
    /// The view settings a new Music Browser starts with, and the folders
    /// shown with their subfolders.
    pub collection: crate::collection::ViewPrefs,
    /// Folders open in Locations.
    pub expanded: Vec<String>,
    /// Version of the single-layout preference written before contextual tables.
    #[serde(skip_serializing)]
    pub columns_version: u32,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            legacy_navigation: LegacyNavigation::default(),
            volume: 0.65,
            full_cd: false,
            layout: None,
            workspace: WorkspaceState::default(),
            columns: vec![],
            table_layouts: None,
            places: None,
            queue: None,
            window: WindowPrefs::default(),
            collection: Default::default(),
            expanded: Vec::new(),
            // Missing from profiles written before the field existed.
            columns_version: 0,
        }
    }
}
pub fn location() -> Option<PathBuf> {
    std::env::var_os("DISQRO_PROFILE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Disqro")))
        .map(|p| p.join("workspace-v1.json"))
}
/// Preserve an unreadable snapshot before starting a fresh workspace.
pub fn restore(p: &Path) -> anyhow::Result<(Profile, Option<String>)> {
    match fs::read(p) {
        Ok(bytes) => match (|| -> anyhow::Result<Profile> {
            let profile: Profile = serde_json::from_slice(&bytes)?;
            if let Some(saved) = &profile.queue {
                saved.queue.validate()?;
            }
            Ok(profile)
        })() {
            Ok(profile) => Ok((profile, None)),
            Err(error) => {
                let mut backup = tempfile::Builder::new()
                    .prefix("workspace-recovered-")
                    .suffix(".json")
                    .tempfile_in(p.parent().unwrap_or(Path::new(".")))?;
                backup.write_all(&bytes)?;
                backup.as_file().sync_all()?;
                let (_, path) = backup.keep()?;
                Ok((
                    Profile::default(),
                    Some(format!(
                        "Opened a fresh workspace. The previous profile was preserved at {} ({error}).",
                        path.display()
                    )),
                ))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Profile::default(), None)),
        Err(e) => Err(e.into()),
    }
}
fn save(p: &Path, s: &Profile) -> anyhow::Result<()> {
    let parent = p.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec_pretty(s)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(p)?;
    Ok(())
}

/// Snapshot revisions are issued on the UI thread, before disk work is scheduled.
pub struct ProfileFile {
    path: PathBuf,
    committed: std::sync::Mutex<u64>,
    _lock: Option<fs::File>,
}
impl ProfileFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            _lock: None,
            committed: Default::default(),
        }
    }
    /// Only one process may replace the primary workspace. A second window
    /// or failed restore gets a named recovery profile of its own.
    pub fn open(path: PathBuf) -> (Self, Profile, Option<String>) {
        let open = || -> anyhow::Result<_> {
            fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path.with_extension("lock"))?;
            lock.try_lock()?;
            let (profile, notice) = restore(&path)?;
            Ok((lock, profile, notice))
        };
        match open() {
            Ok((lock, profile, notice)) => {
                let mut file = Self::new(path);
                file._lock = Some(lock);
                (file, profile, notice)
            }
            Err(error) => {
                let other = path
                    .with_file_name(format!("workspace-session-{:016x}.json", fastrand::u64(..)));
                let notice = format!(
                    "Could not open the primary workspace ({error}). This session saves to {}.",
                    other.display()
                );
                (Self::new(other), Profile::default(), Some(notice))
            }
        }
    }
    pub fn commit(&self, revision: u64, snapshot: &Profile) -> anyhow::Result<()> {
        let mut committed = self.committed.lock().unwrap();
        if revision > *committed {
            save(&self.path, snapshot)?;
            *committed = revision;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_tray_setting_retires_independently_of_close_and_custom_layouts() {
        let mut p: Profile = serde_json::from_str(r#"{"window":{"minimize_to_tray":true,"close_to_tray":true},"columns":[{"field":"Title","width":310},{"field":"Liked","width":40}]}"#).unwrap();
        assert!(p.window.close_to_tray);
        p.table_layouts = Some(Layouts::migrate(&p.columns, p.columns_version));
        let json = serde_json::to_value(p).unwrap();
        assert!(json["window"].get("minimize_to_tray").is_none());
        assert!(json.get("columns").is_none());
        let playlist = &json["table_layouts"]["playlist"]["columns"];
        assert_eq!(playlist[0]["field"], "Cover");
        assert_eq!(playlist[1]["width"], 310.);
    }
    #[test]
    fn an_earlier_profile_keeps_everything_but_its_fixed_panel_layout() {
        let p: Profile = serde_json::from_str(
            r#"{"volume":0.3,"folder":"C:/Music","location":{"Playlist":4},
                "sidebar":{"expanded":["c:/music"],"playlists_closed":true},
                "layout":{"version":2,"center":{"panel_name":"StackPanel","children":[],
                    "info":{"stack":{"sizes":[],"axis":0}}}},
                "collection":{"albums":true,"subfolders":[]}}"#,
        )
        .unwrap();
        assert_eq!(p.volume, 0.3);
        assert!(p.collection.albums);
        assert_eq!(
            p.legacy_navigation.folder.as_deref(),
            Some(Path::new("C:/Music"))
        );
        assert_eq!(
            p.legacy_navigation.location,
            Some(crate::browser::Location::Playlist(4))
        );
        assert_eq!(p.legacy_navigation.sidebar.expanded, ["c:/music"]);
        let canonical = serde_json::to_value(&p).unwrap();
        for retired in ["folder", "location", "sidebar"] {
            assert!(canonical.get(retired).is_none());
        }
        assert_eq!(p.layout.map(|l| l.version), Some(Some(2)));
        assert!(p.workspace.closed.is_empty());
    }
    #[test]
    fn the_workspace_state_round_trips() {
        let closed = ClosedPanel {
            key: "disqro.library".into(),
            record: PanelRecord {
                id: Some(InstanceId(9)),
                ..Default::default()
            },
            beside: vec![InstanceId(4)],
        };
        let p = Profile {
            workspace: WorkspaceState {
                active: Some(InstanceId(2)),
                closed: vec![closed.clone()],
                player_height: Some(120.),
            },
            expanded: vec!["c:/music".into()],
            ..Default::default()
        };
        let back: Profile = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back.workspace.active, Some(InstanceId(2)));
        assert_eq!(back.workspace.closed, vec![closed]);
        assert_eq!(back.workspace.player_height, Some(120.));
        assert_eq!(back.expanded, p.expanded);
    }
    #[test]
    fn late_snapshot_cannot_overwrite_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let file = ProfileFile::new(path.clone());
        let latest = Profile {
            volume: 0.25,
            ..Default::default()
        };
        file.commit(2, &latest).unwrap();
        file.commit(1, &Profile::default()).unwrap();
        assert_eq!(restore(&path).unwrap().0.volume, 0.25);
    }
    #[test]
    fn corrupt_bytes_survive_recovery_and_subsequent_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        fs::write(&path, b"truncated").unwrap();
        let (fresh, warning) = restore(&path).unwrap();
        assert!(warning.is_some());
        let backup = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p != &path)
            .unwrap();
        ProfileFile::new(path.clone()).commit(1, &fresh).unwrap();
        assert_eq!(fs::read(backup).unwrap(), b"truncated");
        assert!(restore(&path).unwrap().1.is_none());
    }
    #[test]
    fn concurrent_sessions_cannot_replace_each_others_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let (first, _, _) = ProfileFile::open(path.clone());
        let (second, _, notice) = ProfileFile::open(path.clone());
        assert!(notice.is_some());
        second
            .commit(
                1,
                &Profile {
                    volume: 0.1,
                    ..Default::default()
                },
            )
            .unwrap();
        first
            .commit(
                1,
                &Profile {
                    volume: 0.2,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(restore(&path).unwrap().0.volume, 0.2);
        assert_eq!(restore(&second.path).unwrap().0.volume, 0.1);
        drop(first);
        assert!(ProfileFile::open(path).2.is_none());
    }
    #[test]
    fn failed_replace_can_retry_same_revision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        fs::create_dir(&path).unwrap();
        let file = ProfileFile::new(path.clone());
        assert!(file.commit(1, &Profile::default()).is_err());
        fs::remove_dir(path).unwrap();
        file.commit(1, &Profile::default()).unwrap();
    }
}

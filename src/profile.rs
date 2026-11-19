use crate::{places::PlacesState, queue::Queue, table::LayoutColumn};
use gpui_kit::component::dock::DockAreaState;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// How the window behaves around the notification area. Both default to off,
/// so closing Disqro quits it unless the listener asks otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPrefs {
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SavedQueue {
    pub queue: Queue,
    pub position: f64,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub folder: Option<PathBuf>,
    pub volume: f32,
    pub layout: Option<DockAreaState>,
    pub columns: Vec<LayoutColumn>,
    pub places: Option<PlacesState>,
    pub queue: Option<SavedQueue>,
    pub window: WindowPrefs,
    pub collection: crate::collection::ViewPrefs,
    /// The view shown at exit: a folder, a collection, Liked Tracks or a playlist.
    pub location: Option<crate::browser::Location>,
    /// Layouts older than [`COLUMNS_VERSION`] gain the columns added since.
    pub columns_version: u32,
}
pub const COLUMNS_VERSION: u32 = 2;
impl Default for Profile {
    fn default() -> Self {
        Self {
            folder: None,
            volume: 0.65,
            layout: None,
            columns: vec![],
            places: None,
            queue: None,
            window: WindowPrefs::default(),
            collection: Default::default(),
            location: None,
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

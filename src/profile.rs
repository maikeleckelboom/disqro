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
}
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
        }
    }
}
pub fn location() -> Option<PathBuf> {
    std::env::var_os("DISQRO_PROFILE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Disqro")))
        .map(|p| p.join("workspace-v1.json"))
}
pub fn read(p: &Path) -> anyhow::Result<Profile> {
    match fs::read(p) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Profile::default()),
        Err(e) => Err(e.into()),
    }
}
pub fn save(p: &Path, s: &Profile) -> anyhow::Result<()> {
    let parent = p.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec_pretty(s)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(p)?;
    Ok(())
}

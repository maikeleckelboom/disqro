use crate::table::LayoutColumn;
use gpui_kit::component::dock::DockAreaState;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub folder: Option<PathBuf>,
    pub volume: f32,
    pub layout: Option<DockAreaState>,
    pub columns: Vec<LayoutColumn>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            folder: None,
            volume: 0.65,
            layout: None,
            columns: vec![],
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

use gpui_kit::{AssetSource, SharedString, component::Icon};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    PlayerIcons,
    [
        SkipBack,
        SkipForward,
        Volume2,
        Square,
        PanelsTopLeft,
        FolderOpen,
        ChevronsUpDown,
        Disc3,
        Music2,
        ChevronRight,
        ChevronDown
    ]
);
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        macro_rules! embedded {
            ($($name:literal),*)=>{match path {$($name=>Some(include_bytes!(concat!("../assets/",$name)).as_slice()),)* _=>None}}
        }
        let custom = embedded!(
            "brand/compact.png",
            "files/16/file-audio.svg",
            "files/16/cue-track.svg",
            "files/16/folder.svg",
            "files/20/folder-music.svg",
            "files/20/drive.svg"
        );
        if let Some(bytes) = custom {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = PlayerIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut names = gpui_kit::assets::Assets.list(path)?;
        names.extend(PlayerIcons.list(path)?);
        Ok(names)
    }
}
pub enum FileIcon {
    Folder,
    Music,
    Drive,
    Recording,
    Chapter,
}
impl FileIcon {
    pub fn icon(self) -> Icon {
        Icon::default().path(match self {
            Self::Folder => "files/16/folder.svg",
            Self::Music => "files/20/folder-music.svg",
            Self::Drive => "files/20/drive.svg",
            Self::Recording => "files/16/file-audio.svg",
            Self::Chapter => "files/16/cue-track.svg",
        })
    }
}

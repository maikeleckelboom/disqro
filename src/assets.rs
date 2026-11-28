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
        ChevronDown,
        Shuffle,
        Repeat,
        Repeat1,
        ListMusic,
        ListTree,
        ListStart,
        ListEnd,
        AudioLines,
        Layers,
        Radar,
        Pause,
        Play,
        SlidersHorizontal,
        Volume1,
        VolumeX,
        Download,
        FolderPlus,
        Pin,
        X,
        Pencil,
        RefreshCw,
        TriangleAlert,
        ListPlus,
        Library,
        Trash,
        Copy,
        FolderSearch,
        Image,
        Clock,
        LayoutPanelLeft,
        SquareSplitHorizontal,
        SquareSplitVertical,
        ListFilter,
        Music,
        Folder,
        Folders,
        MicVocal
    ]
);
/// A filled variant of Lucide's heart (ISC license), for liked tracks.
pub const HEART_FILL: &str = "icons/heart-fill.svg";
/// Filled variants of Lucide's play and pause (ISC license), for the play
/// button's metal face.
pub const PLAY_FILL: &str = "icons/play-fill.svg";
pub const PAUSE_FILL: &str = "icons/pause-fill.svg";
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        macro_rules! embedded {
            ($($name:literal),*)=>{match path {$($name=>Some(include_bytes!(concat!("../assets/",$name)).as_slice()),)* _=>None}}
        }
        let custom = embedded!(
            "icons/heart-fill.svg",
            "icons/play-fill.svg",
            "icons/pause-fill.svg",
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
}
impl FileIcon {
    pub fn icon(self) -> Icon {
        Icon::default().path(match self {
            Self::Folder => "files/16/folder.svg",
            Self::Music => "files/20/folder-music.svg",
            Self::Drive => "files/20/drive.svg",
        })
    }
}

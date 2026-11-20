use image::{ImageFormat, ImageReader, Limits};
use lofty::{picture::PictureType, prelude::*, probe::Probe};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_BYTES: usize = 12 * 1024 * 1024;
fn thumbnail(data: &[u8], edge: u32) -> Option<Vec<u8>> {
    if data.len() > MAX_BYTES {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(96 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(edge, edge);
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}
fn file_thumbnail(path: &Path, edge: u32) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    thumbnail(&bytes, edge)
}

/// A rescan sees a new asset when the cover changes. GPUI requests thumbnails
/// for visible cells; the full-resolution original never enters a table texture.
#[derive(Clone, Hash)]
pub struct CoverFile {
    path: PathBuf,
    stamp: crate::cache::Stamp,
}
impl CoverFile {
    pub fn new(path: PathBuf) -> Option<Self> {
        let stamp = crate::cache::Stamp::read(&path)?;
        Some(Self { path, stamp })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
pub struct CoverThumbnail;
impl gpui_kit::Asset for CoverThumbnail {
    type Source = CoverFile;
    type Output = Option<Arc<gpui_kit::Image>>;
    fn load(
        cover: CoverFile,
        cx: &mut gpui_kit::App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        cx.background_executor().spawn(async move {
            let png = file_thumbnail(&cover.path, 96)?;
            Some(Arc::new(gpui_kit::Image::from_bytes(
                gpui_kit::ImageFormat::Png,
                png,
            )))
        })
    }
}
#[derive(Default)]
pub struct MediaInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover: Option<Vec<u8>>,
}
pub fn for_source(path: &Path) -> MediaInfo {
    let mut info = MediaInfo::default();
    if let Ok(file) = Probe::open(path).and_then(|p| p.read()) {
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            info.title = tag.title().map(|v| v.into_owned());
            info.artist = tag.artist().map(|v| v.into_owned());
            info.album = tag.album().map(|v| v.into_owned());
        }
        let mut pictures = file.tags().iter().flat_map(|t| t.pictures());
        if let Some(pic) = pictures
            .clone()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pictures.next())
            && let Some(bytes) = thumbnail(pic.data(), 288)
        {
            info.cover = Some(bytes);
            return info;
        }
    }
    let Some(parent) = path.parent() else {
        return info;
    };
    for name in [
        "cover.jpg",
        "folder.jpg",
        "front.jpg",
        "cover.png",
        "folder.png",
    ] {
        if let Some(image) = file_thumbnail(&parent.join(name), 288) {
            info.cover = Some(image);
            return info;
        }
    }
    info
}

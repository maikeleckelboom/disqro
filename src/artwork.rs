use image::{ImageFormat, ImageReader, Limits};
use lofty::{picture::PictureType, prelude::*, probe::Probe};
use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
};

const MAX_BYTES: usize = 12 * 1024 * 1024;
fn thumbnail(data: &[u8]) -> Option<Vec<u8>> {
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
    let image = reader.decode().ok()?.thumbnail(288, 288);
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
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
            && let Some(bytes) = thumbnail(pic.data())
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
        if let Ok(file) = fs::File::open(parent.join(name)) {
            let mut bytes = Vec::new();
            if file
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .is_ok()
                && let Some(image) = thumbnail(&bytes)
            {
                info.cover = Some(image);
                return info;
            }
        }
    }
    info
}

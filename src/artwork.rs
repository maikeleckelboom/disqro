use gpui_kit::{App, Asset, Global, RenderImage, Window};
use hashlink::LinkedHashMap;
use image::{ImageFormat, ImageReader, Limits};
use lofty::{picture::PictureType, prelude::*, probe::Probe};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_BYTES: usize = 12 * 1024 * 1024;
fn decode(data: &[u8], edge: u32) -> Option<image::DynamicImage> {
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
    Some(reader.decode().ok()?.thumbnail(edge, edge))
}
fn thumbnail(data: &[u8], edge: u32) -> Option<Vec<u8>> {
    let image = decode(data, edge)?;
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}
fn file_bytes(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// A rescan sees a new asset when the cover changes. GPUI requests thumbnails
/// for visible cells; the full-resolution original never enters a table texture.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct CoverFile {
    path: PathBuf,
    embedded: bool,
    stamp: crate::cache::Stamp,
}
impl CoverFile {
    pub fn new(path: PathBuf) -> Option<Self> {
        let stamp = crate::cache::Stamp::read(&path)?;
        Some(Self {
            path,
            stamp,
            embedded: false,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
/// Resolve paths only for painted Title cells. A fresh folder scan or library
/// listing rechecks revisions, while unchanged images retain their texture.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct TrackCover {
    pub source: PathBuf,
    pub authored_folder: Option<PathBuf>,
    pub disc_cover: Option<PathBuf>,
    pub listing: u64,
}
impl Asset for TrackCover {
    type Source = Self;
    type Output = Arc<[CoverFile]>;
    fn load(key: Self, _: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        key.resolve()
    }
}
impl TrackCover {
    async fn resolve(self) -> Arc<[CoverFile]> {
        let mut files = Vec::new();
        if let Some(mut audio) = CoverFile::new(self.source.clone()) {
            audio.embedded = true;
            files.push(audio);
        }
        let folder = self.source.parent();
        for path in [
            folder.and_then(crate::library::find_cover),
            self.authored_folder
                .as_deref()
                .filter(|dir| Some(*dir) != folder)
                .and_then(crate::library::find_cover),
            self.disc_cover,
            folder
                .filter(|_| crate::library::folder_disc_number(&self.source).is_some())
                .and_then(Path::parent)
                .and_then(crate::library::find_cover),
        ]
        .into_iter()
        .flatten()
        {
            if !files.iter().any(|f| f.path == path)
                && let Some(file) = CoverFile::new(path)
            {
                files.push(file);
            }
        }
        files.into()
    }
}

// GPUI's asset cache is explicitly owned. Evict both the loader and the
// atlas image. Negative results and pending requests count against the cap.
const CAPACITY: usize = 256;
#[derive(Default)]
struct Covers {
    locations: LinkedHashMap<TrackCover, ()>,
    images: LinkedHashMap<CoverFile, ()>,
}
impl Global for Covers {}
fn touch<K: Clone + std::hash::Hash + Eq>(keys: &mut LinkedHashMap<K, ()>, key: &K) -> Option<K> {
    if keys.to_back(key).is_some() {
        return None;
    }
    keys.insert(key.clone(), ());
    (keys.len() > CAPACITY).then(|| keys.pop_front().unwrap().0)
}
impl TrackCover {
    pub fn image(self, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
        if let Some(old) = touch(&mut cx.default_global::<Covers>().locations, &self) {
            cx.remove_asset::<Self>(&old);
        }
        let sources = window.use_asset::<Self>(&self, cx)?;
        for file in sources.iter() {
            // Wait for the preferred source before requesting its fallback.
            if let Some(image) = CoverThumbnail::get(file, window, cx)? {
                return Some(image);
            }
        }
        None
    }
}
pub struct CoverThumbnail;
impl CoverThumbnail {
    pub fn get(
        file: &CoverFile,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Option<Arc<RenderImage>>> {
        let cache = cx.default_global::<Covers>();
        if let Some(d) = crate::diagnostics::get() {
            d.cover_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let old = touch(&mut cache.images, file);
        if let Some(old) = old {
            if let Some(Some(image)) = cx.fetch_asset::<Self>(&old) {
                cx.drop_image(image, Some(window));
            }
            cx.remove_asset::<Self>(&old);
        }
        window.use_asset::<Self>(file, cx)
    }
}
impl Asset for CoverThumbnail {
    type Source = CoverFile;
    type Output = Option<Arc<RenderImage>>;
    fn load(cover: CoverFile, _: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        cover.render_thumbnail()
    }
}
impl CoverFile {
    async fn render_thumbnail(self) -> Option<Arc<RenderImage>> {
        static DECODERS: async_lock::Semaphore = async_lock::Semaphore::new(2);
        let _permit = DECODERS.acquire().await;
        if let Some(d) = crate::diagnostics::get() {
            d.cover_loads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let _timer = crate::diagnostics::Timer::measure(|d| &d.cover_load_us);
        let edge = if self.embedded { 56 } else { 96 };
        let image = if self.embedded {
            embedded_thumbnail(&self.path, edge)?
        } else {
            decode(&file_bytes(&self.path)?, edge)?
        };
        let mut bgra = image.into_rgba8();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Some(Arc::new(RenderImage::new([image::Frame::new(bgra)])))
    }
}

/// Limit actual reads, including after seeks, independently of recording size.
struct TagReader {
    file: fs::File,
    remaining: usize,
}
impl Read for TagReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let count = self.remaining.min(buf.len());
        let n = self.file.read(&mut buf[..count])?;
        self.remaining -= n;
        Ok(n)
    }
}
impl std::io::Seek for TagReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}
fn embedded_thumbnail(path: &Path, edge: u32) -> Option<image::DynamicImage> {
    let reader = TagReader {
        file: fs::File::open(path).ok()?,
        remaining: MAX_BYTES + 256 * 1024,
    };
    let tags = Probe::new(std::io::BufReader::new(reader))
        .guess_file_type()
        .ok()?
        .options(lofty::config::ParseOptions::new().read_properties(false))
        .read()
        .ok()?;
    let picture = front_picture(&tags)?;
    decode(picture.data(), edge)
}
fn front_picture(file: &lofty::file::TaggedFile) -> Option<&lofty::picture::Picture> {
    let mut pictures = file.tags().iter().flat_map(|t| t.pictures());
    pictures
        .clone()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.next())
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
        if let Some(pic) = front_picture(&file)
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
        if let Some(image) = file_bytes(&parent.join(name)).and_then(|bytes| thumbnail(&bytes, 288))
        {
            info.cover = Some(image);
            return info;
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_retains_a_shared_source_and_evicts_the_oldest_other_source() {
        let mut keys = LinkedHashMap::new();
        for k in 0..CAPACITY {
            assert!(touch(&mut keys, &k).is_none());
        }
        for _ in 0..13 {
            assert!(touch(&mut keys, &0).is_none());
        }
        assert_eq!(touch(&mut keys, &CAPACITY), Some(1));
        assert_eq!(keys.len(), CAPACITY);
    }
    #[test]
    fn compact_decode_rejects_corrupt_or_oversized_input() {
        assert!(decode(b"not an image", 56).is_none());
        assert!(decode(&vec![0; MAX_BYTES + 1], 56).is_none());
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(400, 200)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        let image = decode(bytes.get_ref(), 56).unwrap();
        assert_eq!((image.width(), image.height()), (56, 28));
    }
}

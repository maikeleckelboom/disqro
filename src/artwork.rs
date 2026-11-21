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
    let image = reader.decode().ok()?;
    // A small original stays as it is: enlarging it adds memory, not detail.
    Some(if image.width() > edge || image.height() > edge {
        image.thumbnail(edge, edge)
    } else {
        image
    })
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
/// Resolve paths only for painted Cover cells. A fresh folder scan or library
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
/// Thumbnails are decoded at a few fixed edges, in device pixels, so a cover
/// is decoded again only when its settled size needs a larger edge.
const EDGES: [u32; 5] = [64, 96, 128, 192, 256];
/// Decoded thumbnail pixels kept across every edge, about 16 MB of BGRA.
const PIXELS: u64 = 64 * 256 * 256;
/// The edges requested for one cover, as a bit per index into [`EDGES`].
type Requested = u8;

#[derive(Default)]
struct Covers {
    locations: LinkedHashMap<TrackCover, ()>,
    images: LinkedHashMap<CoverFile, Requested>,
    pixels: u64,
}
impl Global for Covers {}
fn touch<K: Clone + std::hash::Hash + Eq>(keys: &mut LinkedHashMap<K, ()>, key: &K) -> Option<K> {
    if keys.to_back(key).is_some() {
        return None;
    }
    keys.insert(key.clone(), ());
    (keys.len() > CAPACITY).then(|| keys.pop_front().unwrap().0)
}
fn edges(requested: Requested) -> impl DoubleEndedIterator<Item = usize> {
    (0..EDGES.len()).filter(move |i| requested & 1 << i != 0)
}
fn area(requested: Requested) -> u64 {
    edges(requested).map(|i| u64::from(EDGES[i]).pow(2)).sum()
}
/// The smallest edge that covers `size` logical pixels on this display.
fn edge_index(size: f32, scale: f32) -> usize {
    EDGES
        .iter()
        .position(|&edge| edge as f32 >= size * scale)
        .unwrap_or(EDGES.len() - 1)
}
impl Covers {
    /// Records a newly requested edge and returns the least recently used
    /// covers that no longer fit, with every edge they hold.
    fn request(&mut self, file: &CoverFile, edge: usize) -> Vec<(CoverFile, Requested)> {
        let bit = 1 << edge;
        match self.images.to_back(file) {
            Some(requested) if *requested & bit != 0 => return Vec::new(),
            Some(requested) => *requested |= bit,
            None => {
                self.images.insert(file.clone(), bit);
            }
        }
        self.pixels += u64::from(EDGES[edge]).pow(2);
        let mut evicted = Vec::new();
        while self.images.len() > 1 && (self.images.len() > CAPACITY || self.pixels > PIXELS) {
            let (old, requested) = self.images.pop_front().unwrap();
            self.pixels -= area(requested);
            evicted.push((old, requested));
        }
        evicted
    }
}
impl TrackCover {
    /// The best available image for a cover drawn at `size` logical pixels.
    pub fn image(self, size: f32, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
        if let Some(old) = touch(&mut cx.default_global::<Covers>().locations, &self) {
            cx.remove_asset::<Self>(&old);
        }
        let sources = window.use_asset::<Self>(&self, cx)?;
        for file in sources.iter() {
            // Wait for the preferred source before requesting its fallback.
            if let Some(image) = CoverThumbnail::get(file, size, window, cx)? {
                return Some(image);
            }
        }
        None
    }
}
/// One cover decoded to fit an edge from [`EDGES`].
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct Thumbnail {
    file: CoverFile,
    edge: u32,
}
pub struct CoverThumbnail;
impl CoverThumbnail {
    /// The thumbnail of `file` for `size` logical pixels: `None` while it
    /// loads, `Some(None)` when the file has no usable image. A loaded larger
    /// edge is reused, and a loaded smaller one stands in while it decodes.
    pub fn get(
        file: &CoverFile,
        size: f32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Option<Arc<RenderImage>>> {
        if let Some(d) = crate::diagnostics::get() {
            d.cover_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let wanted = edge_index(size, window.scale_factor());
        let cache = cx.default_global::<Covers>();
        let requested = cache.images.to_back(file).map_or(0, |r| *r);
        let key = |edge: usize| Thumbnail {
            file: file.clone(),
            edge: EDGES[edge],
        };
        for edge in edges(requested).filter(|&e| e >= wanted) {
            if let Some(result) = cx.fetch_asset::<Self>(&key(edge)) {
                return Some(result);
            }
        }
        if requested & 1 << wanted == 0 {
            for (old, held) in cx.default_global::<Covers>().request(file, wanted) {
                for edge in edges(held) {
                    let old = Thumbnail {
                        file: old.clone(),
                        edge: EDGES[edge],
                    };
                    if let Some(Some(image)) = cx.fetch_asset::<Self>(&old) {
                        cx.drop_image(image, Some(window));
                    }
                    cx.remove_asset::<Self>(&old);
                }
            }
        }
        if let Some(result) = window.use_asset::<Self>(&key(wanted), cx) {
            return Some(result);
        }
        edges(requested)
            .rev()
            .filter(|&e| e < wanted)
            .find_map(|edge| cx.fetch_asset::<Self>(&key(edge)).flatten())
            .map(Some)
    }
}
impl Asset for CoverThumbnail {
    type Source = Thumbnail;
    type Output = Option<Arc<RenderImage>>;
    fn load(cover: Thumbnail, _: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        cover.render()
    }
}
impl Thumbnail {
    async fn render(self) -> Option<Arc<RenderImage>> {
        static DECODERS: async_lock::Semaphore = async_lock::Semaphore::new(2);
        let _permit = DECODERS.acquire().await;
        if let Some(d) = crate::diagnostics::get() {
            d.cover_loads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let _timer = crate::diagnostics::Timer::measure(|d| &d.cover_load_us);
        let Self { file, edge } = self;
        let image = if file.embedded {
            embedded_thumbnail(&file.path, edge)?
        } else {
            decode(&file_bytes(&file.path)?, edge)?
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
    fn covers_decode_at_few_edges_and_evict_by_pixels() {
        // 24 logical pixels at 150% and 200%, then 96 at 100% and 150%.
        assert_eq!(EDGES[edge_index(24., 1.5)], 64);
        assert_eq!(EDGES[edge_index(24., 2.)], 64);
        assert_eq!(EDGES[edge_index(96., 1.)], 96);
        assert_eq!(EDGES[edge_index(96., 1.5)], 192);
        assert_eq!(EDGES[edge_index(96., 4.)], 256);
        // Every size between two edges shares one decode.
        let during: std::collections::HashSet<_> =
            (25..=42).map(|s| edge_index(s as f32, 1.5)).collect();
        assert_eq!(during.len(), 1);

        let file = |n: usize| CoverFile {
            path: PathBuf::from(format!("{n}.jpg")),
            embedded: false,
            stamp: crate::cache::Stamp {
                bytes: 1,
                modified: 1,
            },
        };
        let mut covers = Covers::default();
        for n in 0..CAPACITY {
            assert!(covers.request(&file(n), 0).is_empty());
        }
        assert!(covers.request(&file(0), 0).is_empty());
        assert_eq!(covers.pixels, CAPACITY as u64 * 64 * 64);
        // Large covers fill the pixel budget long before the entry cap, and
        // the least recently used covers go first.
        let largest = EDGES.len() - 1;
        let mut evicted = Vec::new();
        for n in CAPACITY..CAPACITY + 60 {
            evicted.extend(covers.request(&file(n), largest));
        }
        assert!(covers.pixels <= PIXELS);
        assert_eq!(evicted.len(), CAPACITY - 64);
        assert!(evicted.iter().all(|(_, held)| *held == 1));
        // A cover keeps each edge it was asked for until it is evicted.
        let evicted = covers.request(&file(0), largest);
        assert_eq!(evicted.len(), 16);
        assert_eq!(covers.images.back().map(|(_, r)| *r), Some(0b10001));
        assert_eq!(covers.pixels, PIXELS);
    }
    #[test]
    fn small_originals_are_not_enlarged() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(80, 80)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        let image = decode(bytes.get_ref(), 256).unwrap();
        assert_eq!((image.width(), image.height()), (80, 80));
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

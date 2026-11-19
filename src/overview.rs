//! A compact overview for seeking in a recording or a CUE track. Each bin
//! retains both polarities across both channels, so antiphase audio stays visible.
use crate::{audio::Recording, library::Track};
use gpui_kit::{prelude::*, *};
use kira::{Frame, sound::streaming::Decoder as _};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::UNIX_EPOCH,
};

const BINS: usize = 1536;
#[derive(Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct SourceKey {
    path: PathBuf,
    length: u64,
    modified_ns: u128,
    start: u64,
    end: Option<u64>,
}
impl SourceKey {
    fn new(t: &Track) -> Option<Self> {
        let meta = fs::metadata(&t.source).ok()?;
        Some(Self {
            path: t.source.clone(),
            length: meta.len(),
            modified_ns: meta
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos(),
            start: t.disc.as_ref().map_or(0, |d| d.start),
            end: t.disc.as_ref().and_then(|d| d.end),
        })
    }
}
#[derive(Serialize, Deserialize)]
struct Cached {
    key: SourceKey,
    bins: Vec<[f32; 2]>,
}
fn slot(dir: &Path, key: &SourceKey) -> PathBuf {
    let mut hash = DefaultHasher::new();
    key.hash(&mut hash);
    dir.join(format!("overview-v1-{:02}.json", hash.finish() % 32))
}
fn read(path: &Path, key: &SourceKey) -> Option<Vec<[f32; 2]>> {
    let file = fs::File::open(path).ok()?;
    let data: Cached = serde_json::from_reader(file.take(128 * 1024)).ok()?;
    (data.key == *key && !data.bins.is_empty() && data.bins.len() <= BINS).then_some(data.bins)
}
fn save(dir: &Path, key: SourceKey, bins: &[[f32; 2]]) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    let path = slot(dir, &key);
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(
        &mut file,
        &Cached {
            key,
            bins: bins.to_vec(),
        },
    )?;
    file.flush()?;
    file.persist(path)?;
    Ok(())
}
fn accumulate(bins: &mut [[f32; 2]], total: usize, offset: usize, frames: &[Frame]) {
    for (i, f) in frames.iter().enumerate() {
        let bin = (offset + i) as u128 * bins.len() as u128 / total as u128;
        let p = &mut bins[bin as usize];
        p[0] = p[0].min(f.left.min(f.right));
        p[1] = p[1].max(f.left.max(f.right));
    }
}
pub fn load(
    track: &Track,
    cache: Option<PathBuf>,
    generation: &AtomicU64,
    revision: u64,
) -> Option<Arc<[[f32; 2]]>> {
    let key = SourceKey::new(track)?;
    if let Some(dir) = &cache
        && let Some(bins) = read(&slot(dir, &key), &key)
    {
        return Some(bins.into());
    }
    let mut source = Recording::open(track, key.end).ok()?;
    source.seek(0).ok()?;
    let count = source.num_frames();
    let mut bins = vec![[0., 0.]; BINS.min(count)];
    let mut offset = 0;
    while offset < count {
        if generation.load(Ordering::Relaxed) != revision {
            return None;
        }
        let frames = source.decode().ok()?;
        if frames.is_empty() {
            return None;
        }
        accumulate(&mut bins, count, offset, &frames);
        offset += frames.len();
    }
    if SourceKey::new(track).as_ref() != Some(&key) {
        return None;
    }
    if let Some(dir) = cache {
        let _ = save(&dir, key, &bins);
    }
    Some(bins.into())
}
pub fn paint(bins: Arc<[[f32; 2]]>, position: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width).floor() as usize;
            let height = f32::from(bounds.size.height);
            let columns = width.min(bins.len());
            let center = height / 2.;
            for x in 0..columns {
                let section = &bins[x * bins.len() / columns..(x + 1) * bins.len() / columns];
                let [lo, hi] = section
                    .iter()
                    .fold([0f32; 2], |[lo, hi], v| [lo.min(v[0]), hi.max(v[1])]);
                let top = center * (1. - hi.clamp(0., 1.));
                let bottom = center * (1. - lo.clamp(-1., 0.));
                if bottom <= top {
                    continue;
                }
                let x0 = (x * width / columns) as f32;
                let x1 = ((x + 1) * width / columns) as f32;
                let color = if (x as f32) < position * columns as f32 {
                    0x85d9cf
                } else {
                    0x477a78
                };
                window.paint_quad(fill(
                    Bounds::new(
                        bounds.origin + point(px(x0), px(top)),
                        size(px(x1 - x0), px(bottom - top)),
                    ),
                    rgb(color),
                ));
            }
        },
    )
    .size_full()
}
#[cfg(test)]
mod tests {
    use super::{Frame, accumulate};
    #[test]
    fn retains_antiphase_impulses_and_silence() {
        let mut b = [[0.; 2]; 3];
        accumulate(
            &mut b,
            6,
            0,
            &[
                Frame::ZERO,
                Frame::new(-0.8, 0.8),
                Frame::ZERO,
                Frame::ZERO,
                Frame::new(1., 0.),
                Frame::ZERO,
            ],
        );
        assert_eq!(b, [[-0.8, 0.8], [0., 0.], [0., 1.]]);
    }
}

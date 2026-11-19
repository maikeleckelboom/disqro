//! A compact overview for seeking in a recording or a CUE track. Each bin
//! retains both polarities across both channels, so antiphase audio stays
//! visible, and the RMS level, which shows structure inside loud masters.
use crate::cache::{Cache, Stamp};
use crate::{audio::Recording, library::Track};
use gpui_kit::{prelude::*, *};
use kira::{Frame, sound::streaming::Decoder as _};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const BINS: usize = 1536;
/// Minimum, maximum and RMS level of one slice of the recording.
pub type Bin = [f32; 3];
fn accumulate(
    bins: &mut [Bin],
    energy: &mut [(f64, u32)],
    total: usize,
    offset: usize,
    frames: &[Frame],
) {
    for (i, f) in frames.iter().enumerate() {
        let bin = ((offset + i) as u128 * bins.len() as u128 / total as u128) as usize;
        let p = &mut bins[bin];
        p[0] = p[0].min(f.left.min(f.right));
        p[1] = p[1].max(f.left.max(f.right));
        let e = &mut energy[bin];
        e.0 += f64::from(f.left * f.left + f.right * f.right) / 2.;
        e.1 += 1;
    }
}
fn finish(bins: &mut [Bin], energy: &[(f64, u32)]) {
    for (b, (sum, n)) in bins.iter_mut().zip(energy) {
        b[2] = if *n > 0 {
            (sum / f64::from(*n)).sqrt() as f32
        } else {
            0.
        };
    }
}
pub fn load(
    track: &Track,
    cache: Option<PathBuf>,
    generation: &AtomicU64,
    revision: u64,
) -> Option<Arc<[Bin]>> {
    let stamp = Stamp::read(&track.source)?;
    let start = track.disc.as_ref().map_or(0, |d| d.start);
    let end = track.disc.as_ref().and_then(|d| d.end);
    let cache = cache.and_then(|dir| Cache::open(&dir).ok());
    if let Some(data) = cache
        .as_ref()
        .and_then(|c| c.peaks(&track.source, start, end, &stamp))
        && !data.is_empty()
        && data.len() <= BINS * 12
        && data.len() % 12 == 0
    {
        let bins = data
            .as_chunks::<12>()
            .0
            .iter()
            .map(|b| {
                std::array::from_fn(|i| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect::<Vec<Bin>>();
        if bins.iter().flatten().all(|v| v.is_finite()) {
            return Some(bins.into());
        }
    }
    let mut source = Recording::open(track, end).ok()?;
    source.seek(0).ok()?;
    let count = source.num_frames();
    let mut bins = vec![[0., 0., 0.]; BINS.min(count)];
    let mut energy = vec![(0., 0); bins.len()];
    let mut offset = 0;
    while offset < count {
        if generation.load(Ordering::Relaxed) != revision {
            return None;
        }
        let frames = source.decode().ok()?;
        if frames.is_empty() {
            return None;
        }
        accumulate(&mut bins, &mut energy, count, offset, &frames);
        offset += frames.len();
    }
    finish(&mut bins, &energy);
    if Stamp::read(&track.source).as_ref() != Some(&stamp) {
        return None;
    }
    if let Some(cache) = cache {
        let bytes = bins
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let _ = cache.store_peaks(&track.source, start, end, &stamp, &bytes);
    }
    Some(bins.into())
}
/// Draws the overview with the played part in the waveform's own colors.
pub fn paint(bins: Arc<[Bin]>, position: f32, wave: crate::theme::Wave) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width).floor() as usize;
            let height = f32::from(bounds.size.height);
            let columns = width.min(bins.len());
            if columns == 0 {
                return;
            }
            let center = (height / 2.).floor();
            let played = position * columns as f32;
            for x in 0..columns {
                let section = &bins[x * bins.len() / columns..(x + 1) * bins.len() / columns];
                let [lo, hi, rms] = section.iter().fold([0f32; 3], |[lo, hi, rms], v| {
                    [lo.min(v[0]), hi.max(v[1]), rms.max(v[2])]
                });
                let top = center * (1. - hi.clamp(0., 1.));
                let bottom = center + center * (-lo).clamp(0., 1.);
                let x0 = (x * width / columns) as f32;
                let x1 = ((x + 1) * width / columns) as f32;
                let is_played = (x as f32) < played;
                let (peak, body) = if is_played {
                    (wave.played_peak, wave.played_body)
                } else {
                    (wave.ahead_peak, wave.ahead_body)
                };
                // Keep silence visible as a hairline so the track stays legible.
                let (top, bottom) = if bottom - top < 1. {
                    (center - 0.5, center + 0.5)
                } else {
                    (top, bottom)
                };
                let column = |a: f32, b: f32| {
                    Bounds::new(
                        bounds.origin + point(px(x0), px(a)),
                        size(px(x1 - x0), px(b - a)),
                    )
                };
                window.paint_quad(fill(column(top, bottom), peak));
                let r = rms.clamp(0., 1.) * center;
                if r >= 0.5 {
                    window.paint_quad(fill(
                        column((center - r).max(top), (center + r).min(bottom)),
                        body,
                    ));
                }
            }
        },
    )
    .size_full()
}
#[cfg(test)]
mod tests {
    use super::{Frame, accumulate, finish};
    #[test]
    fn retains_antiphase_impulses_and_silence() {
        let mut b = [[0.; 3]; 3];
        let mut e = [(0., 0); 3];
        accumulate(
            &mut b,
            &mut e,
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
        finish(&mut b, &e);
        assert_eq!(b[1], [0., 0., 0.]);
        assert_eq!([b[0][0], b[0][1], b[2][0], b[2][1]], [-0.8, 0.8, 0., 1.]);
        assert!((b[0][2] - 0.32f32.sqrt()).abs() < 1e-6);
        assert!((b[2][2] - 0.5).abs() < 1e-6);
    }
}

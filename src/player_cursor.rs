//! The playhead of the compact waveform. It moves at the display's refresh
//! rate over an overview that stays as it was drawn: each frame redraws only
//! a small GPU target holding the cursor, and the window presents its last
//! scene again, so the overview, artwork, labels and controls are neither
//! rebuilt nor laid out. Its position comes from the same stabilized audio
//! clock and the same rules as the detailed waveform's playhead.
use super::wave::WaveTrack;
use crate::audio::PlaybackIdentity;
use crate::timeline::Program;
use gpui_kit::*;
use std::{sync::Arc, time::Instant};

/// The cursor's width in logical pixels.
pub(super) const WIDTH: f32 = 2.;

/// What the strip shows, so the cursor maps a source frame to the same
/// horizontal position as the overview drawn beneath it.
#[derive(Clone)]
pub(super) struct Mapping {
    pub program: Arc<Program>,
    /// The segment the strip shows, or the playing one in Full CD.
    pub segment: usize,
    pub full_cd: bool,
}

impl Mapping {
    /// The fraction of the strip at source `frame`. Full CD finds the
    /// chapter of the shown source that holds it; Track stays within its
    /// chapter until the strip shows the next one.
    pub(super) fn fraction(&self, frame: f64) -> Option<f64> {
        let program = &self.program;
        let shown = program.segments.get(self.segment)?;
        if !self.full_cd {
            let span = shown.time.end.saturating_sub(shown.time.start) as f64;
            let at = shown.at_fractional(frame) - shown.time.start as f64;
            return Some(if span > 0. {
                (at / span).clamp(0., 1.)
            } else {
                0.
            });
        }
        let segment = if shown.frames.contains(&(frame.max(0.) as usize)) {
            shown
        } else {
            program
                .segments
                .iter()
                .find(|s| s.source == shown.source && s.frames.contains(&(frame.max(0.) as usize)))
                .unwrap_or(shown)
        };
        let duration = program.duration as f64;
        Some(if duration > 0. {
            (segment.at_fractional(frame) / duration).clamp(0., 1.)
        } else {
            0.
        })
    }
}

/// The columns a vertical line `width` device pixels wide, centered at `x`,
/// covers, with the covered share of each, so the line moves continuously
/// between pixels instead of stepping from one to the next.
pub(super) fn columns(x: f64, width: f64, limit: i32) -> impl Iterator<Item = (i32, f32)> {
    let (left, right) = (x - width / 2., x + width / 2.);
    let first = left.floor() as i32;
    let last = right.ceil() as i32;
    (first.max(0)..last.min(limit)).filter_map(move |column| {
        let c = f64::from(column);
        let covered = (right.min(c + 1.) - left.max(c)).clamp(0., 1.) as f32;
        (covered > 1. / 512.).then_some((column, covered))
    })
}

/// The compact playhead's own state: where it last was, and its GPU target.
#[derive(Default)]
pub(super) struct Cursor {
    pub mapping: Option<Mapping>,
    /// A seek preview while the strip is dragged.
    pub preview: Option<f64>,
    last: Option<(PlaybackIdentity, f64)>,
    /// The horizontal position last drawn, in device pixels.
    pub drawn: Option<f64>,
    #[cfg(windows)]
    target: Option<Target>,
    /// Drawing into a GPU target failed; the strip falls back to an element.
    pub unavailable: bool,
}

#[cfg(windows)]
struct Target {
    texture: gpui_windows_gpu::GpuTexture,
    size: Size<DevicePixels>,
    scale: f32,
    bounds: Bounds<Pixels>,
}

/// Where the cursor is at `at`, and whether playback moves it.
pub(super) struct Placement {
    pub fraction: f64,
    pub moving: bool,
}

impl Cursor {
    /// The fraction of the strip the playhead is at when `at` is shown.
    pub(super) fn place(&mut self, track: Option<&WaveTrack>, at: Instant) -> Option<Placement> {
        if let Some(preview) = self.preview {
            return Some(Placement {
                fraction: preview,
                moving: false,
            });
        }
        let track = track?;
        let mapping = self.mapping.as_ref()?;
        let previous = self
            .last
            .filter(|(identity, _)| track.identity.continues(*identity))
            .map(|(_, frame)| frame);
        let presented = track.presented_frame(at, previous);
        self.last = Some((track.identity, presented.frame));
        Some(Placement {
            fraction: mapping.fraction(presented.frame)?,
            moving: presented.advancing,
        })
    }

    /// Forgets the playhead's last position, as when another recording or
    /// view starts.
    pub(super) fn reset(&mut self) {
        self.last = None;
    }

    /// Paints the cursor's target into the scene at `bounds`, drawn at
    /// `fraction`. Returns `false` when the strip must draw the cursor as an
    /// element instead.
    #[cfg(windows)]
    pub(super) fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        fraction: Option<f64>,
        color: Hsla,
        window: &mut Window,
    ) -> bool {
        if self.unavailable {
            return false;
        }
        let scale = window.scale_factor();
        let size = size(
            DevicePixels((f32::from(bounds.size.width) * scale).round().max(1.) as i32),
            DevicePixels((f32::from(bounds.size.height) * scale).round().max(1.) as i32),
        );
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.size != size || target.scale != scale)
        {
            self.target = None;
            self.drawn = None;
            match gpui_windows_gpu::with_d3d11(window, |_, device, _| {
                gpui_windows_gpu::GpuTexture::new(device, size)
            }) {
                Ok(texture) => {
                    self.target = Some(Target {
                        texture,
                        size,
                        scale,
                        bounds,
                    })
                }
                Err(_) => {
                    self.unavailable = true;
                    return false;
                }
            }
        }
        if let Some(target) = &mut self.target {
            target.bounds = bounds;
        }
        if self.draw(fraction, color, window).is_err() {
            self.unavailable = true;
            self.target = None;
            return false;
        }
        let Some(target) = &self.target else {
            return false;
        };
        target.texture.paint(window, bounds).is_ok() && target.texture.written(window).is_ok()
    }

    /// Redraws the cursor in the target the scene already shows. Returns
    /// `false` when the strip must be painted first.
    #[cfg(windows)]
    pub(super) fn repaint(
        &mut self,
        fraction: Option<f64>,
        color: Hsla,
        window: &mut Window,
    ) -> bool {
        if self.target.is_none() || self.unavailable {
            return false;
        }
        let x = fraction.and_then(|fraction| self.device_x(fraction));
        if x.is_some() && x == self.drawn {
            return true;
        }
        self.draw(fraction, color, window).is_ok()
    }

    #[cfg(windows)]
    fn device_x(&self, fraction: f64) -> Option<f64> {
        let target = self.target.as_ref()?;
        Some(fraction.clamp(0., 1.) * f64::from(target.size.width.0))
    }

    #[cfg(windows)]
    fn draw(
        &mut self,
        fraction: Option<f64>,
        color: Hsla,
        window: &mut Window,
    ) -> anyhow::Result<()> {
        use windows::{
            Win32::{Foundation::RECT, Graphics::Direct3D11::ID3D11DeviceContext1},
            core::Interface,
        };
        let Some(target) = &self.target else {
            anyhow::bail!("no cursor target");
        };
        let x = fraction.and_then(|fraction| self.device_x(fraction));
        let (width, height) = (target.size.width.0, target.size.height.0);
        let line = f64::from(WIDTH * target.scale);
        let rgba = Rgba::from(color);
        let view = target.texture.target().clone();
        gpui_windows_gpu::with_d3d11(window, |_, _, context| {
            let context: ID3D11DeviceContext1 = context.cast()?;
            let view: windows::Win32::Graphics::Direct3D11::ID3D11View = view.cast()?;
            // SAFETY: the view belongs to this device and stays alive for the
            // call; every rectangle lies inside the target.
            unsafe {
                context.ClearView(&view, &[0.; 4], None);
                if let Some(x) = x {
                    for (column, covered) in columns(x, line, width) {
                        let rect = RECT {
                            left: column,
                            top: 0,
                            right: column + 1,
                            bottom: height,
                        };
                        context.ClearView(
                            &view,
                            &[rgba.r, rgba.g, rgba.b, rgba.a * covered],
                            Some(&[rect]),
                        );
                    }
                }
            }
            Ok(())
        })?;
        self.drawn = x;
        if let Some((at, x)) = crate::diagnostics::begin_stage().zip(x) {
            crate::diagnostics::stage(Some(at), "wave_compact_cursor", x.to_bits(), width as u64);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Mapping, columns};
    use crate::{audio::tests::fixture, library::DiscContext, timeline::Program};
    use std::{fs, sync::Arc};

    #[test]
    fn track_and_full_cd_place_the_playhead_by_program_time_at_mixed_rates() {
        let dir = tempfile::tempdir().unwrap();
        let mut cue = String::new();
        let mut number = 0;
        for (name, rate, frames, ticks) in [
            ("a", 44100, 44117, &[3, 12][..]),
            ("b", 48000, 48011, &[2, 40]),
            ("c", 96000, 96007, &[0, 7]),
        ] {
            fixture(&dir.path().join(format!("{name}.wav")), rate, frames);
            cue.push_str(&format!("FILE {name}.wav WAVE\n"));
            for tick in ticks {
                number += 1;
                cue.push_str(&format!(
                    "TRACK {number:02} AUDIO\nINDEX 01 00:00:{tick:02}\n"
                ));
            }
        }
        let sheet = dir.path().join("mix.cue");
        fs::write(&sheet, cue).unwrap();
        let track = Arc::new(crate::cue::read(&sheet).unwrap().remove(1));
        let program = Arc::new(Program::resolve(&track, DiscContext::Inferred, None).unwrap());
        let duration = program.duration as f64;
        for (index, segment) in program.segments.iter().enumerate() {
            let track_view = Mapping {
                program: program.clone(),
                segment: index,
                full_cd: false,
            };
            let full_cd = Mapping {
                full_cd: true,
                ..track_view.clone()
            };
            let (start, end) = (segment.frames.start as f64, segment.frames.end as f64);
            assert!(track_view.fraction(start).unwrap().abs() < 1e-6);
            assert!((track_view.fraction(end).unwrap() - 1.).abs() < 1e-6);
            // Before and after the chapter, Track stays at its edges.
            assert_eq!(track_view.fraction(start - 10.).unwrap(), 0.);
            assert!((track_view.fraction(end + 10.).unwrap() - 1.).abs() < 1e-6);
            // Full CD agrees with program time at every whole frame, and
            // moves on between them.
            for frame in [start, ((start + end) / 2.).floor(), end - 1.] {
                let expected = segment.at(frame as usize) as f64 / duration;
                assert!((full_cd.fraction(frame).unwrap() - expected).abs() < 2. / duration);
            }
            let between = full_cd.fraction(start + 0.5).unwrap();
            assert!(between > full_cd.fraction(start).unwrap());
            assert!(between < full_cd.fraction(start + 1.).unwrap());
            // A Full CD playhead past the shown chapter, in the same source,
            // is placed in the chapter that holds it.
            if let Some(next) = program.segments.get(index + 1)
                && next.source == segment.source
            {
                let frame = next.frames.start as f64 + 3.;
                let expected = next.at(next.frames.start + 3) as f64 / duration;
                assert!((full_cd.fraction(frame).unwrap() - expected).abs() < 2. / duration);
            }
        }
    }

    #[test]
    fn the_cursor_moves_continuously_between_device_pixels() {
        // Three device pixels wide, as two logical pixels at 150%.
        let mut previous: Option<f64> = None;
        for step in 0..400 {
            let x = 100. + f64::from(step) * 0.013;
            let covered: Vec<_> = columns(x, 3., 1000).collect();
            let total: f32 = covered.iter().map(|(_, c)| c).sum();
            assert!((total - 3.).abs() < 1e-3, "{covered:?}");
            // The coverage-weighted center follows the position exactly.
            let center: f64 = covered
                .iter()
                .map(|(column, c)| (f64::from(*column) + 0.5) * f64::from(*c))
                .sum::<f64>()
                / f64::from(total);
            assert!((center - x).abs() < 1e-3, "{x} drew at {center}");
            if let Some(previous) = previous {
                assert!(center > previous);
            }
            previous = Some(center);
        }
        // Clipped at both edges of the strip.
        assert!(columns(0.4, 3., 1000).all(|(c, _)| c >= 0));
        assert!(columns(999.8, 3., 1000).all(|(c, _)| c < 1000));
    }
}

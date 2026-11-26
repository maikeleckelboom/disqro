//! In-process GPUI input for the isolated, silent inspection qualification.
use super::*;
use crate::audio::Command;
use std::{path::PathBuf, time::Duration};

fn key(name: &'static str) -> PlatformInput {
    PlatformInput::KeyDown(KeyDownEvent {
        keystroke: Keystroke::parse(name).unwrap(),
        is_held: false,
        prefer_character_input: false,
    })
}

fn hover_point(panel: &Waveform) -> Option<Point<Pixels>> {
    let displayed = panel.displayed.borrow();
    let shown = displayed.as_ref()?;
    let layout = shown.layout;
    // The fixture's signal is near mid-height. Hover well above the trace.
    let frame = shown.viewport.frame_at(0.35).round();
    let column = ((frame + 0.2 - shown.viewport.first) / shown.viewport.span) as f32;
    let point = layout.bounds.origin
        + point(
            layout.bounds.size.width * column,
            layout.bounds.size.height * 0.12,
        );
    let hit = panel.texture.sample_hit(layout.physical(point))?;
    assert_eq!(hit.frame, frame as u64);
    let below = Point {
        y: layout.bounds.bottom() - layout.bounds.size.height * 0.12,
        ..point
    };
    assert_eq!(panel.texture.sample_hit(layout.physical(below)), Some(hit));
    Some(point)
}

struct Journey {
    panel: WeakEntity<Waveform>,
    folder: PathBuf,
}

impl Journey {
    async fn until<T>(
        &self,
        cx: &mut AsyncWindowContext,
        description: &str,
        mut probe: impl FnMut(&mut Waveform, &mut Window, &mut Context<Waveform>) -> Option<T>,
    ) -> anyhow::Result<T> {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(30))
                .await;
            if let Some(value) =
                cx.update(|w, cx| self.panel.update(cx, |p, cx| probe(p, w, cx)))??
            {
                return Ok(value);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "Timed out waiting for {description}"
            );
        }
    }

    async fn capture(&self, cx: &mut AsyncWindowContext, name: &str) -> anyhow::Result<()> {
        eprintln!("capture {name}");
        let path = self.folder.join(format!("{name}.png"));
        self.until(cx, name, |_, _, _| path.is_file().then_some(()))
            .await
    }

    async fn edit(&self, cx: &mut AsyncWindowContext, value: &str) -> anyhow::Result<()> {
        cx.update(|w, cx| self.panel.update(cx, |p, cx| p.zoom_focus.focus(w, cx)))??;
        dispatch(cx, key("enter"))?;
        self.until(cx, "focused zoom editor", |p, w, cx| {
            let editor = p.zoom_editor.as_ref()?;
            editor.update(cx, |input, cx| input.set_value(value.to_owned(), w, cx));
            Some(())
        })
        .await
    }

    async fn run(self, cx: &mut AsyncWindowContext) -> anyhow::Result<()> {
        let minimum_frame = std::env::var("DISQRO_INSPECTION_MIN_FRAME")
            .ok()
            .map(|v| v.parse::<usize>())
            .transpose()?
            .unwrap_or_default();
        self.until(cx, "playing surface", |p, _, cx| {
            if p.player.read(cx).wave_track()?.frame < minimum_frame {
                return None;
            }
            if p.displayed.borrow().is_none() || !p.player.read(cx).status.playing {
                return None;
            }
            p.player.update(cx, |player, cx| {
                player.command(Command::SetPlaying(false), cx)
            });
            Some(())
        })
        .await?;
        let initial = self
            .until(cx, "paused surface", |p, _, cx| {
                (!p.player.read(cx).status.playing)
                    .then_some(p.displayed.borrow().as_ref()?.viewport)
            })
            .await?;
        for (value, dismiss) in [
            ("145 ms", "escape"),
            ("not a duration", "enter"),
            ("1400 samples", "blur"),
        ] {
            self.edit(cx, value).await?;
            if dismiss == "blur" {
                cx.update(|w, cx| self.panel.update(cx, |p, cx| p.focus.focus(w, cx)))??;
            } else {
                dispatch(cx, key(dismiss))?;
            }
            self.until(cx, "uncommitted zoom dismissed", |p, _, _| {
                (p.zoom_editor.is_none() && p.position.viewport == initial).then_some(())
            })
            .await?;
        }
        let position = self
            .until(cx, "zoom readout", |p, _, _| {
                Some(p.displayed.borrow().as_ref()?.layout.bounds.origin + point(px(20.), px(-16.)))
            })
            .await?;
        dispatch(
            cx,
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                click_count: 1,
                ..Default::default()
            }),
        )?;
        dispatch(
            cx,
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                click_count: 1,
                ..Default::default()
            }),
        )?;
        self.until(cx, "clicked zoom editor", |p, w, cx| {
            p.zoom_editor
                .as_ref()?
                .update(cx, |input, cx| input.set_value("32 samples", w, cx));
            Some(())
        })
        .await?;
        self.until(cx, "original span while the edit is pending", |p, _, _| {
            (p.displayed.borrow().as_ref()?.viewport == initial).then_some(())
        })
        .await?;
        dispatch(cx, key("enter"))?;
        // Recovering the center from first + half-span can lose an ulp of
        // the wider viewport when the recording starts near its left edge.
        let center_tolerance = 2. * f64::EPSILON * initial.first.abs().max(initial.span);
        let same_center =
            |v: Viewport| (v.frame_at(0.5) - initial.frame_at(0.5)).abs() <= center_tolerance;
        let pointer = self
            .until(cx, "decoded sample points", |p, _, _| {
                let shown = p.displayed.borrow().as_ref()?.viewport;
                if shown.span != 32. || !same_center(shown) {
                    return None;
                }
                hover_point(p)
            })
            .await?;
        self.edit(cx, "32 samples").await?;
        self.capture(cx, "zoom-editor").await?;
        dispatch(cx, key("escape"))?;
        self.until(cx, "editor dismissed over decoded trace", |p, _, _| {
            (p.zoom_editor.is_none() && hover_point(p).is_some()).then_some(())
        })
        .await?;
        self.capture(cx, "sample-points").await?;
        dispatch(
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: pointer,
                ..Default::default()
            }),
        )?;
        self.capture(cx, "sample-inspector").await?;
        let modifier = |shift| {
            PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers: Modifiers {
                    shift,
                    ..Default::default()
                },
                ..Default::default()
            })
        };
        dispatch(cx, modifier(true))?;
        self.capture(cx, "cue-times").await?;
        dispatch(cx, modifier(false))?;
        cx.update(|_, cx| {
            self.panel.update(cx, |p, cx| {
                assert!(same_center(p.position.viewport));
                p.player.update(cx, |player, cx| {
                    player.command(Command::SetPlaying(true), cx)
                });
            })
        })??;
        self.until(cx, "32-frame playback", |p, _, cx| {
            let view = p.displayed.borrow().as_ref()?.viewport;
            (p.player.read(cx).status.playing
                && view.span == 32.
                && view.first > initial.frame_at(0.5) + 32.
                && hover_point(p).is_none())
            .then_some(())
        })
        .await?;
        self.capture(cx, "playback").await?;
        cx.update(|_, cx| {
            self.panel.update(cx, |p, cx| {
                p.player.update(cx, |player, cx| {
                    player.command(Command::SetPlaying(false), cx)
                })
            })
        })??;
        self.until(cx, "restored sample points", |p, _, cx| {
            (!p.player.read(cx).status.playing)
                .then(|| hover_point(p))
                .flatten()
        })
        .await?;
        let gesture = self
            .until(cx, "stationary scrub origin", |p, _, cx| {
                let track = p.player.read(cx).wave_track()?;
                let from = track.frame as f64;
                let gesture = p.player.update(cx, |player, cx| {
                    player.begin_waveform_scrub(track.seek?, from, false, cx)
                })?;
                gesture.move_to(from + 4., from, 480., Instant::now());
                Some(gesture)
            })
            .await?;
        // Manual scrubbing keeps the decoded markers but suspends the
        // measurement, selection ring and guide under a resting pointer.
        let during = self
            .until(cx, "scrub markers without measurements", |p, _, cx| {
                if !p.player.read(cx).waveform_grabbed() {
                    return None;
                }
                let point = hover_point(p)?;
                (p.requested_points.get() == SamplePoints::Visible).then_some(point)
            })
            .await?;
        dispatch(
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: during,
                ..Default::default()
            }),
        )?;
        self.capture(cx, "scrub-markers").await?;
        cx.update(|_, cx| {
            self.panel.update(cx, |p, cx| {
                assert!(p.player.read(cx).waveform_grabbed());
                assert_eq!(p.requested_points.get(), SamplePoints::Visible);
            })
        })??;
        gesture.finish(crate::audio::scrub::End::Cancel);
        self.until(cx, "measurement restored after scrub", |p, _, cx| {
            (!p.player.read(cx).waveform_grabbed()
                && matches!(p.requested_points.get(), SamplePoints::Highlight(_)))
            .then_some(())
        })
        .await?;
        let previous_width = self
            .until(cx, "registered surface before resize", |p, w, _| {
                let width = p.displayed.borrow().as_ref()?.layout.bounds.size.width;
                let old = w.viewport_size();
                w.resize(size(
                    (old.width - px(520.)).max(px(600.)),
                    old.height - px(60.),
                ));
                Some(width)
            })
            .await?;
        let previous_hit = self
            .until(cx, "new hit transform after resize", |p, _, cx| {
                let layout = p.displayed.borrow().as_ref()?.layout;
                if layout.bounds.size.width == previous_width {
                    return None;
                }
                let point = layout.physical(hover_point(p)?);
                assert!(p.texture.sample_hit(point).is_some());
                // Fail allocation at a stable size. Intermediate resize targets
                // can legitimately recover on the very next layout.
                p.texture.clear();
                waveform::gpui::fail_next_target();
                cx.notify();
                Some(point)
            })
            .await?;
        self.until(
            cx,
            "failed surface rejects previous hit transform",
            |p, _, _| {
                (p.texture.unavailable()
                    && p.displayed.borrow().is_none()
                    && p.texture.sample_hit(previous_hit).is_none())
                .then_some(())
            },
        )
        .await?;
        self.until(cx, "resident samples after target retry", |p, _, _| {
            hover_point(p)
        })
        .await?;
        let narrow = self
            .until(cx, "resized sample", |p, _, _| hover_point(p))
            .await?;
        dispatch(
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: narrow,
                ..Default::default()
            }),
        )?;
        self.capture(cx, "sample-narrow").await?;
        // Marker size follows the physical frame spacing at each level of
        // detail, and markers disappear below five logical pixels per frame.
        let width = self
            .until(cx, "narrow deck width", |p, _, _| {
                Some(f32::from(
                    p.displayed.borrow().as_ref()?.layout.bounds.size.width,
                ))
            })
            .await?;
        // Twelve, six and three logical pixels per frame.
        for (spacing, name) in [(12., "lod-close"), (6., "lod-threshold"), (3., "lod-dense")] {
            let span = (width / spacing).floor().max(32.) as f64;
            self.edit(cx, &format!("{span} samples")).await?;
            dispatch(cx, key("enter"))?;
            let visible = self
                .until(cx, name, |p, _, _| {
                    let (viewport, layout) = {
                        let displayed = p.displayed.borrow();
                        let shown = displayed.as_ref()?;
                        (shown.viewport, shown.layout)
                    };
                    if viewport.span != span {
                        return None;
                    }
                    let scale = layout.scale;
                    let spacing = f32::from(layout.bounds.size.width) * scale / span as f32;
                    let visible = spacing >= 5. * scale;
                    let requested = p.requested_points.get();
                    ((requested != SamplePoints::Hidden) == visible).then_some(visible)
                })
                .await?;
            eprintln!("{name} markers {visible}");
            self.capture(cx, name).await?;
        }
        eprintln!("inspection-complete");
        Ok(())
    }
}

fn dispatch(cx: &mut AsyncWindowContext, event: PlatformInput) -> anyhow::Result<()> {
    cx.update(|w, cx| {
        w.dispatch_event(event, cx);
    })
}

pub(super) fn start(window: &Window, cx: &mut Context<Waveform>) {
    let Some(folder) = std::env::var_os("DISQRO_INSPECTION_JOURNEY").map(PathBuf::from) else {
        return;
    };
    if !crate::audio::silent_qualification() {
        return;
    }
    let journey = Journey {
        panel: cx.weak_entity(),
        folder,
    };
    window
        .spawn(cx, async move |cx| {
            if let Err(error) = journey.run(cx).await {
                eprintln!("inspection-failed {error:#}");
            }
        })
        .detach();
}

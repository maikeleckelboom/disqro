//! In-process GPUI input for the isolated, silent player deep-zoom qualification.
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
        // Input dispatch can finish before GPUI paints the new focus and hover
        // styles. Let one requested layout reach the window before PrintWindow.
        let painted = Rc::new(Cell::new(false));
        let signal = painted.clone();
        cx.update(|window, _| {
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |_, _| signal.set(true));
            });
            window.refresh();
        })?;
        self.until(cx, "paint after input", |_, _, _| {
            painted.get().then_some(())
        })
        .await?;
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
        let minimum_frame = std::env::var("DISQRO_ZOOM_MIN_FRAME")
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
                let track = p.player.read(cx).wave_track()?;
                let clock = track.position(Instant::now())?;
                let shown = p.displayed.borrow().as_ref()?.viewport;
                let held = Viewport::centered(clock.frame, shown.span);
                // A pause command can be acknowledged while the final
                // rendered block is still draining to its held position.
                (!track.playing
                    && !clock.advancing
                    && shown == held
                    && shown == p.position.viewport)
                    .then_some(shown)
            })
            .await?;
        for (value, dismiss) in [
            ("145 ms", "escape"),
            ("not a duration", "enter"),
            ("1.5 ms", "blur"),
        ] {
            self.edit(cx, value).await?;
            if dismiss == "blur" {
                cx.update(|w, cx| self.panel.update(cx, |p, cx| p.focus.focus(w, cx)))??;
            } else {
                dispatch(cx, key(dismiss))?;
            }
            self.until(
                cx,
                &format!("uncommitted zoom dismissed by {dismiss}"),
                |p, _, _| (p.zoom_editor.is_none() && p.position.viewport == initial).then_some(()),
            )
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
                .update(cx, |input, cx| input.set_value("0.1 ms", w, cx));
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
        let (pointer, geometry) = self
            .until(cx, "resident 32-frame continuous trace", |p, _, _| {
                let displayed = p.displayed.borrow();
                let shown = displayed.as_ref()?;
                if shown.viewport.span != 32. || !same_center(shown.viewport) || p.coverage.is_some() {
                    return None;
                }
                let layout = shown.layout;
                let b = layout.bounds;
                let physical = |value: Pixels| (f32::from(value) * layout.scale).round() as i32;
                let local = [physical(b.size.width) as f64 * 0.35, physical(b.size.height) as f64 * 0.3];
                assert!(p.texture.sample_hit(local).is_none());
                let pointer = b.origin + point(b.size.width * 0.35, b.size.height * 0.3);
                Some((pointer, serde_json::json!({
                    "first": shown.viewport.first, "span": shown.viewport.span,
                    "bounds": [physical(b.origin.x), physical(b.origin.y), physical(b.size.width), physical(b.size.height)],
                    "scale": layout.scale, "axis": format!("{:?}", layout.axis),
                })))
            })
            .await?;
        std::fs::write(
            self.folder.join("projection.json"),
            serde_json::to_vec(&geometry)?,
        )?;
        // Leave the zoom button before taking the stationary control so its
        // ordinary hover underline cannot enter the waveform comparison.
        dispatch(
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: pointer + point(px(24.), px(12.)),
                ..Default::default()
            }),
        )?;
        self.capture(cx, "deep-zoom").await?;
        dispatch(
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: pointer,
                ..Default::default()
            }),
        )?;
        self.capture(cx, "hover").await?;
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
        self.capture(cx, "shift-hover").await?;
        dispatch(cx, modifier(false))?;
        dispatch(
            cx,
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Right,
                position: pointer,
                click_count: 1,
                ..Default::default()
            }),
        )?;
        dispatch(
            cx,
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Right,
                position: pointer,
                click_count: 1,
                ..Default::default()
            }),
        )?;
        self.capture(cx, "panel-menu").await?;
        dispatch(cx, key("escape"))?;
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
                && p.texture.sample_hit([10., 10.]).is_none())
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
        self.until(cx, "held playback before scrub", |p, _, cx| {
            let track = p.player.read(cx).wave_track()?;
            (!track.playing && !track.position(Instant::now())?.advancing).then_some(())
        })
        .await?;
        let gesture = self
            .until(cx, "stationary scrub origin", |p, _, cx| {
                let track = p.player.read(cx).wave_track()?;
                let from = track.frame as f64;
                let gesture = p.player.update(cx, |player, cx| {
                    player.begin_waveform_scrub(track.seek?, from, false, cx)
                })?;
                gesture.turn_by(4., 0., 480., Instant::now());
                Some(gesture)
            })
            .await?;
        self.until(cx, "scrubbing at deep zoom", |p, _, cx| {
            let shown = p.displayed.borrow();
            (p.player.read(cx).waveform_grabbed()
                && shown.as_ref()?.viewport.span == 32.
                && p.texture.sample_hit([10., 10.]).is_none())
            .then_some(())
        })
        .await?;
        self.capture(cx, "scrub").await?;
        gesture.finish(crate::audio::scrub::End::Cancel);
        self.until(cx, "completed scrub", |p, _, cx| {
            (!p.player.read(cx).waveform_grabbed()).then_some(())
        })
        .await?;
        let previous_length = self
            .until(cx, "registered surface before resize", |p, w, _| {
                let length = p.displayed.borrow().as_ref()?.layout.time_length();
                let old = w.viewport_size();
                w.resize(size(
                    (old.width - px(520.)).max(px(600.)),
                    old.height - px(60.),
                ));
                Some(length)
            })
            .await?;
        self.until(cx, "32-frame trace after resize", |p, _, _| {
            let shown = p.displayed.borrow();
            let view = shown.as_ref()?;
            (view.layout.time_length() != previous_length
                && view.viewport.span == 32.
                && p.coverage.is_none())
            .then_some(())
        })
        .await?;
        self.capture(cx, "resized").await?;
        self.edit(cx, "125 ms").await?;
        dispatch(cx, key("enter"))?;
        self.until(cx, "time-based zoom", |p, _, cx| {
            let rate = p.player.read(cx).wave_track()?.source.rate;
            (p.displayed.borrow().as_ref()?.viewport.span == f64::from(rate) * 0.125).then_some(())
        })
        .await?;
        eprintln!("zoom-complete");
        Ok(())
    }
}

fn dispatch(cx: &mut AsyncWindowContext, event: PlatformInput) -> anyhow::Result<()> {
    cx.update(|w, cx| {
        w.dispatch_event(event, cx);
    })
}

pub(super) fn start(window: &Window, cx: &mut Context<Waveform>) {
    let Some(folder) = std::env::var_os("DISQRO_ZOOM_JOURNEY").map(PathBuf::from) else {
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
                eprintln!("zoom-failed {error:#}");
            }
        })
        .detach();
}

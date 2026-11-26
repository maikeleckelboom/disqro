//! Pointer ownership and source-space drag interaction for the detail deck.
use super::*;
use crate::audio::scrub::{End as ScrubEnd, Gesture};

/// A press on the detailed waveform: a grip on the recording itself. It
/// holds playback at once, wherever the waveform is grabbed, and moving the
/// pointer turns the recording relative to that grip. It never seeks to the
/// position under the pointer.
pub(super) struct Press {
    scale: f32,
    down: Point<Pixels>,
    frames_per_pixel: f64,
    previous: f64,
    at: Instant,
    gesture: Arc<Gesture>,
    #[cfg(windows)]
    _hover_pause: Option<gpui_windows_gpu::CaptureHoverPause>,
}

impl Press {
    fn travel(&self, position: Point<Pixels>) -> f64 {
        f64::from(f32::from(position.x - self.down.x)) * f64::from(self.scale)
    }
}

impl Drop for Press {
    fn drop(&mut self) {
        self.gesture.finish(ScrubEnd::Cancel);
    }
}

#[cfg(windows)]
fn has_capture(window: &Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetCapture;
    let Ok(RawWindowHandle::Win32(handle)) =
        HasWindowHandle::window_handle(window).map(|h| h.as_raw())
    else {
        return false;
    };
    // SAFETY: GetCapture reads the pointer-capture state of this UI thread.
    (unsafe { GetCapture() }.0 as isize) == handle.hwnd.get()
}

impl Waveform {
    pub(super) fn grabbed(&self) -> bool {
        self.press
            .as_ref()
            .is_some_and(|press| !press.gesture.closed())
    }

    pub(super) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.presented || self.press.is_some() || self.player.read(cx).waveform_grabbed() {
            return;
        }
        {
            let shown = self.displayed.borrow();
            let Some(shown) = shown
                .as_ref()
                .filter(|shown| shown.layout.scale == window.scale_factor())
            else {
                return;
            };
            let Some(track) = self
                .player
                .read(cx)
                .wave_track()
                .filter(|track| track.seek.as_ref() == Some(&shown.anchor))
            else {
                return;
            };
            let at = Instant::now();
            let Some(position) = track.position(at) else {
                return;
            };
            if track.playing
                && !shown
                    .anchor
                    .playable
                    .contains(&(position.frame.floor() as usize))
            {
                return;
            }
            let origin = position.frame.clamp(
                shown.anchor.playable.start as f64,
                (shown.anchor.playable.end - 1) as f64,
            );
            let scale = shown.layout.scale;
            let width = f64::from(f32::from(shown.layout.bounds.size.width) * scale)
                .round()
                .max(1.);
            let (anchor, was_playing) = (shown.anchor.clone(), track.playing);
            let frames_per_pixel = shown.viewport.span / width;
            // Hold at once, anchored to the playing frame rather than the
            // one under the pointer.
            let Some(gesture) = self.player.update(cx, |player, cx| {
                player.begin_waveform_scrub(anchor, origin, was_playing, cx)
            }) else {
                return;
            };
            self.press = Some(Press {
                scale,
                down: event.position,
                frames_per_pixel,
                previous: 0.,
                at,
                gesture,
                #[cfg(windows)]
                _hover_pause: gpui_windows_gpu::pause_hover_during_capture(window),
            });
            if self.position.inspecting.is_some() {
                cx.notify();
            }
            self.motion.request(cx.weak_entity(), window);
        }
        self.focus.focus(window, cx);
        cx.stop_propagation();
        #[cfg(windows)]
        {
            let panel = cx.weak_entity();
            self.capture_watch = Some(window.spawn(cx, async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(24))
                        .await;
                    let keep = cx
                        .update(|window, cx| {
                            panel
                                .update(cx, |deck, cx| {
                                    let Some(press) = &deck.press else {
                                        return false;
                                    };
                                    if press.scale == window.scale_factor() && has_capture(window) {
                                        return true;
                                    }
                                    deck.cancel_press(window, cx);
                                    false
                                })
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            }));
        }
    }

    fn move_press(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = &mut self.press else { return };
        if !self.presented || press.scale != window.scale_factor() || press.gesture.closed() {
            self.cancel_press(window, cx);
            return;
        }
        let offset = -press.travel(position) * press.frames_per_pixel;
        if offset == press.previous {
            return;
        }
        let now = Instant::now();
        let seconds = now
            .duration_since(press.at)
            .as_secs_f64()
            .clamp(0.001, 0.05);
        let velocity = (offset - press.previous) / seconds;
        let start = crate::diagnostics::begin_stage();
        press.gesture.turn_by(offset, press.previous, velocity, now);
        let target = press
            .gesture
            .frame()
            .or_else(|| press.gesture.held())
            .unwrap_or(press.gesture.origin);
        let gesture = press.gesture.id;
        press.previous = offset;
        press.at = now;
        crate::diagnostics::stage(start, "wave_scrub_input", target.to_bits(), gesture);
        self.motion.follow_scrub(cx.entity_id(), window, cx);
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.move_press(event.position, window, cx);
        let Some(press) = self.press.take() else {
            return;
        };
        self.capture_watch = None;
        // Releasing resumes playback when it was playing and leaves it paused
        // otherwise. A press that never moved continues where it held.
        press.gesture.finish(ScrubEnd::Release);
        self.position.inspecting = None;
        cx.notify();
    }

    pub(super) fn cancel_press(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.capture_watch = None;
        let canceled = self.press.take().is_some();
        if canceled && self.presented {
            self.motion.request(cx.weak_entity(), window);
            cx.notify();
        }
        canceled
    }

    pub(super) fn scrub_ended(&self) -> bool {
        self.press
            .as_ref()
            .is_some_and(|press| press.gesture.closed())
    }

    pub(super) fn listen_for_scrub(panel: &WeakEntity<Waveform>, window: &mut Window) {
        let move_panel = panel.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let _ = move_panel.update(cx, |deck, cx| {
                if deck.press.is_none() {
                    return;
                }
                if event.pressed_button == Some(MouseButton::Left) {
                    deck.move_press(event.position, window, cx);
                    cx.stop_propagation();
                } else {
                    deck.cancel_press(window, cx);
                }
            });
        });
        let up_panel = panel.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            let _ = up_panel.update(cx, |deck, cx| {
                if deck.press.is_some() {
                    deck.mouse_up(event, window, cx);
                    cx.stop_propagation();
                }
            });
        });
    }
}

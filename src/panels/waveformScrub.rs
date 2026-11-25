//! Pointer ownership and source-space drag interaction for the detail deck.
use super::*;
use crate::audio::scrub::{End as ScrubEnd, Gesture};

const DRAG_THRESHOLD_PX: f64 = 4.;

pub(super) struct Press {
    anchor: WaveSeekAnchor,
    origin: f64,
    was_playing: bool,
    scale: f32,
    down: Point<Pixels>,
    centre: f64,
    frames_per_pixel: f64,
    click: Option<usize>,
    previous: f64,
    at: Instant,
    gesture: Option<Arc<Gesture>>,
    #[cfg(windows)]
    _hover_pause: Option<gpui_windows_gpu::CaptureHoverPause>,
}

impl Press {
    fn delta(&self, position: Pixels, origin: Pixels) -> f64 {
        f64::from(f32::from(position - origin)) * f64::from(self.scale)
    }

    fn travel(&self, position: Point<Pixels>) -> f64 {
        self.delta(position.x, self.down.x)
    }
}

impl Drop for Press {
    fn drop(&mut self) {
        if let Some(gesture) = &self.gesture {
            gesture.finish(ScrubEnd::Cancel);
        }
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
            .and_then(|press| press.gesture.as_ref())
            .is_some_and(|gesture| !gesture.closed())
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
            let centre = shown.viewport.frame_at(0.5);
            self.press = Some(Press {
                anchor: shown.anchor.clone(),
                origin,
                was_playing: track.playing,
                scale,
                down: event.position,
                centre,
                frames_per_pixel: shown.viewport.span / width,
                click: shown.seek(event.position.x).map(|(_, frame)| frame),
                previous: centre,
                at,
                gesture: None,
                #[cfg(windows)]
                _hover_pause: gpui_windows_gpu::pause_hover_during_capture(window),
            });
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
        if !self.presented
            || press.scale != window.scale_factor()
            || press
                .gesture
                .as_ref()
                .is_some_and(|gesture| gesture.closed())
        {
            self.cancel_press(window, cx);
            return;
        }
        let travel = press.travel(position);
        if travel.hypot(press.delta(position.y, press.down.y)) >= DRAG_THRESHOLD_PX {
            press.click = None;
        }
        let starting = press.gesture.is_none();
        if starting {
            if travel.abs() < DRAG_THRESHOLD_PX {
                return;
            }
            press.gesture = self.player.update(cx, |player, cx| {
                player.begin_waveform_scrub(
                    press.anchor.clone(),
                    press.origin,
                    press.was_playing,
                    cx,
                )
            });
            if press.gesture.is_none() {
                self.cancel_press(window, cx);
                return;
            }
            if self.position.inspecting.is_some() {
                cx.notify();
            }
        }
        let gesture = press.gesture.as_ref().unwrap();
        let clamp =
            |frame: f64| frame.clamp(gesture.bounds.start as f64, (gesture.bounds.end - 1) as f64);
        let target = clamp(press.centre - travel * press.frames_per_pixel);
        let previous = clamp(press.previous);
        if target == previous && !starting {
            return;
        }
        let now = Instant::now();
        let seconds = now
            .duration_since(press.at)
            .as_secs_f64()
            .clamp(0.001, 0.05);
        let velocity = (target - previous) / seconds;
        let start = crate::diagnostics::begin_stage();
        gesture.move_to(target, previous, velocity, now);
        press.previous = target;
        press.at = now;
        crate::diagnostics::stage(start, "wave_scrub_input", target.to_bits(), gesture.id);
        self.motion.follow_scrub(cx.entity_id(), window, cx);
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.move_press(event.position, window, cx);
        let Some(press) = self.press.take() else {
            return;
        };
        self.capture_watch = None;
        if let Some(gesture) = &press.gesture {
            gesture.finish(ScrubEnd::Release);
            self.position.inspecting = None;
            cx.notify();
        } else if let Some(frame) = press.click
            && self.player.update(cx, |player, cx| {
                player.seek_wave_track(press.anchor.clone(), frame, cx)
            })
        {
            self.position.inspecting = None;
            cx.notify();
        }
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
            .and_then(|press| press.gesture.as_ref())
            .is_some_and(|gesture| gesture.closed())
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

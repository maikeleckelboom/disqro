use super::*;
use crate::audio::scrub::{End as ScrubEnd, Gesture};
use crate::audio::{PlaybackIdentity, Presentation, WaveSeekAnchor};
use crate::theme::Materials;
use crate::{
    overview::{self, Surface, Texture, Viewport},
    timeline::{Overview, Source},
    waveform_render::{Page, Role as WaveRole},
};
use gpui_kit::component::Selectable;
use std::collections::BTreeMap;
use std::time::Instant;

const OUTSIDE_QUEUE: &str = "This recording is outside the current queue sequence.";
const UNAVAILABLE_POSITION: &str = "This waveform position is unavailable.";

#[derive(Clone, Copy)]
pub(super) struct StripSeek {
    identity: PlaybackIdentity,
    disc: bool,
    bounds: Bounds<Pixels>,
    preview: Option<f64>,
}

impl StripSeek {
    fn applies_to(self, player: &Player) -> bool {
        player.status.playback_identity() == Some(self.identity)
            && player.disc_view() == self.disc
            && player.seek_bounds.get() == self.bounds
    }

    fn fraction(self, x: Pixels) -> f64 {
        let width = f64::from(f32::from(self.bounds.size.width)).max(1.);
        let offset = f64::from(f32::from(x)) - f64::from(f32::from(self.bounds.origin.x));
        (offset / width).clamp(0., 1.)
    }
}

pub(crate) struct WaveTrack {
    pub generation: u64,
    pub source: Arc<Source>,
    pub range: std::ops::Range<usize>,
    pub overview: Arc<Page>,
    pub identity: PlaybackIdentity,
    pub seek: Option<WaveSeekAnchor>,
    pub frame: usize,
    presentation: Option<Presentation>,
    pub playing: bool,
    pub scrub_frame: Option<f64>,
}

impl WaveTrack {
    pub fn position(&self, at: Instant) -> Option<crate::audio::RendererPosition> {
        self.presentation.as_ref().map_or(
            Some(crate::audio::RendererPosition {
                frame: self.frame as f64,
                advancing: false,
                observation_age_ns: 0,
            }),
            |clock| clock.position(at),
        )
    }
}

impl Player {
    pub(crate) fn wave_view(&self) -> crate::waveform_detail::View {
        self.analysis.view()
    }

    pub(crate) fn wave_track(&self) -> Option<WaveTrack> {
        let index = self.segment?;
        let program = self.program.as_ref()?;
        let segment = program.segments.get(index)?;
        let identity = self.status.playback_identity()?;
        let seek =
            (segment.track.disc.is_none() || program.sheet.is_some()).then(|| WaveSeekAnchor {
                identity,
                source: segment.source.clone(),
                playable: segment.frames.clone(),
                sheet: program.sheet.clone(),
            });
        Some(WaveTrack {
            generation: self.generation,
            source: segment.source.clone(),
            range: segment.frames.clone(),
            overview: self.shapes.get(index)?.track.clone(),
            identity,
            seek,
            frame: segment.elapsed_frame(self.status.position),
            presentation: self.status.presentation.clone(),
            playing: self.status.playing,
            scrub_frame: self
                .waveform_scrub
                .as_ref()
                .filter(|gesture| !gesture.closed() && gesture.anchor.identity == identity)
                .and_then(|gesture| gesture.frame()),
        })
    }

    pub(crate) fn waveform_grabbed(&self) -> bool {
        self.waveform_scrub
            .as_ref()
            .is_some_and(|gesture| !gesture.closed())
    }

    pub(crate) fn begin_waveform_scrub(
        &mut self,
        anchor: WaveSeekAnchor,
        origin: f64,
        was_playing: bool,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Gesture>> {
        if self.waveform_grabbed()
            || self.status.playing != was_playing
            || self.wave_track().and_then(|track| track.seek).as_ref() != Some(&anchor)
        {
            return None;
        }
        let (_, bounds) = crate::audio::scrub_run(&self.queue, &anchor.source)?;
        let gesture = Arc::new(Gesture::new(
            self.transport.sender().scrub_control,
            anchor,
            origin,
            was_playing,
            bounds,
        ));
        match self
            .transport
            .send(Command::BeginWaveScrub(gesture.clone()), self.status.entry)
        {
            Ok(()) => self.waveform_scrub = Some(gesture.clone()),
            Err(error) => {
                cx.emit(PlayerEvent::Notice(error.to_string()));
                return None;
            }
        }
        Some(gesture)
    }

    pub(crate) fn seek_wave_track(
        &mut self,
        anchor: WaveSeekAnchor,
        frame: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.wave_track().and_then(|track| track.seek).as_ref() != Some(&anchor)
            || !anchor.playable.contains(&frame)
        {
            return false;
        }
        self.command(Command::SeekWave { anchor, frame }, cx);
        true
    }
    pub(super) fn request_wave(&mut self) {
        if let Some(gesture) = self.waveform_scrub.take() {
            gesture.finish(ScrubEnd::Cancel);
        }
        self.program = None;
        self.segment = None;
        self.shapes.clear();
        self.unavailable = None;
        self.scrub = None;
        self.generation = 0;
        self.view_trace
            .request
            .set(crate::diagnostics::get().map(|_| std::time::Instant::now()));
        self.view_trace.first_paint.set(None);
        if let Some(entry) = self.playing_entry() {
            self.generation = self
                .analysis
                .request(entry.track.clone(), entry.disc_context);
        } else {
            self.analysis.cancel();
        }
    }
    pub(super) fn wave_update(&mut self, generation: u64, event: Update, cx: &mut Context<Self>) {
        if self.generation != generation {
            return;
        }
        let structural = matches!(
            event,
            Update::Program(_) | Update::Changed | Update::Failed(_)
        );
        match event {
            Update::Program(p) => {
                self.segment = self.status.track.as_ref().and_then(|t| p.index(t));
                if let Some(i) = self.segment {
                    if self
                        .status
                        .source_revision
                        .as_ref()
                        .is_some_and(|r| r != &p.segments[i].source.stamp)
                        || self
                            .status
                            .source_object
                            .is_some_and(|r| Some(r) != p.segments[i].source.object)
                    {
                        self.segment = None;
                        self.generation = 0;
                        self.analysis.cancel();
                        self.unavailable = Some("Recording changed since playback began".into());
                        cx.emit(PlayerEvent::WaveChanged);
                        cx.notify();
                        return;
                    }
                    self.analysis.focus(i);
                }
                self.shapes = vec![Shape::default(); p.segments.len()];
                self.program = Some(p);
                self.update_run();
            }
            Update::Wave(i, shape) => {
                if let Some(slot) = self.shapes.get_mut(i) {
                    *slot = shape;
                    if let Some(d) = crate::diagnostics::get() {
                        if self.segment == Some(i)
                            && let Some(start) = self.view_trace.request.take()
                        {
                            d.first_data
                                .fetch_max(start.elapsed().as_micros() as u64, Ordering::Relaxed);
                            self.view_trace.first_paint.set(Some(start));
                        }
                        let bytes = self.shapes.iter().map(|s| s.track.bytes()).sum::<usize>();
                        d.geometry_bytes.fetch_max(bytes as u64, Ordering::Relaxed);
                    }
                }
            }
            Update::Failed(e) => self.unavailable = Some(e),
            Update::Changed => self.request_wave(),
            Update::Detail => {
                if self.visible {
                    cx.emit(PlayerEvent::WaveEvidence);
                }
                return;
            }
        }
        if self.visible {
            cx.emit(if structural {
                PlayerEvent::WaveChanged
            } else {
                PlayerEvent::WaveEvidence
            });
            cx.notify()
        }
    }
    pub(super) fn update_run(&mut self) {
        self.run = 0..0;
        let (Some(p), Some(i), Some(q)) = (&self.program, self.segment, self.queue.current) else {
            return;
        };
        let matches = |n: usize, k: usize| {
            self.queue.entries.get(k).is_some_and(|e| {
                e.track.source == p.segments[n].track.source
                    && e.track.disc == p.segments[n].track.disc
            })
        };
        if !matches(i, q) {
            return;
        }
        let mut left = i;
        while left > 0 && q > i - left && matches(left - 1, q - (i - left) - 1) {
            left -= 1;
        }
        let mut right = i + 1;
        while right < p.segments.len() && matches(right, q + (right - i)) {
            right += 1;
        }
        self.run = left..right;
    }
    pub(super) fn overview(&self) -> Option<Overview> {
        self.status
            .track
            .as_ref()
            .and_then(|t| Overview::for_track(t, self.program.as_deref()))
    }
    fn full_cd_available(&self) -> bool {
        self.overview().is_some_and(|overview| overview.enabled)
    }
    fn disc_view(&self) -> bool {
        self.full_cd && self.segment.is_some() && self.full_cd_available()
    }
    pub(crate) fn set_perspective(&mut self, full_cd: bool, cx: &mut Context<Self>) {
        if self.full_cd == full_cd || !self.full_cd_available() {
            return;
        }
        let _timer = crate::diagnostics::Timer::measure(|d| &d.switch_us);
        if let Some(d) = crate::diagnostics::get() {
            self.view_trace
                .switch
                .set(Some((std::time::Instant::now(), full_cd)));
            d.entry
                .store(self.status.entry.unwrap_or(0), Ordering::Relaxed);
            d.chapter_ms
                .store((self.status.position * 1000.) as u64, Ordering::Relaxed);
            if let Some((p, i)) = self.program.as_ref().zip(self.segment) {
                let seg = &p.segments[i];
                d.disc_ms.store(
                    seg.at(seg.elapsed_frame(self.status.position)) / 1_000_000,
                    Ordering::Relaxed,
                );
            }
            d.changes.fetch_add(1, Ordering::Relaxed);
        }
        self.full_cd = full_cd;
        self.scrub = None;
        cx.notify();
    }
    pub(super) fn wave_time(&self) -> (f64, f64) {
        let Some((p, (i, frame))) = self.program.as_ref().zip(self.visible_wave_segment()) else {
            return (self.status.position, self.status.duration);
        };
        let s = &p.segments[i];
        let ns = s.at(frame.unwrap_or_else(|| s.elapsed_frame(self.status.position)));
        if self.disc_view() {
            (ns as f64 / SECOND as f64, p.duration as f64 / SECOND as f64)
        } else {
            (
                (ns - s.time.start) as f64 / SECOND as f64,
                (s.time.end - s.time.start) as f64 / SECOND as f64,
            )
        }
    }
    pub(super) fn scrub_fraction(&self) -> Option<f64> {
        self.scrub.and_then(|seek| seek.preview)
    }

    fn visible_wave_segment(&self) -> Option<(usize, Option<usize>)> {
        let active = self.segment?;
        if let Some(position) = self.status.scrub {
            let program = self.program.as_ref()?;
            let source = &program.segments[active].source;
            let frame = position.frame.floor() as usize;
            if let Some(index) = program
                .segments
                .iter()
                .position(|segment| segment.source == *source && segment.frames.contains(&frame))
            {
                return Some((index, Some(frame)));
            }
        }
        Some((active, None))
    }

    fn target_in(&self, fraction: f64, disc: bool) -> Result<Command, &'static str> {
        let Some((p, active)) = self.program.as_ref().zip(self.segment) else {
            return Ok(Command::Seek(fraction));
        };
        let seg = &p.segments[active];
        let range = if disc {
            0..p.duration
        } else {
            seg.time.clone()
        };
        let length = range.end - range.start;
        // Keep the click in f64 and round once to timeline nanoseconds. A
        // boundary pixel belongs to the chapter which starts there.
        let time = range.start + ((fraction * length as f64).round() as u64).min(length - 1);
        let (i, frame) = p.locate(time).ok_or(UNAVAILABLE_POSITION)?;
        self.target_frame(p, active, i, frame)
    }

    fn target_frame(
        &self,
        p: &Program,
        active: usize,
        i: usize,
        frame: usize,
    ) -> Result<Command, &'static str> {
        if !self.run.contains(&i) {
            return Err(OUTSIDE_QUEUE);
        }
        let seg = &p.segments[i];
        if !seg.source.available {
            return Err("This recording is unavailable.");
        }
        let target = self
            .queue
            .current
            .and_then(|current| current.checked_add(i)?.checked_sub(active))
            .and_then(|index| self.queue.entries.get(index))
            .ok_or(OUTSIDE_QUEUE)?;
        Ok(Command::SeekOnDisc {
            target: target.id,
            source_revision: seg.source.stamp.clone(),
            sheet_revision: p.sheet.as_ref().map(|(_, s)| s.stamp.clone()),
            revision: self.status.queue_revision,
            seconds: (frame - seg.frames.start) as f64 / f64::from(seg.source.rate),
            duration: seg.frames.len() as f64 / f64::from(seg.source.rate),
        })
    }
    fn move_scrub(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(mut seek) = self.scrub.take() else {
            return;
        };
        if seek.applies_to(self) {
            let fraction = seek.fraction(x);
            seek.preview = self
                .target_in(fraction, seek.disc)
                .is_ok()
                .then_some(fraction);
            self.scrub = Some(seek);
        }
        cx.notify();
    }

    fn end_scrub(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(seek) = self.scrub.take() else {
            return;
        };
        if seek.applies_to(self) {
            match self.target_in(seek.fraction(x), seek.disc) {
                Ok(command) => self.command(command, cx),
                Err(reason) => {
                    crate::diagnostics::stage(
                        crate::diagnostics::begin_stage(),
                        "wave_compact_blocked",
                        u64::from(reason == OUTSIDE_QUEUE),
                        0,
                    );
                    cx.emit(PlayerEvent::Notice(reason.into()));
                }
            }
        }
        cx.notify();
    }
    /// Tabs on the timeline lane's upper edge name what the lane shows. The
    /// selected perspective shares the lane's surface and opens into it. A
    /// recording without a CD context offers only Track.
    pub(super) fn perspectives(
        &self,
        overview: Option<Overview>,
        m: &Materials,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let reason = self
            .unavailable
            .as_deref()
            .or_else(|| self.program.as_ref()?.limitation.as_deref());
        let mut row = h_flex()
            .id("wave-perspectives")
            .role(Role::TabList)
            .aria_label("Waveform perspective")
            .h(px(PERSPECTIVE_HEIGHT))
            .items_end();
        let choices = match &overview {
            Some(overview) => vec![
                ("wave-track", "Track", false),
                ("wave-disc", overview.label, true),
            ],
            None => vec![("wave-track", "Track", false)],
        };
        for (id, label, full) in choices {
            let enabled = !full || overview.as_ref().is_some_and(|o| o.enabled);
            let selected = self.disc_view() == full;
            let tip = match &overview {
                Some(o) if full && !enabled => o.unavailable_tip(reason, self.program.is_none()),
                Some(_) => format!("{label} waveform · Alt+W"),
                None => "The waveform of this recording".into(),
            };
            row = row.child(
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .h_full()
                    .px(px(10.))
                    .rounded(px(0.))
                    .rounded_t(px(5.))
                    .label(label)
                    .text_size(px(crate::theme::size::TEXT_SMALL))
                    .text_color(if selected { m.text } else { m.text_faint })
                    .selected(selected)
                    .when(selected, |b| b.bg(m.canvas))
                    .disabled(!enabled)
                    .tooltip(tip)
                    .on_click(cx.listener(move |p, _, _, cx| p.set_perspective(full, cx))),
            );
        }
        row
    }
    pub(super) fn waveform(
        &self,
        progress: f32,
        active: bool,
        m: &Materials,
        cx: &Context<Self>,
        images: &mut BTreeMap<usize, Texture>,
    ) -> impl IntoElement {
        let bounds = self.seek_bounds.clone();
        let trace = self.view_trace.clone();
        let phase = if self.disc_view() {
            "wave_compact_disc"
        } else {
            "wave_compact_track"
        };
        let entry = self.status.entry.unwrap_or(0);
        let queue_entry = self.queue.current_entry().map_or(0, |entry| entry.id);
        let queue_len = self.queue.entries.len() as u64;
        let queue_state = if self.status.playing {
            "wave_compact_playing"
        } else {
            "wave_compact_paused"
        };
        let settled = self.scrub_fraction().is_none();
        let mut wave = div()
            .id("seek")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        trace.painted();
                        if settled && let Some(at) = crate::diagnostics::begin_stage() {
                            let scale = window.scale_factor();
                            let left = f64::from(f32::from(b.origin.x) * scale);
                            let width = f64::from(f32::from(b.size.width) * scale);
                            crate::diagnostics::stage(
                                Some(at),
                                "wave_compact_bounds",
                                left.to_bits(),
                                width.to_bits(),
                            );
                            crate::diagnostics::stage(
                                Some(at),
                                phase,
                                f64::from(progress).to_bits(),
                                entry,
                            );
                            crate::diagnostics::stage(
                                Some(at),
                                queue_state,
                                queue_entry,
                                queue_len,
                            );
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        if let Some((p, (k, _))) = self.program.as_ref().zip(self.visible_wave_segment()) {
            let full = self.disc_view();
            let indices = if full { 0..p.segments.len() } else { k..k + 1 };
            for i in indices {
                let s = &p.segments[i];
                let frac = |n: u64| (n as f64 / p.duration as f64) as f32;
                let (left, width) = if full {
                    (frac(s.time.start), frac(s.time.end - s.time.start))
                } else {
                    (0., 1.)
                };
                let page = &self.shapes[i].track;
                let can_seek = s.source.available && self.run.contains(&i);
                let mut part = div()
                    .id(("wave-segment", i))
                    .absolute()
                    .h_full()
                    .left(relative(left))
                    .w(relative(width))
                    .when(full && i == k, |d| d.bg(m.selected.opacity(0.65)))
                    .when(can_seek, |d| d.cursor_pointer());
                if page.frame_count > 0 {
                    let surface = Surface {
                        role: WaveRole::Overview,
                        rate: s.source.rate,
                        playable: s.frames.clone(),
                        viewport: Viewport::fit(s.frames.len()),
                    };
                    part = part.child(overview::paint(
                        overview::Evidence {
                            overview: Some(page.clone()),
                            ..Default::default()
                        },
                        surface,
                        images.entry(i).or_default().clone(),
                    ));
                    if full && i == k {
                        part = part.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(1.))
                                .bg(m.selected),
                        );
                    }
                }
                if full {
                    let mut label = s.track.title.clone();
                    if let Some(n) = s.track.display_number() {
                        label = format!("{n:02} · {label}");
                    }
                    if !s.source.available {
                        label.push_str(" · Recording unavailable");
                    } else if !can_seek {
                        label.push_str(" · Outside the current queue sequence");
                    }
                    part = part.tooltip(move |w, cx| Tooltip::new(label.clone()).build(w, cx));
                    if i > 0 {
                        part = part.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(1.))
                                .bg(m.text_faint.opacity(0.6)),
                        );
                    }
                }
                wave = wave.child(part);
            }
        }
        wave.when(active, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(progress))
                    .w(px(2.))
                    .ml(px(-1.))
                    .bg(m.wave.cursor),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|p, e: &MouseDownEvent, _, cx| {
                    if let Some(identity) = p.status.playback_identity() {
                        p.scrub = Some(StripSeek {
                            identity,
                            disc: p.disc_view(),
                            bounds: p.seek_bounds.get(),
                            preview: None,
                        });
                        p.move_scrub(e.position.x, cx);
                    }
                }),
            )
            .on_drag(Scrub, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| Scrub)
            })
            .on_drag_move(cx.listener(|p, e: &DragMoveEvent<Scrub>, _, cx| {
                p.move_scrub(e.event.position.x, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|p, e: &MouseUpEvent, _, cx| p.end_scrub(e.position.x, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|p, e: &MouseUpEvent, _, cx| p.end_scrub(e.position.x, cx)),
            )
        })
    }
}

/// Reuse the waveform scene on meter-only redraws. Player notifications still
/// invalidate it for transport, scrubbing, perspective and analysis updates.
pub(super) struct WaveStrip {
    player: WeakEntity<Player>,
    images: BTreeMap<usize, Texture>,
    generation: u64,
}

impl WaveStrip {
    pub(super) fn new(player: Entity<Player>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player: player.downgrade(),
            images: BTreeMap::new(),
            generation: 0,
        }
    }
}

impl Render for WaveStrip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.player
            .clone()
            .update(cx, |p, cx| {
                if self.generation != p.generation {
                    self.images.clear();
                    self.generation = p.generation;
                }
                let (position, duration) = p.wave_time();
                let progress = p.scrub_fraction().unwrap_or(if duration > 0. {
                    (position / duration).clamp(0., 1.)
                } else {
                    0.
                }) as f32;
                p.waveform(
                    progress,
                    p.status.track.is_some(),
                    materials(cx),
                    cx,
                    &mut self.images,
                )
                .into_any_element()
            })
            .unwrap_or_else(|_| Empty.into_any_element())
    }
}

use super::*;
use crate::theme::Materials;
use crate::timeline::Overview;
impl Player {
    pub(super) fn request_wave(&mut self) {
        self.program = None;
        self.segment = None;
        self.shapes.clear();
        self.unavailable = None;
        self.scrub = None;
        self.scrub_entry = None;
        self.generation = 0;
        self.view_trace
            .request
            .set(crate::diagnostics::get().map(|_| std::time::Instant::now()));
        self.view_trace.first_paint.set(None);
        if let Some(entry) = self.playing_entry() {
            self.generation = self.analysis.request(
                entry.track.clone(),
                entry.disc_context,
                self.status.position,
            );
        } else {
            self.analysis.cancel();
        }
    }
    pub(super) fn wave_update(&mut self, generation: u64, event: Update, cx: &mut Context<Self>) {
        if self.generation != generation {
            return;
        }
        match event {
            Update::Program(p) => {
                self.segment = self.status.track.as_ref().and_then(|t| p.index(t));
                if let Some(i) = self.segment {
                    if self
                        .status
                        .source_revision
                        .as_ref()
                        .is_some_and(|r| r != &p.segments[i].source.stamp)
                    {
                        self.segment = None;
                        self.generation = 0;
                        self.analysis.cancel();
                        self.unavailable = Some("Recording changed since playback began".into());
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
                        let bytes = self
                            .shapes
                            .iter()
                            .map(|s| {
                                std::mem::size_of_val(&*s.track) + std::mem::size_of_val(&*s.disc)
                            })
                            .sum::<usize>();
                        d.geometry_bytes.fetch_max(bytes as u64, Ordering::Relaxed);
                    }
                }
            }
            Update::Failed(e) => self.unavailable = Some(e),
            Update::Changed => self.request_wave(),
        }
        if self.visible {
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
        self.scrub_entry = None;
        cx.notify();
    }
    pub(super) fn wave_time(&self) -> (f64, f64) {
        let Some((p, i)) = self.program.as_ref().zip(self.segment) else {
            return (self.status.position, self.status.duration);
        };
        let s = &p.segments[i];
        let ns = s.at(s.elapsed_frame(self.status.position));
        if self.disc_view() {
            (ns as f64 / SECOND as f64, p.duration as f64 / SECOND as f64)
        } else {
            (
                (ns - s.time.start) as f64 / SECOND as f64,
                (s.time.end - s.time.start) as f64 / SECOND as f64,
            )
        }
    }
    fn target(&self, f: f32) -> Option<Command> {
        let Some((p, active)) = self.program.as_ref().zip(self.segment) else {
            return Some(Command::Seek(f64::from(f)));
        };
        let seg = &p.segments[active];
        let range = if self.disc_view() {
            0..p.duration
        } else {
            seg.time.clone()
        };
        let time = (range.start + (f64::from(f) * (range.end - range.start) as f64) as u64)
            .min(range.end - 1);
        let (i, frame) = p.locate(time)?;
        if !self.run.contains(&i) {
            return None;
        }
        let seg = &p.segments[i];
        if !seg.source.available {
            return None;
        }
        let q = self.queue.current?.checked_add(i)?.checked_sub(active)?;
        let target = self.queue.entries.get(q)?.id;
        Some(Command::SeekOnDisc {
            target,
            source_revision: seg.source.stamp.clone(),
            sheet_revision: p.sheet.as_ref().map(|(_, s)| s.clone()),
            revision: self.status.queue_revision,
            seconds: (frame - seg.frames.start) as f64 / f64::from(seg.source.rate),
            duration: seg.frames.len() as f64 / f64::from(seg.source.rate),
        })
    }
    fn end_scrub(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.scrub.take()
            && let Some((entry, revision)) = self.scrub_entry.take()
            && Some(entry) == self.status.entry
            && self.status.queue_revision == revision
            && let Some(c) = self.target(f)
        {
            self.command(c, cx);
        }
        self.scrub_entry = None;
        cx.notify();
    }
    pub(super) fn perspectives(
        &self,
        overview: Overview,
        m: &Materials,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let enabled = overview.enabled;
        let reason = self
            .unavailable
            .as_deref()
            .or_else(|| self.program.as_ref()?.limitation.as_deref());
        let mut row = h_flex().items_center().rounded(px(4.)).bg(m.control);
        for (id, label, full) in [
            ("wave-track", "Track", false),
            ("wave-disc", overview.label, true),
        ] {
            let selected = self.disc_view() == full;
            let tip = if full && !enabled {
                overview.unavailable_tip(reason, self.program.is_none())
            } else {
                format!("{label} waveform · Alt+W")
            };
            row = row.child(
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .label(label)
                    .text_color(if selected { m.text } else { m.text_muted })
                    .when(selected, |b| b.bg(m.control_hover))
                    .disabled(full && !enabled)
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
    ) -> impl IntoElement {
        let bounds = self.seek_bounds.clone();
        let trace = self.view_trace.clone();
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
                    move |_, _, _, _| trace.painted(),
                )
                .absolute()
                .size_full(),
            );
        if let Some((p, k)) = self.program.as_ref().zip(self.segment) {
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
                let bins = if full {
                    &self.shapes[i].disc
                } else {
                    &self.shapes[i].track
                };
                let can_seek = s.source.available && self.run.contains(&i);
                let mut part = div()
                    .id(("wave-segment", i))
                    .absolute()
                    .h_full()
                    .left(relative(left))
                    .w(relative(width))
                    .when(full && i == k, |d| d.bg(m.selected.opacity(0.65)))
                    .when(can_seek, |d| d.cursor_pointer());
                if !bins.is_empty() {
                    part = part.child(crate::overview::paint(
                        bins.clone(),
                        (progress - left) / width,
                        m.wave,
                    ));
                }
                if full {
                    let mut label = s.track.title.clone();
                    if let Some(n) = s.track.display_number() {
                        label = format!("{n:02} · {label}");
                    }
                    if !s.source.available {
                        label.push_str(" · Recording unavailable");
                    } else if !can_seek {
                        label.push_str(" · Outside this queue run");
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
                    let f = p.fraction_at(e.position.x, p.seek_bounds.get());
                    if p.target(f).is_some() {
                        p.scrub_entry = p.status.entry.map(|id| (id, p.status.queue_revision));
                        p.scrub = Some(f);
                        cx.notify();
                    }
                }),
            )
            .on_drag(Scrub, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| Scrub)
            })
            .on_drag_move(cx.listener(|p, e: &DragMoveEvent<Scrub>, _, cx| {
                if p.scrub_entry.is_some() {
                    let f = p.fraction_at(e.event.position.x, e.bounds);
                    p.scrub = p.target(f).is_some().then_some(f);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|p, _, _, cx| p.end_scrub(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|p, _, _, cx| p.end_scrub(cx)),
            )
        })
    }
}

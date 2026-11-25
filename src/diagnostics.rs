//! Optional one-second observations from non-realtime threads. Kira owns the
//! audio callback; its existing CPU and device-error queues are polled by the transport.
use std::{
    fs::OpenOptions,
    io::Write,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Counters {
    pub scrub_rendered: AtomicU64,
    pub scrub_render_us: AtomicU64,
    pub scrub_decoder_opens: AtomicU64,
    pub commands: AtomicU64,
    pub opens: AtomicU64,
    pub voices: AtomicU64,
    pub packets: AtomicU64,
    pub decode_us: AtomicU64,
    pub packet_gap_us: AtomicU64,
    pub callbacks: AtomicU64,
    pub cpu_permille: AtomicU64,
    pub device_errors: AtomicU64,
    pub decode_errors: AtomicU64,
    pub stalls: AtomicU64,
    pub position_error_us: AtomicU64,
    pub discarded_errors: AtomicU64,
    pub frames: AtomicU64,
    pub paint_us: AtomicU64,
    pub wave_upload_bytes: AtomicU64,
    pub wave_gpu_draws: AtomicU64,
    pub wave_gpu_errors: AtomicU64,
    pub wave_input_us: AtomicU64,
    pub wave_clock_age_us: AtomicU64,
    pub wave_invalidations: AtomicU64,
    pub ui_us: AtomicU64,
    pub changes: AtomicU64,
    pub switch_us: AtomicU64,
    pub transition_us: AtomicU64,
    pub disc_paint: AtomicU64,
    pub track_paint: AtomicU64,
    pub first_data: AtomicU64,
    pub first_paint: AtomicU64,
    pub geometry_bytes: AtomicU64,
    pub entry: AtomicU64,
    pub chapter_ms: AtomicU64,
    pub disc_ms: AtomicU64,
    pub cover_requests: AtomicU64,
    pub cover_loads: AtomicU64,
    pub cover_load_us: AtomicU64,
    pub page_hits: AtomicU64,
    pub page_misses: AtomicU64,
    pub capture_frames: AtomicU64,
    pub capture_drops: AtomicU64,
    pub fft_pairs: AtomicU64,
    pub columns: AtomicU64,
    pub history_us: AtomicU64,
    pub field_us: AtomicU64,
    pub worker_tick_us: AtomicU64,
    pub visual_latency_us: AtomicU64,
    pub browser_prepaints: AtomicU64,
    pub visual_updates: AtomicU64,
    pub visual_paints: AtomicU64,
    pub visual_paint_us: AtomicU64,
    pub texture_bytes: AtomicU64,
    pub fft_us: AtomicU64,
    pub spectrum_updates: AtomicU64,
    pub spectrum_paints: AtomicU64,
    pub spectrum_paint_us: AtomicU64,
    pub publish_to_paint_us: AtomicU64,
}
static COUNTERS: OnceLock<Counters> = OnceLock::new();
pub fn get() -> Option<&'static Counters> {
    COUNTERS.get()
}

pub fn begin_stage() -> Option<i64> {
    #[cfg(all(windows, feature = "qualification"))]
    return gpui_windows_gpu::frame_statistics::start();
    #[cfg(not(all(windows, feature = "qualification")))]
    None
}

pub fn stage(start: Option<i64>, label: &'static str, value: u64, detail: u64) {
    #[cfg(all(windows, feature = "qualification"))]
    gpui_windows_gpu::frame_statistics::stage(start, label, value, detail);
    #[cfg(not(all(windows, feature = "qualification")))]
    let _ = (start, label, value, detail);
}
pub fn start() {
    let Some(path) = std::env::var_os("DISQRO_PLAYBACK_TRACE") else {
        return;
    };
    let Ok(mut out) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return;
    };
    let d = COUNTERS.get_or_init(Default::default);
    let _ = std::thread::Builder::new()
        .name("disqro-trace".into())
        .spawn(move || {
            let start = Instant::now();
            let columns = [
                ("commands", &d.commands),
                ("opens", &d.opens),
                ("voices", &d.voices),
                ("packets", &d.packets),
                ("decode_max_us", &d.decode_us),
                ("packet_gap_max_us", &d.packet_gap_us),
                ("callbacks", &d.callbacks),
                ("callback_max_permille", &d.cpu_permille),
                ("device_errors", &d.device_errors),
                ("decode_errors", &d.decode_errors),
                ("unchanged_ticks", &d.stalls),
                ("wave_paints", &d.frames),
                ("wave_paint_max_us", &d.paint_us),
                ("wave_source_upload_bytes", &d.wave_upload_bytes),
                ("wave_gpu_draws", &d.wave_gpu_draws),
                ("wave_gpu_errors", &d.wave_gpu_errors),
                ("wave_input_to_draw_max_us", &d.wave_input_us),
                ("scrub_rendered_movements", &d.scrub_rendered),
                ("scrub_pointer_to_audio_max_us", &d.scrub_render_us),
                ("scrub_decoder_opens", &d.scrub_decoder_opens),
                ("wave_clock_observation_age_max_us", &d.wave_clock_age_us),
                ("wave_entity_invalidations", &d.wave_invalidations),
                ("player_render_max_us", &d.ui_us),
                ("switches", &d.changes),
                ("switch_max_us", &d.switch_us),
                ("chapter_sync_max_us", &d.transition_us),
                ("cover_requests", &d.cover_requests),
                ("cover_loads", &d.cover_loads),
                ("cover_load_max_us", &d.cover_load_us),
                ("page_hits", &d.page_hits),
                ("page_misses", &d.page_misses),
                ("disc_switch_to_paint_us", &d.disc_paint),
                ("track_switch_to_paint_us", &d.track_paint),
                ("first_data_us", &d.first_data),
                ("first_wave_to_paint_us", &d.first_paint),
                ("retained_geometry_bytes", &d.geometry_bytes),
                ("last_entry", &d.entry),
                ("last_chapter_ms", &d.chapter_ms),
                ("last_disc_ms", &d.disc_ms),
                ("position_delta_error_max_us", &d.position_error_us),
                ("discarded_device_errors", &d.discarded_errors),
                ("capture_frames", &d.capture_frames),
                ("capture_drops", &d.capture_drops),
                ("fft_pairs", &d.fft_pairs),
                ("stft_columns", &d.columns),
                ("history_max_us", &d.history_us),
                ("field_block_max_us", &d.field_us),
                ("worker_tick_max_us", &d.worker_tick_us),
                ("visual_to_paint_max_us", &d.visual_latency_us),
                ("browser_prepaints", &d.browser_prepaints),
                ("visual_updates", &d.visual_updates),
                ("visual_paints", &d.visual_paints),
                ("visual_paint_max_us", &d.visual_paint_us),
                ("texture_bytes", &d.texture_bytes),
                ("fft_max_us", &d.fft_us),
                ("spectrum_updates", &d.spectrum_updates),
                ("spectrum_paints", &d.spectrum_paints),
                ("spectrum_paint_max_us", &d.spectrum_paint_us),
                ("publish_to_paint_max_us", &d.publish_to_paint_us),
            ];
            let mut out = std::io::BufWriter::new(&mut out);
            write!(out, "ms")?;
            for (name, _) in columns {
                write!(out, ",{name}")?;
            }
            writeln!(out)?;
            for _ in 0..1800 {
                std::thread::sleep(Duration::from_secs(1));
                write!(out, "{}", start.elapsed().as_millis())?;
                for (_, value) in columns {
                    write!(out, ",{}", value.swap(0, Relaxed))?;
                }
                writeln!(out)?;
                out.flush()?;
            }
            std::io::Result::Ok(())
        });
}
pub struct Timer(Instant, &'static AtomicU64);
impl Timer {
    pub fn measure(counter: impl FnOnce(&'static Counters) -> &'static AtomicU64) -> Option<Self> {
        get().map(|d| Self(Instant::now(), counter(d)))
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        self.1
            .fetch_max(self.0.elapsed().as_micros() as u64, Relaxed);
    }
}

#[derive(Default)]
pub struct ViewTrace {
    pub request: std::cell::Cell<Option<Instant>>,
    pub first_paint: std::cell::Cell<Option<Instant>>,
    pub switch: std::cell::Cell<Option<(Instant, bool)>>,
}
impl ViewTrace {
    pub fn painted(&self) {
        if let Some(d) = get() {
            if let Some((at, disc)) = self.switch.take() {
                let counter = if disc { &d.disc_paint } else { &d.track_paint };
                counter.fetch_max(at.elapsed().as_micros() as u64, Relaxed);
            }
            if let Some(at) = self.first_paint.take() {
                d.first_paint
                    .fetch_max(at.elapsed().as_micros() as u64, Relaxed);
            }
        }
    }
}

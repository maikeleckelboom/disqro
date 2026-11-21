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
    pub frames: AtomicU64,
    pub paint_us: AtomicU64,
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
    pub page_hits: AtomicU64,
    pub page_misses: AtomicU64,
}
static COUNTERS: OnceLock<Counters> = OnceLock::new();
pub fn get() -> Option<&'static Counters> {
    COUNTERS.get()
}
pub fn start() {
    let Some(path) = std::env::var_os("DISQRO_PLAYBACK_TRACE") else {
        return;
    };
    let Ok(mut out) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return;
    };
    let d = COUNTERS.get_or_init(Default::default);
    let _ = std::thread::Builder::new().name("disqro-trace".into()).spawn(move || {
        let start = Instant::now();
        let _ = writeln!(out, "ms,commands,opens,voices,packets,decode_max_us,packet_gap_max_us,callbacks,callback_max_permille,device_errors,decode_errors,unchanged_ticks,wave_paints,wave_paint_max_us,player_render_max_us,switches,switch_max_us,chapter_sync_max_us,page_hits,page_misses,disc_switch_to_paint_us,track_switch_to_paint_us,first_data_us,first_wave_to_paint_us,retained_geometry_bytes,last_entry,last_chapter_ms,last_disc_ms");
        for _ in 0..1800 {
            std::thread::sleep(Duration::from_secs(1));
            let values = [&d.commands,&d.opens,&d.voices,&d.packets,&d.decode_us,&d.packet_gap_us,&d.callbacks,&d.cpu_permille,&d.device_errors,&d.decode_errors,&d.stalls,&d.frames,&d.paint_us,&d.ui_us,&d.changes,&d.switch_us,&d.transition_us,&d.page_hits,&d.page_misses,&d.disc_paint,&d.track_paint,&d.first_data,&d.first_paint,&d.geometry_bytes,&d.entry,&d.chapter_ms,&d.disc_ms];
            if write!(out, "{}", start.elapsed().as_millis()).is_err() { break }
            for n in values { let _ = write!(out, ",{}", n.swap(0, Relaxed)); }
            if writeln!(out).is_err() { break }
        }
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

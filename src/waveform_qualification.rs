//! Native fixture replay, presentation traces and device recovery probes.
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{App, Window};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
    sync::Arc,
};
use waveform::{
    EvidencePage, Role, View,
    evidence::{RawEvidencePage, RawLeafEvidence, RawOctaveCells, STROKE_ANALYSIS},
    presentation::SUPPORTED_OCTAVE_RATES,
};

#[derive(Deserialize)]
struct Case {
    id: String,
    pages: String,
    #[serde(flatten)]
    view: View,
}

#[derive(Deserialize)]
struct Replay {
    cases: Vec<Case>,
}

/// Local frozen inputs include 13-fact rows. This decoder is qualification-only.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredPage {
    start_frame: usize,
    frames_per_column: usize,
    frame_count: usize,
    #[serde(default)]
    preview: bool,
    facts: Vec<f32>,
    extrema: Vec<usize>,
    #[serde(default)]
    fallback_energy: Option<Vec<[f32; 3]>>,
    #[serde(default)]
    source_detail: Option<StoredLeaves>,
    #[serde(default)]
    octave: Option<StoredOctave>,
    #[serde(default)]
    spectrum: Option<Vec<f64>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredLeaves {
    first_cell_frame: usize,
    start_frame: usize,
    frame_count: usize,
    leaf_frames: usize,
    bounds: Vec<f32>,
    offsets: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredOctave {
    first_cell_frame: usize,
    analysis: Option<String>,
    powers: Vec<f64>,
}

impl StoredPage {
    fn into_page(mut self, rate: u32) -> Result<EvidencePage> {
        if !self.facts.is_empty() && self.facts.len() == self.extrema.len() / 2 * 13 {
            let historical = std::mem::take(&mut self.facts);
            let rows = historical.as_chunks::<13>().0;
            self.facts = rows
                .iter()
                .flat_map(|row| row[..7].iter().copied())
                .collect();
            if !SUPPORTED_OCTAVE_RATES.contains(&rate) {
                self.fallback_energy =
                    Some(rows.iter().map(|row| [row[10], row[11], row[12]]).collect());
            }
        }
        Ok(EvidencePage::try_from(RawEvidencePage {
            start_frame: self.start_frame,
            frames_per_column: self.frames_per_column,
            frame_count: self.frame_count,
            preview: self.preview,
            facts: self.facts,
            extrema: self.extrema,
            fallback_energy: self.fallback_energy,
            leaves: self.source_detail.map(|l| RawLeafEvidence {
                first_cell_frame: l.first_cell_frame,
                start_frame: l.start_frame,
                frame_count: l.frame_count,
                leaf_frames: l.leaf_frames,
                bounds: l.bounds,
                offsets: l.offsets,
            }),
            octave: self.octave.map(|o| RawOctaveCells {
                first_cell_frame: o.first_cell_frame,
                stroke: o.analysis.as_deref() == Some(STROKE_ANALYSIS),
                powers: o.powers,
            }),
            spectrum: self.spectrum,
        })?)
    }
}

pub fn run(mut args: impl Iterator<Item = OsString>) -> i32 {
    let Some(log) = args.next() else { return 2 };
    let paths: Vec<_> = args.collect();
    let [cases, facts, output] = paths.as_slice() else {
        return 2;
    };
    match replay(Path::new(cases), Path::new(facts), Path::new(output)) {
        Ok(()) => 0,
        Err(error) => {
            let _ = std::fs::write(log, format!("{error:#}\n"));
            1
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_reader(BufReader::new(
        File::open(path).with_context(|| format!("open {}", path.display()))?,
    ))
    .with_context(|| format!("parse {}", path.display()))
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    serde_json::to_writer(BufWriter::new(File::create(path)?), value)?;
    Ok(())
}

fn replay(cases: &Path, facts: &Path, output: &Path) -> Result<()> {
    use waveform::d3d11::{
        Frame,
        headless::{Driver, HeadlessDevice},
    };
    let cases: Replay = read_json(cases)?;
    let device = HeadlessDevice::new(Driver::Auto)?;
    let mut renderer = device.renderer(Default::default())?;
    write_json(
        &output.join("adapter.json"),
        &serde_json::json!({
            "adapter": device.adapter, "software": device.software
        }),
    )?;
    for case in cases.cases {
        let view = case.view;
        ensure!(
            (1..=8192).contains(&view.width)
                && (1..=1024).contains(&view.height)
                && view.pixels_per_second.is_finite()
                && view.pixels_per_second > 0.0,
            "Invalid waveform viewport"
        );
        let stored: Vec<StoredPage> = read_json(&facts.join(&case.pages))?;
        let pages = stored
            .into_iter()
            .map(|page| page.into_page(view.rate).map(Arc::new))
            .collect::<Result<Vec<_>>>()?;
        let frame = if view.role == Role::Overview {
            Frame {
                overview: pages.first(),
                ..Default::default()
            }
        } else {
            Frame {
                pages: &pages,
                ..Default::default()
            }
        };
        let frame = Frame {
            exact_first: view.origin() * f64::from(view.rate) / view.pixels_per_second,
            ..frame
        };
        let pixels = device.render(&mut renderer, &frame, &view)?;
        image::save_buffer(
            output.join(format!("{}.png", case.id)),
            &pixels,
            view.width as u32,
            view.height as u32,
            image::ColorType::Rgba8,
        )?;
        write_json(
            &output.join(format!("{}.json", case.id)),
            &device.geometry(&renderer, &view)?,
        )?;
        renderer.release_unused();
    }
    Ok(())
}

/// Arms the engine's one-shot detail target failure for the lifecycle
/// journey (`DISQRO_QUALIFY_TARGET_FAILURE`, muted qualification only).
#[cfg(windows)]
pub fn arm_target_failure() {
    if crate::audio::silent_qualification()
        && std::env::var_os("DISQRO_QUALIFY_TARGET_FAILURE").is_some()
    {
        waveform::gpui::fail_next_target();
    }
}

#[cfg(windows)]
pub(crate) fn exercise_device_recreation(window: &mut Window, cx: &App) {
    use std::time::Duration;
    if !crate::audio::silent_qualification() {
        return;
    }
    let Some(delay) = std::env::var("DISQRO_QUALIFY_DEVICE_RECREATION_MS")
        .ok()
        .and_then(|delay| delay.parse::<u64>().ok())
    else {
        return;
    };
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            let _ = cx.update(|window, _| gpui_windows_gpu::qualify_device_recreation(window));
        })
        .detach();
}

#[cfg(windows)]
pub fn start_presentation_trace() {
    use std::{
        fs::OpenOptions,
        io::{BufWriter, Write},
        sync::mpsc,
    };
    start_stage_trace();
    let Some(path) = std::env::var_os("DISQRO_DXGI_TRACE") else {
        return;
    };
    let Ok(file) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return;
    };
    let (send, receive) =
        mpsc::sync_channel::<gpui_windows_gpu::frame_statistics::FrameStatistics>(2048);
    gpui_windows_gpu::frame_statistics::listen(move |frame| {
        let _ = send.try_send(frame);
    });
    let _ = std::thread::Builder::new().name("disqro-dxgi-trace".into()).spawn(move || -> std::io::Result<()> {
        let mut out = BufWriter::new(file);
        writeln!(out, "qpc,foreground,queued,present_status,statistics_status,displayed,present_refresh,sync_refresh,sync_qpc,frame,window,chain")?;
        loop {
            let frame = match receive.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => { out.flush()?; continue; }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let d = frame.display;
            writeln!(out, "{},{},{},{},{},{},{},{},{},{},{},{}", frame.qpc, u8::from(frame.foreground), frame.queued,
                frame.present_status, frame.statistics_status, d.PresentCount, d.PresentRefreshCount, d.SyncRefreshCount, d.SyncQPCTime,
                frame.frame, frame.window, frame.chain)?;
            if frame.queued.is_multiple_of(32) { out.flush()?; }
        }
        out.flush()
    });
}

#[cfg(windows)]
fn start_stage_trace() {
    use gpui_kit::profiler::{FrameEvent, FrameTimingCollector, set_trace_enabled};
    use gpui_windows_gpu::frame_statistics::{FrameStage, listen_to_stages, qpc};
    use std::{
        fs::OpenOptions,
        io::{BufWriter, Write},
        sync::{
            atomic::{AtomicU64, Ordering::Relaxed},
            mpsc,
        },
        time::{Duration, Instant},
    };
    static DROPPED: AtomicU64 = AtomicU64::new(0);
    let Some(path) = std::env::var_os("DISQRO_PRESENTATION_STAGES") else {
        return;
    };
    let Ok(file) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return;
    };
    let (send, receive) = mpsc::sync_channel::<FrameStage>(8192);
    listen_to_stages(move |record| {
        if send.try_send(record).is_err() {
            DROPPED.fetch_add(1, Relaxed);
        }
    });
    let mut collector = FrameTimingCollector::new();
    set_trace_enabled(true);
    let _ = std::thread::Builder::new()
        .name("disqro-frame-trace".into())
        .spawn(move || -> std::io::Result<()> {
            let mut frequency = 0;
            // SAFETY: the output is an initialized counter-frequency slot.
            unsafe {
                windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)?;
            }
            let ticks = |time: Duration| (time.as_secs_f64() * frequency as f64).round() as i64;
            let mut out = BufWriter::new(file);
            writeln!(out, "qpc,ticks,stage,value,detail,frame,window,panel")?;
            loop {
                let limit = Instant::now() + Duration::from_millis(100);
                while let Ok(phase) =
                    receive.recv_timeout(limit.saturating_duration_since(Instant::now()))
                {
                    writeln!(
                        out,
                        "{},{},{},{},{},{},{},{}",
                        phase.qpc,
                        phase.ticks,
                        phase.stage,
                        phase.value,
                        phase.detail,
                        phase.frame,
                        phase.window,
                        phase.panel
                    )?;
                    if Instant::now() >= limit {
                        break;
                    }
                }
                let now = Instant::now();
                let counter = qpc();
                for event in collector.collect_unseen() {
                    let FrameEvent::Draw(frame) = event else {
                        continue;
                    };
                    let end = counter - ticks(now.saturating_duration_since(frame.draw_end));
                    writeln!(
                        out,
                        "{end},{},gpui_draw,{},0,0,0,0",
                        ticks(frame.draw_duration()),
                        frame.invalidations
                    )?;
                    for (stage, duration) in [
                        ("gpui_request_layout", frame.phases.request_layout),
                        ("gpui_prepaint", frame.phases.prepaint),
                        ("gpui_scene_paint", frame.phases.paint),
                    ] {
                        writeln!(out, "{end},{},{stage},0,0,0,0,0", ticks(duration))?;
                    }
                }
                let dropped = DROPPED.swap(0, Relaxed);
                if dropped > 0 {
                    writeln!(out, "{counter},0,dropped,{dropped},0,0,0,0")?;
                }
                out.flush()?;
            }
        });
}

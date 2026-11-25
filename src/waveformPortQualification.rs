//! Local-only command entry points for the external waveform parity runner.
use crate::{
    library::Track,
    recording::Recording,
    waveform_port_analysis::{Analyzer, Plan},
    waveform_render::{Page, View, render},
};
use anyhow::{Result, bail, ensure};
use kira::sound::streaming::Decoder;
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    rate: u32,
    origin: usize,
    frame_count: usize,
    pages: Vec<Plan>,
}

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

pub fn run(mut args: impl Iterator<Item = OsString>) -> i32 {
    let Some(log) = args.next() else { return 2 };
    match execute(args.collect::<Vec<_>>().as_slice()) {
        Ok(()) => 0,
        Err(error) => {
            let _ = std::fs::write(log, format!("{error:#}\n"));
            1
        }
    }
}

fn execute(args: &[OsString]) -> Result<()> {
    match args {
        [command, file, pcm, meta] if command == "decode" => {
            let mut decoder = Recording::open(&Track::file(PathBuf::from(file)), None)?;
            let mut pcm = BufWriter::new(File::create(pcm)?);
            let count = decoder.num_frames();
            let mut written = 0;
            while written < count {
                let frames = decoder.decode()?;
                ensure!(!frames.is_empty(), "Unexpected source end");
                written += frames.len();
                for frame in frames {
                    pcm.write_all(&frame.left.to_le_bytes())?;
                    pcm.write_all(&frame.right.to_le_bytes())?;
                }
            }
            pcm.flush()?;
            serde_json::to_writer(
                File::create(meta)?,
                &serde_json::json!({
                    "rate": decoder.sample_rate(), "frameCount": written
                }),
            )?;
        }
        [command, pcm, request, output] if command == "facts" => {
            let spec: Request = serde_json::from_reader(BufReader::new(File::open(request)?))?;
            let mut pcm = BufReader::new(File::open(pcm)?);
            let mut pages = Vec::with_capacity(spec.pages.len());
            for plan in spec.pages {
                let mut analyzer = Analyzer::new(spec.rate, spec.origin, spec.frame_count, plan)?;
                let range = analyzer.decode_range();
                pcm.seek(SeekFrom::Start(((spec.origin + range.start) * 8) as u64))?;
                let mut bytes = [0; 65_536];
                let mut frames = Vec::with_capacity(bytes.len() / 8);
                let mut at = range.start;
                while at < range.end {
                    let size = (range.end - at).min(bytes.len() / 8);
                    pcm.read_exact(&mut bytes[..size * 8])?;
                    frames.clear();
                    for f in bytes[..size * 8].as_chunks::<8>().0 {
                        frames.push([0, 4].map(|offset| {
                            f32::from_le_bytes(f[offset..offset + 4].try_into().unwrap())
                        }));
                    }
                    analyzer.push(&frames)?;
                    at += size;
                }
                pages.push(analyzer.finish()?);
            }
            serde_json::to_writer(BufWriter::new(File::create(output)?), &pages)?;
        }
        [command, cases, facts, output]
            if command == "render" || command == "render-gpu" || command == "bench-gpu" =>
        {
            let cases: Replay = serde_json::from_reader(BufReader::new(File::open(cases)?))?;
            let facts = Path::new(facts);
            let output = Path::new(output);
            #[cfg(windows)]
            let mut gpu = (command != "render")
                .then(crate::waveform_render::gpu::qualification::Renderer::new)
                .transpose()?;
            #[cfg(windows)]
            if let Some(gpu) = &gpu {
                serde_json::to_writer(
                    File::create(output.join("adapter.json"))?,
                    &serde_json::json!({ "adapter": gpu.adapter, "software": gpu.software }),
                )?;
            }
            for case in cases.cases {
                let view = case.view;
                ensure!(
                    (1..=8192).contains(&view.width)
                        && (1..=1024).contains(&view.height)
                        && view.pixels_per_second.is_finite()
                        && view.pixels_per_second > 0.0,
                    "Invalid waveform viewport"
                );
                let mut pages: Vec<Page> =
                    serde_json::from_reader(BufReader::new(File::open(facts.join(case.pages))?))?;
                for page in &mut pages {
                    read_reference_facts(page, view.rate);
                }
                let pages: Vec<_> = pages.into_iter().map(Arc::new).collect();
                #[cfg(windows)]
                if let Some(gpu) = &mut gpu {
                    let evidence = if view.role == crate::waveform_render::Role::Overview {
                        crate::overview::Evidence {
                            overview: pages.first().cloned(),
                            ..Default::default()
                        }
                    } else {
                        crate::overview::Evidence {
                            pages,
                            ..Default::default()
                        }
                    };
                    let first = view.first as f64 * f64::from(view.rate) / view.pixels_per_second;
                    if command == "bench-gpu" {
                        let timing = gpu.benchmark(&evidence, &view)?;
                        serde_json::to_writer(
                            BufWriter::new(File::create(output.join(format!("{}.json", case.id)))?),
                            &timing,
                        )?;
                        continue;
                    }
                    gpu.render(&evidence, &view, first)?
                        .save(output.join(format!("{}.png", case.id)))?;
                    serde_json::to_writer(
                        BufWriter::new(File::create(output.join(format!("{}.json", case.id)))?),
                        &gpu.geometry(&view)?,
                    )?;
                    continue;
                }
                let (pixels, geometry) =
                    render(&pages.iter().map(Arc::as_ref).collect::<Vec<_>>(), &view);
                image::save_buffer(
                    output.join(format!("{}.png", case.id)),
                    &pixels,
                    view.width as u32,
                    view.height as u32,
                    image::ColorType::Rgba8,
                )?;
                serde_json::to_writer(
                    BufWriter::new(File::create(output.join(format!("{}.json", case.id)))?),
                    &geometry,
                )?;
            }
        }
        _ => bail!(
            "Expected decode INPUT PCM META, facts PCM REQUEST OUTPUT, or render[-gpu] CASES FACTS OUTPUT"
        ),
    }
    Ok(())
}

// Electron's saved qualification records predate the single-style player.
// Keep their wire format out of the production page and cache types.
fn read_reference_facts(page: &mut Page, rate: u32) {
    if !page.facts.is_empty() && page.facts.len() == page.extrema.len() / 2 * 13 {
        let historical = std::mem::take(&mut page.facts);
        let rows = historical.as_chunks::<13>().0;
        page.facts = rows
            .iter()
            .flat_map(|row| row[..7].iter().copied())
            .collect();
        if !crate::waveform_render::SUPPORTED_OCTAVE_RATES.contains(&rate) {
            page.fallback_energy =
                Some(rows.iter().map(|row| [row[10], row[11], row[12]]).collect());
        }
    }
}

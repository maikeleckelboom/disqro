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
        [command, cases, facts, output] if command == "render" => {
            let cases: Replay = serde_json::from_reader(BufReader::new(File::open(cases)?))?;
            let facts = Path::new(facts);
            let output = Path::new(output);
            for case in cases.cases {
                let view = case.view;
                ensure!(
                    (1..=8192).contains(&view.width)
                        && (1..=1024).contains(&view.height)
                        && view.pixels_per_second.is_finite()
                        && view.pixels_per_second > 0.0,
                    "Invalid waveform viewport"
                );
                let pages: Vec<Page> =
                    serde_json::from_reader(BufReader::new(File::open(facts.join(case.pages))?))?;
                let (pixels, geometry) = render(&pages.iter().collect::<Vec<_>>(), &view);
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
            "Expected decode INPUT PCM META, facts PCM REQUEST OUTPUT, or render CASES FACTS OUTPUT"
        ),
    }
    Ok(())
}

//! Compiled against each historical dekzer-playback, with its own lockfile.
use dekzer_playback::waveform::{ANALYSIS, Analyzer};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env, fs::File, io::{BufReader, BufWriter, Read, Seek, SeekFrom},
};

// The October revision predates octave color. The runner replaces this
// declaration for #134, which uses the pinned Blackman analyzer below.
mod octave {
    pub struct Work;
    impl Work {
        pub fn new(_: u32, _: u64, _: u64, _: u64, _: bool) -> Result<Self, String> { Ok(Self) }
        pub fn push(&mut self, _: &[[f32; 2]]) -> Result<(), String> { Ok(()) }
        pub fn finish(self, _: u64, _: u64) -> Result<Option<serde_json::Value>, String> { Ok(None) }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    rate: u32,
    origin: u64,
    frame_count: u64,
    pages: Vec<Plan>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    start_frame: u64,
    end_frame: u64,
    frames_per_column: u64,
    overview: bool,
    octave: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|a| a == "identity") {
        println!("{ANALYSIS}");
        return Ok(());
    }
    let mut input = BufReader::new(File::open(&args[1])?);
    let spec: Request = serde_json::from_reader(File::open(&args[2])?)?;
    let mut pages = Vec::<Value>::new();
    for plan in spec.pages {
        let start = plan.start_frame;
        let end = plan.end_frame;
        let before = start.saturating_sub(u64::from(spec.rate) / 5);
        let until = (end + if plan.octave.is_some() { 2048 } else { 0 }).min(spec.frame_count);
        let mut analyzer = Analyzer::new(spec.rate, before, start, plan.frames_per_column, Some(end), plan.overview)?;
        let mut octave = octave::Work::new(spec.rate, before, start, end, plan.octave.is_some())?;
        input.seek(SeekFrom::Start((spec.origin + before) * 8))?;
        let mut buffer = vec![0; 8192 * 8];
        let mut at = before;
        while at < until {
            let count = (until - at).min(8192) as usize;
            input.read_exact(&mut buffer[..count * 8])?;
            let frames: Vec<_> = buffer[..count * 8].as_chunks::<8>().0.iter().map(|frame| {
                [0, 4].map(|i| f32::from_le_bytes(frame[i..i + 4].try_into().unwrap()))
            }).collect();
            analyzer.push(&frames)?;
            octave.push(&frames)?;
            at += count as u64;
        }
        let page = analyzer.page();
        let mut payload = json!({
            "startFrame": start, "frameCount": end - start, "framesPerColumn": plan.frames_per_column,
            "facts": page.facts, "extrema": page.extrema, "temporal": null,
        });
        if let Some(octave) = octave.finish(start, end)? {
            payload["octave"] = octave;
        }
        pages.push(payload);
    }
    serde_json::to_writer(BufWriter::new(File::create(&args[3])?), &pages)?;
    Ok(())
}

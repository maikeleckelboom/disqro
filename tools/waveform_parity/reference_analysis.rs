//! Compiled as an example inside the pinned Dekzer checkout by qualify_waveform_parity.py.
use dekzer_playback::{
    Decoder,
    waveform::{ANALYSIS, Analyzer, Page},
    waveform_detail::SourceDetail,
    waveform_octave::{OctaveAnalysis, OctaveAnalyzer, OctaveDescriptor},
    waveform_spectrum::SpectrumReducer,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env,
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
    leaf_frames: Option<u64>,
    octave: Option<String>,
    overview: bool,
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [action, input, pcm, metadata] if action == "decode" => {
            let mut decoder = Decoder::open_exact(File::open(input)?, 0, None)?;
            let mut output = BufWriter::new(File::create(pcm)?);
            let mut count = 0;
            while let Some(frames) = decoder.next_frames()? {
                count += frames.len() as u64;
                for frame in frames {
                    for value in frame {
                        output.write_all(&value.to_le_bytes())?;
                    }
                }
            }
            output.flush()?;
            serde_json::to_writer(
                File::create(metadata)?,
                &json!({"rate": decoder.rate, "frameCount": count, "analysis": ANALYSIS}),
            )?;
        }
        [action, pcm, request, output] if action == "facts" => {
            let spec: Request = serde_json::from_reader(File::open(request)?)?;
            let mut reader = BufReader::new(File::open(Path::new(pcm))?);
            let mut pages = Vec::with_capacity(spec.pages.len());
            for plan in &spec.pages {
                pages.push(facts(&mut reader, &spec, plan)?);
            }
            serde_json::to_writer(BufWriter::new(File::create(output)?), &pages)?;
        }
        _ => return Err("expected decode INPUT PCM META or facts PCM REQUEST OUTPUT".into()),
    }
    Ok(())
}

fn emit_detail(detail: &SourceDetail) -> Value {
    json!({
        "version": 1, "leafFrames": detail.leaf_frames, "startFrame": detail.start_frame,
        "firstCellFrame": detail.first_cell_frame, "frameCount": detail.frame_count,
        "bounds": detail.bounds, "offsets": detail.offsets
    })
}

fn facts(input: &mut BufReader<File>, spec: &Request, plan: &Plan) -> Result<Value> {
    let start = plan.start_frame;
    let end = plan.end_frame;
    if start >= end || end > spec.frame_count {
        return Err("invalid reference page range".into());
    }
    let before = start.saturating_sub(u64::from(spec.rate) / 5);
    let mut source_detail = plan
        .leaf_frames
        .and_then(|width| SourceDetail::with_leaf(spec.origin + start, width));
    let mut analyzer = if source_detail.is_some() {
        None
    } else {
        Some(Analyzer::new(
            spec.rate,
            before,
            start,
            plan.frames_per_column,
            Some(end),
            plan.overview,
        )?)
    };
    let variant = match plan.octave.as_deref() {
        Some("stroke-hann512") => Some(OctaveAnalysis::StrokeHann512),
        Some("blackman512") | None => None,
        _ => return Err("unknown reference octave variant".into()),
    };
    let mut octave = plan
        .octave
        .as_ref()
        .map(|_| OctaveAnalyzer::with_analysis(spec.rate, before, start, end, variant))
        .transpose()?;
    let mut powers = Vec::new();
    let mut reducer = if plan.overview && octave.is_some() {
        Some(SpectrumReducer::new(plan.frames_per_column)?)
    } else {
        None
    };
    let mut reduction_error = Ok(());
    let mut emit = |cell: OctaveDescriptor| {
        if let Some(reducer) = &mut reducer {
            if reduction_error.is_ok() {
                reduction_error = reducer.push(cell);
            }
        } else {
            powers.extend(cell.energy);
        }
    };
    let read_end = if octave.is_some() {
        (end + 2048).min(spec.frame_count)
    } else {
        end
    };
    input.seek(SeekFrom::Start((spec.origin + before) * 8))?;
    let mut at = before;
    let mut bytes = vec![0; 8192 * 8];
    while at < read_end {
        let size = (read_end - at).min(8192) as usize;
        input.read_exact(&mut bytes[..size * 8])?;
        let frames: Vec<[f32; 2]> = bytes[..size * 8]
            .as_chunks::<8>().0.iter().map(|frame| {
                [0, 4].map(|offset| {
                    let mut value = [0; 4];
                    value.copy_from_slice(&frame[offset..offset + 4]);
                    f32::from_le_bytes(value)
                })
            }).collect();
        if let Some(analyzer) = &mut analyzer {
            analyzer.push(&frames)?;
        }
        if let Some(detail) = &mut source_detail {
            let lo = start.saturating_sub(at).min(size as u64);
            let hi = end.saturating_sub(at).min(size as u64);
            for &frame in &frames[lo as usize..hi as usize] {
                detail.push(frame)?;
            }
        }
        if let Some(octave) = &mut octave {
            octave.push(&frames, &mut emit)?;
        }
        at += size as u64;
    }
    if let Some(octave) = &mut octave {
        if !octave.is_complete() && read_end != spec.frame_count {
            return Err("reference analysis ran short of lookahead".into());
        }
        octave.finish(&mut emit)?;
    }
    reduction_error?;
    let Page { facts, extrema, .. } = analyzer.map_or_else(
        || Page {
            start_frame: start, frame_count: end - start, frames_per_column: plan.frames_per_column,
            facts: Vec::new(), extrema: Vec::new(), temporal: None, octave: None, spectrum: None,
            source_detail: None, stroke: Some(1),
        },
        |analyzer| analyzer.page(),
    );
    let mut value = json!({
        "startFrame": start, "frameCount": end - start, "framesPerColumn": plan.frames_per_column,
        "facts": facts, "extrema": extrema, "temporal": null
    });
    if let Some(detail) = source_detail {
        value["sourceDetail"] = emit_detail(&detail);
        value["stroke"] = json!(1);
    }
    if let Some(reducer) = reducer {
        value["spectrum"] = json!(reducer.powers(0, true));
    } else if octave.is_some() {
        value["octave"] = json!({
            "version": 1, "analysis": variant, "startFrame": start, "frameCount": end - start,
            "firstCellFrame": start / 256 * 256, "framesPerColumn": 256, "powers": powers
        });
    }
    Ok(value)
}

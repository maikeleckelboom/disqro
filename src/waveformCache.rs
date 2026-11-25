//! The waveform cache stores only rebuildable facts. The request key supplies
//! all array sizes and coordinates. The record is little-endian and preserves
//! the full precision of the analysed octave powers.
use super::Evidence;
use crate::{
    waveform_port_analysis::Spectrum,
    waveform_render::{FACTS, Octave, Page, SourceDetail},
};

pub(super) fn encode(page: &Page) -> Vec<u8> {
    let mut data = Vec::with_capacity(page.bytes());
    data.extend(page.facts.iter().flat_map(|v| v.to_le_bytes()));
    data.extend(page.extrema.iter().flat_map(|&v| (v as u32).to_le_bytes()));
    if let Some(energy) = &page.fallback_energy {
        data.extend(
            energy
                .iter()
                .flatten()
                .flat_map(|value| value.to_le_bytes()),
        );
    }
    if let Some(detail) = &page.source_detail {
        data.extend(detail.bounds.iter().flat_map(|v| v.to_le_bytes()));
        data.extend(detail.offsets.iter().flat_map(|v| v.to_le_bytes()));
    }
    if let Some(powers) = page
        .spectrum
        .as_ref()
        .or(page.octave.as_ref().map(|s| &s.powers))
    {
        data.extend(powers.iter().flat_map(|v| v.to_le_bytes()));
    }
    data
}

pub(super) fn decode(mut data: &[u8], request: &Evidence) -> Option<Page> {
    let plan = request.plan;
    let count = plan.end_frame - plan.start_frame;
    let width = plan.frames_per_column;
    let columns = if plan.measure_columns() {
        count.div_ceil(width)
    } else {
        0
    };
    let facts = read(&mut data, columns * FACTS, f32::from_le_bytes)?;
    let extrema = read(&mut data, columns * 2, |word| {
        u32::from_le_bytes(word) as usize
    })?;
    let mut page = Page {
        start_frame: plan.start_frame,
        frames_per_column: width,
        frame_count: count,
        preview: plan.preview,
        facts,
        extrema,
        ..Default::default()
    };
    if plan.fallback_energy {
        page.fallback_energy = Some(read(&mut data, columns, |row: [u8; 12]| {
            let values: &[[u8; 4]; 3] = row.as_chunks::<4>().0.try_into().unwrap();
            values.map(f32::from_le_bytes)
        })?);
    }
    if let Some(leaf) = plan.leaf_frames {
        let start = request.playable.start + plan.start_frame;
        let base = start / leaf * leaf;
        let leaves = (start + count - base).div_ceil(leaf);
        page.source_detail = Some(SourceDetail {
            first_cell_frame: base,
            start_frame: start,
            frame_count: count,
            leaf_frames: leaf,
            bounds: read(&mut data, leaves * 2, f32::from_le_bytes)?,
            offsets: read(&mut data, leaves, u16::from_le_bytes)?,
        });
    }
    if let Some(spectrum) = plan.octave {
        if plan.overview {
            page.spectrum = Some(read(&mut data, columns * 8, f64::from_le_bytes)?);
        } else {
            let base = plan.start_frame / 256 * 256;
            let cells = (plan.end_frame - base).div_ceil(256);
            page.octave = Some(Octave {
                first_cell_frame: base,
                analysis: (spectrum == Spectrum::Stroke).then(|| "stroke-hann512".into()),
                powers: read(&mut data, cells * 8, f64::from_le_bytes)?,
            });
        }
    }
    data.is_empty().then_some(page)
}

fn read<T, const N: usize>(
    data: &mut &[u8],
    count: usize,
    decode: impl Fn([u8; N]) -> T,
) -> Option<Vec<T>> {
    let (head, tail) = data.split_at_checked(count.checked_mul(N)?)?;
    *data = tail;
    Some(
        head.as_chunks::<N>()
            .0
            .iter()
            .copied()
            .map(decode)
            .collect(),
    )
}

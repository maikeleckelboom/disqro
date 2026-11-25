//! Translate the reference Canvas paths into horizontal sample ownership.
//! Each word describes a column of four vertical sample intervals. The
//! fragment shader evaluates coverage and combines their colours at draw time.
use super::{Page, Parameters, View, Word};
use crate::waveform_render::{
    COVERAGE_SAMPLES, Rect, overview_colours, overview_columns, overview_paths, snapped_path,
};

pub(super) fn columns(page: Option<&Page>, view: &View, params: &mut Parameters) -> Vec<Word> {
    let Some(page) = page else { return vec![] };
    let projected = overview_columns(page, view);
    if projected.is_empty() {
        return vec![];
    }
    params.extent[2] = 3;
    let mut words = vec![[0; 4]; view.width * 3];
    for (band, path) in overview_paths(&projected, view).iter().enumerate() {
        params.extent[3] |= u32::from(rows(&mut words, view, band, path)) << band;
    }
    for (column, [r, g, b]) in words
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(overview_colours(page, &projected, view))
    {
        column[2] = [r, g, b, 0.0].map(|v| (v as f32).to_bits());
    }
    words
}

fn rows(words: &mut [Word], view: &View, band: usize, path: &[Rect]) -> bool {
    let (atlas, intervals) = snapped_path(path);
    let columns = words.as_chunks_mut::<3>().0;
    let row = |y: f64| y.ceil().clamp(0.0, view.height as f64) as u32;
    for [left, right, top, bottom] in intervals {
        let first = left.floor().max(0.0) as usize;
        let end = (right.ceil() as usize).min(view.width);
        for (s, [dx, dy]) in COVERAGE_SAMPLES.into_iter().enumerate() {
            let lo = row(top - dy);
            let hi = row(bottom - dy);
            if lo >= hi {
                continue;
            }
            let a = ((left - dx).ceil() as usize).max(first);
            let b = ((right - dx).ceil() as usize).min(end);
            if a < b {
                for column in &mut columns[a..b] {
                    column[band][s] = lo | (hi << 16);
                }
            }
        }
    }
    atlas
}

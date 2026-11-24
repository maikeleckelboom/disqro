use super::*;

fn intensity(hits: f32) -> u8 {
    intensity_table(Ink::Density)[(hits.to_bits() >> 17) as usize]
}

fn mass(field: &Field) -> f32 {
    field
        .active
        .iter()
        .map(|&cell| field.cells[cell as usize] * field.scale)
        .sum()
}

fn trace(side: usize) -> Field {
    Field::new(
        Analysis {
            ink: Ink::Trace,
            axes: Axes::XY,
            ..Default::default()
        },
        48000,
        side,
    )
}

#[test]
fn deposition_and_resize_conserve_dwell_including_at_full_scale() {
    let mut f = Field::new(
        Analysis {
            axes: Axes::XY,
            ..Default::default()
        },
        48000,
        321,
    );
    f.push(&[
        Frame::new(-1., 1.),
        Frame::new(1., -1.),
        Frame::new(0.65, 0.15),
    ]);
    for side in [321, 2049, 145, 777, 3] {
        f.resize(side);
        assert!((mass(&f) - 3.).abs() < 1e-4);
        assert!(f.cells.len() <= MAX_SIDE * MAX_SIDE);
    }
}

#[test]
fn small_subscriber_sums_linear_dwell_before_display_compression() {
    let mut f = Field::new(Default::default(), 48000, 513);
    // Adjacent source cells resolve independently at native size, but share
    // exactly one destination cell at 129 px.
    f.add(256 * 513 + 256, 3.);
    f.add(256 * 513 + 257, 5.);
    let compact = f.reading(Stereo::default(), 129);
    assert_eq!(compact.bands.len(), 1);
    assert_eq!(compact.bands[0].intensity, [intensity(8. * 0.5)]);
    assert_eq!(
        f.reading(Stereo::default(), 513).bands[0].intensity,
        [intensity(3. * 513. / 769.), intensity(5. * 513. / 769.)]
    );
}

#[test]
fn response_separates_octaves_and_half_life_follows_pcm_time() {
    let codes = [0., 0.02, 0.1, 0.5, 1., 4., 16., 64., 256., 1024.].map(intensity);
    assert!(codes.windows(2).all(|pair| pair[0] < pair[1]));
    for persistence in [Persistence::Short, Persistence::Normal, Persistence::Long] {
        let mut f = Field::new(
            Analysis {
                persistence,
                ..Default::default()
            },
            48000,
            257,
        );
        f.push(&[Frame::new(0.75, 0.25); 128]);
        let silence = vec![Frame::ZERO; (48000. * persistence.half_life()).round() as usize];
        for block in silence.chunks(128) {
            f.push(block);
        }
        assert!((mass(&f) - 64.).abs() < 0.01);
    }
}

#[test]
fn trace_covers_fast_contiguous_segments_and_breaks_at_missing_samples() {
    let (a, b) = (Frame::new(-0.75, 0.125), Frame::new(0.75, 0.125));
    let mut f = trace(513);
    let mut pulse = [a; 128];
    pulse[127] = b;
    f.push(&pulse);
    let columns = |f: &Field| -> std::collections::BTreeSet<_> {
        f.active.iter().map(|&i| i as usize % f.side).collect()
    };
    let image = f.reading(Stereo::default(), 513);
    let band = &image.bands[0];
    let width = band.bounds[2] - band.bounds[0];
    assert!((0..width).all(|x| band.intensity[x..].iter().step_by(width).any(|&v| v > 0)));
    let occupied = columns(&f);
    assert_eq!(
        occupied.len(),
        occupied.last().unwrap() - occupied.first().unwrap() + 1
    );
    assert!((mass(&f) - 128.).abs() < 0.001);
    for gap in [Frame::ZERO, Frame::new(f32::NAN, 0.), Frame::new(1.1, 0.)] {
        let mut f = trace(513);
        f.push(&[a, gap, b]);
        assert!(columns(&f).len() <= 4);
    }
}

#[test]
fn separated_transients_upload_only_their_occupied_strips() {
    let mut f = Field::new(
        Analysis {
            axes: Axes::XY,
            ..Default::default()
        },
        48000,
        MAX_SIDE,
    );
    f.push(&[Frame::new(-0.9, 0.9), Frame::new(0.9, -0.9)]);
    let r = f.reading(Stereo::default(), MAX_SIDE);
    assert_eq!(r.bands.len(), 2);
    assert!(r.bands.iter().map(|b| b.intensity.len()).sum::<usize>() <= 8);
}

#[test]
fn swept_mono_line_keeps_its_exposure_and_single_pixel_width_on_resize() {
    let mut levels = vec![];
    for side in [513, 769, 1537] {
        let mut f = Field::new(Analysis::default(), 48000, side);
        let signal: Vec<_> = (0..24000)
            .map(|i| {
                let sample = (i % 4000) as f32 / 2500. - 0.8;
                Frame::new(sample, sample)
            })
            .collect();
        for frames in signal.chunks(128) {
            f.push(frames);
        }
        let image = f.reading(Stereo::default(), side);
        let center = side / 2;
        let band = image
            .bands
            .iter()
            .find(|b| b.bounds[1] <= center && center < b.bounds[3])
            .unwrap();
        assert_eq!((band.bounds[0], band.bounds[2]), (center, center + 1));
        // Average a common nonzero portion of the sweep. Exact digital zero is a pen-up.
        let rows = side * 3 / 8..side * 7 / 16;
        let count = rows.len();
        let sum: usize = rows
            .map(|row| usize::from(band.intensity[row - band.bounds[1]]))
            .sum();
        levels.push(sum / count);
    }
    assert!(
        levels.iter().max().unwrap() - levels.iter().min().unwrap() <= 3,
        "{levels:?}"
    );
}

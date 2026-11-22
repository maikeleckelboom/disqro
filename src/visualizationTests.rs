use super::*;
use crate::{capture::Block, spectral_history as spectral, stereo_field as field};
use kira::Frame;
use std::f32::consts::TAU;

#[test]
fn spectral_rows_are_frequency_and_sine_peak_calibrated_in_both_scales() {
    for rate in [32000, 48000, 96000] {
        let n = spectral::WindowLength::Balanced.len(rate);
        let tone = |bin: usize, i: usize| (TAU * bin as f32 * i as f32 / n as f32).sin();
        let frames: Vec<_> = (0..n)
            .map(|i| {
                let common = 0.5 * tone(128, i);
                let difference = 0.25 * tone(320, i);
                Frame::new(common + difference, common - difference)
            })
            .collect();
        let mut fft = Transform::new(n);
        fft.measure(&frames);
        for mapping in [spectral::Mapping::Linear, spectral::Mapping::Log] {
            for (mode, first, second) in [
                (Mode::Stereo, -6., -12.),
                (Mode::Left, -6., -12.),
                (Mode::Right, -6., -12.),
                (Mode::Mid, -6., -120.),
                (Mode::Side, -120., -12.),
            ] {
                let a = spectral::Analysis {
                    mapping,
                    mode,
                    ..Default::default()
                };
                let col = spectral::Scale::new(a, rate, n).column(&fft);
                let high = a.range.high(rate);
                assert!(high <= rate as f32 / 2.);
                for (bin, expected) in [(128, first), (320, second)] {
                    let hz = bin as f32 * rate as f32 / n as f32;
                    let row = (mapping.fraction(hz, high) * spectral::ROWS as f32) as usize;
                    assert!(
                        (spectral::db(col[row]) - expected).abs() <= 0.51,
                        "{rate} {mode:?} {mapping:?} {hz} {:?}",
                        spectral::db(col[row])
                    );
                }
            }
        }
        fft.measure(&vec![Frame::ZERO; n]);
        assert_eq!(
            spectral::Scale::new(Default::default(), rate, n).column(&fft),
            [0; spectral::ROWS]
        );
    }
}

#[test]
fn every_hop_survives_a_slow_consumer_and_frozen_tiles_stay_immutable() {
    let rate = 48000;
    let mut streams = Streams::default();
    let request = Request::Spectral(Default::default());
    let (send, a) = async_channel::bounded(1);
    let (other, b) = async_channel::bounded(1);
    streams.add(
        1,
        request,
        Delivery {
            speed: Speed::Slow,
            statistics: false,
        },
        send,
        0,
    );
    streams.add(
        2,
        request,
        Delivery {
            speed: Speed::Fast,
            statistics: false,
        },
        other,
        0,
    );
    let mut history = History::new();
    let mut plans = HashMap::new();
    let n = spectral::WindowLength::Balanced.len(rate);
    let hop = n / 4;
    let mut feed = |streams: &mut Streams,
                    h: &mut History,
                    start: usize,
                    len: usize,
                    epoch: u64,
                    rate: u32| {
        for i in (start..start + len).step_by(BLOCK) {
            let impulse = i == 4096;
            let mut pcm = [Frame::ZERO; BLOCK];
            if impulse {
                pcm[0] = Frame::new(1., -1.);
            }
            let continuity = h.continuity;
            h.push(Block {
                epoch,
                rate,
                start: i as u64,
                frames: pcm,
            });
            if h.continuity != continuity {
                streams.reset();
            }
            streams.observe(h, &mut plans);
        }
    };
    feed(&mut streams, &mut history, 0, 16 * n, 1, rate);
    streams.publish(&history, true, 0);
    let frozen = a.try_recv().unwrap().unwrap();
    assert!(Arc::ptr_eq(&frozen, &b.try_recv().unwrap().unwrap()));
    let Payload::Spectral(ring) = &frozen.data else {
        panic!()
    };
    assert_eq!(ring.tiles[0].center, n as u64 / 2);
    assert_eq!(ring.hop, hop as u64);
    assert_eq!(
        ring.tiles.iter().map(|t| t.columns.len()).sum::<usize>(),
        (16 * n - n) / hop + 1
    );
    let at = |i| ring.column_at(i).unwrap().iter().copied().max().unwrap();
    assert_eq!(at(n as u64 / 2), 0);
    assert!(at(4096) > 0);
    assert_eq!(at(8192), 0);
    streams.remove(1);
    let frozen_end = ring.end;
    feed(&mut streams, &mut history, 16 * n, 8 * n, 1, rate);
    streams.publish(&history, true, 4);
    assert_eq!(ring.end, frozen_end);
    let live = b.try_recv().unwrap().unwrap();
    assert!(matches!(&live.data,Payload::Spectral(r) if r.end>frozen_end));
    // An overflow, a replacement device starting at zero and a rate-only
    // change in a contiguous sequence each need a complete fresh window.
    for (start, epoch, rate) in [(32 * n, 1, 48000), (0, 2, 96000), (2 * n, 2, 44100)] {
        let n = spectral::WindowLength::Balanced.len(rate);
        feed(&mut streams, &mut history, start, BLOCK, epoch, rate);
        streams.publish(&history, true, 8);
        assert!(b.try_recv().unwrap().is_none());
        feed(
            &mut streams,
            &mut history,
            start + BLOCK,
            n - BLOCK,
            epoch,
            rate,
        );
        streams.publish(&history, true, 12);
        let fresh = b.try_recv().unwrap().unwrap();
        assert_eq!(fresh.rate, rate);
        assert!(
            matches!(&fresh.data,Payload::Spectral(r) if r.tiles.len()==1 && r.tiles[0].columns.len()==1 && r.tiles[0].center==(start+n/2) as u64)
        );
    }
    streams.remove(2);
    assert!(streams.is_empty());
}

#[test]
fn history_evicts_at_sixty_audio_seconds_and_keeps_frozen_tail() {
    let mut h = spectral::History::new(48000, 2048);
    h.push(1024, [17; spectral::ROWS]);
    let frozen = h.clone();
    for i in 1..12000 {
        h.push(1024 + i * h.hop, [32; spectral::ROWS]);
    }
    assert_eq!(frozen.tiles[0].columns.len(), 1);
    assert_eq!(frozen.column_at(1024), Some(&[17; spectral::ROWS]));
    assert!(h.column_at(1024).is_none());
    let span = h.end - h.tiles[0].center;
    assert!((60 * 48000..60 * 48000 + spectral::TILE as u64 * h.hop).contains(&span));
}

#[test]
fn paired_geometry_and_density_match_phase_balance_and_silence() {
    for (l, r, x, y, corr, mono) in [
        (1., 1., 0., 1., Some(1.), 1.),
        (1., -1., 1., 0., Some(-1.), 0.),
        (1., 0., 0.5, 0.5, None, 0.5),
        (0., 1., -0.5, 0.5, None, 0.5),
    ] {
        let f = Frame::new(l, r);
        assert_eq!(field::Axes::XY.coordinates(f), (l, r));
        assert_eq!(field::Axes::MidSide.coordinates(f), (x, y));
        let mut g = field::Field::new(Default::default(), 48000);
        let block = [f; BLOCK];
        g.push(&block);
        let reading = g.reading(Stereo::measure(&block));
        assert_eq!(reading.stereo.correlation, corr);
        assert_eq!(reading.stereo.mono_retained(), Some(mono));
        let k = (reading.side - 2) as f32 * 0.5;
        let raster = reading.raster(reading.side).unwrap();
        let [left, top, right, _] = raster.bounds;
        let width = right - left;
        for (i, _) in raster.intensity.iter().enumerate().filter(|(_, v)| **v > 0) {
            let column = (left + i % width) as f32;
            let row = (top + i / width) as f32;
            assert!(
                (column - ((1. + x) * k + 0.5)).abs() <= 1.
                    && (row - ((1. - y) * k + 0.5)).abs() <= 1.
            );
        }
        assert!(raster.intensity.iter().any(|v| *v > 0));
        for _ in 0..2000 {
            g.push(&[Frame::ZERO; BLOCK]);
        }
        let silent = g.reading(Stereo::default());
        assert!(g.settled && silent.raster(silent.side).is_none());
        assert!(silent.stereo.correlation.is_none() && silent.stereo.mono_retained().is_none());
    }
    let mut g = field::Field::new(Default::default(), 48000);
    let mut fixtures = Vec::new();
    for i in 0..8192 {
        fixtures.push(Frame::new(
            (i as f32 * 0.037).sin(),
            (i as f32 * 0.113).cos(),
        ));
    }
    for block in fixtures.chunks(BLOCK) {
        g.push(block);
    }
    let reading = g.reading(Stereo::measure(&fixtures));
    let raster = reading.raster(reading.side).unwrap();
    assert!(raster.intensity.iter().filter(|&&v| v > 0).count() > 5000);
    assert!(reading.stereo.correlation.unwrap().abs() < 0.01);
}

#[test]
fn stereo_field_resolves_sub_cell_detail_when_the_plot_grows() {
    let first = Frame::new(0.5, 0.5);
    let second = Frame::new(0.505, 0.505);
    let mut field = field::Field::new(Default::default(), 48000);
    field.push(&[first; BLOCK]);
    field.push(&[second; BLOCK]);
    let reading = field.reading(Stereo::default());
    let occupied_rows = |side| {
        let raster = reading.raster(side).unwrap();
        let [left, top, right, _] = raster.bounds;
        raster
            .intensity
            .chunks_exact(right - left)
            .enumerate()
            .filter_map(|(row, values)| values.iter().any(|&v| v > 0).then_some(row + top))
            .collect::<Vec<_>>()
    };
    let compact = occupied_rows(256);
    assert!(compact.windows(2).all(|rows| rows[1] - rows[0] == 1));
    let full = occupied_rows(1440);
    assert!(full.windows(2).any(|rows| rows[1] - rows[0] > 1));
}

#[test]
fn independent_dock_settings_round_trip_with_partial_records() {
    let mut a = spectral::Settings::default();
    let b = a;
    a.view = spectral::View::Waterfall;
    a.analysis.mode = Mode::Side;
    let saved = serde_json::to_value(a).unwrap();
    assert_eq!(
        serde_json::from_value::<spectral::Settings>(saved).unwrap(),
        a
    );
    assert_ne!(a, b);
    let f: field::Settings = serde_json::from_str(r#"{"axes":"XY","statistics":false}"#).unwrap();
    assert_eq!(f.analysis.axes, field::Axes::XY);
    assert!(!f.statistics);
    assert_eq!(f.speed, Speed::Normal);
}

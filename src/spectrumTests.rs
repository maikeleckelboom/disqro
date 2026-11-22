use super::*;

fn tone(n: usize, bin: f64, l: f32, r: f32) -> Vec<Frame> {
    (0..n)
        .map(|i| {
            let sample = (i as f64 * std::f64::consts::TAU * bin / n as f64).sin() as f32;
            Frame::new(sample * l, sample * r)
        })
        .collect()
}
fn near(a: f64, b: f64, tolerance: f64) {
    assert!((a - b).abs() <= tolerance, "{a} != {b} ± {tolerance}");
}
#[test]
fn coherent_gain_frequency_and_leakage_across_windows_and_rates() {
    for rate in [32000, 44100, 48000, 96000, 192000] {
        for resolution in Resolution::ALL {
            let n = resolution.len(rate);
            let k = (997. * n as f64 / rate as f64).round() as usize;
            let mut t = Transform::new(n);
            for amplitude in [1., 0.5, 1e-8] {
                t.measure(&tone(n, k as f64, amplitude, amplitude));
                let best = (0..t.bins.len())
                    .max_by(|&a, &b| {
                        t.power(a, Mode::Stereo)
                            .total_cmp(&t.power(b, Mode::Stereo))
                    })
                    .unwrap();
                assert_eq!(best, k);
                near(
                    10. * t.power(k, Mode::Stereo).log10(),
                    20. * f64::from(amplitude).log10(),
                    0.0001,
                );
                near(
                    k as f64 * rate as f64 / n as f64,
                    997.,
                    rate as f64 / n as f64 / 2.,
                );
                assert!(t.power(k + 4, Mode::Left) < t.power(k, Mode::Left) * 1e-10);
            }
            t.measure(&tone(n, k as f64 + 0.5, 1., 1.));
            let peak = t.power(k, Mode::Left).max(t.power(k + 1, Mode::Left));
            assert!((0.74..0.80).contains(&peak)); // Blackman half-bin scalloping
            assert!(t.power(k + 11, Mode::Left) < peak * 1e-7);
        }
    }
}

#[test]
fn endpoints_have_no_one_sided_doubling_and_bands_integrate_power() {
    let n = 8192;
    let mut t = Transform::new(n);
    t.measure(&vec![Frame::new(0.5, 0.); n]);
    near(t.power(0, Mode::Left), 0.25, 1e-6);
    t.measure(
        &(0..n)
            .map(|i| Frame::new(if i % 2 == 0 { 1. } else { -1. }, 0.))
            .collect::<Vec<_>>(),
    );
    near(t.power(n / 2, Mode::Left), 1., 1e-6);
    let mut out = [FLOOR; POINTS];
    for (rate, bin) in [(48000, 180), (32000, 256)] {
        t.measure(&tone(n, bin as f64, 1., 1.));
        let bands = Projection::new(
            rate,
            n,
            Settings {
                style: Style::Bands,
                ..Default::default()
            },
        );
        bands.targets(&t, &mut out);
        near(out[3].into(), -3.0103, 0.0002); // in-band full-scale sine RMS
        assert!(out[0] == FLOOR);
        let plot = Projection::new(rate, n, Settings::default());
        assert!(plot.cells.last().unwrap().hi <= n as f64 / 2. + 1e-8);
        plot.targets(&t, &mut out);
        let (point, value) = out
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .unwrap();
        near(f64::from(*value), 0., 0.01);
        assert!((plot.cells[point].center / bin as f64).ln().abs() < 0.02);
    }
}
#[test]
fn stereo_modes_preserve_phase_and_single_channel_energy() {
    let n = 8192;
    let mut t = Transform::new(n);
    for (l, r, corr, mono) in [
        (1., 1., Some(1.), Some(1.)),
        (1., -1., Some(-1.), Some(0.)),
        (0., 1., None, Some(0.5)),
        (1., 0., None, Some(0.5)),
        (0., 0., None, None),
    ] {
        t.measure(&tone(n, 104., l, r));
        let (l, r) = (f64::from(l), f64::from(r));
        for (mode, expected) in [
            (Mode::Left, l * l),
            (Mode::Right, r * r),
            (Mode::Stereo, (l * l + r * r) / 2.),
            (Mode::Mid, ((l + r) / 2.).powi(2)),
            (Mode::Side, ((l - r) / 2.).powi(2)),
        ] {
            near(t.power(104, mode), expected, 1e-6);
        }
        let [left, right, mid, side] = t.stereo.energy;
        near(left, l * l / 2., 1e-6);
        near(right, r * r / 2., 1e-6);
        near(mid + side, (left + right) / 2., 1e-9);
        assert_eq!(t.stereo.correlation, corr);
        assert_eq!(t.stereo.mono_retained(), mono);
        assert_eq!(t.stereo.opposed(), corr == Some(-1.));
    }
    let mut input = tone(n, 104., 0., 1.);
    input[0].left = f32::NAN;
    input[1].left = f32::INFINITY;
    t.measure(&input);
    assert_eq!(t.stereo.correlation, None);
    assert!(t.bins.iter().all(|b| b.iter().all(|p| p.is_finite())));
}

#[test]
fn peak_hold_accounts_for_elapsed_time_including_the_expiry_frame() {
    let s = Settings::default();
    let mut a = Ballistics::default();
    let mut b = Ballistics::default();
    a.step(&[0.], 0.012, s);
    near(a.levels[0].into(), f64::from(FLOOR) * (-1f64).exp(), 0.0001);
    for env in [&mut a, &mut b] {
        env.step(&[0.], 1., s);
    }
    a.step(&[FLOOR], 0.75, s);
    near(a.peaks[0].into(), 0., 0.001);
    a.step(&[FLOOR], 0.5, s);
    for _ in 0..125 {
        b.step(&[FLOOR], 0.01, s);
    }
    near(a.peaks[0].into(), -9., 0.001);
    near(a.peaks[0].into(), b.peaks[0].into(), 0.001);
    near(a.levels[0].into(), -45., 0.001);
}
#[test]
fn smoothing_averages_power_and_tilt_is_referenced_to_one_kilohertz() {
    let n = 8192;
    let mut t = Transform::new(n);
    t.measure(&tone(n, 1024., 1., 1.));
    let mut a = [FLOOR; POINTS];
    let mut b = a;
    let raw = Projection::new(48000, n, Settings::default());
    raw.targets(&t, &mut a);
    let k = a.iter().position(|p| *p > -0.001).unwrap();
    Projection::new(
        48000,
        n,
        Settings {
            smoothing: Smoothing::Third,
            ..Default::default()
        },
    )
    .targets(&t, &mut b);
    assert!(b[k] < -12. && b[k] > -26.);
    Projection::new(
        48000,
        n,
        Settings {
            tilt: Tilt::Three,
            ..Default::default()
        },
    )
    .targets(&t, &mut b);
    near(
        (b[k] - a[k]).into(),
        3. * (raw.cells[k].center * 48000. / n as f64 / 1000.).log2(),
        0.001,
    );
    // Old and partial panel records retain sane defaults.
    let saved: Settings = serde_json::from_str(r#"{"mode":"Side"}"#).unwrap();
    assert_eq!(
        saved,
        Settings {
            mode: Mode::Side,
            ..Default::default()
        }
    );
}

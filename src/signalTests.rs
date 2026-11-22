use super::*;
use crate::levels::dbfs;
use kira::info::MockInfoBuilder;

fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "{a} vs {b}");
}

#[test]
fn silence_constants_sine_and_over_level_are_channel_independent() {
    let measure = |frames: &[Frame]| {
        let mut meter = Measurement::new(48_000);
        let mut last = None;
        for part in frames.chunks(137) {
            meter.observe(part, |levels| last = Some(levels));
        }
        last.unwrap()
    };
    let silent = measure(&[Frame::ZERO; 4800]);
    assert_eq!(dbfs(silent.rms[0]), f32::NEG_INFINITY);
    assert_eq!(dbfs(silent.peak[1]), f32::NEG_INFINITY);
    let dc = measure(&[Frame::new(-0.5, 1.25); 4800]);
    close(dbfs(dc.rms[0]), -6.0206);
    close(dbfs(dc.peak[0]), -6.0206);
    close(dbfs(dc.rms[1]), 1.9382);
    assert_eq!(dc.held, [0.5, 1.25]);
    // An integer number of cycles, with the extrema on sample positions.
    let sine: Vec<_> = (0..4800)
        .map(|i| {
            let phase = std::f64::consts::TAU * (i % 48) as f64 / 48.;
            Frame::new(phase.sin() as f32, 0.)
        })
        .collect();
    let tone = measure(&sine);
    close(dbfs(tone.rms[0]), -3.0103);
    close(dbfs(tone.peak[0]), 0.);
    assert_eq!(tone.rms[1], 0.);
}

#[test]
fn rolling_window_and_hold_use_rendered_frames_including_startup() {
    // At 400 Hz each publication covers ten new frames.
    let mut meter = Measurement::new(400);
    let mut snapshots = vec![];
    meter.observe(&[Frame::new(1., 0.); 7], |_| panic!("incomplete bucket"));
    meter.observe(&[Frame::ZERO; 3], |v| snapshots.push(v));
    close(snapshots[0].rms[0], 0.7f32.sqrt());
    meter.observe(&[Frame::ZERO; 400], |v| snapshots.push(v));
    close(snapshots[3].rms[0], (7f32 / 40.).sqrt());
    assert_eq!(snapshots[4].peak[0], 0.);
    assert_eq!(snapshots[39].held[0], 1.);
    assert_eq!(snapshots[40].held[0], 0.);

    let data: Vec<_> = (0..11025)
        .map(|i| Frame::new((i % 11) as f32 * 0.1, (i % 17) as f32 * -0.03))
        .collect();
    let run = |size| {
        let mut meter = Measurement::new(44_100);
        let mut result = vec![];
        for block in data.chunks(size) {
            meter.observe(block, |v| result.push(v));
        }
        result
    };
    assert_eq!(run(1), run(480));
    assert_eq!(run(128), run(data.len()));
}

#[test]
fn effect_preserves_float_bits_and_resets_on_rate_and_subscription_changes() {
    let monitor = SignalMonitor::default();
    let (mut tap, ()) = monitor.clone().build();
    tap.init(400, 128);
    let info = MockInfoBuilder::new().build();
    let reader = monitor.subscribe();
    let nan = f32::from_bits(0x7fc0_1234);
    let mut input = [
        Frame::new(nan, 0.5),
        Frame::new(0.5, f32::NEG_INFINITY),
        Frame::new(-0.0, f32::INFINITY),
        Frame::new(-1.25, -0.5),
        Frame::ZERO,
        Frame::ZERO,
        Frame::ZERO,
        Frame::ZERO,
        Frame::ZERO,
        Frame::ZERO,
    ];
    let bits = |f: Frame| [f.left.to_bits(), f.right.to_bits()];
    let original = input.map(bits);
    let pcm = monitor.pcm();
    let _pcm = pcm.subscribe();
    tap.process(&mut input, 1. / 400., &info);
    assert_eq!(original, input.map(bits));
    let levels = reader.latest().unwrap();
    assert_eq!(levels.peak, [1.25, 0.5]);
    close(levels.rms[0], ((0.25f32 + 1.5625) / 10.).sqrt());
    close(levels.rms[1], (0.5f32 / 10.).sqrt());
    let other = monitor.subscribe();
    assert_eq!(other.latest(), Some(levels));
    drop(reader);
    tap.process(&mut input[..5], 1. / 400., &info);
    let old_pcm_epoch = pcm.epoch();
    tap.on_change_sample_rate(800);
    assert_ne!(pcm.epoch(), old_pcm_epoch);
    tap.process(&mut [Frame::ZERO; 19], 1. / 800., &info);
    assert_eq!(other.latest(), Some(Levels::default()));
    tap.process(&mut [Frame::new(0.5, 0.); 1], 1. / 800., &info);
    close(other.latest().unwrap().rms[0], (0.25f32 / 20.).sqrt());
    drop(other);
    tap.process(&mut input, 1. / 800., &info);
    assert_eq!(original, input.map(bits));
    let pcm_epoch = pcm.epoch();
    let reopened = monitor.subscribe();
    assert_eq!(
        pcm.epoch(),
        pcm_epoch,
        "opening Levels preserves the spectrum window"
    );
    assert_eq!(reopened.latest(), None);
    tap.process(&mut [Frame::ZERO; 20], 1. / 800., &info);
    assert_eq!(reopened.latest(), Some(Levels::default()));
    monitor.reset();
    assert_eq!(reopened.latest(), None);
}

#[test]
fn mixed_voices_are_measured_after_voice_gain_before_main_gain_and_clamp() {
    use kira::{
        AudioManager, AudioManagerSettings, Decibels,
        backend::mock::{MockBackend, MockBackendSettings},
        sound::static_sound::StaticSoundData,
        track::MainTrackBuilder,
    };
    let monitor = SignalMonitor::default();
    let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
        backend_settings: MockBackendSettings {
            sample_rate: 48_000,
        },
        main_track_builder: MainTrackBuilder::new()
            .volume(Decibels(-12.))
            .with_effect(monitor.clone()),
        ..Default::default()
    })
    .unwrap();
    let reader = monitor.subscribe();
    for (left, right, gain) in [(0.8, 0.25, 1.), (0.8, -0.25, 0.5)] {
        let data = StaticSoundData {
            sample_rate: 48_000,
            frames: vec![Frame::new(left, right); 14400].into(),
            settings: Default::default(),
            slice: None,
        }
        .volume(Decibels(dbfs(gain)));
        manager.play(data).unwrap();
    }
    for _ in 0..75 {
        manager.backend_mut().on_start_processing();
        manager.backend_mut().process();
    }
    let levels = reader.latest().unwrap();
    for values in [levels.rms, levels.peak] {
        close(values[0], 1.2);
        close(values[1], 0.125);
    }
}

#[test]
#[ignore = "release-mode tap timing, printed for performance qualification"]
fn time_observation() {
    use std::{hint::black_box, time::Instant};
    let monitor = SignalMonitor::default();
    let (mut tap, ()) = monitor.clone().build();
    tap.init(48_000, 128);
    let mut block =
        std::array::from_fn::<_, 128, _>(|i| Frame::new((i as f32).sin(), (i as f32).cos()));
    let info = MockInfoBuilder::new().build();
    let pcm = monitor.pcm();
    for (levels, capture) in [(false, false), (true, false), (false, true), (true, true)].repeat(2)
    {
        let _reader = levels.then(|| monitor.subscribe());
        let _demand = capture.then(|| pcm.subscribe());
        let mut nanos = 0;
        for _ in 0..2000 {
            let start = Instant::now();
            for _ in 0..250 {
                tap.process(black_box(&mut block), 1. / 48000., black_box(&info));
            }
            nanos += start.elapsed().as_nanos();
            pcm.drain(|block| {
                black_box(block);
            });
        }
        println!(
            "levels={levels} pcm={capture} mean {:.1} ns / 128 frames, tap {} B, shared {} B",
            nanos as f64 / 500_000.,
            size_of::<SignalTap>(),
            size_of::<Shared>()
        );
    }
}

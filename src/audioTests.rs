use super::*;
use crate::library::DiscTrack;
use crate::recording::disc_frame;
use kira::Frame;
use std::path::Path;

fn samples(i: usize) -> [i16; 2] {
    [
        ((i * 173 % 32749) as i16) - 16374,
        ((i * 101 % 26339) as i16) - 13169,
    ]
}
fn fixture(path: &Path, rate: u32, n: usize) {
    let len = (n * 4) as u32;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((len + 36).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 4).to_le_bytes());
    wav.extend(4u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(len.to_le_bytes());
    for i in 0..n {
        for v in samples(i) {
            wav.extend(v.to_le_bytes());
        }
    }
    std::fs::write(path, wav).unwrap();
}
fn read_all(dec: &mut Recording) -> Vec<Frame> {
    let mut frames = vec![];
    loop {
        let block = dec.decode().unwrap();
        if block.is_empty() {
            return frames;
        }
        frames.extend(block);
    }
}

#[test]
fn renderer_preserves_stereo_samples_across_a_disc_chapter() {
    use kira::backend::{Backend, Renderer};
    use std::time::Instant;

    struct Capture(Option<Renderer>);
    impl Backend for Capture {
        type Settings = ();
        type Error = ();
        fn setup(_: (), _: usize) -> std::result::Result<(Self, u32), ()> {
            Ok((Self(None), 48000))
        }
        fn start(&mut self, renderer: Renderer) -> std::result::Result<(), ()> {
            self.0 = Some(renderer);
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    fixture(&path, 48000, 9600);
    let sheet = dir.path().join("disc.cue");
    std::fs::write(
        &sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:00:08",
    )
    .unwrap();
    let tracks = crate::cue::read(&sheet).unwrap();
    let recording = Recording::open(&tracks[0], tracks[1].disc.as_ref().unwrap().end).unwrap();
    let mut output = AudioManager::<Capture>::new(Default::default()).unwrap();
    let voice = output
        .play(StreamingSoundData::from_decoder(recording))
        .unwrap();
    let mut frames = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && voice.state() != PlaybackState::Stopped {
        let renderer = output.backend_mut().0.as_mut().unwrap();
        let mut block = [0.; 256];
        renderer.on_start_processing();
        renderer.process(&mut block, 2);
        frames.extend_from_slice(block.as_chunks::<2>().0);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(voice.state(), PlaybackState::Stopped);
    let start = frames.iter().position(|s| s[0] != 0.).unwrap();
    let boundary = disc_frame(tracks[1].disc.as_ref().unwrap().start, 48000);
    for i in boundary - 512..boundary + 512 {
        assert_eq!(
            frames[start + i],
            samples(i).map(|s| f32::from(s) / 32768.),
            "rendered frame {i}"
        );
    }
}
#[test]
fn cue_seek_and_both_channels_match_original_samples() {
    let dir = tempfile::tempdir().unwrap();
    for rate in [44100, 48000, 96000] {
        let path = dir.path().join("disc.wav");
        fixture(&path, rate, rate as usize);
        let mut t = Track::file(path);
        t.disc = Some(DiscTrack {
            sheet: "disc.cue".into(),
            number: 2,
            start: 11,
            end: Some(47),
        });
        let mut d = Recording::open(&t, Some(47)).unwrap();
        let start = disc_frame(11, rate);
        for offset in [0, 1, 313, 1503, 0] {
            d.seek(offset).unwrap();
            let f = d.decode().unwrap()[0];
            let [l, r] = samples(start + offset);
            assert_eq!(
                f,
                Frame::new(l as f32 / 32768., r as f32 / 32768.),
                "rate={rate}, relative={offset}"
            );
        }
        d.seek(0).unwrap();
        assert_eq!(read_all(&mut d).len(), disc_frame(47, rate) - start);
    }
}
#[test]
fn unavailable_alternate_rip_is_not_a_second_disc() {
    let dir = tempfile::tempdir().unwrap();
    fixture(&dir.path().join("disc.wav"), 44100, 44100);
    for (name, source) in [("present", "disc.wav"), ("alternative", "disc.flac")] {
        std::fs::write(dir.path().join(format!("{name}.cue")),format!("FILE \"{source}\" WAVE\nTRACK 01 AUDIO\nTITLE First\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE Second\nINDEX 01 00:00:25")).unwrap();
    }
    let f = crate::library::browse(dir.path()).unwrap();
    assert_eq!(f.tracks.len(), 2);
    assert!(f.notices.is_empty());
    assert_eq!(f.tracks[1].title, "Second");
}

#[test]
fn paused_seek_updates_the_position_without_opening_output() {
    let mut engine = Engine::default();
    engine.status.duration = 10.;
    engine.command(Command::Seek(0.5)).unwrap();
    assert!((engine.status.position - 5.).abs() < 0.001);
    assert!(!engine.status.playing);
    assert!(engine.voice.is_none() && engine.device.is_none());
}

#[test]
fn cached_overview_is_reused_only_for_the_same_source_range() {
    use std::sync::atomic::AtomicU64;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    fixture(&path, 48000, 48000);
    let cache = Some(dir.path().join("cache"));
    let generation = AtomicU64::new(1);
    let mut track = Track::file(path);
    let bins = crate::overview::load(&track, cache.clone(), &generation, 1).unwrap();
    // A cancelled analysis can return already-persisted data, but cannot decode.
    let cached = crate::overview::load(&track, cache.clone(), &generation, 2).unwrap();
    assert_eq!(bins, cached);
    track.disc = Some(DiscTrack {
        sheet: "disc.cue".into(),
        number: 2,
        start: 1,
        end: None,
    });
    assert!(crate::overview::load(&track, cache, &generation, 2).is_none());
}
#[test]
#[ignore = "uses FFmpeg to encode the test recording"]
fn encoded_formats_open_decode_and_seek() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    fixture(&source, 48000, 48000);
    for (name, codec) in [
        ("lossless.flac", "flac"),
        ("lossless.aiff", "pcm_s16be"),
        ("apple.m4a", "alac"),
        ("lossy.mp3", "libmp3lame"),
        ("aac.m4a", "aac"),
        ("music.opus", "libopus"),
        ("vorbis.ogg", "libvorbis"),
    ] {
        let path = dir.path().join(name);
        assert!(
            std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&source)
                .args(["-c:a", codec])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let t = Track::file(path);
        let mut d = Recording::open(&t, None).unwrap();
        let start = std::time::Instant::now();
        let original = read_all(&mut d);
        assert_eq!(original.len(), 48000, "{name} playable sample count");
        assert!(
            original
                .iter()
                .any(|s| s.left.abs() > 0.1 && s.right.abs() > 0.1)
        );
        let mut d = Recording::open(&t, None).unwrap();
        d.seek(20000).unwrap();
        let seek = d.decode().unwrap();
        let error = seek
            .iter()
            .zip(&original[20000..])
            .map(|(a, b)| (a.left - b.left).abs().max((a.right - b.right).abs()))
            .fold(0f32, f32::max);
        println!(
            "{name}: {} frames, decode + reopen/seek {:?}, peak sample difference {error}",
            original.len(),
            start.elapsed()
        );
        if codec == "libopus" {
            let reference = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&t.source)
                .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
                .output()
                .unwrap();
            assert!(reference.status.success());
            assert_eq!(original.len() * 8, reference.stdout.len());
            let max = original
                .iter()
                .zip(reference.stdout.as_chunks::<8>().0)
                .map(|(f, x)| {
                    (f.left - f32::from_le_bytes(x[..4].try_into().unwrap()))
                        .abs()
                        .max((f.right - f32::from_le_bytes(x[4..].try_into().unwrap())).abs())
                })
                .fold(0f32, f32::max);
            assert!(max < 0.001, "Opus differs from reference by {max}");
            println!("Opus vs FFmpeg maximum absolute sample difference: {max}");
        }
        if ["flac", "pcm_s16be", "alac"].contains(&codec) {
            assert_eq!(error, 0., "{name}");
        }
    }
}

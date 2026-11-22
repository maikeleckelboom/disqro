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
pub(crate) fn fixture(path: &Path, rate: u32, n: usize) {
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
    let monitor = crate::signal::SignalMonitor::default();
    let reader = monitor.subscribe();
    let mut output = AudioManager::<Capture>::new(AudioManagerSettings {
        main_track_builder: MainTrackBuilder::new().with_effect(monitor),
        ..Default::default()
    })
    .unwrap();
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
    assert!(
        reader
            .latest()
            .unwrap()
            .held
            .iter()
            .all(|peak| *peak > 0.35)
    );
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

#[test]
fn zero_volume_is_digital_silence() {
    assert_eq!(super::db(0.).as_amplitude(), 0.);
    assert!((super::db(0.5).as_amplitude() - 0.5).abs() < 1e-6);
}

#[test]
#[ignore = "requires a Windows output device; exercises the actual CPAL stream"]
fn output_levels_follow_transport_and_contiguous_chapters() {
    use crate::levels::Levels;
    use crate::signal::LevelReader;
    fn until(e: &mut Engine, reader: &LevelReader, predicate: impl Fn(&Engine, Levels) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            e.tick().unwrap();
            if reader.latest().is_some_and(|levels| predicate(e, levels)) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out at {:?}, levels {:?}",
                e.status,
                reader.latest()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    fixture(&path, 48_000, 48_000 * 6);
    let sheet = dir.path().join("disc.cue");
    std::fs::write(
        &sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:00:45",
    )
    .unwrap();
    let chapters = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let mut e = Engine {
        volume: 0.125,
        ..Default::default()
    };
    let reader = e.signal.subscribe();
    e.command(Command::Play(chapters, 0)).unwrap();
    let audible = |_: &Engine, levels: Levels| levels.rms.iter().all(|v| *v > 0.015);
    let silent = |_: &Engine, levels: Levels| levels.peak == [0.; 2];
    until(&mut e, &reader, audible);
    let epoch = reader.epoch();
    until(&mut e, &reader, |e, levels| {
        e.chapter == 1 && audible(e, levels)
    });
    assert_eq!(reader.epoch(), epoch, "same decoder/voice across the join");
    e.command(Command::Volume(0.)).unwrap();
    until(&mut e, &reader, silent);
    e.command(Command::Volume(0.125)).unwrap();
    until(&mut e, &reader, audible);
    e.command(Command::SetPlaying(false)).unwrap();
    until(&mut e, &reader, silent);
    e.command(Command::SetPlaying(true)).unwrap();
    until(&mut e, &reader, audible);
    for fraction in [0.7, 0.1, 0.4, 0.2] {
        e.command(Command::Seek(fraction)).unwrap();
    }
    until(&mut e, &reader, audible);
    assert!(e.status.position >= 0.2 * e.status.duration - 0.01);
    e.command(Command::Stop).unwrap();
    until(&mut e, &reader, silent);

    let replacement = dir.path().join("replacement.wav");
    fixture(&replacement, 96_000, 96_000);
    let replacement = Arc::new(Track::file(replacement));
    e.command(Command::Play(vec![replacement.clone()].into(), 0))
        .unwrap();
    until(&mut e, &reader, audible);
    assert_eq!(e.status.track.as_ref(), Some(&replacement));
    assert_eq!(e.rate, 96_000);
}

#[test]
fn seek_bursts_coalesce_without_crossing_transport_actions() {
    use super::{Command, Commands, Shared};
    use std::sync::{Arc, Mutex};
    let shared = Arc::new(Mutex::new(Shared {
        status: Default::default(),
        queue: Default::default(),
    }));
    shared.lock().unwrap().status.entry = Some(7);
    let (tx, rx) = Commands::channel(shared, 3);
    for n in 0..1000 {
        tx.send(Command::SeekTo(f64::from(n))).unwrap();
    }
    tx.send(Command::Stop).unwrap();
    tx.send(Command::Seek(0.5)).unwrap();
    assert!(tx.send(Command::Step(1)).is_err());
    let take = || rx.try_recv().unwrap().lock().unwrap().take().unwrap();
    let seek = take();
    assert!(matches!(seek.command, Command::SeekTo(999.)));
    assert!(seek.applies_to(Some(7)));
    assert!(!seek.applies_to(Some(8)));
    assert!(matches!(take().command, Command::Stop));
    assert!(matches!(take().command, Command::Seek(0.5)));
    assert!(rx.try_recv().is_err());
}

/// Run against a local long recording or CUE. Only this qualification graph
/// silences the main fader, downstream of the production observation point.
#[test]
#[ignore = "ten-minute CPAL soak; set DISQRO_SOAK_SOURCE to a long audio file or CUE"]
fn sustained_output_observation() {
    use std::time::Instant;
    let source = std::path::PathBuf::from(
        std::env::var_os("DISQRO_SOAK_SOURCE").expect("set DISQRO_SOAK_SOURCE"),
    );
    let tracks = if source
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("cue"))
    {
        crate::cue::read(&source)
            .unwrap()
            .into_iter()
            .map(Arc::new)
            .collect()
    } else {
        vec![Arc::new(Track::file(source))].into()
    };
    let signal = crate::signal::SignalMonitor::default();
    let device = AudioManager::new(AudioManagerSettings {
        main_track_builder: MainTrackBuilder::new()
            .with_effect(signal.clone())
            .volume(Decibels::SILENCE),
        ..Default::default()
    })
    .unwrap();
    let reader = signal.subscribe();
    let mut engine = Engine {
        signal,
        device: Some(device),
        volume: 1.,
        ..Default::default()
    };
    crate::diagnostics::start();
    engine.command(Command::Play(tracks, 0)).unwrap();
    let epoch = reader.epoch();
    let mut audible = 0;
    let mut peak = [0f32; 2];
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(600) {
        std::thread::sleep(Duration::from_millis(40));
        engine.tick().unwrap();
        assert!(engine.status.playing, "the source must cover ten minutes");
        assert_eq!(
            reader.epoch(),
            epoch,
            "the source must be one continuous voice"
        );
        if let Some(levels) = reader.latest() {
            audible += usize::from(levels.rms.iter().all(|v| *v > 0.));
            for (maximum, level) in peak.iter_mut().zip(levels.peak) {
                *maximum = maximum.max(level);
            }
        }
    }
    assert!(audible > 0, "no stereo signal observed");
    println!(
        "Elapsed {:.3}s, non-silent reads {audible}, sample peaks {peak:?}, chapter {}",
        started.elapsed().as_secs_f64(),
        engine.chapter + 1
    );
}

#[test]
fn shutdown_receipt_includes_pending_queue_edits() {
    let transport = Transport::new(0.);
    let tracks = vec![Arc::new(Track::file("first.wav".into()))].into();
    transport.send(Command::Enqueue(tracks), None).unwrap();
    let saved = transport.sender().snapshot().unwrap().unwrap();
    assert_eq!(saved.queue.entries.len(), 1);
    assert_eq!(saved.queue.entries[0].track.title, "first");
}

#[test]
fn continuing_cd_reuses_active_decoder_range_and_entry_after_queue_edits() {
    let mut e = Engine::default();
    let program: crate::library::TrackList = (0..4)
        .map(|n| {
            let mut t = Track::file("disc.flac".into());
            t.disc = Some(DiscTrack {
                sheet: "disc.cue".into(),
                number: n + 1,
                start: u64::from(n) * 75,
                end: Some(u64::from(n + 1) * 75),
            });
            Arc::new(t)
        })
        .collect();
    e.queue.replace(&program, 1);
    let current = e.queue.current_entry().unwrap().id;
    e.select(Some(1), 0.);
    e.group = vec![Chapter {
        id: current,
        offset: 0.,
    }];
    e.rate = 48000;
    e.length = 3.;
    e.revalidate();
    e.update_position(0.42);
    e.status.playing = true;
    e.command(Command::ClearUpcoming).unwrap();
    assert_eq!((e.group.len(), e.limit), (1, 1.));
    e.command(Command::ContinueDisc(
        current,
        program.clone(),
        DiscContext::Inferred,
    ))
    .unwrap();
    assert_eq!((e.group.len(), e.limit), (3, 3.));
    assert_eq!(
        (e.status.entry, e.status.position, e.status.playing),
        (Some(current), 0.42, true)
    );
    let revision = e.queue.revision;
    e.command(Command::ContinueDisc(
        current,
        program.clone(),
        DiscContext::Inferred,
    ))
    .unwrap();
    assert_eq!(e.queue.revision, revision);
    e.update_position(1.125);
    assert_eq!(e.queue.current, Some(2));
    assert_eq!(e.status.position, 0.125);
    e.command(Command::ContinueDisc(
        current,
        program,
        DiscContext::Inferred,
    ))
    .unwrap();
    assert_eq!(e.queue.current, Some(2));
}

#[test]
fn a_release_sequence_opens_each_disc_and_ends_with_the_release() {
    let chapter = |disc: &str, n: u8, start: u64, end: Option<u64>| {
        let mut t = Track::file(format!("{disc}.flac").into());
        t.disc = Some(DiscTrack {
            sheet: format!("{disc}.cue").into(),
            number: n,
            start,
            end,
        });
        Arc::new(t)
    };
    // CD 1 ends where its image ends; CD 2 has a recorded lead-in.
    let release: crate::library::TrackList = [
        chapter("cd1", 1, 0, Some(75)),
        chapter("cd1", 2, 75, None),
        chapter("cd2", 0, 0, Some(33)),
        chapter("cd2", 1, 33, Some(150)),
        chapter("cd2", 2, 150, None),
    ]
    .into();
    // Chapters of one disc share a decoder; the next disc opens its own.
    assert!(contiguous(&release[0], &release[1]));
    assert!(!contiguous(&release[1], &release[2]));
    assert!(contiguous(&release[2], &release[3]));
    let mut q = Queue::default();
    q.replace(&release, 0);
    q.set_current(Some(1));
    assert_eq!(q.next_index(true), Some(2), "CD 2 follows CD 1");
    q.set_current(Some(4));
    assert_eq!(q.next_index(true), None, "nothing follows the release");
    let ids: std::collections::HashSet<_> = q.entries.iter().map(|e| e.id).collect();
    assert_eq!(
        ids.len(),
        release.len(),
        "each chapter is its own occurrence"
    );
}

#[test]
fn disc_seek_is_atomic_and_rejects_changed_queue_or_recording() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.wav");
    fixture(&path, 32000, 32000);
    let sheet = dir.path().join("set.cue");
    std::fs::write(&sheet,"FILE source.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:00:01").unwrap();
    let tracks: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let mut e = Engine::default();
    e.queue.replace(&tracks, 0);
    e.select(Some(0), 0.);
    let origin = e.status.entry;
    let target = e.queue.entries[1].id;
    let before = e.queue.revision;
    let stamp = crate::cache::Stamp::read(&path).unwrap();
    let seek = |revision| Command::SeekOnDisc {
        target,
        revision,
        seconds: 0.125,
        duration: (32000 - 427) as f64 / 32000.,
        source_revision: stamp.clone(),
        sheet_revision: crate::cache::Stamp::read(&sheet),
    };
    assert!(
        !Pending {
            entry: origin,
            command: seek(before)
        }
        .applies_to(Some(target))
    );
    e.command(seek(before + 1)).unwrap();
    assert_eq!(e.status.entry, origin);
    e.command(seek(before)).unwrap();
    assert_eq!(
        (e.status.entry, e.status.position, e.status.playing),
        (Some(target), 0.125, false)
    );
    assert_eq!(e.queue.entries.len(), 2);
    assert!(e.device.is_none() && e.voice.is_none());
    e.select(Some(0), 0.);
    fixture(&path, 32000, 32001);
    e.command(seek(e.queue.revision)).unwrap();
    assert_eq!(e.status.entry, origin);
}

#[test]
fn restoring_a_final_chapter_without_tag_duration_keeps_saved_position() {
    let mut e = Engine::default();
    let mut q = Queue::default();
    let track = Arc::new(Track::file("unknown-length.wav".into()));
    q.replace(&[track], 0);
    e.command(Command::Restore(Box::new(q), 12.25)).unwrap();
    assert_eq!(e.status.position, 12.25);
    assert!(!e.status.playing && e.voice.is_none());
}

#[test]
fn queue_heading_tracks_pause_finish_stop_and_restoration() {
    let mut s = Status {
        duration: 5.,
        playing: true,
        ..Default::default()
    };
    assert_eq!(s.playback_state(), TransportState::Playing);
    s.playing = false;
    s.source_revision = Some(crate::cache::Stamp {
        bytes: 20,
        modified: 1,
    });
    assert_eq!(s.playback_state(), TransportState::Paused);
    s.source_revision = None;
    s.position = 2.;
    assert_eq!(s.playback_state(), TransportState::Paused);
    s.position = s.duration;
    assert_eq!(s.playback_state(), TransportState::Finished);
    s.position = 0.;
    assert_eq!(s.playback_state(), TransportState::Stopped);
}

use super::*;
use crate::library::DiscTrack;
use crate::recording::disc_frame;
use kira::Frame;
use kira::backend::{Backend, Renderer};
use std::path::Path;
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

fn render(output: &mut AudioManager<Capture>, block: &mut [f32]) {
    let renderer = output.backend_mut().0.as_mut().unwrap();
    renderer.on_start_processing();
    renderer.process(block, 2);
}

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
fn scrub_renders_decoded_pcm_in_both_directions_and_reaches_silence() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("scrub.wav");
    fixture(&source, 48_000, 48_000);
    let track = Arc::new(Track::file(source));
    let program = crate::timeline::Program::resolve(&track, DiscContext::default(), None).unwrap();
    let segment = &program.segments[0];
    let output_source = segment.source.clone();
    let control: Arc<scrub::Control> = Arc::default();
    let gesture = scrub::Gesture::new(
        control.clone(),
        WaveSeekAnchor {
            identity: PlaybackIdentity {
                entry: 1,
                occurrence: 1,
                seek: 0,
                voice: None,
                queue: 1,
            },
            source: output_source,
            playable: segment.frames.clone(),
            sheet: None,
        },
        16_020.,
        false,
        segment.frames.clone(),
    );
    let mut device = AudioManager::<Capture>::new(AudioManagerSettings::default()).unwrap();
    let mut sound = scrub::Output::install(&mut device, control).unwrap();
    sound.start(&gesture, 1.);
    let first = 16_020;
    let last = first + 750; // crosses a separately decoded PCM page
    let mut read = || {
        let mut block = [0.; 256];
        render(&mut device, &mut block);
        *block.as_chunks::<2>().0.as_array::<128>().unwrap()
    };
    let mut heard = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        gesture.move_to(last as f64, first as f64, 48_000., Instant::now());
        let block = read();
        if let Some(first_sample) = block.iter().position(|frame| *frame != [0.; 2]) {
            heard.extend_from_slice(&block[first_sample..]);
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!heard.is_empty(), "decoded samples never reached the mixer");
    for _ in 0..8 {
        heard.extend(read());
    }
    let expected = |frame| samples(frame).map(|sample| f32::from(sample) / 32768.);
    for (offset, &actual) in heard[160..700].iter().enumerate() {
        let i = 160 + offset;
        assert_eq!(actual, expected(first + i), "forward source frame {i}");
    }
    assert!(heard[910..].iter().all(|frame| *frame == [0.; 2]));

    gesture.move_to(first as f64, last as f64, -48_000., Instant::now());
    let reversed: Vec<_> = (0..8).flat_map(|_| read()).collect();
    for (offset, &actual) in reversed[160..700].iter().enumerate() {
        let i = 160 + offset;
        assert_eq!(actual, expected(last - i), "reverse source frame {i}");
    }

    gesture.move_to(first as f64 + 300.5, first as f64, 24_000., Instant::now());
    let slowed: Vec<_> = (0..7).flat_map(|_| read()).collect();
    for (offset, &actual) in slowed[160..580].iter().enumerate() {
        let i = 160 + offset;
        let a = expected(first + i / 2);
        let b = expected(first + i / 2 + 1);
        let want = if i % 2 == 0 {
            a
        } else {
            [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
        };
        assert_eq!(actual, want, "interpolated source frame {i}");
    }
    assert!(read().iter().all(|frame| *frame == [0.; 2]));
}

#[test]
fn scrub_keeps_queue_occurrences_and_listening_separate_until_release() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    let sheet = dir.path().join("disc.cue");
    fixture(&path, 48_000, 144_000);
    std::fs::write(
        &sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:01:00",
    )
    .unwrap();
    let tracks: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let program =
        crate::timeline::Program::resolve(&tracks[0], DiscContext::Selected, None).unwrap();
    let database = dir.path().join("library.sqlite3");
    let (writer, _) = listening::Writer::spawn(database.clone());
    let mut engine = Engine {
        listening: Some(writer.clone()),
        ..Engine::default()
    };
    let unrelated = Arc::new(Track::file(dir.path().join("another.wav")));
    engine
        .queue
        .replace(tracks.iter().cloned().chain([unrelated]).collect(), 0);
    engine.select(Some(0), 0.5);
    engine.refresh_flags();
    let make_gesture = |engine: &Engine, index: usize, origin: f64| {
        let segment = &program.segments[index];
        let (_, bounds) = scrub_run(&engine.queue, &segment.source).unwrap();
        Arc::new(scrub::Gesture::new(
            engine.scrub_control.clone(),
            WaveSeekAnchor {
                identity: engine.status.playback_identity().unwrap(),
                source: segment.source.clone(),
                playable: segment.frames.clone(),
                sheet: program.sheet.clone(),
            },
            origin,
            false,
            bounds,
        ))
    };
    let gesture = make_gesture(&engine, 0, 24_000.);
    assert_eq!(gesture.bounds, 0..144_000);
    engine.scrubbing = Some(ActiveScrub {
        gesture: gesture.clone(),
        updated: Instant::now() - Duration::from_millis(33),
    });
    engine.group = vec![
        Chapter {
            id: engine.queue.entries[0].id,
            offset: 0.,
        },
        Chapter {
            id: engine.queue.entries[1].id,
            offset: 1.,
        },
    ];
    engine.limit = 3.;
    engine.pause_checkpoint = true;
    engine.voice = Some(OutputVoice {
        state: Arc::new(OutputState {
            cursor: AtomicUsize::new(48_010),
            state: AtomicU8::new(PlaybackState::Paused as u8),
            ..Default::default()
        }),
        rate: 48_000.,
        clock: Arc::new(clock::Clock::new(0)),
        #[cfg(all(windows, feature = "qualification"))]
        trace: None,
    });
    engine.command(Command::Volume(0.5)).unwrap();
    engine.tick().unwrap();
    assert_eq!(engine.queue.current, Some(0));
    assert!(!engine.pause_checkpoint && !gesture.closed());

    for (from, to) in [(24_000., 120_000.), (120_000., 15_000.), (15_000., 72_000.)] {
        gesture.move_to(to, from, (to - from) * 50., Instant::now());
        engine.update_scrub().unwrap();
        assert_eq!(engine.queue.current, Some(0));
        assert!(engine.occurrence.is_none());
    }
    gesture.finish(scrub::End::Release);
    engine.update_scrub().unwrap();
    assert!(gesture.closed());
    assert_eq!(engine.queue.current, Some(1));
    assert_eq!(engine.status.position, 0.5);
    assert!(!engine.status.playing && engine.voice.is_none());
    assert_eq!(engine.status.playback_state(), TransportState::Paused);
    engine.refresh_flags();

    let reverse = make_gesture(&engine, 1, 72_000.);
    reverse.move_to(12_000., 72_000., -60_000., Instant::now());
    reverse.finish(scrub::End::Release);
    engine
        .command(Command::BeginWaveScrub(reverse.clone()))
        .unwrap();
    assert!(reverse.closed());
    assert_eq!(engine.queue.current, Some(0));
    assert_eq!(engine.status.position, 0.25);
    engine.refresh_flags();

    let stale = make_gesture(&engine, 0, 12_000.);
    stale.move_to(72_000., 12_000., 60_000., Instant::now());
    stale.finish(scrub::End::Release);
    engine.command(Command::Repeat(Repeat::One)).unwrap();
    engine.refresh_flags();
    assert_eq!(
        scrub_run(&engine.queue, &stale.anchor.source).unwrap().1,
        0..48_000
    );
    engine
        .command(Command::BeginWaveScrub(stale.clone()))
        .unwrap();
    assert!(stale.closed());
    assert_eq!(engine.queue.current, Some(0));
    assert_eq!(engine.status.position, 0.25);

    let canceled = make_gesture(&engine, 0, 12_000.);
    canceled.move_to(24_000., 12_000., 60_000., Instant::now());
    engine.scrubbing = Some(ActiveScrub {
        gesture: canceled.clone(),
        updated: Instant::now(),
    });
    engine.command(Command::ClearUpcoming).unwrap();
    assert!(canceled.closed());
    assert_eq!(engine.status.position, 0.25);
    engine.refresh_flags();

    // Escape from a scrub entered after natural completion must preserve EOF
    // and the identity that makes the next Play a new occurrence.
    engine.status.duration = 1.;
    engine.status.position = 1.;
    let finished_identity = engine.status.playback_identity();
    let finished = make_gesture(&engine, 0, 47_999.);
    finished.move_to(24_000., 47_999., -60_000., Instant::now());
    engine.scrubbing = Some(ActiveScrub {
        gesture: finished.clone(),
        updated: Instant::now(),
    });
    finished.finish(scrub::End::Cancel);
    engine.update_scrub().unwrap();
    assert!(finished.closed());
    assert_eq!(engine.status.position, 1.);
    assert_eq!(engine.status.playback_state(), TransportState::Finished);
    assert_eq!(engine.status.playback_identity(), finished_identity);
    engine.status.position = 0.25;

    let old_sheet = make_gesture(&engine, 0, 12_000.);
    old_sheet.move_to(24_000., 12_000., 60_000., Instant::now());
    old_sheet.finish(scrub::End::Release);
    let changed_sheet = std::fs::read_to_string(&sheet).unwrap() + "\nREM revised\n";
    std::fs::write(&sheet, changed_sheet).unwrap();
    engine
        .command(Command::BeginWaveScrub(old_sheet.clone()))
        .unwrap();
    assert!(old_sheet.closed());
    assert_eq!(engine.status.position, 0.25);

    let interrupted = make_gesture(&engine, 0, 12_000.);
    engine.scrubbing = Some(ActiveScrub {
        gesture: interrupted.clone(),
        updated: Instant::now(),
    });
    let error = kira::backend::cpal::cpal::Error::new(
        kira::backend::cpal::cpal::ErrorKind::DeviceNotAvailable,
    );
    assert!(engine.output_error(error).is_err());
    assert!(interrupted.closed());
    assert!(!engine.status.playing);

    writer.barrier().unwrap();
    let history = crate::store::Store::open(&database)
        .unwrap()
        .history_page((i64::MIN, i64::MAX), "", true, None)
        .unwrap();
    assert!(history.is_empty());
}

#[test]
fn full_cd_scrub_reaches_unqueued_chapters_and_reconciles_only_on_release() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    let sheet = dir.path().join("disc.cue");
    fixture(&path, 48_000, 144_000);
    std::fs::write(
        &sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:01:00",
    )
    .unwrap();
    let tracks: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let program =
        crate::timeline::Program::resolve(&tracks[0], DiscContext::Selected, None).unwrap();
    let database = dir.path().join("library.sqlite3");
    let (writer, _) = listening::Writer::spawn(database.clone());
    let mut engine = Engine {
        listening: Some(writer.clone()),
        ..Engine::default()
    };
    // Play Only This Track, then something unrelated was queued after it.
    let unrelated = Arc::new(Track::file(dir.path().join("another.wav")));
    engine
        .queue
        .replace([tracks[0].clone(), unrelated].into_iter().collect(), 0);
    let ids: Vec<u64> = engine.queue.entries.iter().map(|e| e.id).collect();
    engine.select(Some(0), 0.5);
    engine.refresh_flags();
    let segment = &program.segments[0];
    assert_eq!(
        scrub_run(&engine.queue, &segment.source).unwrap().1,
        0..48_000
    );
    let (chapters, bounds) = program_run(&tracks, &segment.source, &segment.frames).unwrap();
    assert_eq!((chapters, bounds.clone()), (0..2, 0..144_000));
    let control = engine.scrub_control.clone();
    let identity = engine.status.playback_identity().unwrap();
    let gesture = |origin: f64| {
        Arc::new(
            scrub::Gesture::new(
                control.clone(),
                WaveSeekAnchor {
                    identity,
                    source: segment.source.clone(),
                    playable: segment.frames.clone(),
                    sheet: program.sheet.clone(),
                },
                origin,
                false,
                bounds.clone(),
            )
            .within_program(tracks.clone()),
        )
    };

    // Escape after crossing into the unqueued chapter changes nothing.
    let canceled = gesture(24_000.);
    canceled.move_to(120_000., 24_000., 96_000. * 50., Instant::now());
    let revision = engine.queue.revision;
    engine
        .command(Command::BeginWaveScrub(canceled.clone()))
        .unwrap();
    assert!(!canceled.closed());
    canceled.finish(scrub::End::Cancel);
    engine.update_scrub().unwrap();
    assert!(canceled.closed());
    assert_eq!(engine.queue.revision, revision);
    assert_eq!(
        (engine.queue.current, engine.status.position),
        (Some(0), 0.5)
    );

    // Crossing both ways while held keeps the original occurrence.
    let released = gesture(24_000.);
    engine
        .command(Command::BeginWaveScrub(released.clone()))
        .unwrap();
    for (from, to) in [
        (24_000., 130_000.),
        (130_000., 10_000.),
        (10_000., 120_000.),
    ] {
        released.move_to(to, from, (to - from) * 50., Instant::now());
        engine.update_scrub().unwrap();
        assert_eq!(engine.queue.current, Some(0));
        assert_eq!(engine.queue.entries.len(), 2);
        assert!(engine.occurrence.is_none());
    }
    released.finish(scrub::End::Release);
    engine.update_scrub().unwrap();
    assert!(released.closed());
    // One new occurrence after the active entry; the unrelated entry keeps
    // its identity and place after it.
    assert_eq!(engine.queue.entries.len(), 3);
    assert_eq!(engine.queue.current, Some(1));
    assert_eq!(engine.queue.entries[0].id, ids[0]);
    assert!(!ids.contains(&engine.queue.entries[1].id));
    assert!(engine.queue.entries[1].track.same_item(&tracks[1]));
    assert_eq!(engine.queue.entries[2].id, ids[1]);
    assert_eq!(engine.status.position, 1.5);
    assert!(!engine.status.playing && engine.voice.is_none());

    writer.barrier().unwrap();
    let history = crate::store::Store::open(&database)
        .unwrap()
        .history_page((i64::MIN, i64::MAX), "", true, None)
        .unwrap();
    assert!(history.is_empty(), "the audition earned listening credit");
}

#[test]
fn renderer_preserves_stereo_samples_across_a_disc_chapter() {
    check_chapter_render(false);
}

#[test]
fn qualification_tap_reads_full_level_while_every_output_sample_is_zero() {
    check_chapter_render(true);
}

// The scheduler owns this decoder until it has queued the final source
// frame. Packet decode counters alone can run ahead of the playback ring.
impl Drop for ObservedDecoder {
    fn drop(&mut self) {
        let (done, signal) = &self.state.decoder_finished;
        *done.lock().unwrap() = true;
        signal.notify_one();
    }
}

fn await_queued_fixture(voice: &OutputVoice, expected_frames: usize) {
    let (done, signal) = &voice.state.decoder_finished;
    let (done, _) = signal
        .wait_timeout_while(done.lock().unwrap(), Duration::from_secs(3), |done| !*done)
        .unwrap();
    assert!(*done, "decoder did not finish queuing the fixture");
    assert_eq!(voice.state.decoded.load(Ordering::Acquire), expected_frames);
    assert!(voice.pop_error().is_none());
}

fn check_chapter_render(silent: bool) {
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
        main_track_builder: output_track(monitor, silent),
        ..Default::default()
    })
    .unwrap();
    let voice = output
        .play(ObservedData {
            recording,
            start: 0,
            volume: 1.,
            fade_in: None,
            progress: None,
        })
        .unwrap();
    // The 9,600-frame disc fits in Kira's 16,384-frame stream ring. This
    // offline backend can consume faster than real time, so begin the sample
    // comparison only once the scheduler has queued the whole fixture.
    await_queued_fixture(&voice, 9600);
    let mut frames = Vec::new();
    let mut block = [0.; 256];
    // Allow one extra block to drain the interpolator's history frame.
    for _ in 0..=9600_usize.div_ceil(block.len() / 2) {
        if voice.state() == PlaybackState::Stopped {
            break;
        }
        render(&mut output, &mut block);
        frames.extend_from_slice(block.as_chunks::<2>().0);
    }
    assert_eq!(voice.state(), PlaybackState::Stopped);
    let presented = voice
        .clock
        .position(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(presented.frame, 9600.0);
    assert!(!presented.advancing);
    assert!(
        reader
            .latest()
            .unwrap()
            .held
            .iter()
            .all(|peak| *peak > 0.35)
    );
    if silent {
        assert!(frames.iter().all(|s| *s == [0., 0.]));
        return;
    }
    let start = frames.iter().position(|s| s[0] != 0.).unwrap();
    let boundary = disc_frame(tracks[1].disc.as_ref().unwrap().start, 48000);
    assert_eq!(boundary, 5120);
    for i in 0..9600 {
        assert_eq!(
            frames[start + i],
            samples(i).map(|s| f32::from(s) / 32768.),
            "rendered frame {i} (chapter boundary {boundary})"
        );
    }
}

#[test]
fn renderer_credits_a_drained_tail_and_never_the_truncated_remainder() {
    let dir = tempfile::tempdir().unwrap();
    for (truncated, failed_before_eof) in [(false, false), (true, false), (false, true)] {
        let path = dir.path().join("source.wav");
        let available = 30_041;
        let expected = if truncated { 96_000 } else { available };
        fixture(&path, 48000, expected);
        if truncated {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len((44 + available * 4) as u64)
                .unwrap();
        }
        let mut track = Track::file(path);
        track.duration = Some(40.); // Display metadata does not control credit.
        let recording = Recording::open(&track, None).unwrap();
        assert_eq!(recording.num_frames(), expected);
        let rate = f64::from(recording.sample_rate());
        let (publish, mut progress) = rtrb::RingBuffer::new(8192);
        let mut output = AudioManager::<Capture>::new(AudioManagerSettings::default()).unwrap();
        let voice = output
            .play(ObservedData {
                recording,
                start: 0,
                volume: 1.,
                fade_in: None,
                progress: Some(publish),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        if failed_before_eof {
            // Exercise a terminal state with a completed decoded extent
            // and an earlier error. The stopped state cannot prove a drain.
            voice.state.decode_failed.store(true, Ordering::Release);
        }
        let mut consumed = 0;
        while voice.state() != PlaybackState::Stopped {
            render(&mut output, &mut [0.; 256]);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
            while let Ok(block) = progress.pop() {
                assert_eq!(block.from, consumed);
                consumed = block.to;
            }
        }
        let actual = position_us(available as f64 / rate);
        assert_eq!(voice.pop_error().is_some(), truncated);
        if truncated {
            assert!(consumed > 0 && consumed <= actual);
        } else if failed_before_eof {
            assert!(consumed > 0 && consumed < actual);
        } else {
            assert_eq!(consumed, actual);
        }
        assert_eq!(position_us(voice.position()), consumed);
    }
}

fn delayed_transport_fixture(
    path: &std::path::Path,
    start: i64,
    capacity: usize,
) -> (
    Engine,
    listening::Writer,
    rtrb::Producer<RenderProgress>,
    Arc<OutputState>,
) {
    let writer = listening::Writer::spawn(path.join("library.sqlite3")).0;
    let source = path.join("source.wav");
    fixture(&source, 8000, 8000);
    let recording = listening::Recording {
        track: Arc::new(Track::file(source.clone())),
        source: crate::cache::Stamp::read(&source),
        sheet: None,
        source_object: None,
        sheet_object: None,
    };
    let state = Arc::new(OutputState::default());
    let (publish, progress) = rtrb::RingBuffer::new(capacity);
    let engine = Engine {
        listening: Some(writer.clone()),
        limit: 1.,
        voice: Some(OutputVoice {
            rate: 1_000_000.,
            state: state.clone(),
            clock: Arc::new(clock::Clock::new(0)),
            #[cfg(all(windows, feature = "qualification"))]
            trace: None,
        }),
        credit: Some(VoiceCredit {
            progress,
            cursor: 0,
            activity_at: start,
            segments: vec![CreditSegment {
                id: 1,
                start: 0,
                recording,
            }],
        }),
        ..Default::default()
    };
    (engine, writer, publish, state)
}

fn assert_delayed_credit(path: &std::path::Path, expected: &[(i64, i64)]) {
    let db = rusqlite::Connection::open(path.join("library.sqlite3")).unwrap();
    let mut stmt = db
        .prepare("SELECT minute,credited_us FROM listening_minutes ORDER BY minute")
        .unwrap();
    let bins: Vec<_> = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(bins, expected);
    let total: (i64, i64) = db
        .query_row(
            "SELECT count(*),sum(credited_us) FROM listening_occurrences",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(total, (1, expected.iter().map(|(_, us)| us).sum()));
}

#[test]
fn delayed_transport_credits_both_blocks_before_midnight() {
    let dir = tempfile::tempdir().unwrap();
    let midnight = listening::midnight(
        &chrono::Local,
        chrono::NaiveDate::from_ymd_opt(2026, 11, 24).unwrap(),
    );
    let (mut engine, writer, mut publish, state) =
        delayed_transport_fixture(dir.path(), midnight - 20_000, 4);
    state.publish_cursor(
        10_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 0,
                to: 10_000,
                at: midnight - 20_000,
            },
        )),
        || {},
    );
    assert_eq!(engine.credit_output(), 0.01);
    state.publish_cursor(
        20_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 10_000,
                to: 20_000,
                at: midnight - 10_000,
            },
        )),
        || {
            // The renderer has queued the second timestamp, but has yet to
            // commit its cursor. Neither an observation nor a regression may
            // move the listening timeline ahead of the first block.
            assert_eq!(engine.credit_output(), 0.01);
            assert_eq!(engine.credit_output(), 0.01);
            engine.credit_progress(9_000, midnight);
            assert_eq!(
                engine.credit.as_ref().unwrap().activity_at,
                midnight - 10_000
            );
        },
    );
    assert_eq!(engine.credit_output(), 0.02);
    assert_eq!(engine.credit_output(), 0.02);
    engine.end_voice(End::Stopped);
    writer.barrier().unwrap();
    assert_delayed_credit(dir.path(), &[(midnight - listening::MINUTE_US, 20_000)]);
}

#[test]
fn delayed_transport_estimates_only_missing_committed_progress_after_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let boundary = 30 * listening::MINUTE_US;
    let (mut engine, writer, mut publish, state) =
        delayed_transport_fixture(dir.path(), boundary - 30_000, 1);
    state.publish_cursor(
        10_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 0,
                to: 10_000,
                at: boundary - 30_000,
            },
        )),
        || {},
    );
    state.publish_cursor(
        20_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 10_000,
                to: 20_000,
                at: boundary - 18_000,
            },
        )),
        || assert_eq!(engine.credit_output(), 0.01),
    );
    state.publish_cursor(
        30_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 20_000,
                to: 30_000,
                at: boundary - 8_000,
            },
        )),
        || {},
    );
    // Fill the interior gap against the next exact block. That block crosses
    // the minute, and the trailing overflow extends its accounted edge.
    state.publish_cursor(
        40_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 30_000,
                to: 40_000,
                at: boundary + 20_000,
            },
        )),
        || {},
    );
    assert_eq!(engine.credit_output(), 0.04);
    assert_eq!(engine.credit_output(), 0.04);
    state.publish_cursor(
        50_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 40_000,
                to: 50_000,
                at: boundary + listening::MINUTE_US,
            },
        )),
        || assert_eq!(engine.credit_output(), 0.04),
    );
    assert_eq!(engine.credit_output(), 0.05);
    engine.end_voice(End::Stopped);
    writer.barrier().unwrap();
    assert_delayed_credit(
        dir.path(),
        &[
            (boundary - listening::MINUTE_US, 28_000),
            (boundary, 12_000),
            (boundary + listening::MINUTE_US, 10_000),
        ],
    );
}

#[test]
fn delayed_transport_keeps_render_timestamps_when_a_cue_limit_is_restored() {
    let dir = tempfile::tempdir().unwrap();
    let boundary = 30 * listening::MINUTE_US;
    let (mut engine, writer, mut publish, state) =
        delayed_transport_fixture(dir.path(), boundary - 100_000, 2);
    state.publish_cursor(
        20_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 0,
                to: 20_000,
                at: boundary - 100_000,
            },
        )),
        || {},
    );
    state.publish_cursor(
        30_000,
        Some((
            &mut publish,
            RenderProgress {
                from: 20_000,
                to: 30_000,
                at: boundary + 10_000,
            },
        )),
        || {},
    );
    engine.limit = 0.01;
    assert_eq!(engine.credit_output(), 0.03);
    assert_eq!(engine.credit_output(), 0.03);
    assert_eq!(engine.credit.as_ref().unwrap().cursor, 10_000);
    engine.limit = 0.03;
    assert_eq!(engine.credit_output(), 0.03);
    engine.end_voice(End::Stopped);
    writer.barrier().unwrap();
    assert_delayed_credit(
        dir.path(),
        &[
            (boundary - listening::MINUTE_US, 20_000),
            (boundary, 10_000),
        ],
    );
}

#[test]
fn cue_queue_edit_uses_the_same_completed_block_for_the_playhead_and_credit() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    let writer = listening::Writer::spawn(db.clone()).0;
    let source = dir.path().join("disc.wav");
    fixture(&source, 8000, 48000);
    let sheet = dir.path().join("disc.cue");
    std::fs::write(&sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nTITLE First\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE Second\nINDEX 01 00:02:00\nTRACK 03 AUDIO\nTITLE Third\nINDEX 01 00:04:00"
    ).unwrap();
    let tracks: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let mut queue = Queue::default();
    queue.replace(tracks.into(), 0);
    let group: Vec<_> = queue
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| Chapter {
            id: entry.id,
            offset: (i * 2) as f64,
        })
        .collect();
    let segments = group
        .iter()
        .zip(queue.entries.iter())
        .map(|(chapter, entry)| CreditSegment {
            id: chapter.id,
            start: position_us(chapter.offset),
            recording: listening::Recording {
                track: entry.track.clone(),
                source: crate::cache::Stamp::read(&source),
                sheet: crate::cache::Stamp::read(&sheet),
                source_object: None,
                sheet_object: None,
            },
        })
        .collect();
    let (mut publish, progress) = rtrb::RingBuffer::new(4);
    let state = Arc::new(OutputState {
        cursor: AtomicUsize::new(16_800),
        ..Default::default()
    });
    let at = listening::timestamp();
    publish
        .push(RenderProgress {
            from: 0,
            to: 2_100_000,
            at,
        })
        .ok()
        .unwrap();
    let mut engine = Engine {
        queue,
        group,
        listening: Some(writer.clone()),
        limit: 6.,
        length: 6.,
        rate: 8000,
        voice: Some(OutputVoice {
            state: state.clone(),
            rate: 8000.,
            clock: Arc::new(clock::Clock::new(0)),
            #[cfg(all(windows, feature = "qualification"))]
            trace: None,
        }),
        credit: Some(VoiceCredit {
            progress,
            cursor: 0,
            activity_at: i64::MIN,
            segments,
        }),
        ..Default::default()
    };
    engine.command(Command::ClearUpcoming).unwrap();
    assert_eq!(engine.queue.current, Some(1));
    assert_eq!(engine.limit, 4.);
    assert_eq!(engine.group.len(), 2);
    publish
        .push(RenderProgress {
            from: 2_100_000,
            to: 4_100_000,
            at: at + 2_100_000,
        })
        .ok()
        .unwrap();
    state.cursor.store(32_800, Ordering::Release);
    engine.end_voice(End::Stopped);
    writer.barrier().unwrap();
    let rows = crate::store::Store::open(&db)
        .unwrap()
        .history_page((i64::MIN, i64::MAX), "", true, None)
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.title.as_str(), r.credited_us))
            .collect::<Vec<_>>(),
        [("First", 2_000_000), ("Second", 2_000_000)]
    );
}

#[test]
fn listening_voice_lifecycle_uses_output_cursor_and_splits_cue_boundaries() {
    use crate::{cache::Stamp, store::listening::HistoryRow};
    const SECOND: i64 = 1_000_000;
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("library.sqlite3");
    let (writer, _) = listening::Writer::spawn(database.clone());
    let path = dir.path().join("disc.wav");
    fixture(&path, 8000, 8000 * 45);
    let sheet = dir.path().join("disc.cue");
    std::fs::write(&sheet,"FILE disc.wav WAVE\nTRACK 01 AUDIO\nTITLE First\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE Second\nINDEX 01 00:35:00").unwrap();
    let tracks: Vec<_> = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let wrap = |i: usize| listening::Recording {
        track: tracks[i].clone(),
        source: Stamp::read(&path),
        sheet: Stamp::read(&sheet),
        source_object: None,
        sheet_object: None,
    };
    let voice = |start| VoiceCredit {
        progress: rtrb::RingBuffer::new(1).1,
        cursor: start,
        activity_at: i64::MIN,
        segments: vec![
            CreditSegment {
                id: 1,
                start: 0,
                recording: wrap(0),
            },
            CreditSegment {
                id: 2,
                start: 35 * SECOND,
                recording: wrap(1),
            },
        ],
    };
    let mut engine = Engine {
        listening: Some(writer.clone()),
        limit: 45.,
        ..Default::default()
    };
    let at = 1_800_000_000 * SECOND;
    let rows = || {
        writer.barrier().unwrap();
        crate::store::Store::open(&database)
            .unwrap()
            .history_page((i64::MIN, i64::MAX), "", true, None)
            .unwrap()
    };
    engine.credit = Some(voice(0));
    engine.credit_progress(0, at);
    assert!(rows().is_empty());
    engine.credit_progress(10 * SECOND, at + 10 * SECOND);
    assert_eq!(rows()[0].credited_us, 10 * SECOND);
    assert!(!rows()[0].qualified);
    // Forward seek, paused interval, then an ordinary seek-to-zero.
    engine.credit = Some(voice(20 * SECOND));
    engine.credit_progress(30 * SECOND, at + 20 * SECOND);
    engine.credit_progress(30 * SECOND, at + 320 * SECOND);
    assert_eq!(rows()[0].credited_us, 20 * SECOND);
    engine.credit = Some(voice(0));
    engine.credit_progress(15 * SECOND, at + 335 * SECOND);
    assert_eq!(rows()[0].credited_us, 35 * SECOND);
    assert!(rows()[0].qualified);
    // One renderer interval crosses the authored boundary.
    engine.credit_progress(41 * SECOND, at + 361 * SECOND);
    engine.finish_occurrence(End::Completed, at + 361 * SECOND);
    let snapshot = rows();
    let fields = |row: &HistoryRow| (row.title.clone(), row.credited_us, row.qualified);
    assert_eq!(
        snapshot.iter().map(fields).collect::<Vec<_>>(),
        [
            ("First".into(), 55 * SECOND, true),
            ("Second".into(), 6 * SECOND, true),
        ]
    );
    assert_ne!(
        snapshot[0].track.as_ref().unwrap().id,
        snapshot[1].track.as_ref().unwrap().id
    );
    // An explicit replay/Repeat One is a fresh occurrence, even with the
    // same queue entry id. A prepared voice at its new cursor earns nothing.
    engine.credit = Some(voice(0));
    engine.credit_progress(0, at + 365 * SECOND);
    assert_eq!(rows().len(), 2);
    engine.credit_progress(31 * SECOND, at + 396 * SECOND);
    engine.finish_occurrence(End::Stopped, at + 396 * SECOND);
    assert_eq!(
        rows()
            .iter()
            .filter(|row| row.title == "First" && row.qualified)
            .count(),
        2
    );
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
    let chapters: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let mut e = Engine {
        volume: 0.125,
        ..Default::default()
    };
    let reader = e.signal.subscribe();
    let pcm = e.signal.pcm();
    let _demand = pcm.subscribe();
    e.command(Command::Play(chapters.into(), 0)).unwrap();
    let audible = |_: &Engine, levels: Levels| levels.rms.iter().all(|v| *v > 0.015);
    let silent = |_: &Engine, levels: Levels| levels.peak == [0.; 2];
    until(&mut e, &reader, audible);
    let epoch = reader.epoch();
    let pcm_epoch = pcm.epoch();
    until(&mut e, &reader, |e, levels| {
        e.chapter == 1 && audible(e, levels)
    });
    assert_eq!(reader.epoch(), epoch, "same decoder/voice across the join");
    assert_eq!(pcm.epoch(), pcm_epoch);
    let mut observed = false;
    pcm.drain(|b| {
        observed |= b.epoch == pcm_epoch && b.frames.iter().any(|f| f.left != 0. && f.right != 0.);
    });
    assert!(observed);
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
    assert_ne!(pcm.epoch(), pcm_epoch);
    e.command(Command::Stop).unwrap();
    until(&mut e, &reader, silent);

    let replacement = dir.path().join("replacement.wav");
    fixture(&replacement, 96_000, 96_000);
    let replacement = Arc::new(Track::file(replacement));
    e.command(Command::Play(
        [replacement.clone()].into_iter().collect(),
        0,
    ))
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
    let tracks: crate::library::TrackList = if source
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
    let analyzer = crate::spectrum_worker::Hub::new(signal.pcm());
    let (_spectrum, readings) = analyzer.subscribe(Default::default()).unwrap();
    let mut engine = Engine {
        signal,
        device: Some(device),
        volume: 1.,
        ..Default::default()
    };
    crate::diagnostics::start();
    engine.command(Command::Play(tracks.into(), 0)).unwrap();
    let epoch = reader.epoch();
    let mut audible = 0;
    let mut spectra = 0;
    let mut last_spectrum = Instant::now();
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
        if let Ok(Some(r)) = readings.try_recv() {
            assert_eq!(r.epoch, analyzer.capture.epoch());
            assert!(r.levels.iter().all(|level| level.is_finite()));
            if r.stereo.energy.iter().any(|power| *power > 0.) {
                spectra += 1;
            }
            last_spectrum = Instant::now();
        }
        // Settled silence intentionally suppresses publication.
        if reader
            .latest()
            .is_some_and(|l| l.rms[0].max(l.rms[1]) > 0.001)
        {
            assert!(
                last_spectrum.elapsed() < Duration::from_secs(2),
                "stale spectrum during playback"
            );
        } else {
            last_spectrum = Instant::now();
        }
        if let Some(levels) = reader.latest() {
            audible += usize::from(levels.rms.iter().all(|v| *v > 0.));
            for (maximum, level) in peak.iter_mut().zip(levels.peak) {
                *maximum = maximum.max(level);
            }
        }
    }
    assert!(audible > 0, "no stereo signal observed");
    assert!(spectra > 0, "no spectrum observed");
    println!(
        "Elapsed {:.3}s, non-silent reads {audible}, spectra {spectra}, sample peaks {peak:?}, chapter {}",
        started.elapsed().as_secs_f64(),
        engine.chapter + 1
    );
}

#[test]
fn shutdown_receipt_includes_pending_queue_edits() {
    let transport = Transport::new(0., None);
    let tracks = [Track::file("first.wav".into())].into_iter().collect();
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
    e.queue.replace(program.clone().into(), 1);
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
fn queue_edits_never_restart_replace_or_eject_the_active_recording() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("library.sqlite3");
    let (writer, _) = listening::Writer::spawn(database.clone());
    let track = |name: &str| Arc::new(Track::file(dir.path().join(name)));
    let same = track("same.wav");
    let tracks: crate::library::TrackList =
        [track("played.wav"), same.clone(), track("next.wav"), same].into();
    for playing in [true, false] {
        let mut engine = Engine {
            listening: Some(writer.clone()),
            rate: 48_000,
            length: 3.,
            limit: 3.,
            ..Engine::default()
        };
        engine.queue.replace(tracks.clone().into(), 1);
        engine.select(Some(1), 0.25);
        let active = engine.queue.current_entry().unwrap().id;
        let duplicate = engine.queue.entries[3].id;
        let state = Arc::new(OutputState {
            cursor: AtomicUsize::new(12_000),
            state: AtomicU8::new(if playing {
                PlaybackState::Playing
            } else {
                PlaybackState::Paused
            } as u8),
            ..Default::default()
        });
        engine.voice = Some(OutputVoice {
            state: state.clone(),
            rate: 48_000.,
            clock: Arc::new(clock::Clock::new(0)),
            #[cfg(all(windows, feature = "qualification"))]
            trace: None,
        });
        engine.group = vec![Chapter {
            id: active,
            offset: 0.,
        }];
        engine.status.playing = playing;
        let revisions = (
            engine.status.occurrence_revision,
            engine.status.seek_revision,
        );
        let edits = [
            // A delayed removal that names the entry which became active.
            Command::Remove(vec![active]),
            Command::Remove(vec![active, duplicate]),
            Command::MoveUpcoming(active, None),
            Command::ScheduleNext(active),
            Command::Shuffle(true),
            Command::Repeat(Repeat::All),
            Command::Shuffle(false),
            Command::Repeat(Repeat::One),
            Command::ClearHistory,
            Command::ClearUpcoming,
            Command::Remove(vec![active]),
        ];
        for edit in edits {
            engine.command(edit).unwrap();
            assert!(Arc::ptr_eq(&engine.voice.as_ref().unwrap().state, &state));
            assert_eq!(engine.queue.current_entry().map(|e| e.id), Some(active));
            assert_eq!(
                (engine.status.entry, engine.status.playing),
                (Some(active), playing)
            );
            assert_eq!(engine.status.position, 0.25);
            assert_eq!(
                (
                    engine.status.occurrence_revision,
                    engine.status.seek_revision
                ),
                revisions
            );
        }
        // Clearing leaves the active recording alone, with an empty queue.
        assert!(engine.queue.upcoming().is_empty());
        assert!(engine.queue.history().is_empty());
        assert_eq!(engine.queue.entries.len(), 1);
        assert_eq!(engine.queue.repeat, Repeat::One);
    }
    writer.barrier().unwrap();
    let history = crate::store::Store::open(&database)
        .unwrap()
        .history_page((i64::MIN, i64::MAX), "", true, None)
        .unwrap();
    assert!(
        history.is_empty(),
        "queue edits ended a listening occurrence"
    );
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
    q.replace(release.clone().into(), 0);
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
fn program_seek_is_atomic_and_rejects_changed_entries_or_recordings() {
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
    e.queue.replace(tracks.clone().into(), 0);
    e.select(Some(0), 0.);
    let origin = e.status.entry.unwrap();
    let target = e.queue.entries[1].id;
    let stamp = crate::cache::Stamp::read(&path).unwrap();
    let seek = |origin, seconds| {
        Command::SeekProgram(Box::new(ProgramSeek {
            origin,
            program: tracks.clone(),
            chapter: 1,
            source_revision: stamp.clone(),
            sheet_revision: crate::cache::Stamp::read(&sheet),
            seconds,
            duration: (32000 - 427) as f64 / 32000.,
        }))
    };
    assert!(
        !Pending {
            entry: Some(origin),
            command: seek(origin, 0.125)
        }
        .applies_to(Some(target))
    );
    let revision = e.queue.revision;
    // A seek aimed from another entry, or beyond the chapter, changes nothing.
    e.command(seek(target, 0.125)).unwrap();
    e.command(seek(origin, 2.)).unwrap();
    assert_eq!((e.status.entry, e.queue.revision), (Some(origin), revision));
    e.command(seek(origin, 0.125)).unwrap();
    assert_eq!(
        (e.status.entry, e.status.position, e.status.playing),
        (Some(target), 0.125, false)
    );
    assert_eq!(e.queue.entries.len(), 2, "the queued chapter is reused");
    assert!(e.device.is_none() && e.voice.is_none());

    // Play Only This Track: the next chapter is not queued, yet reachable.
    e.queue.replace(tracks[..1].iter().cloned().collect(), 0);
    e.select(Some(0), 0.);
    let only = e.status.entry.unwrap();
    e.command(seek(only, 0.25)).unwrap();
    assert_eq!(e.queue.entries.len(), 2);
    assert_eq!(e.queue.current, Some(1));
    assert_eq!(e.queue.entries[0].id, only);
    assert!(e.status.entry.is_some_and(|id| id != only));
    assert_eq!(e.status.position, 0.25);

    // A changed recording or sheet rejects the seek before the queue changes.
    e.queue.replace(tracks[..1].iter().cloned().collect(), 0);
    e.select(Some(0), 0.);
    let origin = e.status.entry.unwrap();
    let revision = e.queue.revision;
    fixture(&path, 32000, 32001);
    e.command(seek(origin, 0.125)).unwrap();
    assert_eq!((e.status.entry, e.queue.revision), (Some(origin), revision));
    let stamp = crate::cache::Stamp::read(&path).unwrap();
    let mut fresh = seek(origin, 0.125);
    if let Command::SeekProgram(seek) = &mut fresh {
        seek.source_revision = stamp;
    }
    std::fs::write(
        &sheet,
        std::fs::read_to_string(&sheet).unwrap() + "\nREM revised\n",
    )
    .unwrap();
    e.command(fresh).unwrap();
    assert_eq!((e.status.entry, e.queue.revision), (Some(origin), revision));
    assert_eq!(e.queue.entries.len(), 1);
}

#[test]
fn restoring_a_final_chapter_without_tag_duration_keeps_saved_position() {
    let mut e = Engine::default();
    let mut q = Queue::default();
    let track = Arc::new(Track::file("unknown-length.wav".into()));
    q.replace([track].into_iter().collect(), 0);
    e.command(Command::Restore(Box::new(q), 12.25)).unwrap();
    assert_eq!(e.status.position, 12.25);
    assert!(!e.status.playing && e.voice.is_none());
}

#[test]
fn waveform_seek_rejects_rapid_obsolete_gestures_and_file_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    let sheet = dir.path().join("disc.cue");
    fixture(&path, 48_000, 96_000);
    std::fs::write(
        &sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:01:01",
    )
    .unwrap();
    let tracks: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let program =
        crate::timeline::Program::resolve(&tracks[0], DiscContext::Selected, None).unwrap();
    let mut engine = Engine::default();
    engine.queue.replace(tracks.clone().into(), 0);
    engine.select(Some(0), 0.);
    engine.refresh_flags();
    let anchor = |engine: &Engine, index| {
        let segment: &crate::timeline::Segment = &program.segments[index];
        WaveSeekAnchor {
            identity: engine.status.playback_identity().unwrap(),
            source: segment.source.clone(),
            playable: segment.frames.clone(),
            sheet: program.sheet.clone(),
        }
    };
    let command = |anchor, frame| Command::SeekWave { anchor, frame };
    let initial = anchor(&engine, 0);
    engine.command(command(initial.clone(), 24_000)).unwrap();
    assert_eq!(engine.status.position, 0.5);
    engine.command(command(initial, 12_000)).unwrap();
    assert_eq!(engine.status.position, 0.5);
    let before_edit = anchor(&engine, 0);
    engine.queue.clear_upcoming();
    engine
        .command(command(before_edit.clone(), 12_000))
        .unwrap();
    assert_eq!(engine.status.position, 0.5);
    engine.queue.replace(tracks.into(), 1);
    engine.select(Some(1), 0.);
    engine.refresh_flags();
    engine.command(command(before_edit, 12_000)).unwrap();
    let next = anchor(&engine, 1);
    let start = next.playable.start;
    assert!(engine.accepts_wave_seek(&next, start));
    assert!(!engine.accepts_wave_seek(&next, start - 1));
    let file = std::fs::File::open(&sheet).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    drop(file);
    let old = dir.path().join("old.cue");
    std::fs::rename(&sheet, &old).unwrap();
    std::fs::copy(&old, &sheet).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&sheet)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert!(!engine.accepts_wave_seek(&next, start));
    let mut replaced = next;
    replaced.sheet = crate::cache::Revision::read(&sheet).map(|revision| (sheet, revision));
    assert!(engine.accepts_wave_seek(&replaced, start));
    fixture(&path, 48_000, 96_003);
    engine.command(command(replaced, start + 4_800)).unwrap();
    assert_eq!(engine.status.position, 0.0);
}

#[test]
fn guarded_seek_checks_the_rendered_cue_boundary_before_the_next_transport_tick() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disc.wav");
    let sheet = dir.path().join("disc.cue");
    fixture(&path, 48_000, 96_000);
    std::fs::write(
        &sheet,
        "FILE disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:01:00",
    )
    .unwrap();
    let tracks: crate::library::TrackList = crate::cue::read(&sheet)
        .unwrap()
        .into_iter()
        .map(Arc::new)
        .collect();
    let program =
        crate::timeline::Program::resolve(&tracks[0], DiscContext::Selected, None).unwrap();
    let mut engine = Engine::default();
    engine.queue.replace(tracks.into(), 0);
    engine.select(Some(0), 0.);
    engine.refresh_flags();
    engine.status.playing = true;
    engine.status.duration = 1.;
    engine.group = vec![
        Chapter {
            id: engine.queue.entries[0].id,
            offset: 0.,
        },
        Chapter {
            id: engine.queue.entries[1].id,
            offset: 1.,
        },
    ];
    engine.limit = 2.;
    engine.voice = Some(OutputVoice {
        state: Arc::new(OutputState {
            cursor: AtomicUsize::new(48_480),
            ..OutputState::default()
        }),
        rate: 48_000.,
        clock: Arc::new(clock::Clock::new(0)),
        #[cfg(all(windows, feature = "qualification"))]
        trace: None,
    });
    let anchor = WaveSeekAnchor {
        identity: engine.status.playback_identity().unwrap(),
        source: program.segments[0].source.clone(),
        playable: program.segments[0].frames.clone(),
        sheet: program.sheet.clone(),
    };
    engine
        .command(Command::SeekWave {
            anchor,
            frame: 12_000,
        })
        .unwrap();
    assert_eq!(engine.status.entry, Some(engine.queue.entries[1].id));
    assert!((engine.status.position - 0.01).abs() < 1e-9);
    assert!(engine.voice.is_some() && engine.device.is_none());
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

#[test]
fn stale_context_cannot_replace_a_later_play_pause_or_media_stop() {
    let (commands, rx) = Commands::channel(
        Arc::new(Mutex::new(Shared {
            queue: Default::default(),
            status: Default::default(),
        })),
        4,
    );
    let play = || Command::Play(crate::library::TrackList::default().into(), 0);
    let first = commands.play_request.next();
    let second = commands.play_request.next();
    assert!(!commands.play_if_current(play(), first).unwrap());
    assert!(commands.play_if_current(play(), second).unwrap());
    for interrupt in [Command::SetPlaying(false), Command::Stop] {
        let request = commands.play_request.next();
        commands.send(interrupt).unwrap();
        assert!(!commands.play_if_current(play(), request).unwrap());
    }
    assert_eq!(rx.try_iter().count(), 3);
}

#[test]
fn recording_recovery_is_bounded_and_interruptible() {
    let root = tempfile::tempdir().unwrap();
    let missing = Track::file(root.path().join("absent.wav"));
    let mut e = Engine::default();
    e.queue
        .replace(std::iter::repeat_n(missing, 40).collect(), 0);
    e.queue.set_repeat(Repeat::All);
    e.advance(true).unwrap();
    assert!(e.retry_next);
    e.command(Command::Stop).unwrap();
    e.tick().unwrap();
    assert_eq!(e.queue.current, Some(1));
    e.queue.set_current(Some(38));
    e.advance(true).unwrap();
    for _ in 1..31 {
        e.tick().unwrap();
    }
    assert!(e.tick().unwrap_err().is::<RecordingFailure>());
    assert!(!e.retry_next && e.voice.is_none());
    assert_eq!(e.consecutive_failures, 32);
    e.queue.set_repeat(Repeat::One);
    e.consecutive_failures = 0;
    e.queue.set_current(Some(37));
    e.advance(true).unwrap();
    assert_eq!(e.queue.current, Some(37));
    e.tick().unwrap();
    assert_eq!(e.queue.current, Some(38));
    e.command(Command::SetPlaying(false)).unwrap();
    e.tick().unwrap();
    assert_eq!(e.queue.current, Some(38));
    e.command(Command::SetPlaying(true)).unwrap();
    assert!(e.tick().is_err());
    assert_eq!(e.queue.current, Some(39));
}

#[test]
#[ignore = "requires a Windows output device; physical output is muted after the PCM observer"]
fn separate_files_continue_past_source_and_decoder_failures() {
    use std::time::Instant;
    let dir = tempfile::tempdir().unwrap();
    let files: crate::library::TrackList = [
        "first.wav",
        "absent.wav",
        "invalid.wav",
        "broken.wav",
        "last.wav",
    ]
    .iter()
    .map(|name| Arc::new(Track::file(dir.path().join(name))))
    .collect();
    fixture(&files[0].source, 48000, 24000);
    std::fs::write(&files[2].source, b"unsupported file").unwrap();
    // The PCM container opens, then our streaming decoder rejects three channels.
    fixture(&files[3].source, 48000, 48000);
    let mut wav = std::fs::read(&files[3].source).unwrap();
    wav[22..24].copy_from_slice(&3u16.to_le_bytes());
    wav[28..32].copy_from_slice(&(48000u32 * 6).to_le_bytes());
    wav[32..34].copy_from_slice(&6u16.to_le_bytes());
    std::fs::write(&files[3].source, wav).unwrap();
    let mut decoder = Recording::open(&files[3], None).unwrap();
    assert!(decoder.decode().is_err());
    fixture(&files[4].source, 48000, 24000);
    let mut e = Engine {
        volume: 0.5,
        ..Default::default()
    };
    let reader = e.signal.subscribe();
    e.command(Command::Play(files.clone().into(), 0)).unwrap();
    let mut heard_last = false;
    let mut failed = Vec::new();
    let begin = Instant::now();
    while e.status.playing || e.retry_next {
        assert!(begin.elapsed() < Duration::from_secs(6), "{:?}", e.status);
        e.tick().unwrap();
        if let Some((id, _)) = &e.status.skipped
            && failed.last() != Some(id)
        {
            failed.push(*id);
        }
        if e.queue.current == Some(4) && reader.latest().is_some_and(|m| m.peak[0] > 0.) {
            heard_last = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(heard_last);
    assert_eq!(e.queue.current, Some(4));
    assert_eq!(
        failed,
        e.queue.entries[1..4]
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(e.status.playback_state(), TransportState::Finished);
}

#[test]
#[ignore = "measures separate WAV joins on the silent Windows endpoint"]
fn separate_file_transition_observation() {
    let dir = tempfile::tempdir().unwrap();
    let tracks: crate::library::TrackList = (0..4)
        .map(|i| {
            let path = dir.path().join(format!("{i}.wav"));
            fixture(&path, 48000, 24000);
            let mut wav = std::fs::read(&path).unwrap();
            let value = if i % 2 == 0 { 8000i16 } else { -8000 };
            for sample in wav[44..].as_chunks_mut::<2>().0 {
                sample.copy_from_slice(&value.to_le_bytes());
            }
            std::fs::write(&path, wav).unwrap();
            Arc::new(Track::file(path))
        })
        .collect();
    let t = Transport::new(0.5, None);
    let capture = t.signal.pcm();
    let _demand = capture.subscribe();
    t.send(Command::Play(tracks.into(), 0), None).unwrap();
    let mut sign = 0;
    let mut last_signal = 0;
    let mut next_block = None;
    let mut gaps = vec![];
    let deadline = std::time::Instant::now() + Duration::from_secs(7);
    while gaps.len() < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "missing transition: {:?}",
            t.state().0
        );
        capture.drain(|b| {
            assert!(
                next_block.is_none_or(|end| end == b.start),
                "capture lost frames"
            );
            next_block = Some(b.start + b.frames.len() as u64);
            for (i, frame) in b.frames.iter().enumerate() {
                if frame.left.abs() < 0.01 {
                    continue;
                }
                let next = if frame.left > 0. { 1 } else { -1 };
                let index = b.start + i as u64;
                if sign != 0 && sign != next {
                    gaps.push((index - last_signal - 1) as f64 * 1000. / f64::from(b.rate));
                }
                sign = next;
                last_signal = index;
            }
        });
        std::thread::sleep(Duration::from_millis(5));
    }
    t.send(Command::Stop, None).unwrap();
    eprintln!("Separate WAV transitions, below-threshold PCM milliseconds: {gaps:.2?}");
}

#[test]
fn losing_output_during_recovery_keeps_the_occurrence_and_cancels_skips() {
    use kira::backend::cpal::cpal::{Error, ErrorKind};
    let track = Track::file("absent.wav".into());
    let mut e = Engine::default();
    e.queue
        .replace([track.clone(), track].into_iter().collect(), 0);
    e.select(Some(0), 0.);
    e.retry_next = true;
    e.status.playing = true;
    e.output_error(Error::new(ErrorKind::Xrun)).unwrap();
    assert!(e.retry_next);
    let err = e
        .output_error(Error::new(ErrorKind::DeviceNotAvailable))
        .unwrap_err();
    assert!(!err.is::<RecordingFailure>());
    assert!(!e.retry_next && !e.status.playing);
    e.tick().unwrap();
    assert_eq!(e.queue.current, Some(0));
    assert!(e.status.skipped.is_none());
}

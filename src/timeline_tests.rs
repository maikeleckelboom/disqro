use crate::{
    audio::tests::fixture,
    cache::Cache,
    library::{DiscContext, Track},
    timeline::*,
    waveform,
};
use lofty::{
    config::WriteOptions,
    prelude::*,
    tag::{Tag, TagType},
};
use std::{fs, path::Path, sync::Arc};

fn overview(track: &Track, program: Option<&Program>) -> Option<(&'static str, bool)> {
    Overview::for_track(track, program).map(|o| (o.label, o.enabled))
}

fn tagged_mp3(dir: &Path, name: &str, album: &str, disc: u32, number: u32) -> Arc<Track> {
    let path = dir.join(name);
    // Silent MPEG-1 Layer III frames at 44.1 kHz and 128 kbit/s.
    let mut frame = [0; 417];
    frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
    let mut info = frame;
    info[36..40].copy_from_slice(b"Info");
    info[40..44].copy_from_slice(&1_u32.to_be_bytes());
    info[44..48].copy_from_slice(&8_u32.to_be_bytes());
    let mut bytes = info.to_vec();
    bytes.extend_from_slice(&frame.repeat(8));
    fs::write(&path, bytes).unwrap();
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_album(album.into());
    tag.set_disk(disc);
    tag.set_track(number);
    tag.save_to_path(&path, WriteOptions::default()).unwrap();
    let mut track = Track::file(path);
    assert!(track.read_tags());
    Arc::new(track)
}

#[test]
fn mixed_rate_program_round_trips_every_decoded_frame_and_excludes_eof() {
    let dir = tempfile::tempdir().unwrap();
    let mut cue = String::new();
    let mut number = 0;
    for (name, rate, frames, ticks) in [
        ("a", 44100, 44117, &[3, 12][..]),
        ("b", 48000, 48011, &[2, 40]),
        ("c", 96000, 96007, &[0, 7]),
        ("d", 32000, 32001, &[0, 1, 2, 74]),
    ] {
        fixture(&dir.path().join(format!("{name}.wav")), rate, frames);
        cue.push_str(&format!("FILE {name}.wav WAVE\n"));
        for tick in ticks {
            number += 1;
            cue.push_str(&format!(
                "TRACK {number:02} AUDIO\nINDEX 01 00:00:{tick:02}\n"
            ));
        }
    }
    let sheet = dir.path().join("mix.cue");
    fs::write(&sheet, cue).unwrap();
    let track = Arc::new(crate::cue::read(&sheet).unwrap().remove(1));
    let p = Program::resolve(&track, DiscContext::Inferred, None).unwrap();
    assert!(p.full_cd);
    assert_eq!(p.segments.len(), 12);
    let mut count = 0;
    for (i, s) in p.segments.iter().enumerate() {
        for f in s.frames.clone() {
            assert_eq!(p.locate(s.at(f)), Some((i, f)));
            assert_eq!(p.locate(s.at(f + 1) - 1), Some((i, f)));
            count += 1;
        }
        if let Some(next) = p.segments.get(i + 1) {
            assert_eq!(p.locate(s.time.end), Some((i + 1, next.frames.start)));
        }
    }
    assert_eq!(count, 220136);
    assert_eq!(p.segments[3].time.start, 1_000_385_488);
    assert_eq!(p.locate(p.duration), None);
}
#[test]
fn standalone_recordings_and_one_track_albums_have_no_second_perspective() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("single.wav");
    fixture(&wav, 32000, 32000);
    let single = Arc::new(Track::file(wav));
    let p = Program::resolve(&single, DiscContext::Selected, None).unwrap();
    assert!(!p.full_cd && p.limitation.is_some());
    assert_eq!(overview(&single, None), None);
    assert_eq!(overview(&single, Some(&p)), None);

    let only = tagged_mp3(dir.path(), "only.mp3", "Single", 1, 1);
    let p = Program::resolve(&only, DiscContext::Inferred, None).unwrap();
    assert!(p.full_cd && p.segments.len() == 1);
    assert_eq!(overview(&only, Some(&p)), None);
}

#[test]
fn tagged_album_overview_follows_track_numbers_and_stays_on_the_playing_disc() {
    let dir = tempfile::tempdir().unwrap();
    let album = "Serato Scratch Beats";
    let two = tagged_mp3(dir.path(), "a.mp3", album, 1, 2);
    let one = tagged_mp3(dir.path(), "b.mp3", album, 1, 1);
    tagged_mp3(dir.path(), "other.mp3", "Other Album", 1, 1);

    let resolve = || Program::resolve(&two, DiscContext::Inferred, None).unwrap();
    assert_eq!(overview(&two, None), Some(("Album", false)));
    let p = resolve();
    assert_eq!(overview(&two, Some(&p)), Some(("Album", true)));
    assert_eq!(p.segments.len(), 2);
    assert_eq!(p.index(&one), Some(0));
    assert_eq!(p.index(&two), Some(1));

    let unrelated = tagged_mp3(dir.path(), "disc2.mp3", album, 2, 1);
    let p = resolve();
    assert_eq!(overview(&two, Some(&p)), Some(("Disc", true)));
    assert_eq!(p.segments.len(), 2);
    assert_eq!(p.index(&unrelated), None);
    assert_eq!(p.locate(p.segments[1].time.start), Some((1, 0)));
}

#[test]
fn explicit_cd_cue_and_other_authored_recordings_label_the_same_timeline_differently() {
    let dir = tempfile::tempdir().unwrap();
    fixture(&dir.path().join("session.wav"), 48000, 96000);
    for (name, label) in [
        ("Live (CD1).cue", "Full CD"),
        ("Live.cue", "Full Recording"),
    ] {
        let path = dir.path().join(name);
        fs::write(
            &path,
            "FILE session.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:01:00",
        )
        .unwrap();
        let chapter = Arc::new(crate::cue::read(&path).unwrap().pop().unwrap());
        let p = Program::resolve(&chapter, DiscContext::Inferred, None).unwrap();
        assert_eq!(overview(&chapter, None), Some((label, false)));
        assert_eq!(overview(&chapter, Some(&p)), Some((label, true)));
        assert_eq!(p.index(&chapter), Some(1));
        assert_eq!(p.locate(SECOND), Some((1, 48000)));
        assert_eq!(p.duration, 2 * SECOND);
    }
}

#[test]
fn selecting_loose_files_can_resolve_a_program_without_claiming_cd_origin() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["one.wav", "two.wav"] {
        fixture(&dir.path().join(name), 32000, 32000);
    }
    let one = Arc::new(Track::file(dir.path().join("one.wav")));
    let p = Program::resolve(&one, DiscContext::Inferred, None).unwrap();
    assert_eq!(overview(&one, Some(&p)), None);
    let p = Program::resolve(&one, DiscContext::Selected, None).unwrap();
    assert_eq!(overview(&one, Some(&p)), Some(("Full Program", true)));
    assert_eq!(p.segments.len(), 2);
}

#[test]
fn stale_tagged_album_context_has_a_disabled_album_overview() {
    let dir = tempfile::tempdir().unwrap();
    let mut track = Track::file(dir.path().join("unretagged.wav"));
    fixture(&track.source, 32000, 32000);
    // A restored queue can still know the album after its file loses the tag.
    track.album = "Old Album".into();
    let track = Arc::new(track);
    let p = Program::resolve(&track, DiscContext::Inferred, None).unwrap();
    let o = Overview::for_track(&track, Some(&p)).unwrap();
    assert_eq!((o.label, o.enabled), ("Album", false));
    assert_eq!(
        o.unavailable_tip(p.limitation.as_deref(), false),
        "Album waveform unavailable · No larger sequence was found"
    );
    assert_eq!(p.segments.len(), 1);
}
#[test]
fn missing_recording_requires_a_verified_duration_for_this_sheet_revision() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    fixture(&a, 32000, 32100);
    let cue = dir.path().join("set.cue");
    let text = "FILE b.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE a.wav WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:01";
    fs::write(&cue, text).unwrap();
    let track = Arc::new(crate::cue::read(&cue).unwrap().pop().unwrap());
    let db = Cache::open(dir.path()).unwrap();
    let resolve = || Program::resolve(&track, DiscContext::Inferred, Some(&db)).unwrap();
    let incomplete = resolve();
    assert!(!incomplete.full_cd);
    assert_eq!(
        overview(&track, Some(&incomplete)),
        Some(("Full Recording", false))
    );
    fixture(&b, 48000, 24111);
    let present = resolve();
    assert!(present.full_cd);
    fs::remove_file(&b).unwrap();
    let offline = resolve();
    assert!(offline.full_cd);
    assert_eq!(
        overview(&track, Some(&offline)),
        Some(("Full Recording", true))
    );
    assert_eq!(present.duration, offline.duration);
    assert!(!offline.segments[0].source.available);
    fs::write(&cue, format!("REM changed\n{text}")).unwrap();
    assert!(!resolve().full_cd);
}
#[test]
fn source_page_reused_across_chapters_and_invalidated_on_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stereo.wav");
    let rate = 32000;
    let frames = rate as usize;
    fixture(&path, rate, frames);
    let mut wav = fs::read(&path).unwrap();
    wav[44..].fill(0);
    let boundary = crate::recording::disc_frame(1, rate);
    let offset = 44 + (boundary - 1) * 4;
    wav[offset..offset + 4].copy_from_slice(&[0xff, 0x7f, 0x00, 0x80]);
    fs::write(&path, &wav).unwrap();
    let cue = dir.path().join("s.cue");
    fs::write(&cue,"FILE stereo.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:00:01").unwrap();
    let track = Arc::new(crate::cue::read(&cue).unwrap().pop().unwrap());
    let cache = Cache::open(dir.path()).unwrap();
    let p = Program::resolve(&track, DiscContext::Inferred, Some(&cache)).unwrap();
    let source = &p.segments[0].source;
    let exact = waveform::Evidence {
        playable: 0..source.frames,
        plan: crate::waveform_port_analysis::Plan {
            start_frame: boundary - 1,
            end_frame: boundary + 1,
            frames_per_column: 32,
            leaf_frames: Some(32),
            octave: None,
            overview: false,
            with_bands: false,
        },
    };
    let page = waveform::load(source, &exact, Some(&cache), &mut None, &|| true).unwrap();
    let mut reader = None;
    let cached = waveform::load(
        &p.segments[1].source,
        &exact,
        Some(&cache),
        &mut reader,
        &|| true,
    )
    .unwrap();
    assert!(reader.is_none());
    assert_eq!(
        cache
            .0
            .query_row("SELECT count(*) FROM wave_evidence_v1", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let leaf = cached.source_detail.unwrap();
    let offset = (boundary - 1 - leaf.first_cell_frame) as u16;
    assert_eq!(leaf.offsets[0], offset | (offset << 5));
    assert_eq!(leaf.bounds[0], -1.0);
    assert!(leaf.bounds[1] > 0.99);
    assert_eq!(leaf.bounds, page.source_detail.unwrap().bounds);
    let next_chapter = waveform::Evidence {
        playable: p.segments[1].frames.clone(),
        plan: crate::waveform_port_analysis::Plan::overview(rate, p.segments[1].frames.len(), 128),
    };
    let silent =
        waveform::load(source, &next_chapter, Some(&cache), &mut reader, &|| true).unwrap();
    assert!(silent.facts.iter().all(|value| *value == 0.0));
    let samples =
        waveform::samples(source, boundary - 1..boundary + 1, &mut reader, &|| true).unwrap();
    assert_eq!(samples[0].right, -1.);
    assert!(samples[0].left > 0.99);
    assert_eq!([samples[1].left, samples[1].right], [0., 0.]);
    fixture(&path, rate, frames + 1);
    assert!(!p.unchanged());
    assert!(waveform::load(source, &exact, Some(&cache), &mut None, &|| true).is_err());
    let changed = Program::resolve(&track, DiscContext::Inferred, Some(&cache)).unwrap();
    assert!(
        waveform::load(
            &changed.segments[0].source,
            &exact,
            Some(&cache),
            &mut None,
            &|| false
        )
        .is_err()
    );
}

#[test]
fn spectral_pages_agree_from_a_seek_and_sequential_decode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stereo.wav");
    fixture(&path, 8000, 48000);
    let track = Arc::new(Track::file(path));
    let program = Program::resolve(&track, DiscContext::Inferred, None).unwrap();
    let source = &program.segments[0].source;
    let mut decoder = None;
    let request = |start_frame, end_frame| waveform::Evidence {
        playable: 0..source.frames,
        plan: crate::waveform_port_analysis::Plan {
            start_frame,
            end_frame,
            frames_per_column: 32,
            leaf_frames: None,
            octave: None,
            overview: false,
            with_bands: true,
        },
    };
    waveform::load(source, &request(0, 32000), None, &mut decoder, &|| true).unwrap();
    let next = request(32000, 48000);
    let sequential = waveform::load(source, &next, None, &mut decoder, &|| true).unwrap();
    let seek = waveform::load(source, &next, None, &mut None, &|| true).unwrap();
    assert_eq!(sequential.facts, seek.facts);
    assert_eq!(sequential.extrema, seek.extrema);
}

#[cfg(windows)]
#[test]
fn replacing_a_source_with_the_same_length_and_time_invalidates_wave_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.wav");
    let replacement = dir.path().join("replacement.wav");
    fixture(&path, 8000, 8000);
    let track = Arc::new(Track::file(path.clone()));
    let cache = Cache::open(dir.path()).unwrap();
    let resolve = || Program::resolve(&track, DiscContext::Inferred, Some(&cache)).unwrap();
    let original = resolve();
    let source = &original.segments[0].source;
    assert!(source.object.is_some());
    let request = waveform::Evidence {
        playable: 0..source.frames,
        plan: crate::waveform_port_analysis::Plan::overview(source.rate, source.frames, 128),
    };
    let old_page = waveform::load(source, &request, Some(&cache), &mut None, &|| true).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes[44..].fill(0);
    fs::write(&replacement, bytes).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    fs::rename(&path, dir.path().join("retained.wav")).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert!(!original.unchanged());
    let new = resolve();
    let changed = &new.segments[0].source;
    assert_eq!(source.stamp, changed.stamp);
    assert_ne!(source.object, changed.object);
    assert!(waveform::cached(changed, &request, Some(&cache)).is_none());
    let new_page = waveform::load(changed, &request, Some(&cache), &mut None, &|| true).unwrap();
    assert!(old_page.facts.iter().any(|m| *m != 0.));
    assert!(new_page.facts.iter().all(|m| *m == 0.));
}

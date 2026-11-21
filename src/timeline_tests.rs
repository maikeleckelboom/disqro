use crate::{audio::tests::fixture, cache::Cache, library::DiscContext, timeline::*, waveform};
use std::{fs, sync::Arc};

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
    assert!(!resolve().full_cd);
    fixture(&b, 48000, 24111);
    let present = resolve();
    assert!(present.full_cd);
    fs::remove_file(&b).unwrap();
    let offline = resolve();
    assert!(offline.full_cd);
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
    let page = waveform::load(source, 0, Some(&cache), &mut None, &|| true).unwrap();
    let mut reader = None;
    let cached = waveform::load(&p.segments[1].source, 0, Some(&cache), &mut reader, &|| {
        false
    })
    .unwrap();
    assert!(reader.is_none());
    assert_eq!(
        cache
            .0
            .query_row("SELECT count(*) FROM wave_pages_v1", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let split = waveform::ceil_cell(boundary, rate);
    assert_eq!(split, 4);
    assert_eq!(cached[split - 1].lo[1], -1.);
    assert!(cached[split - 1].hi[0] > 0.99);
    assert_eq!(cached[split - 1].energy, page[split - 1].energy);
    assert!(
        page[split..]
            .iter()
            .all(|m| m.energy == 0. && m.hi == [0.; 2] && m.lo == [0.; 2])
    );
    fixture(&path, rate, frames + 1);
    assert!(!p.unchanged());
    assert!(waveform::load(source, 0, Some(&cache), &mut None, &|| true).is_err());
    let changed = Program::resolve(&track, DiscContext::Inferred, Some(&cache)).unwrap();
    assert!(
        waveform::load(
            &changed.segments[0].source,
            0,
            Some(&cache),
            &mut None,
            &|| false
        )
        .is_err()
    );
}

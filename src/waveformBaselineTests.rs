use super::*;
use std::{cell::RefCell, env, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
struct Geometry {
    bytes: usize,
    digest: u64,
}

fn geometry(shapes: &[Shape]) -> Geometry {
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut hash = |value: u64| {
        for byte in value.to_le_bytes() {
            digest = (digest ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    };
    let mut bytes = 0;
    for shape in shapes {
        let page = &shape.track;
        bytes += page.bytes();
        hash(page.frame_count as u64);
        hash(page.frames_per_column as u64);
        for &fact in &page.facts {
            hash(u64::from(fact.to_bits()));
        }
        for &witness in &page.extrema {
            hash(witness as u64);
        }
        if let Some(power) = &page.spectrum {
            // Compare the spectrally relevant precision across JSON cache reopen.
            for &value in power {
                hash(u64::from((value as f32).to_bits()));
            }
        }
    }
    Geometry { bytes, digest }
}

#[test]
#[ignore = "headless baseline benchmark; set DISQRO_WAVE_BASELINE_SOURCE to a recording or CUE"]
fn timeline_waveform_baseline() {
    let source = PathBuf::from(
        env::var_os("DISQRO_WAVE_BASELINE_SOURCE").expect("set DISQRO_WAVE_BASELINE_SOURCE"),
    );
    let (track, segments) = if source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cue"))
    {
        let mut tracks = crate::cue::read(&source).unwrap();
        let count = tracks.len();
        (tracks.remove(count / 2), count)
    } else {
        (Track::file(source), 1)
    };
    let track = Arc::new(track);
    let temp = tempfile::tempdir().unwrap();
    let mut cold_geometry = None;
    println!(
        "cache,segments,source_seconds,first_geometry_ms,active_complete_ms,program_complete_ms,retained_bins_bytes,sqlite_wave_pages,geometry_digest"
    );
    for kind in ["cold", "warm"] {
        let cache = Cache::open(temp.path()).unwrap();
        let began = Instant::now();
        let program = Program::resolve(&track, DiscContext::Inferred, Some(&cache)).unwrap();
        assert_eq!(program.segments.len(), segments);
        let active = program.index(&track).unwrap();
        let focus = AtomicUsize::new(active);
        let observations =
            RefCell::new((vec![Shape::default(); program.segments.len()], None, None));
        analyse(
            &program,
            active,
            &focus,
            DecoderSession {
                cache: Some(&cache),
                reader: &mut None,
            },
            || true,
            &|update| {
                let Update::Wave(index, shape) = update else {
                    panic!("expected waveform geometry from an unchanged fixture");
                };
                let (shapes, first, active_complete) = &mut *observations.borrow_mut();
                let elapsed = began.elapsed();
                if index == active {
                    if shape.track.frame_count > 0 {
                        first.get_or_insert(elapsed);
                    }
                    if shape.track.frame_count == program.segments[index].frames.len() {
                        active_complete.get_or_insert(elapsed);
                    }
                }
                shapes[index] = shape;
                true
            },
            &|_| {},
        );
        let finished = began.elapsed();
        let (shapes, first, active_complete) = observations.into_inner();
        assert!(
            shapes
                .iter()
                .zip(&program.segments)
                .all(|(s, segment)| s.track.frame_count == segment.frames.len())
        );
        let geometry = geometry(&shapes);
        let pages: i64 = cache
            .0
            .query_row("SELECT count(*) FROM wave_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let ms = |time: Duration| time.as_secs_f64() * 1000.0;
        println!(
            "{kind},{},{:.3},{:.3},{:.3},{:.3},{},{},{:016x}",
            shapes.len(),
            program.duration as f64 / crate::timeline::SECOND as f64,
            ms(first.unwrap()),
            ms(active_complete.unwrap()),
            ms(finished),
            geometry.bytes,
            pages,
            geometry.digest,
        );
        if let Some(cold) = &cold_geometry {
            assert_eq!(cold, &geometry);
        } else {
            cold_geometry = Some(geometry);
        }
    }
}

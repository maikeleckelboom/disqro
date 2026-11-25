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
            // Round at the display-input precision for comparisons with the checkpoint.
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
        "cache,segments,source_seconds,first_geometry_ms,full_overview_ms,exact_visible_ms,complete_ms,retained_bins_bytes,sqlite_pages,sqlite_bytes,geometry_digest"
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
        let detail_time = std::cell::Cell::new(None);
        let measure_detail = env::var_os("DISQRO_WAVE_BASELINE_DETAIL").is_some();
        let segment = &program.segments[active];
        let span = (segment.source.rate as usize * 8).min(segment.frames.len());
        let first = (segment.frames.len() - span) / 2;
        let detail = if measure_detail {
            crate::waveform_detail::Key::geometry(
                &segment.source,
                segment.frames.clone(),
                first.saturating_sub(2048)..(first + span + 2048).min(segment.frames.len()),
                1920.0,
                span as f64 / f64::from(segment.source.rate),
            )
        } else {
            vec![]
        };
        let detail = RefCell::new(std::collections::VecDeque::from(detail));
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
                let (shapes, first, full_overview) = &mut *observations.borrow_mut();
                let elapsed = began.elapsed();
                if index == active && shape.track.frame_count > 0 {
                    first.get_or_insert(elapsed);
                }
                shapes[index] = shape;
                if shapes
                    .iter()
                    .zip(&program.segments)
                    .all(|(s, p)| s.track.frame_count == p.frames.len())
                {
                    full_overview.get_or_insert(elapsed);
                }
                true
            },
            &|session| {
                let mut pending = detail.borrow_mut();
                if let Some(key) = pending.pop_front() {
                    key.load(session.cache, session.reader, &|| true).unwrap();
                    if pending.is_empty() {
                        detail_time.set(Some(began.elapsed()));
                    }
                }
            },
        );
        let finished = began.elapsed();
        let (shapes, first, full_overview) = observations.into_inner();
        assert!(
            shapes
                .iter()
                .zip(&program.segments)
                .all(|(s, segment)| !s.track.preview && s.track.frame_count == segment.frames.len())
        );
        let geometry = geometry(&shapes);
        let pages: i64 = cache
            .0
            .query_row("SELECT count(*) FROM wave_evidence_v2", [], |row| {
                row.get(0)
            })
            .unwrap();
        let ms = |time: Duration| time.as_secs_f64() * 1000.0;
        println!(
            "{kind},{},{:.3},{:.3},{:.3},{},{:.3},{},{},{},{:016x}",
            shapes.len(),
            program.duration as f64 / crate::timeline::SECOND as f64,
            ms(first.unwrap()),
            ms(full_overview.unwrap()),
            detail_time
                .get()
                .map(|t| format!("{:.3}", ms(t)))
                .unwrap_or_default(),
            ms(finished),
            geometry.bytes,
            pages,
            cache
                .0
                .query_row(
                    "SELECT sum(length(data)) FROM wave_evidence_v2",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            geometry.digest,
        );
        if let Some(cold) = &cold_geometry {
            assert_eq!(cold, &geometry);
        } else {
            cold_geometry = Some(geometry);
        }
    }
}

# Waveform Native hardening and integration qualification

The integration branch is reviewable. Production remains on its accepted
baseline. The Waveform Native repository is private, so clean public
dependency resolution awaits the owner's distribution decision.
[Disqro integration branch](https://github.com/maikeleckelboom/disqro/tree/feat/waveform-native-engine)

## Findings and regression coverage

| Finding | Change and verification |
| --- | --- |
| Resident-only budgeting could over-admit detail | A reservation is charged before scheduling and identified by a unique attempt. Regressions cover two 65,536-byte outputs with one slot, oversized demand, partial headroom at 100,000 bytes, idle re-pinning, budget reduction, failure, abandonment, and retirement. |
| Concurrent overview opens and working sets escaped the detail budget | The reusable worker reserves overview working bytes and separately budgets idle pages. It gives visible detail a turn before each overview admission or decode step. The controlled provider test queues 20 overviews, releases a blocked source and checks that unrelated detail completes before subsequent overview opens. Disqro retains its own progressive overview and decoder orchestration. |
| GPU targets had only individual dimension bounds | GPUI charges texture payloads against one app cap. Deferred targets wait for capacity changes. An accounting test exercises two targets, resize, release, device replacement and failed allocation. |
| Disqro lost the replacement signal between key creation and registration | Registration and retirement now happen in the evidence-owning store as views submit demand. The adapter test uses normal key construction, checks actual retirement and release of old output reservations, rejects the old result, and proves that an unrelated active source still has exact evidence. Source metadata follows retained tiles and demand. A bounded-idle test checks collection and monotonic source IDs. The playback generation check remains active. |
| Several reusable deck transitions dropped a press silently | Hide, removal, replacement, logical segment changes, capture loss, Escape and display-transform invalidation use the same cancellation path. The reference viewer owns the paused clock and restores the clock that started the gesture. Disqro continues to use its own gesture and audible audition implementation. |
| Malformed evidence and sample coordinates could reach arithmetic and indexing | Checked nested intervals, sample blocks, source/request validation, codec and JSON validation, and bounded renderer inputs. Tests reject nested overflow and a finite `f64::MAX` sample origin, followed by valid work. |
| D3D11 inherited caller GS and predication | The renderer clears unused graphics stages, stream output and predication. Its embedding contract specifies the same-device immediate context. A headless test binds disruptive caller state and compares the resulting pixels. Deferred-context rejection is followed by a valid immediate draw. |
| The Disqro source-upload counter included descriptor traffic | `wave_source_upload_bytes` counts evidence-arena writes. `wave_total_upload_bytes` includes all draw uploads. Warm redraws increment the total and leave source bytes at zero when evidence is already resident. The observer reports real draws and failures. |
| Derived records changed from v3 to v4 | An isolated fixture profile goes through the v3 transition, rejects three v4 record defects, rebuilds via the actual decoder, and reopens without opening a decoder again. It verifies the original track identity, ratings, likes, listening totals, duplicate queue occurrences and unrelated cached tags. Other timeline tests cover native file and logical/physical mapping. |
| Archived summaries could silently omit cases | The parity gate checks the fixed 103- and 148-case manifests, report scores and rendered output files, with bidirectional byte comparisons. The standalone loading tables use the final archived records. |
| First-party Waveform Native spelling varied | The engine uses American English in code, shaders, examples, and documentation, with a pinned CI spelling check. The audit preserves evidence field names, codec identifiers, and fixture bytes. Disqro consumes the new revision and does not directly name the renamed APIs. |

The prior qualification established 103 frozen Disqro views and 214 identical
outputs, 210 passing tests with 12 opt-in tests ignored, and successful
muted private-desktop deck, lifecycle, recovery, transport and scrub
journeys. The engine's prior fresh gate covered all 148 views. These are
archived results for the original integration revision.

The hardening pin for both the engine and GPUI platform patch is
`6c638e34e0fa05acd0298fd9a44107b7595cc298`. The engine's private hosted
checks use the exact revision. For authenticated local consumption outside
the engine checkout, select `RUSTUP_TOOLCHAIN=1.99.1` explicitly before
running the independent Git fixtures.
[Engine CI](https://github.com/maikeleckelboom/waveform-native/actions/runs/45727846614)

## Tests transferred from Disqro

| Production Disqro test | Engine test and assertion |
| --- | --- |
| `centred_follow_and_pointer_zoom_keep_exact_source_coordinates` | `viewport::tests::centered_follow_and_pointer_zoom_keep_exact_source_coordinates` checks repeated zoom about exact pointers at large source coordinates and centered entry. |
| `complementary_leaves_distinguish_silence_missing_and_out_of_cell_witnesses` | `presentation::stroke::tests::complementary_leaves_distinguish_silence_missing_and_out_of_cell_witnesses` checks fine/coarse ownership, zero extrema and rejection of witnesses outside their cell. |
| `every_visible_panel_can_finish_while_another_panel_moves` | `store::tests::every_visible_view_can_finish_while_another_view_moves` checks every stable view receives a scheduling turn while a ninth view changes demand. |
| `inactive_tiles_are_evicted_without_displacing_a_visible_tile` | `store::tests::inactive_tiles_are_evicted_without_displacing_a_visible_tile` checks retained visible results and bounded idle entries under repeated changes. |
| `paired_channels_match_scalar_facts_bit_for_bit` | `analysis::filters::tests::paired_channels_match_scalar_facts_bit_for_bit` checks energy from paired filters against an independent scalar recurrence. |
| `views_share_tiles_and_ignore_superseded_publication` | `store::tests::views_share_tiles_and_ignore_superseded_publication` checks a single reservation, shared immutable output, and rejection after retirement. The Disqro adapter adds a separate normal-path replacement test. |
| `wide_views_cover_all_supported_rates_and_unaligned_ranges` | `planning::tests::wide_views_cover_all_supported_rates_and_unaligned_ranges` checks 120-second requests across supported rates, widths and segment alignment. |
| `sample_zoom_keeps_both_decoded_channels_and_clips_at_the_playable_edges` | `waveform-d3d11/tests/headless.rs` uses the same name. It checks distinct stereo traces and empty pixels beyond the segment. |
| `shader_projection_matches_the_reference_at_device_pixel_edges` | `waveform-d3d11/tests/headless.rs` uses the same name. It checks light/dark overview/detail against the CPU reference within one code. |
| `silent_stroke_columns_stay_uncoloured_when_sample_inspection_is_absent` | `waveform-d3d11/tests/headless.rs::silent_stroke_columns_stay_uncolored_when_sample_inspection_is_absent` checks that exact silent columns carry no RGB energy. |
| `wide_stroke_keeps_coarse_witnesses_across_the_full_surface_and_new_devices` | `waveform-d3d11/tests/headless.rs` uses the same name. It checks wide-view witness agreement and identical pixels on an independent device. |

## Quiet-machine gate

**Pending an owner-confirmed quiet window.** The previous application
comparisons were inconclusive. One overlapped local compilation and the
others overlapped interactive use. Preserve their CSV and JSON artifacts
as diagnostic evidence. The application threshold is a one-sided 5%
increase in wall-clock medians. The separate standalone loading threshold
is the larger of 2 ms and 5% of the baseline.

The paired gate runs four recordings with and without exact detail, eight
baseline/candidate pairs per workload, after one ungated WAV warm-up per
build. Each executable opens a fresh temporary SQLite evidence cache and
then reopens that populated cache for its warm pass. Windows file caching
runs normally. Each side therefore has identical application-cache
preparation. Four metrics are checked for each applicable cold and warm
workload: first active geometry, full overview geometry, completion of
exact visible detail, and total analysis completion. Each statistic is
the candidate median wall milliseconds divided by the baseline median.
The paired bootstrap interval flags uncertainty when it straddles 1.05.
Raw pairs, process wall/CPU time, correctness digests, retained bytes,
execution order, environment and every attempt stay with the run.

Allow about 25 minutes on the recorded system. Prebuild the locked
qualification test binaries into separate target directories. During the
window run the command below serially, with local builds and qualification
jobs stopped. Create an empty `STOP` file in the new output directory or
press Ctrl+C if the owner resumes using the machine. Preserve partial logs.
Use the same power and system configuration for both sides.

```powershell
$local = 'C:/dev/disqro/.local/waveform-native-hardening'
& "$local/en-us/run-quiet.ps1" `
  -QuietWindow '<confirmed local date, start and end, timezone>' `
  -Configuration '<power source, OS build and settings>'
```

The local wrapper validates the locked executable and fixture hashes in
`prepared-timing.json` before invoking `tools/qualify_waveform_loading.py`.
The runner also accepts these inputs directly for reproduction elsewhere.

Physical audibility, endpoint behavior outside the muted harness, physical
display cadence, monitor transfer and actual driver resets require their
own native measurements.

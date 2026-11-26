# Continuous waveform evidence residency

The detail renderer needs the visible interval and a small Stroke halo. At
1.799 seconds visible, a new fine geometry page used to enter that footprint
before its decode was requested. The combined geometry and spectral pages had
the same lifecycle at their own boundaries. PCM read-ahead did not apply at
this span. A missing page returned the deck to approximate coverage, and the
change in refinement status could invalidate the retained scene.

The Waveform Native planner now predicts demand at the selected resolution for
every evidence kind. The shared store schedules visible work across panels
before deadline-ordered read-ahead, and caches speculative completions without
invalidating the scene. Disqro keeps independent detail and overview decoder
cursors while interleaving their work on the same analysis thread. A source
generation change releases old view demand, and cancellation releases a
reservation without recording the missing output as a decode failure.

## Deterministic qualification

The shared engine replayed 35 spans at 44.1 kHz and 1,280 device pixels with two
viewports, 120 Hz sampling, a simulated overview workload, and forward and
reverse motion. It covered 120, 78.846, 51.805, 8, 4, 2.739, 1.799, and 0.145
seconds, the envelope-to-sample transition, and 1,400, 113, and 32 samples.

After a one-second initial allowance, every geometry page, spectral page, and
PCM block required by either renderer footprint was exact. The same workload
with reactive requests recorded 2,464 misses summed across panels, frames,
and evidence kinds. Across nine tier transitions, the largest measured total
of resident and reserved detail bytes was 5,013,516. Three speculative
completions crossed their forecast deadlines without a visible gap.

## Native qualification

Run the qualification binary silently on a private Windows desktop with an
isolated profile. The full wheel sweep and reverse pan can reuse an existing
FLAC or CUE fixture.

```powershell
cargo build --locked --release --features qualification
python -B tools/qualify_waveform_motion.py target/release/disqro.exe <source.cue> <output> --residency --panels 2 --reverse
```

Use `--reactive` for the same build with read-ahead disabled and `--warm-from`
to copy a previous run's persistent evidence cache into a new profile. The
trace records first demand and first exact consumption by page key, worker
service time and forecast misses, exact and missing counts by evidence kind
and panel, scene draws, and application Present timing. The report includes
process and UI-thread CPU time, sampled private memory, detail residency, and
per-process GPU counters where the driver supplies them. The private desktop
measures application rendering and Present calls. Physical scanout is outside
its scope.

The Windows sweeps used the Radeon 780M on a Ryzen 7 8845HS. The CUE fixture
has 15 chapters over a 75-minute 44.1 kHz FLAC. Two decks rendered at 1,215
device pixels each while the overview worker was active. The forward results
count the observations after the one-second settle at each wheel position.

| Run | Geometry misses | Spectral misses | PCM misses | Approximate scenes | Detail bytes, sampled peak |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cold CUE, reactive | 460 / 23,599 | 44 / 23,599 | 305 / 12,073 | 94 | 9,117,708 |
| Cold CUE, predictive | 0 / 23,068 | 0 / 23,068 | 0 / 11,203 | 0 | 8,962,060 |
| Warm CUE, predictive | 0 / 25,426 | 0 / 25,426 | 0 / 11,552 | 0 | 9,052,172 |
| Cold three-hour FLAC, predictive | 0 / 12,749 | 0 / 12,749 | 0 / 5,771 | 0 | 8,863,744 |

At 1.799 seconds, the reactive run recorded 15 geometry misses, perfect
spectral coverage, and five approximate scenes. The predictive run had
every required page on time and drew 478.7 retained updates per second
across the two panels. The five ahead-of-need completions in that interval
had a p95 service time of 0.935 ms. Across the forward sweep, the predictive
run recorded 331 completions and zero missed forecast deadlines.
The full forward sweep recorded 897 deck scene builds with reactive demand
and 124 with predictive demand. The standalone FLAC's reverse pass moved
backward on 60 consecutive inputs and covered 1,264,943 source frames with
zero visible misses and zero missed forecast deadlines. A separate two-deck
scrub journey crossed a CUE chapter boundary and released at the selected
frame after reversing during playback.

The two cold journeys sampled 68.1% and 71.4% of one CPU core in the reactive
and predictive runs, respectively. UI-thread readings were 41.8% and 46.2%,
peak sampled private memory was 181.7 and 212.7 MiB, and the busiest GPU engine
reported 41.0% and 52.1%. These figures include the host UI, tracing, and
overview work in each run. They describe the measured journeys on this
machine.

## Regression checks

The Disqro release suite passed 234 tests, and strict Clippy passed. The
Waveform Native synthetic Stroke gate passed all 12 cases. The 103 frozen
D3D11 renders were byte-identical to the earlier skipped-cell correction
replay. The frozen gate reported the same 20 differences against the older
reference images with the existing tolerances.

## Limits

Cold seeks and previously unseen resolution tiers can require initial
refinement. The planner's 0.5–2 second horizon and per-tier page cap bound the
work offered to the store. The defaults allow 256 MiB of visible evidence and
output reservations and 8 MiB or 64 idle entries shared across panels.
The worker services overview analysis between detail
jobs, so a costly seek, a slow decoder, or enough unrelated panels can exhaust
that lead or the shared budgets. The existing approximation and resource
statuses continue to reflect measured coverage.

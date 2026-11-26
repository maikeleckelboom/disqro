# Dekzer waveform recovery

The implementation now lives in Waveform Native; file paths in this record refer to the historical Disqro revisions it describes. [Waveform Native in Disqro](waveform-native.md)

This document records the accepted visual checkpoint and its historical layered comparisons. The production follow-up uses Stroke as the single waveform presentation. [Native GPU renderer and qualification](waveform-native-gpu.md)

The checkpoint ports the Electron Stroke and layered Three-band presentations
to Disqro's native surfaces. It uses decoded source frames for waveform
analysis, keeps track and disc transport coordinates in the program timeline,
and retains separate left and right traces for decoded sample inspection.
Visual acceptance is an owner decision. The comparison runner and the
production checkpoint use the same Rust presentation functions.

## Reference selection

| Evidence | Commit | Tree | Presentation |
| --- | --- | --- | --- |
| Selected Electron source | `8107c46fd9c7cbb697927a59def9a52f9079dc4f` | `951a946513e7f6b13410b4bf8a68c9605b6ee0b2` | Default Stroke, current Three-band and overview |
| Complementary tiers | `e3a7d420bf1a9ca8584d8e854fd316229d335db1` | `c92c0d6d663a2f810458d166cc1b0f6d3f6e536b` | Fine and coarse spans can contribute to the same view |
| Direct fragment pass | `06d27de71b7a6f651b606ed8e0c7963af35e6f19` | `4344ba7928d3a1fececfcb09104d7f153a41e391` | Replaced the zoom-scaled Stroke strips with source-grid columns |
| Close detail from PR #134 | `312d6c42dd48b40a5bd9126cab9757fbc8f995c2` | `5055fb019429e8479dead773721ebd2ee9832ef3` | Historical Spectral ribbon with denser 2–4 s analysis and no close centre stripe |
| Refined October presentations | `3ada7bcc58e1b9ab8726bdd3afb15b9f773f561a` | `94147f13757a2568e65bf4b7cf2171ba6524d437` | Historical Spectral signed envelope and Layered bands |
| Reviewed Disqro PR #23 | `a2e1590882fa9ff5b03d0ba9c5ee566590a68119` | `36afebac3e6a4ac55a2e7337103d00ddc0d9b29e` | Baseline for the rejected appearance |

The retained visual-excellence handoff has two checksum-listed Dekzer
screenshots. Its manifest identifies image bytes and broad/close zoom, while
their exact PCM interval, settings and executable revision remain unknown.
The 2026-10-30 17:47 package directory retained a candidate manifest, including
the renderer and native service hashes. The investigated direct-render and
source-tier worktrees retained build manifests and unpacked renderer/service
outputs. Those records support the source history. The link from a specific
packaged executable to the owner's remembered view remains unverified.
Dekzer's `docs/electron-stems.md` singles out #134 as the stable close-detail
reference. That path uses a moving signed range with a 3.49 to 1.5 device-pixel
footprint and 4.65 to 0 ms empirical registration over its 8–4 s transition.
Its capture is a separate history replay. The selected port follows the later
default Stroke source unchanged in its cell and colour laws.

The replay reads exact Git snapshots. The selected analysis identity is
`facts=1 decoder=symphonia-0.6.1 canonical=indexed-mp3-v1 overview-columns=4096 overview-spectrum=octave-blackman512-44100-source256-v1`.
Detail uses `octave-hann512-frame-centred256-log1p-v1` and source-geometry
leaves with version 1 witnesses. Electron 44.9.0 runs the original renderer
modules in a hidden window on a private Windows desktop. Stroke uses the
original WebGL2 fragment program with an opaque sRGB black canvas, disabled
multisampling, gain and intensity 1, and neutral edge contribution 54.
Three-band detail runs the selected revision's actual direct Canvas projection,
which Dekzer uses while zooming or where settled tiles have yet to arrive.
Its original tiered worker strips are a separate presentation path.

## Analysis and geometry contract

| Dekzer source | Recovered behavior | Disqro implementation |
| --- | --- | --- |
| `crates/playback/src/waveform_source.rs` and `waveform_detail.rs` | Half-open source buckets. Signed stereo extrema use the lowest and highest channel values. Earliest frame wins ties. Endpoints and RMS belong to whole columns. Stroke uses 32-frame evidence leaves and optionally finer dyadic leaves. Absolute source witnesses survive clipped CUE edges. | `src/waveform_port_analysis.rs` |
| `crates/playback/src/waveform_bands.rs` | Four second-order Butterworth sections, with low-pass 200 Hz, middle high-pass 200 then low-pass 2500 Hz, and high-pass 2500 Hz. Independent stereo f64 filter states. Track-local 200 ms preroll for detail, clipped at a real boundary. Each band's absolute peak and mean of 12 ms causal smoothed stereo power are independent. RMS is the square root of the bucket power. | `src/waveformFilters.rs` |
| `crates/playback/src/waveform_octave.rs` | Stroke takes `(L+R)/2` in f64 and symmetric Hann512 with a 256-frame source hop. At cell `k`, the read starts at `256k − 256`. Missing material outside the playable boundaries is zero-padded. FFT bins split at `[1,3,5,9,17,33,65,129,257]`. Power scale is `16 / (512 × Σ(window²))`. Detail measurements stay on the native 44.1 or 48 kHz grid. | `src/waveformOctave.rs` |
| `apps/desktop/native/src/waveform_source.rs`, `waveform-model.ts`, `waveformGeometry.ts`, and `waveformPages.ts` | 262,144-frame evidence tiles reuse 32-frame leaves and octave cells across zoom. Denser leaves refine narrow cores, with PCM refinement for held intervals that lack useful witnesses. A cell sees its own interval and one neighbor on either side. Colour requests carry their own halo. | `src/waveform_source.rs`, `src/waveform_detail.rs`, and `src/waveform_render.rs` |
| `apps/desktop/src/waveformStroke.ts` | Geometry uses whole source blocks counted from the playable segment's zero. At 48 kHz the anchor densities are 45, 90, 180 and 360 px/s. The 360 px/s block has 133 frames, skip 1, skip phase 0 and screen delta 0.39 px. The 44.1 kHz model has one measured 180 px/s anchor, a 122-frame block at 360 px/s, skip 1.001436, skip phase 1.02 and screen delta −0.19 px. Screen boundaries round to physical pixels. Source boundaries interpolate in projected time on a logarithmic density axis, then ceil once. Exact anchors use their original integer boundaries. | `src/waveformStroke.rs` |
| `apps/desktop/src/waveformSourceCells.ts`, `waveformSourceTruth.ts` | The finest available evidence owns each disjoint source span. An extremum affects a cell only if its witness belongs to that intersection. A constant span proves the value across its range. Missing coverage and held coverage without usable witnesses have separate states. Exact silence has no body. | `src/waveformStroke.rs` |
| `apps/desktop/src/waveformStrokeColour.ts` | Model time comes from the nominal block law at the physical column centre, then shifts by −60 source frames for colour only. Compress each band's square-root power using `log1p(exp(K) × amplitude) / log1p(exp(K))`, with `K = 2.8606101989383417`. Round that encoding to f32. Each of three adjacent hop cells contributes eight coefficients to each RGB channel. Fold and store in f32, interpolate adjacent folded cells, clamp each channel to 40–255, then apply intensity about the 150-code dark neutral. | `src/waveformStroke.rs` |
| `apps/desktop/src/waveformDirect.ts` | Draw the measured signed core at `height/2 − clamp(gain × amplitude, −1, 1) × height × .46`. A nonzero constant core gets a minimum total height of 2 px. Adjacent measured cells extend only the outer fringe. Missing neighboring witnesses leave that side at the current bound. Write core/fringe boundaries and RGB as f32. | `src/waveformStroke.rs` |

The measured 48 kHz pairs of cell ratio and phase are
`(1.36986, .63)`, `(1.36985, .6)`, `(1.354208, .644)`, and
`(1.34637, .62)`, in order of increasing density. The measured 44.1 kHz
pair is `(1.35811, 1.335)`. A source block `m` belongs to cell
`floor((floor(m × skip + skipPhase) + phase) / cellRatio)`.
These numbers describe display allocation. Transport still seeks to the
viewport's decoded frame, independent of its nearest Stroke cell or colour time.

The fragment pass sums the overlap of a device row with the solid core and
the two linear fringe ramps. The ramps transfer coverage as `2u − u²`, using
`u² − u³/3` as the integral. With integrated vertical coverage `a`,
horizontal coverage `q` (including a partial EOF pixel), encoded RGB `C`,
and neutral value `n`, the emitted RGB is `q × (aC + n × a × (1 − a))`.
The canvas contributes `background × (1 − a × q)`. The fragment differs
from the older worker's byte alpha carrier, implemented in Dekzer's
`waveformStroke.ts` and invoked through `waveform-raster.ts`. Wide views
retain the reference's orientation mark on otherwise empty, known rows.
Its opacity fades from 8 s to zero at 4 s and stays zero at 2 s. Unknown
coverage carries a different state and receives neither the mark nor a body.

The frozen WF-111 transfer is Dekzer's fitted approximation. Dekzer's
experiments found weaker prediction on rapidly changing music (WF-114).
Only 48 kHz has the full range of measured density anchors.
Colour analysis on the mono sum can cancel anti-phase material, while
geometry, band analysis and stereo inspection retain both channels.
The fixed-grid Blackman octave path serves other supported analysis rates
and the whole-source overview. Blackman512 uses a fixed 44.1 kHz grid,
Kaiser resampling where required and the original octave palette. Rates
outside its supported set use the original three-band display fallback.

## Three-band and overview

The selected `waveformPresentation.ts` projects complete native columns
from the finest held tier into physical source intervals. It pools powers by
their measured frame counts. The independent low, mid and high shapes fill
in that order with gains `1, .78, .8` and dark colours `#3578f6`,
`#3bd581` and `#f4f8ff`. Low and mid are band RMS times √2. A high band
accent fades in as its RMS rises from .25 to .60 of the strongest band.
The fine view uses its independently measured peak. As time per pixel
increases through 1–16 ms, the smooth summary law moves 85% of the way
toward the lesser of its peak and sine-equivalent RMS. The overview uses
the bounded RMS target directly and draws white at .75 opacity.
The raw signed bounds gate silence. High-only events can extend beyond
the blue and green bodies without a normalizer or per-column sort.

Whole-source overviews pool complete geometry buckets, around three device
pixels per group. Stroke overview draws signed peaks at .65 opacity and a
bounded full-band RMS body before applying the colour strip. Overview colour
pools Blackman powers on the source grid before temporal interpolation and
sRGB encoding. It uses a 1 ms node lag and the original octave palette.
The chosen Canvas path coverage is reproduced on the CPU with four rotated
samples and 1/16-pixel raster vertices. Small geometry paths use independently
resolved coverage. Larger paths composite their layers per sample. The
corresponding Skia rule uses integer-outset path bounds, a 1024-pixel side
limit and a 128² area limit with multisampling available. [Skia Ganesh atlas path selection](https://skia.googlesource.com/skia/+/2e551697dc56/src/gpu/ganesh/ops/AtlasPathRenderer.cpp)

Disqro's rejected branch used a 300 Hz analysis grid, unsmoothed band
powers, a convex blend of pale RGB descriptors and independent rectangular
peak/RMS quads. Three-band had separate lanes. The port retires those
production paths and their derived cache. The source-replay runner extracts
the reviewed analysis, projection and drawing routines from Git and
composites their GPUI quad commands on the same black comparison surface.
This baseline replay covers the old waveform math. Its quad edge pixels
come from the harness's CPU compositor.

## Reproducing the checkpoint

The local checkout needs Dekzer's pinned desktop dependencies, including its
Electron binary and esbuild. The runner uses the repository-pinned Rust
toolchains, Python 3.11 or later, NumPy and Pillow. Use a new output folder
for each evidence run. Keep recordings, decoded PCM, source paths and
private captures under the ignored local qualification directory.

```powershell
$dekzer = 'C:/dev/dekzer'
$work = '.local/waveform-parity/oracle'
$recording = 'path-to-authorized-recording.wav'
$reference = '.local/waveform-parity/recording-reference'
$comparison = '.local/waveform-parity/recording-comparison'

python -B tools/qualify_waveform_parity.py --dekzer $dekzer --work $work prepare
cargo build --release --locked --features qualification
python -B tools/qualify_waveform_parity.py --dekzer $dekzer --work $work replay --audio $recording --center 79 --output $reference
python -B tools/qualify_waveform_parity.py --dekzer $dekzer --work $work compare --reference $reference --audio $recording --exe target/release/disqro.exe --output $comparison
```

The manifest records actual physical dimensions, DPR, source frames, gain,
intensity and renderer revision per view. The standard ladder runs 64, 32,
16, 8, 4 and 2 seconds at 1920 × 166 device pixels and DPR 1.5, plus
1200 × 132 at DPR 1 and 560 × 36 at DPR 2 for close and overview work.
`--all-sizes` runs each span at all three sizes. `--origin` and `--frames`
clip a shared PCM file at exact CUE frames. `--revision ribbon` and
`--revision spectral` compile and replay the matching historical analysis
and Canvas modules on the same explicitly supplied PCM. A short view of
the direct renderer can run below Dekzer's interactive zoom floor for a
stress control. Disqro's native inspection instead displays the actual
separate signed samples at that density.

Review each case's PNG and `parity.json`. The runner tests deterministic
analysis on the same decoded PCM, then compares the original renderer and
Rust against the reference facts, and finally against Rust's own facts.
It exports source boundaries, band energies and peaks, octave cells, leaf
witnesses, per-column geometry and RGB, final pixels and an amplified
absolute difference. An independent decode can hash both applications'
stereo f32 PCM. The frozen limits are in
[`tolerances.json`](../tools/waveform_parity/tolerances.json). Source-cell
edges, extrema and leaf witnesses must match exactly. Original Stroke
geometry has a 0.002-device-pixel ceiling, colour 0.005 code and final
channels two codes. Canvas views have separate active-pixel P99 and
worst-pixel ceilings, in addition to the independent geometry check.

Native Disqro uploads the CPU-composited pixels as a GPUI image for this
checkpoint. The image and its source revision stay owned by the panel or
player strip. Capture in the actual application exercises the native
renderer, layered view and stereo inspection. The application keeps chapter
overlays on the Full CD strip, while a whole-source replay isolates its
single-recording waveform. The retained DX12/WGSL Dekzer experiment
establishes a native technique for same-device offscreen work. Its shader
uses a different envelope and cannot establish Stroke parity. The separate
GPUI host composition, device lifetimes and physical 120/240 Hz cadence
require qualification after owner approval.

## Checkpoint results

The original Electron replay returned identical hashes on all 26 Breathe
PNGs across two independent windows before the Rust comparison. The 206
final cases passed the frozen gate. The tested Stroke detail cases had
zero device-pixel geometry error, zero RGB transfer error, exact source
edges and exact measured cores. Each reported witness was verified against
its source cell and the underlying stereo samples. The maximum final
Stroke detail discrepancy was one 8-bit code. Complete band facts and
octave powers matched in every tested analysis page. Each Canvas case
had an active-pixel P99 of at most one code and an active-pixel mean
below 0.21 code. The highest single-channel Canvas difference was 64
codes at a sampled edge in Oldskool Fire’s 2 s layered view. Its
column boundaries and band facts matched independently.

| Recording or scope | Rate | Two-second interval in absolute source frames | Stereo f32 PCM SHA-256 |
| --- | ---: | --- | --- |
| Breathe | 48,000 | `[3744000,3840000)` | `76ceb77f074c28380135a50acb29e60741bf9f4ece2e6bb5b46558da3205b731` |
| Oldskool Fire | 48,000 | `[4560000,4656000)` | `8a399987a9d95f593d0bf7285186f4c9e7b75b730eec2a4e412f9fb577e07a42` |
| Thunderdome ’96 CD1 | 44,100 | `[52875900,52964100)` | `06fe513c7b2a15bcd2764ff1649df92d149e7a643b23f28427bc3799c9db1c55` |
| Track 02 of that CUE source | 44,100 | `[34299216,34387416)` | Same CD image PCM, playable interval `[29712816,67403616)` |
| Sequence Blue | 48,000 | `[4896000,4992000)` | `9bf8729996a20d1e845d158897c6cbd87dc6a80429d3ef0bff8f446bb5a4d711` |
| Move Your Body | 48,000 | `[3168000,3264000)` | `492b0d99fe1a31f4e772d6b50f21bd4f2dd044c2e29a1e90a19799c9f271069e` |
| Retained changing-music study | 44,100 | `[4277700,4365900)` | `6016c43d6d82e5ab0cc0acc000a9d238f3b05d66b2c3e2e7749c3b4e943cdaf9` |
| Synthetic control | 44,100 | `[308700,396900)` | `3e5b798228d36af7cc5fd03124c6687b72f54b710f39f05719163433bddd75c0` |

Those intervals use 1920 × 166 device pixels at DPR 1.5. All other case
intervals are in the local render manifests. The original FLAC and WAV
inputs for the first three recordings independently decoded to those
identical hashes. CUE evidence reuses the full-disc PCM. The techno and
hard-dance Opus inputs and the retained AAC study were converted privately
to f32 WAV before supplying both applications. The selected Dekzer decoder
refused the study's original MP4 edit. The two applications independently
decoded the converted input to equal hashes. The Project, Baddadan, and
confirmed drum and bass, rock/metal, acoustic and classical recordings
remain outside the scored set.

The approved-scope muted Windows journeys used the native application on an
undisplayed desktop at 150% scaling, including Full CD and a minimum-sized
window. They exercised viewport zoom and reversal, the layered view and
decoded stereo inspection. The Breathe and full-disc journeys reported zero
decoder and device errors and worst CPU waveform paint calls of 21.8 ms and
17.3 ms respectively, under concurrent qualification. Those are individual
paint callbacks. GPU pass timing, device recovery and physical presentation
cadence remain pending. Review sheets, numeric JSON and native captures are
retained under `.local/waveform-parity/checkpoint`. Owner visual approval
was confirmed for Stroke before the native GPU follow-up.

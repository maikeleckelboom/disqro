# Native Stroke waveform

The implementation now lives in Waveform Native; file paths in this record refer to the historical Disqro revisions it describes. [Waveform Native in Disqro](waveform-native.md)

Disqro uses the accepted Dekzer Stroke appearance in the detailed Waveform panel and the compact Track and Full CD views. The detail deck holds the playhead at the center and scrolls physical-source geometry underneath it. Each Waveform panel keeps its chosen zoom in seconds when playback moves to another recording. Adjacent CUE chapters in the same image share the source coordinates and resident detail pages. Follow mode zooms around the playhead. Panning holds an inspection position with pointer-anchored zoom. **Follow playback** returns the deck to the playhead.

The detail projection uses the current viewport density on each draw. At three frames per device pixel the same Stroke view starts revealing separate left and right source-sample lines. The lines grow into the full presentation by 0.75 frames per pixel. They connect consecutive decoded samples within the physical source. A seek targets the logical interval drawn with the gesture and is checked again by the playback transport. Musical context outside that interval remains visible for inspection.

## Renderer

GPUI Kit 0.10.0 selects its published GPUI Fast 0.1.17 backend on Windows. The vendored platform patch lends the Windows renderer's D3D11 device and accepts a GPU-written BGRA8 UNORM texture through the existing dynamic texture path. GPUI keeps control of scene order, clipping, window composition, and the swap chain. Disqro uses HLSL on that device after reviewing Dekzer's same-device DX12/WGSL renderer. A wholesale host replacement would also change the workspace and component stack. [GPUI Kit backend selection](https://docs.rs/gpui-kit/0.10.0/gpui_kit/) · [GPUI native surface extension discussion](https://github.com/longbridge/gpui-kit/issues/3168)

The Windows dependency graph has no active wgpu dependency. The checked-in backend uses about 0.71 MiB of upstream source, with the bridge confined to the Windows platform crate. The local release executable with qualification enabled grew from 40.93 to 46.20 MiB. This comparison includes the change of GPUI backend.

The detail shader selects source-owned extrema from the finest held interval, folds the 44.1 and 48 kHz octave cells into Stroke color, and evaluates the reference core and fringe coverage. The CPU supplies the reference f64-to-integer partition for the current viewport so large offsets retain the accepted boundary decisions. Compact overviews and other-rate palette pooling keep the reference f64 projection and send one-dimensional geometry or color parameters to the shaders. The application has one device-owned set of pipelines and a 16 MiB detail evidence arena shared by all panels. Immutable pages and PCM tiles upload on admission. Device replacement invalidates the associated GPU state. Pixel and geometry readbacks live in the qualification build.

The worker retains complementary evidence on bounded source tiles, including loaded pages during zoom reversals. A missing detail interval can use projected overview bins while evidence arrives. Missing and measured silence have distinct states. The overview starts with signed extrema and stereo RMS from decoded material. They become the geometry for the final spectral overview. The analyzer uses the existing planned real FFT and reuses overlapping resampled windows while retaining the reference power calculation. SQLite stores packed, versioned evidence. The old waveform tables held derived data and are retired independently of library, queue, and listening data. [Final hardening and Windows qualification](waveform-final-hardening.md)

## Visual qualification

The recovered CPU renderer remains a frozen oracle in the test and qualification builds. The accepted source comparison at `743ec97bda061a3acacea80a7d0ef925d1b9591e` contains 103 Stroke views and 103 historical layered views. The layered outlier remains recorded in the recovery evidence. Three-band rendering, the separate Precision style, their saved style choices, and the standard-rate layered peak and energy analysis have left the application. For rates outside the supported octave grid, the original Stroke color fallback still uses its band energy measurements. The independent live visualization panels keep their own analysis. [Visual recovery and original tolerances](waveform-visual-recovery.md)

All 103 Stroke views met the original limits on a Radeon 780M, including the 44.1 and 48 kHz recordings, CUE segment, and anti-phase control. The same 103 views also passed with fresh evidence from the new analyzer. The replay used 560, 1,200, and 1,920 device-pixel widths and 100%, 150%, and 200% scale. Where the reference exposes detail columns, source boundaries, extrema witnesses, and coverage states matched exactly. Core bounds differed from Rust by at most 0.000016 device pixels. The maximum composed channel difference from Rust was one 8-bit code. The highest active-pixel P99 against Electron was one code. The largest individual composed difference against Electron was 30 codes, inside the Canvas fallback limits.

The 32 and 96 kHz anti-phase controls met the frozen limits in the overview and at 8 s and 2 s. At 21 ms with only envelope evidence, each agrees pixel-for-pixel with Rust and exceeds the Electron limit along a single scanline. The maximum difference is 63 codes over 728 pixels at 32 kHz and 649 at 96 kHz. At these scales the live deck introduces decoded sample geometry as evidence becomes available. Separate shader checks verify decoded stereo and CUE clipping, along with the spectral fallback at 40,123 Hz in both themes.

Five openly licensed recordings passed 45 additional views using fresh evidence. Each recording has an overview and 8 s and 2 s detail at all three sizes and scales. Both applications independently decoded the same converted float WAV to identical PCM. The native GPU and Rust differed by at most one code. The highest active-pixel P99 against Electron was one code. The conversion preserved the original sample rates and limited the long recordings to their first 180 s. The Project and Baddadan still need authorized recordings.

| Material | Rate | Detail center | Recording and license |
| --- | ---: | ---: | --- |
| Drum and bass | 48 kHz | 65 s | TetraHydroCannabolis, [Liquid Drum And Bass](https://commons.wikimedia.org/wiki/File:Liquid_Drum_And_Bass.wav), [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) |
| Rock | 44.1 kHz | 11 s | Skimel, [pop-rock double-tracking example](https://commons.wikimedia.org/wiki/File:Example_of_double_tracking_in_a_pop-rock_song_(3_guitar_tracks).ogg), [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
| Metal | 44.1 kHz | 54 s | Carmina, [En mon nom](https://commons.wikimedia.org/wiki/File:Carmina_-_En_mon_nom.ogg), [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) |
| Acoustic guitar | 44.1 kHz | 24 s | RyGuy, [Acoustic Guitar Sample](https://commons.wikimedia.org/wiki/File:AcousticGuitarSample.ogg), public domain |
| Classical | 48 kHz | 129 s | Czech National Symphony Orchestra via Musopen, [Beethoven's Eroica, II. Marcia funebre](https://commons.wikimedia.org/wiki/File:Beethoven_SymphonyNo.3Eroica_LudwigVanBeethoven-SymphonyNo.3InEFlatMajorEroicaOp.55-02-MarciaFunebreAdagioAssai.ogg), [CC0](https://creativecommons.org/publicdomain/zero/1.0/) |

To replay accepted matches from the local checkpoint against the shader, build with `qualification` and run the following command. The runner compares the GPU's own source witnesses and column data with Rust, and compares pixels with Rust and Electron. The local manifests identify the registered physical extent and source frames of each view.

```powershell
cargo build --release --locked --features qualification
python -B tools/qualify_waveform_parity.py --dekzer C:/dev/dekzer --work .local/waveform-parity/oracle checkpoint-gpu --checkpoint .local/waveform-parity/checkpoint --exe target/release/disqro.exe --output .local/waveform-native-gpu/replay
```

## Loading and performance protocol

The 75-minute fixture is a 44.1 kHz stereo FLAC image registered as 15 CUE chapters. Cold runs use a new waveform cache. Warm runs reopen the populated SQLite cache in the same process. Filesystem caches follow normal Windows behavior. The headless benchmark measures geometry publication, full program coverage, visible detail demand, and completed spectral analysis separately.

| Headless 75-minute fixture | First useful geometry | Full overview geometry | Exact 8 s detail | Complete requested analysis |
| --- | ---: | ---: | ---: | ---: |
| Original Disqro, cold / warm | 3.32 / 0.78 ms | 2.70 s / 88.91 ms | — | 2.70 s / 88.91 ms |
| Accepted CPU Stroke, cold / warm | 16.32 / 10.87 ms | 14.67 s / 95.72 ms | — | 14.67 s / 95.72 ms |
| Native Stroke, cold / warm | 1.71 / 1.82 ms | 2.82 s / 15.99 ms | — | 9.47 s / 16.00 ms |
| Native Stroke with visible detail demand, cold / warm | 16.17 / 2.01 ms | 2.91 s / 13.64 ms | 15.49 / 2.42 ms | 9.32 s / 13.65 ms |

The original Disqro overview measured extrema and RMS. The accepted CPU checkpoint performed spectral analysis before publishing the full overview. Each row is one run in the stated cache state. The new overview held 6.33 MiB of source measurements in memory and 5.86 MiB of SQLite payload, excluding indexes. Cold and warm geometry digests matched. [Original baseline measurements](evidence/waveform-baseline.csv)

The three-hour single-file FLAC opened with first geometry in 17.93 ms cold and 2.22 ms cached. Exact visible detail arrived in 24.19 / 3.21 ms, full overview geometry in 10.31 s / 2.22 ms, and all requested evidence in 26.54 s / 3.21 ms. It held 432 KiB of overview measurements and 832 KiB of SQLite payload including the visible tiles. The original RMS-only run took 9.98 s / 440.06 ms for full overview coverage.

```powershell
$env:DISQRO_WAVE_BASELINE_SOURCE=(Resolve-Path .local/waveform-mission/fixtures/disc.cue).Path
cargo test --release --locked timeline_waveform_baseline -- --ignored --nocapture
$env:DISQRO_WAVE_BASELINE_DETAIL='1'
cargo test --release --locked timeline_waveform_baseline -- --ignored --nocapture
```

The native journey uses a muted playback graph, two detail panels, Full CD and an isolated profile. Its offscreen mode uses a private desktop. The foreground mode requires permission to bring the test player to the front. Native diagnostic draws and input handling are timed independently from offscreen D3D11 timestamp queries and foreground DXGI display statistics. DXGI's flip-model counters track the latest observation and can have gaps during resizing or compositor transitions. They cannot establish hardware input-to-photon latency. [Microsoft's DXGI timing API](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-getframestatistics)

```powershell
python -B tools/qualify_waveform.py target/release/disqro.exe .local/waveform-mission/fixtures/disc.cue .local/waveform-native-gpu/native
```

The GPU timestamp run used 180 offscreen zoom updates per view with warm source buffers. The CPU columns measure waveform preparation and submission. GPU timestamps bracket the detail compute passes and Stroke draw on the Radeon 780M. This test excludes GPUI composition, presentation, and output sampling.

| Offscreen detail view | CPU P50 / P95 / P99 | GPU P50 / P95 / P99 | GPU max |
| --- | ---: | ---: | ---: |
| 8 s at 1,920 × 166 | 0.06 / 0.09 / 0.94 ms | 0.05 / 0.07 / 0.31 ms | 2.13 ms |
| 2 s at 1,920 × 166 | 0.06 / 0.09 / 0.10 ms | 0.15 / 0.19 / 2.98 ms | 5.44 ms |
| 8 s at 2,560 × 420 | 0.08 / 0.11 / 0.14 ms | 0.10 / 0.13 / 0.16 ms | 4.15 ms |

## Initial foreground display observations

The foreground run used the 2560 × 1600 display at 240 Hz and 150% scaling. It used two detail panels, compact Full CD, a cold profile, and continuous muted playback. PresentMon could not start ETW under the standard account. DXGI reported a 4.167 ms refresh period in the foreground. The table counts distinct display updates observed by the swap-chain sampler. An observation can skip intervening display updates. The interval columns therefore describe the recorded gaps.

| Final workload | Distinct observed updates/s | P50 | P95 | P99 | Gaps over two refreshes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Playback and full-CD analysis | 76.7 | 8.33 ms | 25.00 ms | 33.33 ms | 48 of 115 |
| Continuous zoom and library scrolling | 92.7 | 8.33 ms | 25.00 ms | 33.33 ms | 40 of 173 |
| Step zoom and library scrolling | 92.1 | 8.33 ms | 16.67 ms | 33.33 ms | 26 of 112 |
| Sample zoom, pan, and rapid reversals | 104.1 | 8.33 ms | 16.67 ms | 16.67 ms | 43 of 256 |
| Resize | 85.7 | 8.33 ms | 29.17 ms | 50.00 ms | 23 of 95 |

An earlier run observed 218 updates/s during continuous zoom and library scrolling, with an 8.33 ms P95. Its sample phase reached 50.00 ms P99 before sample-segment lookup moved into the per-column compute pass. Sustained 120 and 240 Hz remain unqualified across the complete workload.

The final run peaked at 206.9 MiB of process private memory. WDDM attributed 6.65 MiB of dedicated and 74.47 MiB of shared GPU memory to the process at close detail. Waveform paint callbacks reached 2.63 ms. The longest recorded interval from wheel handling to the waveform draw was 7.88 ms. The session reported 1,867 audio callbacks at up to 0.8% of an audio period, with zero decode, GPU, and device errors. The native capture and raw timing logs are in `.local/waveform-native-gpu/foreground-final`.

# Native Stroke hardening

## Evidence and transport

The detail deck requests the full physical-source interval needed by the Stroke projection. Its planner uses the available source extent, sample rate, physical pixel width and viewport span. As the viewport grows, the planner increases the width of the measured extrema cells and bounds the power data for rates outside the fitted 44.1 and 48 kHz Stroke colour model. Coarse cells still store the actual decoded minima, maxima and their source positions. Wide views skip close-up leaf tiers. The renderer continues to resolve each interval using the finest held evidence. It can show the available chapter overview or useful prior tiers while a new tile is pending. An unreadable tile remains distinguishable from that approximation and from measured silence.

The analysis worker services visible panels round-robin. Each active view pins its complete plan, and identical keys share one allocation. Active CPU bytes are the union of the held plans. Each geometry tile scans at most 1,048,576 input frames, retains at most 8,192 regular leaves plus a possible unaligned leaf, and uses wider leaves as viewport density falls. Retired entries share an additional 8 MiB and 64-entry CPU budget. The GPU retains a 16 MiB shared evidence arena and can evict other panels' uploads while preparing the current draw. Each visible surface adds a BGRA8 texture sized to its snapped physical bounds. The shared pipeline owns the scratch buffers and a single overview preview buffer. Waveform cache v3 replaces the derived v2 data. Library, queue and listening data use their existing stores.

Kira 0.12.6 and CPAL 0.18.2 render device callbacks in Kira's 128-frame internal chunks. CPAL produces WASAPI callback and estimated playback timestamps. Kira discards that timestamp at its desktop backend boundary. Disqro observes the monotonic time when the sound starts processing each callback and pairs it with source advancement after the renderer completes each chunk. Projection uses at most eight recent renderer spans. Small changes in callback arrival time preserve the output sample grid within a 2 ms window. A larger discontinuity re-anchors to the observation. Projection interpolates only within source advance the renderer has already committed and holds at the edge during buffering, a stall, or output failure. This is renderer timing. The WASAPI queue and physical endpoint latency were outside the measurement.

The renderer publishes each span as one atomic snapshot with a single writer and bounded readers. The callback uses preallocated state and atomic operations. The original committed cursor and timestamped accounting queue still control listening credit. The 40 ms transport poll continues to deliver queue and logical recording state. Waveform animation samples the renderer directly between those updates.

Each deck retains its zoom in seconds and its inspection position while contiguous CUE chapters share the same source. A new physical source returns the deck to Follow at the chosen zoom. The center playhead stays fixed. A click uses the transform of the frame the deck last drew. Playback occurrence, voice, seek generation, queue generation, logical interval, and source and CUE revisions travel with the seek. The transport checks the live file identities and the completed renderer position before applying it. Context beyond the displayed logical interval is available for inspection.

## Windows host

The host remains GPUI Kit 0.10.0, `gpui-fast` 0.1.17 and `gpui-fast-windows` 0.1.17. The published Windows archive has SHA-256 `59fb3d59d263955c73b7acf8b19df9ade3ef2805eae81ef740a91217b38e04e2`. The checked-in patch uses `fast/dynamic_textures.rs` to register same-device D3D11 render targets in the existing sprite path. `window.rs` and `events.rs` add a per-window request to present the retained scene. `directx_renderer.rs`, `platform.rs` and `frame_statistics.rs` add opt-in stage, device recreation and frame-marker qualification. `gpui_windows.rs` exports the narrow bridge. The local `build.rs` and `fast/rounded_clip.rs` changes track the generated shader's source and gate its debug path. The Windows crate's additional feature enables the qualification hooks. [Pinned Windows platform archive](https://crates.io/crates/gpui-fast-windows/0.1.17)

One GPUI callback services all visible decks in the window. The callback updates the existing textures before GPUI composes its retained scene. Layout still owns placement, focus, clipping and hit testing. Zoom and pan can update their textures between layout passes. A zoom label refresh is coalesced to at most once per 60 ms of input. The window stops requesting waveform animation when every deck is hidden, inspecting without input, or paused. GPU allocation errors remove the failed surface from the scene and use a 500 ms retry interval while drawing. Device change bypasses the old failure interval and rebuilds the resources on the replacement device.

## Foreground presentation

Two muted runs used the 75-minute CUE fixture, two Waveform decks, a 2560 × 1600 240 Hz display at 150% scale, and the Radeon 780M. The second run reused the cached analysis. The 240 Hz zoom and pan scripts send input to the qualification window only. The same release binary ran both passes and all parity checks. The Dekzer `present-probe` built from `8107c46fd9c7cbb697927a59def9a52f9079dc4f` read a six-colour marker embedded in the composed frame. Its own host cadence measurements informed the same-device approach. Disqro retains its D3D11 host and GPUI scene. [Dekzer reference measurements](https://github.com/maikeleckelboom/dekzer/blob/8107c46fd9c7cbb697927a59def9a52f9079dc4f/docs/native-desktop-foundation.md)

PresentMon 2.6.0 received access denied from ETW. Desktop Duplication recorded independent composed updates with zero foreground-loss samples in both passes. DXGI swap-chain samples and app `Present` calls were captured alongside them. Those sources measure different boundaries. The counts below use all elapsed workload time. The four intervals are p50, p95, p99 and worst. Repeated markers count desktop updates that kept the same test frame. The gap count uses two measured 240 Hz refresh intervals. [Stage timings, display counts and sampled DXGI evidence](evidence/waveform/final-hardening.json)

| Workload | Cache | Distinct composed updates / s | Intervals in ms | Gaps over two refreshes | Repeated markers |
| --- | --- | ---: | --- | ---: | ---: |
| Startup playback | Cold | 173.3 | 4.56 / 8.73 / 12.19 / 17.22 | 116 | 34 |
| Startup playback | Warm | 125.6 | 8.15 / 12.63 / 16.07 / 17.97 | 168 | 325 |
| Steady playback | Cold | 179.6 | 4.57 / 8.81 / 11.79 / 14.83 | 107 | 15 |
| Steady playback | Warm | 131.3 | 7.68 / 13.78 / 16.29 / 19.64 | 150 | 258 |
| Continuous zoom | Cold | 173.9 | 4.58 / 8.89 / 14.05 / 33.13 | 99 | 31 |
| Continuous zoom | Warm | 124.9 | 7.97 / 13.98 / 16.66 / 20.60 | 167 | 287 |
| Continuous pan | Cold | 138.2 | 7.59 / 13.19 / 15.06 / 18.51 | 147 | 257 |
| Continuous pan | Warm | 134.2 | 7.81 / 13.05 / 15.79 / 18.04 | 162 | 256 |
| Playback at 32-sample zoom | Cold | 128.7 | 7.81 / 14.35 / 17.17 / 23.67 | 162 | 283 |
| Playback at 32-sample zoom | Warm | 131.7 | 7.75 / 14.14 / 17.32 / 26.36 | 172 | 283 |
| Library browsing | Cold | 121.1 | 8.19 / 15.81 / 20.06 / 24.57 | 99 | 185 |
| Library browsing | Warm | 120.2 | 7.79 / 17.03 / 20.35 / 25.98 | 88 | 145 |

The steady, continuous zoom, and pan phases lasted three seconds apiece. They reached the 120 updates/s target in both passes. Sustained 240 and longer foreground runs remain unqualified. During repeated resize the app attempted 119–121 Presents/s, GPUI draw p95 reached 10.60 ms, and the duplication probe had 54 and 58 unreadable samples. Treat the resize display counts as incomplete. The two paused phases had zero and one app Present calls while desktop updates continued to repeat the same marker. The observer also reported coalesced desktop updates. A refresh slot without a new observed marker can include a probe miss. The measurement stops at composed desktop frames and gives no physical display latency.

The profiling data separates the bottlenecks. During steady playback the decks shared 711 and 415 retained requests with only 13 and 3 GPUI draws. In the cold steady phase, 11 of 12 Present-call gaps above 8.33 ms contained a GPUI draw of at least 4 ms. In the warm steady phase, 91 of 92 contained a VSync wait of at least 6 ms, and only three contained a GPUI draw of at least 4 ms. The warm zoom and pan traces show the same wait pattern. The retained waveform callback p95 was at most 0.58 ms across those phases, GPUI D3D11 composition p95 at most 1.49 ms, and the `Present` call p95 at most 0.47 ms. Scene invalidation was avoidable during motion and input. The compositor wake path remains the largest measured delay during most long gaps after that change. The probe itself can perturb desktop composition.

Neither deck corrected backward in the steady or 32-sample playback phases. At the normal 8 s zoom the median forward step was 185 and 335 source frames in the two passes. At 32 samples it was 341 and 332. The decks sampled the same physical position at shared frame times. Both foreground runs recorded 2,241 audio callbacks with zero decode, audio-device, and waveform-GPU errors. The worst callback occupied 2.7% of its allotted audio period. Process private memory peaked at 184.2 and 185.8 MiB. At close detail, WDDM attributed 7.4–13.9 MiB of dedicated and 64.2–72.7 MiB of shared GPU memory to the process.

## Visual parity and recovery

All 103 accepted Stroke views and all 45 additional music views passed the frozen tolerances, including the three display scales in the reference. The largest CPU-to-GPU channel difference was one 8-bit code. The largest registered geometry error was 0.000016 device pixels. Cell boundaries, measured witnesses and coverage states matched wherever the reference exposes them. The old 21 ms envelope comparison at 32 and 96 kHz still has a Canvas residual. Above one code, the differences were confined to physical scanline 83, with 728 and 649 affected pixels respectively and a peak of 63 codes. Every affected column was held without a usable extrema witness. Canvas painted the neutral line at RGB 52/59/63 there, while Rust and D3D11 rendered black. The current GPU rendered both envelopes byte-for-byte like the Rust oracle. The residual predates the new presentation clock and occurs with sample traces disabled. The live sample trace has separate qualification for decoded stereo, interpolation and clipping.

Deterministic tests cover the 120 s planner at all supported octave rates and uncommon rates through 384 kHz, wide device-pixel widths, unaligned CUE intervals, an eight-panel residency workload, coarse witness cache round trips, fine/coarse ownership, publication races, seeks across queue and CUE changes, and sample transitions. The hardware test rendered a 5,120-pixel 120 s view at 192 kHz on two D3D11 device instances and compared the source witnesses and pixels. The separate native journey recreated the live GPUI D3D11 device through the recovery path, injected one render-target allocation failure, resumed rendering and exercised repeated resize and pause. That journey ran on a private desktop at 144 DPI. Private native tests also followed unaligned CUE boundaries without backward steps, switched through synthetic Windows DPI changes at 96, 192 and 144 DPI, and closed both waveform tabs while playback continued. No detailed waveform work was scheduled after closure. Real driver reset and physical monitor transfer remain untested.

The native instrument smoke ran live spectrum, stereo field and history through steady playback, 240 resize operations, and minimize. Metering ran in a separate 25-second test. The phases recorded zero audio-device errors, decode errors and dropped capture blocks. The minimized phase produced no spectrum or field work. The foreground runs exercised library row selection and transport pause.

## Full-CD loading

The optimized headless benchmark opened a new cache for each cold pass and reopened it for its warm pair. The 4,500-second, 15-chapter fixture requested exact detail using the live deck's physical-source plan. Cold describes an empty analysis cache. Windows file caching operated normally. All four runs produced the same geometry digest `977f1aeb40e0f543`. Final overview measurements occupied 6.33 MiB. SQLite held 18 evidence records with 6.33 MiB of raw payload.

| Pass | Cache | First useful geometry | Full overview geometry | Exact 8 s detail | Complete analysis |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | Cold | 16.68 ms | 2.91 s | 25.76 ms | 9.19 s |
| 1 | Warm | 2.58 ms | 14.14 ms | 4.76 ms | 14.14 ms |
| 2 | Cold | 14.58 ms | 2.80 s | 22.18 ms | 8.92 s |
| 2 | Warm | 1.42 ms | 7.75 ms | 2.39 ms | 7.76 ms |

## Reproduction

Run the commands from the repository root with Rust 1.99.1. The ignored audio tests exercise the muted Windows endpoint and FFmpeg format fixtures. The Desktop Duplication step requires foreground authorization. The native harness uses a private desktop by default.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
.\build.ps1
.\build.ps1 -Release
cargo test --release --locked --features qualification
cargo build --release --locked --features qualification
$env:DISQRO_WAVE_BASELINE_SOURCE=(Resolve-Path .local/waveform-mission/fixtures/disc.cue).Path
$env:DISQRO_WAVE_BASELINE_DETAIL='1'
cargo test --release --locked --features qualification timeline_waveform_baseline -- --ignored --nocapture
python -B tools/qualifyWaveform.py target/release/disqro.exe $env:DISQRO_WAVE_BASELINE_SOURCE .local/recovery --exercise-host-recovery
```

For composed-frame qualification, provide Dekzer's `present-probe.exe` with `--foreground --display-probe PATH`. Supply a new output directory each time. Use `--warm-cache` with the first run's `Cache` directory for the second pass. To replay the frozen visual fixtures, use the checkpoint command in the native Stroke record. [Parity fixture workflow](waveform-native-gpu.md#visual-qualification)

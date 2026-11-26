# Foreground playback presentation

The accepted baseline submitted approximately 240 frames per second in Composed Flip. PresentMon classified 19.5–22.1% of the ordinary release submissions as undisplayed. The p99 displayed interval was about 12.5 ms. The trace build sometimes moved roughly two refreshes' worth of distance during a single displayed refresh. Differences in GPU workload prevent the old release and trace captures from isolating instrumentation overhead. The old 1.799-second trace used 1,799 samples and is excluded from exact-span motion comparisons.

## Windows changes

The ordinary release sends a coalesced window message when an animation asks GPUI to present a retained scene. A pending Windows paint can consume the same request. Regular painting, redraw-on-resize, and device recovery keep their existing paths. The UI thread admits queued input ahead of a posted frame and the shared window guard defers reentrant draws. The frame worker sleeps when there is no demand. Win32 gives posted messages priority over synthesized `WM_PAINT` messages. [Win32 message retrieval](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-peekmessagew)

GPUI's scene preparation hook invalidates the renderer's instance-buffer reuse state. Waveform-only updates keep the scene and sample the updated waveform texture when drawing into the next back buffer. Resize, device replacement, and offscreen or layer replay invalidate reuse. A tile or rounded-clip draw that overwrites the shared buffers also requires the window's instance data to be uploaded again. The shared swap chain still composites the complete window. Main scene uploads include static control, text, and image primitives. The trace reports their byte count and timing separately from waveform preparation and texture updates.

The qualification executable can compare dispatch, scene uploads, compositor phase, and the presentation queue. `paint` uses the original dispatch and uploads. `posted-uncached` isolates message dispatch. `posted` also reuses scene instance buffers. `early` multiplexes a high-resolution one-shot timer and the compositor's own pulse. The observed pulse recalibrates the timer. The lead is at most one third of a refresh and 1.5 ms. The ordinary compositor wait handles stale timing and missed timer slots. [Compositor clock](https://learn.microsoft.com/en-us/windows/win32/directcomp/compositor-clock/compositor-clock)

`sync-one` tests interval 1 with the original three buffers. `waitable-one` and `waitable-two` retain the three buffers and cap the queue at one and two outstanding frames. `waitable-double` tests the one-frame limit with two buffers. The UI thread checks readiness without blocking. It retains the request if the queue is full so the next refresh can prepare the current motion sample. The base swap chain falls back to its ordinary configuration if waitable creation fails. Layered GPUI swap chains keep their existing sync interval. With flip sequential, interval 0 permits DXGI to discard an older queued update. Queue size and refresh timing still matter when interval 1 is used. [DXGI Present](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-present), [DXGI waitable chains](https://learn.microsoft.com/en-us/windows/uwp/gaming/reduce-latency-with-dxgi-1-3-swap-chains)

## Controlled comparison

The capture runner uses PresentMon 2.6 and the same Windows x64 qualification feature set in both builds. Each take opens an isolated profile with the same layout, starts the same silent recording, enters a duration with an explicit `s` suffix, and checks its source-frame span before recording. At 48 kHz, 1.799 seconds is 86,352 source frames. It captures the same excerpt at 6.000, 2.000, 1.799, 1.000, and 0.500 seconds in two rounds with reversed order. Optional screening repeats the other presentation policies at 0.500 seconds in the same ETW session.

The runner records the build and source hashes, process and UI-thread CPU, sampled private memory, and per-process GPU usage. Its report keeps the p50, p95, p99, and maximum displayed intervals, a refresh-count histogram, discarded and unresolved submissions, join counts, and the distribution of display-linked velocity error. It matches by process, foreground window, DXGI chain, and QPC at the Present boundary. A missed or ambiguous join excludes its adjoining velocity intervals. The reported compositor targets are estimates, and PresentMon's display events provide the cadence observations.

The first matched run used a 1,860 × 1,170 physical-pixel window and a 1,215-pixel waveform. The default candidate submitted 240 updates per second, like the baseline. Its p99 display interval stayed within 8.418–8.488 ms across the ten takes. The baseline ranged from 8.413 to 16.699 ms, with a 58.191 ms maximum in one take. The candidate's worst interval was 16.704 ms. The candidate still discarded 13.40–14.31% of submissions and showed approximately doubled displacement during some single-refresh intervals. At 0.500 seconds, 159 of 199 discard separations in one baseline take were exactly eight submissions. The corresponding candidate count was 156 of 195.

The main scene uploaded 28,400–28,464 bytes on a full upload. With reuse, the median and p95 were zero in the measured foreground takes. Median GPU active time at 0.500 seconds remained near 2.3 ms because the shared swap chain still draws the scene. The two audio-clock runs at that zoom kept phase error below 0.50 ms through p99. These observations narrow the remaining problem to frame delivery and its relationship to rendering cost.

Across the first ten takes in each build, median UI-thread CPU was 11.51% of one core in the baseline and 11.38% in the candidate. Median process CPU was 25.22% and 29.49%, respectively, with wide take-to-take variation. Median sampled private memory was 155.60 and 155.26 MiB. The candidate had a 179.34 MiB sample. These short takes cannot establish an end-to-end input-latency or resource regression bound.

In the first policy screen, interval 1 alone still discarded 12.57% with a p99 of 8.411 ms. The waitable limits of one and two frames had p99 intervals of 12.595 and 12.502 ms. They deferred 220 and 52 of 1,440 dispatch opportunities, respectively. The first early-wake prototype, based on the DWM timing snapshot, used its timer only twice and did not test sustained headroom. The collector expired during the last two policies. Their partial or missing ETW data cannot qualify those policies.

## Window size and recording density

The width study ran 24 six-second foreground takes at 0.500 seconds on the 240 Hz display. It compared one simple sine recording and one denser multitone recording. Both were deterministic, 48 kHz, 16-bit stereo PCM with matched RMS. Their peaks differed. Every pair used the same excerpt and display settings.

| Layout | Waveform width | Posted discards across four takes | DWM missed refreshes across all eight takes |
| --- | ---: | ---: | ---: |
| Split, 1,860 × 1,170 client | 1,215 px | 20.69–21.53% | 133–146 |
| Full-width, 1,860 × 1,170 client | 1,860 px | 21.11–25.42% | 131–162 |
| Maximized, 2,560 × 1,528 client | 2,560 px | 24.58–25.21% | 341–363 |

The maximized route repeatedly showed more missed DWM updates and more application discards. The app's median GPU-active time was about 2.17–2.20 ms maximized and 2.39–2.43 ms windowed. That rules out a simple increase in app GPU-active time as the explanation for the maximized result. The two recordings had overlapping discard and GPU-active results at fixed width. This experiment does not bound the cost of every recording or decoder.

The first pulse-calibrated build recorded too few early dispatches for a sustained timing comparison. A foreground trace showed the worker begin an ordinary wait before the retained callback rearmed. That wait dispatched the real pulse for a slot already delivered by the timer. The Windows worker now carries the outstanding preview across that intervening wake and advances past its confirmation before dispatching another frame.

The corrected scheduler used the timer for all 1,440 submissions in each of two private takes, one split and one maximized. Median lead was 1.124 ms split and 1.107 ms maximized. Those takes qualify frame preparation while the swap chain is occluded. Foreground display events are needed to qualify the effect on displayed cadence.

## Foreground result for corrected early scheduling

The last comparison used the same 48 kHz CUE excerpt in both modes. It completed two takes for each mode at 6.000, 2.000, 1.799, 1.000, and 0.500 seconds in the split layout. It also repeated 0.500 seconds maximized. In the split layout, early scheduling brought displayed p99 to 8.347–8.359 ms across the ten takes and one-refresh velocity error p99 to 1.91–8.61%. Two split takes had isolated 79.175 and 200.017 ms display holds while the app submitted 19 and 48 updates, respectively. Regular two-refresh holds remained.

| Maximized 0.500 s | p50 / p95 / p99 / max display interval, ms | 1 / 2 / 3+ refresh intervals | Undisplayed | One-refresh velocity error p99 |
| --- | --- | ---: | ---: | ---: |
| Posted, take 1 | 4.170 / 8.352 / 8.450 / 12.659 | 924 / 251 / 4 | 18.06% | 102.46% |
| Early, take 1 | 4.166 / 4.197 / 8.320 / 8.460 | 1,392 / 24 / 0 | 1.67% | 99.84% |
| Early, take 2 | 4.167 / 4.301 / 8.343 / 12.643 | 1,321 / 55 / 3 | 4.23% | 100.76% |
| Posted, take 2 | 4.171 / 8.357 / 12.505 / 12.619 | 933 / 232 / 14 | 18.06% | 106.34% |

The release selects early scheduling for a maximized foreground window when the compositor period is at most 5 ms. Other cases use ordinary posted scheduling. The comparison exercised the same early scheduler through its explicit qualification mode at 240 Hz. Display-linked velocity error at 0.500 seconds still approached a doubled step in the worst one percent of single-refresh intervals. Steady one-refresh delivery and manual speaker-to-screen and input-latency acceptance remain open.

In the maximized comparisons, modeled audio-clock phase error p99 stayed at or below 0.52 ms. Process CPU samples varied too widely to establish a regression bound. The four sampled private-memory readings stayed within 196.65–197.09 MiB. The busiest GPU engine averaged 52.15–52.24% in early mode and 54.32–54.57% in posted mode.

## Retained composition surface

The existing GPUI composition code can attach and clip a native child visual and rebind it on device recovery. Its ordinary split-scene path still renders and presents all GPUI layers on each frame. Isolating the waveform's presentation would require retaining the base window content across waveform frames and binding a separate composition-compatible surface. The retained waveform texture is an atlas input to GPUI and cannot be attached to DirectComposition in its current form. [DirectComposition visual content](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositionvisual-setcontent)

DXGI dirty rectangles are available on the shared swap chain. A partial update would need valid contents across the rotating buffers and correct damage from selection, sample markers, and the surrounding window. A composition swap chain can also attach explicit presentation times and report when its buffers can be reused. Direct scanout eligibility depends on hardware and allocation support. Either route would need a foreground comparison in the maximized layout, where the waveform already covers most of the window. [DXGI dirty rectangles](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-1-2-presentation-improvements), [Windows composition swap chains](https://learn.microsoft.com/en-us/windows/win32/comp_swapchain/comp-swapchain)

## Reproducing the capture

Run the collector from a normal PowerShell session and click **Start capture** in the launcher. The launcher transfers focus only to the test window it creates. Close the ordinary player first, approve PresentMon's elevation prompt, and let the test windows remain in the foreground through the run.

```powershell
python -B tools/capture_waveform_foreground.py --baseline $baselineTraceExe `
    --candidate $candidateTraceExe --presentmon $presentMon --source $recording `
    --source-rate 48000 --refresh 240 --output $capture --screen --limit 540
```

Pass `--analyze-existing` with the same output directory to regenerate the summary after the collector releases its files. The fixture source must contain the full measured excerpt and be playable in the regular Disqro input path.

Use `--layouts split wide maximized` to compare the split view with a full-width waveform in the same window and in the maximized window. Use `--comparison-source` to repeat the matrix with a second recording at the same sample rate. The width comparison holds audio fixed. The recording comparison holds layout, zoom, and rendering settings fixed.

Use `--modes posted default --close-layouts maximized` to compare the delivered policy with the ordinary posted scheduler. For manual acceptance, use the regular release at 240 Hz, watch steady playback near 0.500 seconds in the maximized layout, and check responsiveness while scrubbing plus speaker-to-screen synchronization.

## Regression checks

The delivered build passed local formatting, strict Clippy, 243 Disqro tests, and a fresh-profile startup and shutdown. The engine tests, focused Win32 pacing tests, and D3D11 WARP regression suite passed. The muted two-deck scrub and device-recovery journeys completed. In a quiet private comparison with 469 recorded pointer inputs per build, pointer-to-waveform draw p99 was 4.305 ms for the baseline and 4.400 ms for the delivered build.

The delivered trace build produced 103 frozen native images and geometry files byte-identical to the accepted native baseline. The older Electron-reference gate still reports the same 20 historical differences. The hosted Disqro workflow failed while authenticating to the private Waveform Native dependency. Local locked builds used the verified remote revision.

## Build identity

The accepted baseline pinned Waveform Native and the GPUI Windows patch to `aebd66b9de6bb2a9e1f1a60c093caeb711d9a654`. The final foreground comparison used `0e0202e2f34b99370ed952f7ad7d6999d134e81d` for both. The release and its trace companion pin both to `3d9a8f5149483b051df7f0219bf1cbaf4c4c2555`, with the same revision in the lockfile. This revision selects the tested early scheduler for the maximized high-refresh case.

| Windows x64 build | SHA-256 |
| --- | --- |
| `Disqro-foreground-final.exe` | `EA8EBB39C34011A48C4624ABCC21A69402C7CA1A5C37A32CDDC83D23A8FADE9C` |
| `Disqro-foreground-final-trace.exe` | `C80443C5A0C3634643B806336E7247EB347669648ED3E8EAD66161B4A7E774FE` |

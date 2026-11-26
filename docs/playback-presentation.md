# Playback presentation

Disqro samples waveform motion on the Windows window’s current refresh grid. The renderer clock interpolates committed source frames, bridges late publications for up to 4 ms when decoded PCM is available, and corrects sustained callback phase error by slewing the visual rate by at most 0.5%. Paused, starved, and drained renders stop prediction. The transport cursor and listening-history accounting still use renderer commits.

## Baseline and root causes

The baseline is Disqro PR #33 at `fb3abe6a8e29796716388b896602d9ea77ba84e1`. Both the Waveform Native dependency and GPUI Windows patch were pinned to Waveform Native PR #4 at `2357a769c3cd87823273708019ea3f2c8b645808`. The two working branches were created in independent worktrees from those verified remote heads. The integrated build and lockfile use Waveform Native `aebd66b9de6bb2a9e1f1a60c093caeb711d9a654` for both dependencies.

- The old Windows 11 path added a full refresh sleep after a successful compositor wait shorter than 1 ms. The new path uses the compositor’s documented status codes for success, timeout, and display-off. A failure or an unpaced legacy flush sleeps only for the unused part of its local interval. The worker still sleeps while there are no pending frame requests. [Microsoft compositor clock documentation](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-dcompositionwaitforcompositorclock)
- Sampling a global last-wake time exposed animation to worker jitter and let other requests reuse an old tick. Each window now consumes its own timing record. The worker normalizes small timing deviations to the refresh grid and the window advances past expired slots after delayed dispatch.
- A delayed audio callback could leave the old projection capped at its last rendered frame. A callback more than 2 ms out of phase could also move the next block abruptly. The renderer grid now stays continuous during ordinary jitter. A real interruption can re-anchor the grid. CPAL’s WASAPI backend estimates endpoint playback using its device clock and queued frames. The pinned Kira backend discards that callback metadata, so this build measures phase relative to renderer arrival. Absolute speaker-to-screen delay still requires endpoint and display measurements.
- A timing error creates a displacement error of `device width × time error / visible duration`. At 1,215 pixels, a 1 ms error moves the waveform by 0.135 px at 9 s and 0.203 px at 6 s, with larger displacement as the view narrows. This relationship applies across evidence tiers. At 48 kHz and 240 Hz, a refresh advances roughly 200 source samples. A 32-sample view can show a completely different region of the signal on every refresh even with perfect timing.
- Disqro reuses its panel list and projects the retained view once. Waveform Native reuses composed columns when an envelope view stays within a device pixel and the evidence and column footprint match. The coverage shader still runs with the exact phase. PCM views and changed footprints prepare and dispatch their exact projection. The focused WARP test confirms a 160-byte upload for the shader constants on reuse and matches existing fractional pixel and geometry output.

## Qualification on Windows

The host compositor reported a 4.1666–4.1667 ms period on AMD Radeon 780M Graphics. The baseline and integrated captures used a muted 48 kHz CUE program, one 1,215-pixel deck, and six seconds per span. The motion-step distribution improved through p99 at both owner-reported spans. The longer draw-interval tails remained visible in application telemetry.

| Visible span | Build | Motion step p50 / p95 / p99 / max, px | Draw interval p50 / p95 / p99 / max, ms |
| --- | --- | --- | --- |
| 9.000 s | Baseline | 0.562 / 0.574 / 0.599 / 0.647 | 4.166 / 4.421 / 4.690 / 5.119 |
| 9.000 s | Integrated | 0.562 / 0.567 / 0.567 / 3.377 | 4.169 / 4.631 / 6.879 / 28.546 |
| 6.000 s | Baseline | 0.844 / 0.860 / 0.911 / 0.997 | 4.166 / 4.386 / 4.643 / 5.292 |
| 6.000 s | Integrated | 0.844 / 0.850 / 0.851 / 3.369 | 4.169 / 4.477 / 6.505 / 20.419 |

- At 9.000 and 6.000 s, integrated renderer phase-error p99 was 0.825 and 0.816 ms. The maxima were 1.974 and 1.120 ms. The one-deck traces spent up to 0.264 ms in prediction in those holds. Three successful sub-1 ms compositor waits across those holds incurred zero fallback sleeps. GPU instrumentation was disabled for the one-deck comparison.
- The two-deck CUE sweep covered 58 holds, including all wheel notches from 120 s to 32 samples, exact 9.000 and 6.000 s spans, and both sides of 11 geometry/PCM boundaries. It logged zero repeated origins, visible evidence misses, approximate scenes, and dropped trace records in the held phases. A short run at each boundary qualifies the sampled playback, with isolation from the interactive desktop.
- The three-hour 44.1 kHz FLAC also played at the exact spans with two decks and two competing CPU workers. The request for a larger WASAPI buffer still yielded approximately 10 ms callback delivery on this endpoint. The deterministic callback regression exercises 3, 5, 10, and 20 ms callbacks and 60, 120, 144, and 240 Hz sampling. The native trials used one compositor period and one observed endpoint callback cadence.
- Under that load at 6.000 s, draw interval p99 was 19.7 ms and the maximum was 38.6 ms. GPU timestamp span p99 was 19.1 ms and worker-to-UI dispatch p99 was 15.5 ms. Some Present calls took approximately 20 ms while the waveform callback on the same frame took less than 0.7 ms. These traces locate the unresolved tail in UI scheduling and GPU/presentation delay. They cannot identify how much of it would affect an unoccluded swap chain.
- Average shared-device uploads per retained draw fell from about 34.3 to 19.6 kB at 9.000 s and from 34.9 to 29.6 kB at 6.000 s. These include occasional source uploads. The one-deck runs sampled peak private memory of 177.3 and 181.0 MiB. Process CPU averaged 34.4% and 28.4% of one core, with 20.3% and 13.3% on the UI thread. The busiest GPU engine reached 35.0% and 45.4%. These are measurements of the captured runs, with scheduler and GPU load affecting the tails.
- The regular plateaus produced roughly three GPUI scene rebuilds per second in both builds. A retained waveform update does not require a GPUI scene rebuild. Window and player chrome still update when they receive their own invalidations.
- The 103 native frozen renders were byte-identical to the integrated baseline. Both builds had the same 20 pre-existing mismatches against the older Electron reference. The WARP compute reuse test covers fractional phases, both fitted rates, light and dark themes, and the integer boundary. The Disqro suite passed 243 tests with 13 opt-in tests ignored. Waveform Native’s core, GPUI, and D3D11 suites passed, as did the focused Windows pacing and idle-worker tests. The private host-recovery journey recorded the injected target error, rebuilt the device, and resumed waveform draws.

The muted two-deck CUE scrub journey passed pause and resume, reverse release, cancel, sample zoom, and minimize/restore. It used one scrub decoder and reopened no extra transport voices. Both executables were built with the locked Rust 1.99.1 Windows x64 toolchain. The regular release window launched and closed on a private desktop with a fresh profile.

| Executable | SHA-256 |
| --- | --- |
| `Disqro-playback-presentation.exe` | `A5FF0DC37F1789DBF0098654B987F9AEA885B57B088050F4A12C638B445D4240` |
| `Disqro-playback-presentation-trace.exe` | `B2EC7A395A6A32188523E540DC4555A5DAB5CB1E041B870404FAD6CF74102934` |

## Trace contract

Stage CSVs use QPC ticks and carry frame, window, and deck IDs. `compositor_wait` is a pacing pulse and `worker_invalidated` joins a window frame to that pulse. `wave_frame_target` is the estimated next refresh target and `wave_frame_signal` records the wake delay. `wave_motion` and `wave_scene_motion` record generated panel draws. A repaint in the same frame replaces the earlier sample. `present` records the base-chain call and its HRESULT. A successful Present records submission. DXGI counters identify observed display changes when the OS reports foreground data. Gaps in those observations can leave individual Present IDs unresolved.

`audio_callback` and `audio_commit` come from the audio renderer through a bounded SPSC queue. The per-frame clock records phase error, source position, observation age and prediction. Evidence preparation, engine uploads, and UI paint have their own stages. With GPU timestamps enabled, a bounded query pool records elapsed GPU time between the first frame marker and the pre-Present marker on the shared immediate context. This can include gaps between GPU operations. Completion is observed by a later nonblocking poll, so the reported poll delay is an upper bound. ETW can further attribute GPU activity and display outcomes.

## Foreground capture for the owner

Use the qualification release executable. Close any running Disqro instance first so the opted-in process owns the window and trace. Use separate capture folders for 9.000 s and 6.000 s. Play a recording in the foreground at the usual monitor refresh rate and use the zoom readout to enter the exact span.

```powershell
$traceFolder = Join-Path $env:TEMP ('disqro-motion-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $traceFolder | Out-Null
$env:DISQRO_PRESENTATION_STAGES = Join-Path $traceFolder 'stages.csv'
$env:DISQRO_DXGI_TRACE = Join-Path $traceFolder 'dxgi.csv'
$env:DISQRO_GPU_TIMESTAMPS = '1'
$player = Start-Process -FilePath 'C:\path\to\Disqro-playback-presentation-trace.exe' -PassThru
```

For display telemetry, run PresentMon 2.6 during a steady 30-second playback interval. Its delay gives time to return to the player.

```powershell
$captureArgs = @('--process_id', $player.Id, '--session_name', "DisqroMotion-$($player.Id)",
    '--delay', '4', '--timed', '30', '--terminate_after_timed', '--v2_metrics',
    '--qpc_time', '--write_display_metadata', '--no_console_stats', '--no_track_input',
    '--output_file', (Join-Path $traceFolder 'presentmon.csv'))
& 'C:\path\to\PresentMon-2.6.0-x64.exe' @captureArgs
```

Close the player to finish the capture, then run this from the Disqro checkout on the same host.

```powershell
$stageTrace = Join-Path $traceFolder 'stages.csv'
$displayTrace = Join-Path $traceFolder 'dxgi.csv'
python -B tools/qualify_waveform_motion.py --trace $stageTrace --dxgi $displayTrace
Remove-Item Env:DISQRO_PRESENTATION_STAGES, Env:DISQRO_DXGI_TRACE, Env:DISQRO_GPU_TIMESTAMPS
```

PresentMon can identify displayed and dropped frames where ETW supplies those events. The private-desktop results establish application and GPU behavior. Physical display cadence and speaker-to-screen synchronization remain open.

# Continuous waveform motion

The detailed Waveform moves continuously at every zoom level. Earlier builds
drew the following deck at a whole device pixel, so at wide zoom the waveform
stood still for many refreshes and then jumped one pixel. Waveform Native
now draws a detail view at the continuous device position of its viewport,
and the deck samples the renderer clock at the compositor tick that
schedules each frame. [Waveform Native in Disqro](waveform-native.md)

## Root causes

| Cause | Measurement | Correction |
| --- | --- | --- |
| Integer horizontal origin | The projection rounded the viewport's left edge to a device pixel. At 240 Hz on 1,920 px, 120 s visible moves 1/15 px per refresh: the deck showed 8 distinct images in 96 refreshes and held one image for up to 14. 78.846 s and 51.805 s showed 11 and 16. Below 0.1 s visible the decoded sample traces were already continuous, which is why the owner saw smooth motion there. | `View::phase` places the edge continuously. A fractional pixel composes the two Stroke columns it covers by their covered widths, so an integer-aligned view renders byte-for-byte as before. |
| Redraw decision | The retained surface redrew only when the integer view changed, or when sample traces were present. | A changed phase is a changed view, so every motion step redraws; paused views still skip the GPU. |
| Frame tick | GPUI's VSync worker waited on `DwmFlush`, which waits for the caller's outstanding DirectX updates. Earlier foreground runs found a VSync wait of at least 6 ms in 91 of 92 long Present gaps. On this machine, nothing presenting, `DwmFlush` returned at p95 4.90 ms and up to 6.41 ms against 4.17 ms refreshes; the Windows 11 compositor clock returned at p95 4.27 ms. | The platform waits on `DCompositionWaitForCompositorClock` where available. |
| Sampling time | Each frame sampled the renderer clock when the UI thread woke, so wake latency changed the distance moved between frames. | The deck samples at the compositor tick that scheduled the frame (`gpui_windows::frame_tick`) and falls back to the wake time without a recent tick. |

The engine's constant-speed harness ran the same sweep against the previous
engine revision and this one: 29,904 scenarios across 60, 120, 144 and
240 Hz, 44.1/48/96 kHz, 14 device widths at 100–200 % and nine starting
phases. The previous projection erred by up to 0.5 px and repeated the
previous position in 2,758,381 refreshes; the continuous origin has no
error, no repeats and one distinct position per refresh at every duration.
Headless D3D11 renders of transient, bass, swept and impulse material at
the reported durations register within 0.0025 px of the requested origin
(0.012 px for isolated impulses). Waveform Native's
`docs/qualification.md` holds the full tables.

## Native qualification

`tools/qualify_waveform_motion.py` plays the 75-minute CUE fixture muted on a
private desktop with one deck (1,215 device pixels), zooms out to 120 s and
in one wheel notch at a time, and holds each duration for four seconds. It
compares the `e758d7d` qualification build with this branch on the same
machine, which was in interactive use. It records what the application drew
and when; a private desktop has no physical display cadence.

| Visible | Build | Retained draws/s | Distinct origins/s | Repeated origins | VSync wait p50 / p95 |
| --- | --- | ---: | ---: | ---: | --- |
| 120.000 s | `e758d7d` | 238.7 | 10.2 | 914 | 4.08 / 4.58 ms |
| 120.000 s | continuous | 237.2 | 237.0 | 1 | 4.05 / 4.24 ms |
| 78.846 s | `e758d7d` | 195.5 | 15.7 | 719 | 4.12 / 8.98 ms |
| 78.846 s | continuous | 239.2 | 239.2 | 0 | 4.06 / 4.22 ms |
| 51.805 s | `e758d7d` | 127.2 | 23.5 | 415 | 8.07 / 11.99 ms |
| 51.805 s | continuous | 240.0 | 240.0 | 0 | 4.07 / 4.32 ms |
| 8.000 s | `e758d7d` | 120.7 | 108.5 | 49 | 8.33 / 12.91 ms |
| 8.000 s | continuous | 238.2 | 238.2 | 0 | 4.07 / 4.37 ms |

With `DwmFlush`, the wait doubled whenever the deck's content changed every
frame, holding the deck near 120 draws per second; this matches the earlier
foreground result of 131 updates per second during steady playback. On the
compositor clock, 99 % or more of frames were scheduled by compositor ticks,
and the deck sampled the renderer clock a median 0.09–0.11 ms after its tick
(p99 0.6–3.5 ms). The remaining variation in the distance moved per frame is
the variation of the tick intervals themselves.

Physical 240 Hz presentation on the owner's display, composed desktop
updates, and input-to-photon latency remain an owner-authorized foreground
measurement.

## Stroke cells without source frames

The fitted 44.1 kHz partition skips a display cell about every 500 cells.
Earlier builds drew that cell as silence and pulled its neighbors' fringes to
the zero axis: a thin gap between two vertical spikes, which the owner saw at
0.145 s visible in a 44.1 kHz house recording. The cell now shows the
next cell with frames, and fringes look past it. Cell boundaries and
measurements are unchanged. The accepted frozen references contain the same
defect in 20 of 103 views, so those views now differ from them in the
affected columns only (Waveform Native, Stroke specification §3).

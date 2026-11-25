# Disqro waveform candidate

This document records the reviewed PR #23 baseline. The
[Dekzer waveform recovery checkpoint](waveform-visual-recovery.md) describes
the current implementation and links the reproduced comparison procedure.

The transport's Track and Full CD/Album perspectives use the same spectral data as the new Waveform dock panel. Add more than one Waveform panel to hold different views of the playing recording. The mouse wheel zooms around the pointer, Shift+wheel or horizontal scroll pans, a click seeks to the corresponding decoded frame, and **Fit** restores the whole recording or active CUE chapter. Each dock instance retains its own style.

| Style | What it shows |
| --- | --- |
| Spectral | Signed peak silhouette and RMS body with the same low-orange, mid-violet and high-cyan mapping in the panel and compact player. |
| Three-band | Three separate lanes with a fixed amplitude scale. They expose concurrent frequency activity even when one band dominates the mix color. |
| Precision | A neutral signed envelope. At the closest zoom, all views use a separate left and right trace from actual decoded frames joined by straight segments. |

## Source ownership

- `timeline::Program` retains the decoded absolute source frame interval and the integer disc-time interval. The dock viewport projects chapter-relative frames onto its current bounds. A click inverts that transform and sends an occurrence-scoped seek through the player. Playhead position comes from transport status.
- One background worker owns the decoded packet, short spectral filter history and cache access. Each source cell records signed stereo extrema, the frame of each extremum, stereo power and three band powers. Each spectral page starts with the same 100 ms of available preceding source samples for both a seek and sequential analysis. Its color estimates are descriptive bands.
- The compact projections and dock tiles share the same source and measurements. The disk cache keeps four-second spectral pages with a new derived version, native file identity where available, size and modification time. It retires the old peak-only derivative. The library and other player data use their existing storage.
- The 32-entry resident detail store admits tiles from visible panels, deduplicates matching source revisions, and evicts inactive data. Maximum resident measurement payload is about 3 MiB plus the currently decoded packet, overview bins and paint state. At deep zoom, the worker keeps only bounded PCM tiles for the requested source positions. A revision change discards old publications.
- The zoom curve changes continuously on animation ticks. The current and destination ranges can both request detail during a gesture. Missing input coverage does not supply a peak or a silence sample. When a projected interval cuts through a coarser bin, only extrema with witnesses inside that interval contribute. Band and RMS weighting at such edges is an approximation from the held facts. The closest trace uses source positions and signed stereo values directly.

## GPU decision

The candidate uses GPUI's pinned D3D11 scene for quads and paths. This preserves the native window, docking and input stack. Source projection and geometry preparation run on the CPU, and GPUI uploads scene data for drawing. The candidate does not establish a shader-resident waveform pipeline. The separate Dekzer run demonstrates that its same-device DX12 pass is viable offscreen. A Disqro host integration must demonstrate comparable window behavior, a bounded cache of GPU-resident source pages, and better end-to-end timing before a host change can be selected. [Research and backend comparison](waveform-research.md)

## Optimized headless measurements

Same synthetic recordings and machine as the [extrema/RMS baseline](waveform-research.md#optimized-headless-baseline). Each result is a single optimized run. A cold derivative run began with an empty database and a warm run reopened the populated database in the same process. The cold and warm candidate geometry digests matched in all five cases. [Candidate measurements](evidence/waveform-spectral-candidate.csv)

| Fixture | First geometry cold / warm | Complete candidate cold / warm | Complete baseline cold / warm |
| --- | ---: | ---: | ---: |
| 24 s control WAV | 4.369 / 0.832 ms | 24.047 / 2.090 ms | 8.598 / 0.827 ms |
| 3 min WAV | 6.027 / 0.680 ms | 204.997 / 9.559 ms | 54.264 / 4.077 ms |
| 10 min FLAC | 6.766 / 0.743 ms | 868.333 / 27.331 ms | 355.018 / 13.158 ms |
| 75 min FLAC/CUE | 7.557 / 1.400 ms | 7.836 s / 224.576 ms | 2.697 s / 88.906 ms |
| 3 h FLAC | 12.168 / 1.014 ms | 14.484 s / 466.019 ms | 9.978 s / 440.064 ms |

The 75-minute source's active chapter completed in 452.876 ms cold and 13.276 ms warm. The final Track and CD bins retained 1,172,880 bytes, double the smaller peak-only bins. Full-disc completion is slower with spectral filtering and the wider cache records. Reusing SQLite statements and parsing the borrowed page bytes brought warm completion down from 379.799 ms in the previous candidate run. The three-hour source published initial geometry sooner by starting its publication throttle after its first usable bin. These are analysis timings through the callback.

## Isolated Windows qualification

The release qualification harness used a separate Windows desktop, an isolated profile and a hard-muted audio graph. At 150% scaling it opened Spectral and Three-band dock instances beside the library, with the transport waveform below. The journey exercised animated wheel zoom, rapid direction changes, library scrolling during playback and a resize to 720 × 460 logical pixels. [Whole-source control](evidence/waveform/overview.png) · [Bass decay at 834 ms visible](evidence/waveform/bass-tail.png) · [1,277 source samples in stereo](evidence/waveform/stereo-samples.png) · [Compact 75-minute CUE perspective](evidence/waveform/full-cd-compact.png)

| Synthetic journey | First canvas paint | Worst waveform canvas CPU | Peak process private memory | GPU memory at close detail, device / shared | Analysis pages from decode / cache |
| --- | ---: | ---: | ---: | ---: | ---: |
| Short control | 16.265 ms | 2.704 ms | 183.1 MiB | 5.25 / 100.22 MiB | 6 / 6 |
| 75-minute CUE, cold derivative | 20.174 ms | 4.161 ms | 196.7 MiB | 10.26 / 95.72 MiB | 1,125 / 7 |
| 75-minute CUE, copied warm derivative | 15.204 ms | 4.015 ms | 203.1 MiB | 7.76 / 100.72 MiB | 0 / 1,132 |

These are single application runs with a sampled private-memory peak and one GPU memory sample per run. The memory counters cover the whole app and graphics context. The callback counts were 571, 567 and 568, with a maximum recorded callback budget use of 0.6%, 0.6% and 0.5%, respectively. No decoder, device or discarded-device errors were recorded. Canvas timings cover CPU work in the paint callback. This harness cannot measure physical scanout or input-to-display latency. [Native run reports](evidence/waveform/native-runs.json)

The control's low, mid and high blocks stayed distinguishable in the compact player and large view. The strongest impulse column was x=1001, about one physical pixel from the source-time prediction of x=1002.1 in the wide capture. The bass region showed a decaying envelope at 834 ms, then separate signed traces spanning 1,277 frames, about 29 ms. At that scale a 70 Hz cycle should span about 598.9 physical pixels. Two neighboring troughs in both traces were 599.5 pixels apart. The measured right/left vertical excursion ratio was about 0.78 against the fixture's 0.80. The decoder test separately checks both signed channel values around a one-frame impulse and CUE boundary. Continuous viewport geometry has deterministic anchor tests. Visual continuity through detail arrivals and LOD changes still needs frame-by-frame assessment.

Two scope-matched comparisons used the archived audio copies. Near the archived Oldskool Fire passage at 1:36, the 8.415-second Disqro view separates a short lower-energy interval from dense blocks. The bands view makes the bass envelope easier to follow across the whole source. The spectral mix is still pale in sustained distorted sections compared with the Studio reference. Near 1:19 in Breathe, the 7.964-second view shows narrow attacks above the quieter RMS body. The previous Studio captures give a coarser timestamp and descriptive zoom. Both comparisons use the same recordings and approximate regions, with independent display bounds. The Disqro originals are in `.local/waveform-mission/native-archive-hardcore/` and `.local/waveform-mission/native-archive-vocal/`.

The compact layout held the browser, both independent waveform surfaces and the existing transport at minimum size. The width and height inspection used 150% Windows scaling. Foreground motion, native device-loss recovery, a second DPI setting and physical 120/240 Hz presentation remain open.

The local gate passed formatting, strict Clippy, 205 locked tests with 12 opt-in checks excluded, the additional release codec/decode/seek check, and locked Windows debug and release builds. The waveform-specific cases cover large-offset zoom anchoring and reversal, exact signed samples at a CUE join, equal page results with sequential and random decode, verified cache reuse, same-path replacement with an unchanged size and mtime, source revision rejection and shared eviction.

Reproduce the isolated control run with the existing qualification flag and a fresh output directory.

```powershell
cargo build --release --locked --features qualification
python tools/qualifyWaveform.py target/release/disqro.exe .local/waveform-mission/fixtures/control.wav .local/waveform-mission/native-replay
```

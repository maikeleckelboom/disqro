# Disqro waveform research

## Production baseline

The Disqro branch starts at `a7d71f078bf7adac254cad093bbdc2acdefe5092`, tree `18a26e68301121bcc39556da7919b791837fe735`. Local `main`, the local tracking ref and the live `origin/main` agreed on 2026-11-24. The Disqro worktree was clean and was the only registered one. Dekzer was inspected by reading its files and Git objects. Its pre-existing `.gitignore` edit and other worktrees were left in place.

| Layer | What the pinned source does | What a replacement needs to preserve |
| --- | --- | --- |
| Decode and ownership | `Recording` uses Symphonia and Kira's stereo frame type. The analysis worker owns one decoder and a packet buffer. Playback owns a separate streaming voice. Both apply the existing format, trim and sample-rate rules. | Authoritative decoded frames and bounded background work. No new waveform responsibilities in the audio callback. |
| Source facts | `waveform::Reader` measures signed left/right extrema and accumulated stereo power in 300 cells per second. Four cell boundaries per CUE tick preserve the supported INDEX boundaries. The last cell stops at decoded EOF. | Exact half-open source intervals, channel polarity, peaks, power and explicit coverage. Add frequency information and fine detail to the same source facts. |
| Cache | Four-second pages in SQLite, keyed by encoded path, byte length, mtime and page number. `Stamp` checks bracket uncached analysis. Page pruning limits the store to about 4,159 pages. | Bounded, disposable, versioned derivatives and revision checks. Verify stable file identity before considering reuse across relocation. |
| Projection | `Analysis` resolves the playback program, starts at the active position, and publishes immutable Track and Full CD/Album projections as coverage completes. The output width is capped at 1,536 bins per track. Each disc projection gets a proportionate budget. | Fast first useful coverage. A missing interval stays distinct from measured silence. Surfaces share facts and own only their projections and interaction state. |
| Rendering and input | The transport strips render a peak silhouette, RMS body and played/ahead colors using GPUI quads. The view reduces the projected values again per logical-pixel column. Track/Full CD switches and drag previews are present. The current strip has no wheel zoom or raw-sample mode. | Preserve the transport placement, CUE labels, seeking gesture, pause state and source geometry in the comparison. Both the detail panel and compact player should benefit from spectral rendering. |
| Time authority | `Program` links a segment's half-open decoded frame interval to its half-open disc-nanosecond interval using widened integer arithmetic. A join belongs to the next segment. Player progress comes from transport status and seeks are occurrence-, queue-, source- and sheet-revision scoped. A generation check rejects outdated analysis updates. | Keep one authoritative transport position. Use the same viewport transform for waveform geometry, overlays and input. Preserve authored gaps, offline chapters, queue edits and Mission 22 accounting. |
| Dock integration | `WaveStrip` is a player entity. The dock has separate live-spectrum, spectral-history and stereo-field instruments. The pinned live spectrogram describes playback history and has a different data lifetime from a seekable source waveform. | Use a dock panel and the existing transport consumer. Share decoded-source waveform data across both. |

The transport baseline can be replayed from Git. Existing [Track and Full CD captures and timings](musical-timeline.md) preserve its native appearance. The tracked pin does not contain a source-indexed spectral waveform renderer. The term *spectral waveform* in the mission may refer to a local or historical candidate outside the inspected tracked baseline. Keep any such evidence if located.

### Optimized headless baseline

Windows 11 build 26300, Ryzen 7 8845HS, 44.1 kHz stereo synthetic inputs. Each row is one release measurement per cache condition. The timer includes program resolution and analysis through the callback. First geometry requires at least one complete bin for the active track. Active complete requires both of that chapter's projections to be complete. Cold starts with an empty derivative database. Warm opens a fresh connection to the same database in the same process. OS filesystem caches were left alone. The retained-byte figure covers the final bins only. [Raw results](evidence/waveform-baseline.csv)

| Fixture | First geometry cold / warm | Active complete cold / warm | Program complete cold / warm | Final bins |
| --- | ---: | ---: | ---: | ---: |
| 24 s control WAV | 2.110 / 0.359 ms | 8.551 / 0.825 ms | 8.598 / 0.827 ms | 69,120 B |
| 3 min WAV | 1.803 / 0.365 ms | 54.244 / 4.071 ms | 54.264 / 4.077 ms | 72,000 B |
| 10 min FLAC | 2.970 / 0.598 ms | 354.992 / 13.155 ms | 355.018 / 13.158 ms | 73,248 B |
| 75 min FLAC/CUE, 15 chapters | 3.315 / 0.781 ms | 181.433 / 6.729 ms | 2,697.316 / 88.906 ms | 586,440 B |
| 3 h FLAC | 74.085 / 70.597 ms | 9,978.111 / 440.059 ms | 9,978.140 / 440.064 ms | 73,728 B |

The cold and warm geometry digests matched for all five fixtures. The multi-hour overview needs several four-second pages to complete its first bin. The existing 70 ms publication throttle helps explain the delayed first geometry even after the cache is primed. These measurements supply the CPU-side baseline. Native paint, GPU timings and process memory need separate qualification.

Generate the 24 s control using the [Studio procedure](serato-waveform-procedure.md). For the duration sweep, loop the two-second source below with FFmpeg 8.1.1. Give the CUE image 15 chapters at five-minute intervals.

```powershell
$dir = '.local/waveform-mission/fixtures'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$signal = 'aevalsrc=if(lt(mod(t\,2)\,1.6)\,0.46*sin(2*PI*70*t)+0.21*sin(2*PI*880*t)+0.06*sin(2*PI*6300*t)\,0)|if(lt(mod(t\,2)\,1.6)\,0.38*sin(2*PI*70*t)+0.19*sin(2*PI*1320*t)+0.08*sin(2*PI*5100*t)\,0):s=44100:d=2'
ffmpeg -v error -nostdin -n -f lavfi -i $signal -c:a pcm_s16le "$dir/loop.wav"
ffmpeg -v error -nostdin -n -stream_loop -1 -i "$dir/loop.wav" -t 180 -c:a pcm_s16le "$dir/track.wav"
foreach ($seconds in 600,4500,10800) {
    ffmpeg -v error -nostdin -n -stream_loop -1 -i "$dir/loop.wav" -t $seconds -c:a flac "$dir/$seconds.flac"
}
@('FILE "4500.flac" WAVE') + @(0..14 | ForEach-Object {
    '  TRACK {0:00} AUDIO' -f ($_ + 1)
    '    INDEX 01 {0:00}:00:00' -f ($_ * 5)
}) | Set-Content -Encoding ascii -LiteralPath "$dir/disc.cue"

$env:DISQRO_WAVE_BASELINE_SOURCE = (Resolve-Path "$dir/disc.cue").Path
cargo test --release --locked timeline_waveform_baseline -- --ignored --nocapture
Remove-Item Env:DISQRO_WAVE_BASELINE_SOURCE
```

## Comparative evidence

| Reference | What the evidence supports | What to carry forward |
| --- | --- | --- |
| Disqro transport | The original 77-minute CD qualification reported 55.899 ms to first verified chapter geometry and 68.793 ms to first GPUI paint on the cold run. The warm run reported 68.435/81.026 ms. These are single historical startup measurements. | Independent Track/CD perspectives, meaningful quiet and pending states, and chapters synchronized to the transport. |
| Dekzer native, album playback (`bfeff88`) | A single GPUI-owned Direct3D 12 window composites custom `wgpu` targets on the same device. A historical 77-minute run had a 63 ms amplitude preview, 4.58 s to first spectral draw and 731.6 MiB peak process private memory. An earlier matched run measured 4.15 s shader compilation and about 153 MiB of allocated waveform textures. The native window's application render timing did not measure scanout. | Reuse the ownership and device-generation precedent. Qualify pipeline creation, bounded tile residency and interaction in Disqro before selecting an architecture. |
| Historical Serato DJ Pro/Lite studies | Dekzer preserves separate edition-tagged captures, source hashes, counterexamples and independently replayed observations. The inferred node models failed some fixed-origin and stationary controls. | Reuse the control design and comparison methods. Keep edition labels with every artifact. |
| Serato Studio | The Sample Deck showed a full-source overview, frequency-dominance colors and a fixed center playhead. One wheel notch changed the observed detail scale by about 2×. Near the closest scale, the 70 Hz control exposed individual cycles. The source-sample limit and stems behavior remain open. [Sample Deck documentation](https://support.serato.com/hc/en-us/articles/360001445655-Sample-Deck-Deck-Waveform-Area) · [Stems documentation](https://support.serato.com/hc/en-us/articles/6214861307663-Misc-Stems) | Carry forward readability at both scales. Test continuous anchoring independently of Studio's observed wheel increments. |

Local Dekzer evidence is indexed by `docs/waveform-visual-excellence.md`, `docs/waveform-signal-facts.md`, `docs/waveform-direct-presentation.md`, `docs/waveform-stroke-resolution.md`, `docs/native-album-playback.md` and `docs/research/waveform/evidence-and-recovery.md`. The `feat/native-album-playback` Git ref also contains the renderer factory, GPU-residency code and historical qualification report. Keep private media and captures at their original locations.

### Serato Studio control session

Serato Studio 2.5.0, build 2.5.0.52, ran on the 2560 × 1600, 240 Hz Radeon 780M display at 150% Windows scale. The captured window was 1707 × 1019 logical pixels and the Sample Deck detail was about 772 × 426 logical pixels. The project read 120 BPM. The deck showed 123 BPM with sync controls highlighted. Captures used stopped transport and source events known from `control.wav`. The imported WAV and a regenerated original had identical decoded PCM (SHA-256 `27b7e62e9818042b54c09150cbd5f13c36ab6a5f5c9f95a0aae66bce8afca1b2`). The capture tool produced original JPEGs archived under `.local/waveform-mission/serato-studio/`. Use them for qualitative color and approximate pixel measurements.

| Control | Direct observation | Capture ID |
| --- | --- | --- |
| 70, 880 and 6,300 Hz at equal input amplitude, mixture and sweep | Red, green and blue runs separated cleanly. The mixture was largely neutral gray. The sweep changed bands at a continuous source position. The same blocks were distinguishable in the thin overview. | `control-frequency-blocks` |
| 70 Hz block | The onset remained the same estimated distance in source time from the centered playhead. A zoom-out notch compressed the cycles from roughly 26 to 13 logical pixels apart, even with the pointer left of center. A further zoom-in notch at the closest stop did not increase the scale. | `control-low-oscillations`, `control-low-one-notch-out` |
| One-frame stereo impulse at frame 308,737 | A thin, dark gray mark extended above and below the zero line. At x≈424 on one close view and x≈453 on the next, displacement from the x≈395 playhead doubled even though the pointer was at x≈620. Adjacent silence stayed on the zero line. | `control-impulse-phrase`, `control-impulse-close`, `control-impulse-closest` |
| Mixed attack at 9 s | The dark red bass tail was legible in the phrase view and resolved into individual cycles at the following two scales. The fast treble burst did not show a distinct hue in these captures. The waveform became visibly asymmetric at close scale. | `control-attack-phrase`, `control-attack-beat`, `control-attack-closest` |

Registered by the source control, the closest 70 Hz view corresponds to roughly 24 source frames per captured logical pixel. Thus the capture supports oscillation-level inspection. It cannot establish exact sample-level display or distinguish sample interpolation methods. A difference between the high-amplitude gray impulse and the red tail shows why amplitude and color should be independently inspectable in Disqro. Neither the pixel snapshots nor the advertised display refresh rate measure presentation cadence.

### November 16 Serato archive

The existing `Serato-Studio-Waveform-Research-2026-11-16` collection supplies 70 original 2560 × 1528 RGB PNGs from Studio 2.5.0. All 70 PNGs independently matched their adjacent SHA-256 records and passed an image-integrity and dimension check. The archive contains full-source strips and multiple views of 19 identified sources plus one existing CD2 session. Capture provenance, source checks and acquisition limitations are recorded in `capture-manifest.md`, `visual-research-report.md` and `capture-index.csv` in the archive. The originals and working audio are retained there.

| Corpus | Observation from selected archived frames | Comparison boundary |
| --- | --- | --- |
| Thunderdome III CD2, Oldskool Fire and The Project | The CD's thin overview remains colored. Dense blocks can fill nearly all of the detail pane. A closer frame exposes the attack and decay beside a dense block. The Project shows a diminishing tail after high-energy peaks. | The CD file, sample position and load timing are unverified. Visual saturation alone says little about clipping. |
| Techno, hip-hop, house and DnB | Magnification exposes contours around repeated attacks and their tails. The closest techno frame shows a thin oscillating contour. DnB can retain strong hue changes inside a dense envelope. | Dates and displayed positions are recorded. Zoom labels are descriptive and per-capture PCM registration is unavailable. |
| Acoustic, rock/metal, ambient and classical | Acoustic attacks remain separate from quieter intervals. Rock and ambient examples retain local color changes. Classical examples are comparatively dim at both archived scales. | Individual recordings provide regressions to inspect. The captures cannot establish a genre-wide rule or a defensible gain curve. Several working sources are decoded from lossy originals. |
| Breathe and Baddadan stem sequences | When all stems were muted, the gray contour in the detail pane visually corresponded to the full mix's outer silhouette. With a subset active, colored structures remained inside and alongside the gray. The thin strip kept original color in these frames. | The comparison supports preserving original context for future stem surfaces. It cannot determine the underlying geometry algorithm or sample alignment. |

Re-use the archived musical passages to check visual readability. Use the generated PCM fixtures to verify exact geometry, stable timing and zoom registration. Preserve the classical case as a contrast check at the same fixed gain and panel height as dense material. Keep stems and original context as separate visual semantics in any future stem-specific surface.

## GPU candidates

1. Measure a GPUI-native scene built from exact, reusable projected data. The pinned Windows build already renders quads on D3D11. Its renderer uploads scene buffers, so measure CPU primitive construction, per-frame upload and presentation separately. This sets the integration and performance floor.
2. Measure same-device `wgpu` composition in GPUI. The lock pins `gpui-kit` 0.10.0 and `gpui-pre` 0.3.12. That version's `Window::paint_surface` is Apple-only and exposes no Windows `gpu_canvas` or renderer factory. The Windows dependency graph does not currently build `wgpu`, even though it appears in the cross-platform lockfile. The published `gpui-pre` 0.3.14 source retains that limitation. Dekzer's proven host uses a different distribution, `bite-gpui` and `bite-gp-wgpu` 1.25.203, on `wgpu` 30.0.1. Check GPUI Kit API compatibility, device recovery, text and popup composition, DPI and window presentation in an isolated prototype before adopting a host change. [Pinned GPUI window source](https://docs.rs/gpui-pre/0.3.12/src/gpui/window.rs.html) · [Same-device canvas API](https://docs.rs/bite-gp-authoring/latest/gpui_authoring/fn.gpu_canvas.html)
3. Hold cross-device shared textures as a secondary experiment if the same-device host fails qualification. `wgpu` uses DX12 on the Windows Direct3D path. D3D11 composition via sharing would introduce a second device and synchronization. The atlas image path would add a GPU-to-CPU readback for computed pixels and a CPU-to-GPU upload. Exclude that transfer path from interactive candidates. [wgpu backend documentation](https://wgpu.rs/doc/wgpu_types/backend/enum.Backend.html)

Grade candidates together on source truth, musical readability, loading and native presentation latency, memory at several source lengths, resource recovery, and maintenance cost. Share source identity, reuse cached CPU data across device recreation, and share uploaded read-only facts per device and source revision. Keep viewport, style and interaction state per view. Preserve old behavior through commits, fixtures and capture provenance.

The retained Dekzer native offscreen executable rendered the 24-second control on the Radeon 780M with DX12. At 2560 × 420, the 1-to-64-second zoom sweep used 0.312 ms median, 0.571 ms p95, 0.602 ms p99 and 0.903 ms maximum GPU pass time over 1,200 submissions. The 8-second scrolling workload used 0.543/0.771/1.194/1.263 ms. The source allocated 3.42 MiB of GPU facts and uploaded 0.76 MiB. CPU page submission ranged up to 4.360 ms over two samples. The run produced nine local renders with no GPU validation errors. These numbers characterize an offscreen Dekzer pass on a prebuilt historical binary (SHA-256 `a6359687d8a1cf51acf446dd21a5179a9cef3d6ee579f167b6292e7e7cdf66f8`). They establish the potential cost of a native shader on this adapter. They do not qualify a Disqro host change or displayed-frame cadence.

## Findings ledger

| Status | Finding | Follow-through |
| --- | --- | --- |
| Verified | Existing source pages and CUE offsets support mixed sample rates, chapter joins and exclusive EOF; the eight optimized timeline tests passed. | Keep these assertions as the source/transport regression floor. |
| Confirmed defect | Track projection knows a bin's end frame but loses the individual extrema locations and original samples. Rendering uses both extrema for every destination column that overlaps a bin, even when the actual witness is outside that column. | Compare exact visible-interval output against enumerated PCM. Retain extrema witnesses and fetch boundary detail. |
| Observed | The worker's bounded UI channel uses blocking sends. Cancellation is observed at packet/page boundaries and before publishing. A worker stalled on a full channel can wait for a reader even after supersession. The player still rejects old generations. | Test rapid replacements under backpressure and move the candidate toward bounded, latest-revision publication. |
| Hypothesis | A compact spectral overview can retain percussion legibility with a quieter peak silhouette, an energy body, and per-interval band dominance. | Compare it with signed geometry and independent band layers using the same measured facts and fixtures. |
| Hypothesis | A small persistent source overview plus a bounded resident detail window can decouple long-recording memory from duration. | Measure cold/warm completion and random access, and verify the behavior at tile boundaries and under simultaneous demand. |
| Rejected alternative | A separate DSP and cache path for each presentation would duplicate work and invite inconsistent musical semantics. | Use unified signed extrema, RMS, stereo and spectral facts. Compact, detailed and precision views can consume different levels from the same source. |
| Observed | In the Studio control session, broad red/green/blue separation persisted into the overview. Close detail revealed individual low-frequency oscillations. Wheel zoom used the playhead as its anchor and approximately doubled or halved the scale per notch. | Use the same source control for candidate comparisons. Keep the color interpretation independent of signed peaks and preserve continuous viewport motion. |
| Unresolved | Neither GPU candidate has been measured in the Disqro host. Studio load timing, full-CD interaction, stems, matched real-music comparisons and sample-level behavior remain unqualified. | Continue the [Studio procedure](serato-waveform-procedure.md) and compare focused native candidates. |

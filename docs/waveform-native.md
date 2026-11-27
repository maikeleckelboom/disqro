# Waveform Native in Disqro

Disqro's waveforms are drawn by Waveform Native, an independent engine
pinned in `Cargo.toml`. Disqro depends on it; it never depends on Disqro.

## What the engine provides

Analysis of decoded PCM (signed extrema with witnesses, RMS, leaf evidence,
Hann512 and Blackman512 octave spectra, fallback bands), progressive
overviews, the self-checking evidence encoding, multiresolution planning,
the source-scoped evidence store, viewport and pixel registration, the
Stroke display laws, the Direct3D 11 renderer and the GPUI surfaces that
composite it on GPUI's device.

## What stays in Disqro

| Module | Responsibility |
| --- | --- |
| `waveform_source.rs` | Symphonia/Kira decoding through `Recording`, file identity and revision checks, the SQLite table `wave_evidence` and its pruning |
| `waveform_detail.rs` | Maps validated sources to engine source keys exactly, and fences in-flight work by playback generation |
| `analysis.rs` | Program resolution, active-chapter focus, overview passes and detail between chunks, updates to the player |
| `overview.rs` | Disqro's paint helper over the engine surface |
| `player/wave.rs`, `panels/waveform.rs`, `panels/waveform/scrub.rs` | Track and Full CD perspectives, queue-scoped seeks, the dock panel, coordinated motion and its diagnostics, the audible scrub gesture |
| `audio/clock.rs`, `audio/scrub.rs`, listening | Renderer clock, audition, Listening Intelligence |
| `waveform_qualification.rs` | `--render-waveform-cases` commands over the engine, presentation traces, device recreation, target-failure injection |

## Building

The engine and the GPUI platform patch come from the same pinned Git
revision. Cargo applies `[patch.crates-io]` only from this workspace, so the
patch entry must keep the engine's revision.

While the engine repository is private, fetching it requires Git
credentials for `github.com/maikeleckelboom/waveform-native` and Cargo's Git
CLI transport:

```powershell
$env:CARGO_NET_GIT_FETCH_WITH_CLI = 'true'
.\build.ps1
```

The Windows workflow fetches the locked private dependency with read-only access
on owner-controlled main builds and trusted same-repository `integrate/` pull
requests targeting main. Other pull requests run the public source checks.
[Windows workflow](../.github/workflows/windows.yml)

## Continuous motion

The pinned revision draws the detailed Waveform at a continuous device
origin, paces frames on the Windows 11 compositor clock, and no longer draws
44.1 kHz display cells without source frames as silence.
[Continuous waveform motion](waveform-continuous-motion.md)

[Playback presentation timing and foreground capture](playback-presentation.md)

## Derived data

Disqro opens the current rebuildable cache in `music-cache-v2.sqlite3`. The database filename versions the complete schema. Older cache files can be removed independently of the library and workspace.

## Archived extraction qualification

The original integration's locked suite passed with 210 tests. The
release build with `qualification`
passed all 103 frozen Stroke views in the frozen native replay, byte-identical to the previous production build, and the
overview digests of `timeline_waveform_baseline` are unchanged on all
fixtures. The private-desktop journeys passed: two decks, deck lifecycle
with an injected target failure (recovered while paused in 505.5 ms), host
device recreation, Full CD seeking across single-image and multi-file CDs
including replay after natural completion, and audible scrubbing. Timing
overlapped compilation or interactive use and is excluded from acceptance.
The hardening record covers the changed contracts, maps the eleven moved
tests, and specifies the remaining quiet-machine gate.
[Hardening and qualification](waveform-native-hardening.md)

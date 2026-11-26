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
| `waveform.rs` (`waveform_source`) | Symphonia/Kira decoding through `Recording`, file identity and revision checks, the SQLite table `wave_evidence_v4` and its pruning |
| `waveformDetail.rs` | Maps validated sources to engine source keys exactly, and fences in-flight work by playback generation |
| `analysis.rs` | Program resolution, active-chapter focus, overview passes and detail between chunks, updates to the player |
| `overview.rs` | Disqro's paint helper over the engine surface |
| `player_wave.rs`, `panels/waveform.rs`, `panels/waveformScrub.rs` | Track and Full CD perspectives, queue-scoped seeks, the dock panel, coordinated motion and its diagnostics, the audible scrub gesture |
| `audioClock.rs`, `audioScrub.rs`, listening | Renderer clock, audition, Listening Intelligence |
| `waveformPortQualification.rs` | `--waveform-parity` commands over the engine, presentation traces, device recreation, target-failure injection |

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

Public CI cannot fetch a private dependency. This integration must not be
merged until the engine is reachable by public builds (see the engine's
`docs/distribution.md`).

## Continuous motion

The pinned revision draws the detailed Waveform at a continuous device
origin, paces frames on the Windows 11 compositor clock, and no longer draws
44.1 kHz display cells without source frames as silence.
[Continuous waveform motion](waveform-continuous-motion.md)

[Playback presentation timing and foreground capture](playback-presentation.md)

## Derived data

Encoded evidence moved from `wave_evidence_v3` to `wave_evidence_v4`. The
old table held rebuildable measurements only and is dropped on open; the
library, queue and listening data are untouched.

## Archived extraction qualification

The original integration's locked suite passed with 210 tests. The
release build with `qualification`
passed all 103 frozen Stroke views through `tools/qualifyWaveformParity.py
checkpoint-gpu`, byte-identical to the previous production build, and the
overview digests of `timeline_waveform_baseline` are unchanged on all
fixtures. The private-desktop journeys passed: two decks, deck lifecycle
with an injected target failure (recovered while paused in 505.5 ms), host
device recreation, Full CD seeking across single-image and multi-file CDs
including replay after natural completion, and audible scrubbing. Timing
overlapped compilation or interactive use and is excluded from acceptance.
The hardening record covers the changed contracts, maps the eleven moved
tests, and specifies the remaining quiet-machine gate.
[Hardening and qualification](waveform-native-hardening.md)

# Precision waveform inspection

Click the visible-span readout in a Waveform panel, or focus it with Tab and press Enter. Enter applies the requested span around the current inspection center or following playhead. Escape and focus loss discard the edit. Opening the editor selects the full-precision frame span.

| Input | Meaning |
| --- | --- |
| `78.846 s` | Seconds of source audio |
| `145 ms` | Milliseconds of source audio |
| `1400 samples` | Decoded stereo sample frames |
| `180 px/s` | Physical display pixels per source second |
| `0.5 samples/px` | Source frames per physical display pixel |

The wheel and inline editor share the existing 32-frame minimum and 120-second maximum. Density inputs use the panel's registered device-pixel width. Each panel owns its zoom.

Right-click a waveform and enable **Show Sample Points** or **Show Sample Ruler**. Each panel saves the two preferences independently. Both start disabled. Overlays appear while stationary with decoded PCM resident and at least eight physical pixels between adjacent frames. Playback and audible scrubbing hide them.

Hover over either channel's dot to inspect the stereo frame. A dashed guide marks its position. The warm ring identifies L and the cool ring identifies R. The rings nest when the vertices coincide. The right edge of the header shows the absolute zero-based source frame and exact L/R amplitudes. The two channel fields are sized for the visible samples. Hold **Shift** over a sample to see its source time and sample rate. CUE recordings also show track and program time where space permits. Release Shift to return to the amplitudes. At very narrow widths, the header keeps as many complete fields as fit on one line. Frame divided by rate gives the exact source timestamp in seconds. The decimal is rounded to twelve places.

The ruler counts visible sample positions from **1**. Labels stay centered beneath their ticks. As space narrows, the ruler labels fewer ticks and leaves an edge tick unlabeled when its number would cross the panel boundary. A missing PCM block leaves a gap in the ruler at its true position.

## Qualification

The Windows fixture uses a private test desktop, an isolated profile, in-process GPUI input, and the silent qualification master fader. It checks inline commit, Escape, blur, invalid input, centered zoom, stereo PCM values, 32-frame playback, scrub suppression, hit registration after resize, and recovery after a failed target allocation.

```powershell
cargo build --locked --features qualification
python -B tools/qualifyWaveformInspection.py target/debug/disqro.exe <new-output-directory>
```

On the AMD Radeon 780M, 32 consecutive hardware D3D11 renders of a moving 32-frame view matched the accepted engine byte for byte. Both builds rendered the same 48 kHz stereo fixture at 640 × 180 pixels with gain 1, an initial center of 800.375 frames, and a 200-frame advance per refresh. The concatenated 14,745,600 RGBA bytes had SHA-256 `2542892c34cca0811a99b34215b86465860773e4d835c256355018a5b60f05d6`. The reference was `22d740d4ee1c0fbc3a42435b59c015fabde30291` and the inspected engine was `203a2327230e617a85dab038a64dfb405638b5a4`.

The earlier zoom-editor image was captured while the text `32 samples` was pending. The recorded viewport still spanned 384,000 frames at 48 kHz. The green block was the eight-second envelope of the stationary-amplitude fixture at the source boundary. The journey now checks the uncommitted viewport and captures the editor after reopening it over resident PCM at 32 frames.

Native captures at 144 DPI include the [zoom editor](evidence/precision-inspection/zoom-editor.png), [paused markers](evidence/precision-inspection/sample-points.png), [one-based ruler at a half-frame origin](evidence/precision-inspection/sample-ruler.png), [inline measurements](evidence/precision-inspection/sample-inspector.png), [CUE times](evidence/precision-inspection/cue-times.png), [narrow header](evidence/precision-inspection/sample-narrow.png), and [32-frame playback](evidence/precision-inspection/playback.png).

For additional fixtures, pass `--band bass` or `--control-wav <control.wav>` to the same command and use a new output directory. The [bass capture](evidence/precision-inspection/sample-bass.png) uses 83 and 127 Hz stereo tones. The [synthetic control capture](evidence/precision-inspection/sample-control.png) uses the 6.3 kHz mono section of Waveform Native’s frozen 44.1 kHz control. Each journey compares the displayed stereo measurement against the WAV at its absolute frame. Physical display cadence and cross-monitor DPI transfers remain outside this qualification.

The [chapter-render investigation](chapter-render-qualification.md) covers stereo continuity, decoder readiness under concurrent test load, and the remaining underrun recovery limitation.

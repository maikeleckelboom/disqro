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

Right-click a waveform and select **Show Sample Points**. Each panel saves its own preference, initially off. The dots appear on resident stereo vertices while stationary at a spacing of at least eight physical pixels. Playback and scrubbing suppress them. At a vertex, the tooltip shows the physical source frame, sample rate, source time, and both unscaled decoder values. CUE chapters also show track and program time. Frame divided by rate gives the exact timestamp in seconds. The source-time decimal is rounded to twelve places.

## Qualification

The Windows fixture uses a private test desktop, an isolated profile, in-process GPUI input, and the silent qualification master fader. It checks inline commit, Escape, blur, invalid input, centered zoom, stereo PCM values, 32-frame playback, scrub suppression, hit registration after resize, and recovery after a failed target allocation.

```powershell
cargo build --locked --features qualification
python -B tools/qualifyWaveformInspection.py target/debug/disqro.exe <new-output-directory>
```

On the AMD Radeon 780M, 32 consecutive hardware D3D11 renders of a moving 32-frame view matched the accepted engine byte for byte. Both builds rendered the same 48 kHz stereo fixture at 640 × 180 pixels with gain 1, an initial center of 800.375 frames, and a 200-frame advance per refresh. The concatenated 14,745,600 RGBA bytes had SHA-256 `2542892c34cca0811a99b34215b86465860773e4d835c256355018a5b60f05d6`. The reference was `22d740d4ee1c0fbc3a42435b59c015fabde30291` and the inspected engine was `61a576f5881b4e7161862f45d895c3dce55f798a`.

Native captures at 144 DPI show the [inline editor](evidence/precision-inspection/zoom-editor.png), [paused sample points](evidence/precision-inspection/sample-points.png), [CUE sample measurement](evidence/precision-inspection/sample-inspector.png), and [32-frame playback](evidence/precision-inspection/playback.png). Physical display cadence and cross-monitor DPI transfers remain outside this qualification.

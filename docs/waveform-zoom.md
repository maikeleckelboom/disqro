# Waveform zoom

Scroll in a Waveform panel to zoom continuously. Click its visible-duration readout, or focus it with Tab and press Enter, to type a duration such as `8 s` or `1.5 ms`. Enter applies the edit. Escape and focus loss cancel it. Each panel owns its zoom and can pan independently or follow playback.

At the deepest zoom, the panel connects the decoded left and right samples into continuous traces. Their positions follow the source clock and the selected time axis. The visible duration can range from 32 source frames to 120 seconds. The panel shows the duration in milliseconds or seconds.

Disqro uses Waveform Native's player presentation in every transport state, including pause, drag and playback. Existing workspaces keep their panel orientation and direction. Obsolete sample-point and ruler preferences are ignored on load and omitted on save.

## Native verification

The Windows journey loads a workspace with both obsolete preferences enabled, opens a stereo CUE fixture on a private desktop, and enters a sub-millisecond duration. At the minimum span it checks the image against the decoded WAV at the sample vertices and halfway between them. Hover and Shift must leave the waveform and header pixels identical. It also exercises playback, scrubbing, panel resize and returning to a wider duration.

At 144 DPI, all four time axes passed 126 trace-location checks each on the
32-frame stereo fixture. Hover and Shift produced zero changed pixels in
the waveform and its header. The panel menu offered the two time-axis
choices. [Captured player menu](evidence/deep-zoom-menu.png)

~~~powershell
cargo build --release --locked --features qualification
python -B tools/qualify_waveform_zoom.py target/release/disqro.exe .local/zoom-up --axis up
~~~

The fixture also supports `right`, `left` and `down` axes. Each run writes a full-window PNG for each step, the registered projection and a pixel comparison report to a new output directory. [Panel orientation](waveform-panel-orientation.md)

An earlier Radeon 780M comparison replayed 32 consecutive moving views of a
48 kHz stereo fixture at 640 × 180 pixels. Both builds produced identical
14,745,600-byte RGBA sequences with point overlays disabled. That historical
check covers the renderer. The current journey additionally checks the
displayed player pixels against the WAV. [Chapter-render qualification](chapter-render-qualification.md)

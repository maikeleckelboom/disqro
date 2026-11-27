# Native waveform rendering qualification

The current manifest pairs the reviewed signed overview from Waveform Native PR #8 with the detailed Stroke images and geometry from Disqro PR #41. Its 103 Windows D3D11 views each have separate SHA-256 digests for the PNG and geometry JSON. [Current manifest](../tools/waveform_parity/native_cases.json) · [Historical manifest with all RMS-body hashes](https://github.com/maikeleckelboom/disqro/blob/321deed4438add27e66b0f8eaff7b8cefdae9cbd/tools/waveform_parity/native_cases.json)

The signed-envelope replay changed 24 overview PNGs across eight fixture groups at widths of 560, 1,200, and 1,920 physical pixels. All 79 detailed PNGs and all 103 geometry files matched the historical hashes. On the synthetic control, each new overview matched the approved engine image pixel for pixel. The retained-changing recording shows a uniform fill through the loud sections and keeps the gaps visible. The whole-disc and CUE chapter views retain their distinct extents.

## Replay on Windows

Build the qualification executable separately from the production player. Supply the frozen local fixture directory containing one subdirectory per recording, each with its original facts directory. The replay consumes the stored evidence directly. It needs no media decoding, network access or external renderer. The supplied fixture pack stays local.

~~~powershell
cargo build --locked --release --features qualification
python -B tools/qualify_waveform_parity.py --exe target/release/disqro.exe --fixtures .local/waveform-parity/checkpoint --output .local/native-waveform-replay
~~~

Use a new output directory for each run. The harness creates an isolated profile, starts the headless native renderer on a private desktop, and records the graphics adapter. A missing or different file fails the gate. The result records the tested binary digest, baseline revisions, number of comparisons and differing paths. Regenerating expected digests requires independent approval of a new visual baseline.

## Coverage

The fixture set includes an anti-phase control, music recordings, whole-disc and CUE chapter extents. It covers overview and detailed views, multiple device widths, color and geometry, and near-sample zoom. Frozen facts include the 13-value measurements used to produce the approved views. Only the qualification reader consumes that fixture format.

The source frame range, chapter bounds and zoom determine visible geometry. Detailed Stroke and overview rendering share native measurement data. Live hold, scrub, device recovery, exact inspection and playhead timing have separate private-desktop qualifications. [Native waveform integration](waveform-native.md) · [GPU rendering and performance](waveform-native-gpu.md)

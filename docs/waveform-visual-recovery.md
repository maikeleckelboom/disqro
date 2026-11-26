# Native waveform rendering qualification

The accepted visual baseline is the Windows D3D11 output delivered in Disqro PR #41. Each of the 103 Stroke views has two immutable SHA-256 values, one for the encoded PNG and one for the renderer's geometry JSON. The repository manifest contains the view parameters, local evidence filename and both digests. Its source is the accepted PR #41 output. [Native case manifest](../tools/waveform_parity/native_cases.json)

## Replay on Windows

Build the qualification executable separately from the production player. Supply the frozen local fixture directory containing one subdirectory per recording, each with its original facts directory. The replay consumes the stored evidence directly. It needs no media decoding, network access or external renderer. The supplied fixture pack stays local.

~~~powershell
cargo build --locked --release --features qualification
python -B tools/qualify_waveform_parity.py --exe target/release/disqro.exe --fixtures .local/waveform-parity/checkpoint --output .local/native-waveform-replay
~~~

Use a new output directory for each run. The harness creates an isolated profile, starts the headless native renderer on a private desktop, and records the graphics adapter. A missing or different file fails the gate. The result records the tested binary digest, baseline commit, number of comparisons and differing paths. Regenerating expected digests requires independent approval of a new visual baseline.

## Coverage

The fixture set includes an anti-phase control, music recordings, whole-disc and CUE chapter extents. It covers overview and detailed views, multiple device widths, color and geometry, and near-sample zoom. Frozen facts include the 13-value measurements used to produce the approved views. Only the qualification reader consumes that fixture format.

The source frame range, chapter bounds and zoom determine visible geometry. Detailed Stroke and overview rendering share native measurement data. Live hold, scrub, device recovery, exact inspection and playhead timing have separate private-desktop qualifications. [Native waveform integration](waveform-native.md) · [GPU rendering and performance](waveform-native-gpu.md)

# Native waveform rendering qualification

The current manifest uses the source-centered overview color from Waveform Native Mission 3A. All 24 overview PNGs were reviewed at 560, 1,200 and 1,920 physical pixels. All three synthetic controls match the approved engine RGB values exactly. Across all eight fixture groups, the 79 detailed Stroke PNGs and all 103 geometry files retain their production hashes.

[Current manifest](../tools/waveform_parity/native_cases.json) · [Previous signed overview with end-anchored color](https://github.com/maikeleckelboom/disqro/blob/a0cdcbf1d775204d7f9aced77937194de4198400/tools/waveform_parity/native_cases.json) · [Historical RMS-body manifest](https://github.com/maikeleckelboom/disqro/blob/321deed4438add27e66b0f8eaff7b8cefdae9cbd/tools/waveform_parity/native_cases.json)

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

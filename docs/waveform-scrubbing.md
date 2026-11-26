# Waveform scrubbing

Hold the left mouse button and drag the detailed Waveform. Drag left to advance and right to move backward. The fixed center line marks the position that will be used on release. The gesture uses the displayed source frames per physical pixel at mouse down. A stationary click seeks, and Escape returns to the position and playback state captured at mouse down. Shift+wheel and horizontal scroll still hold an independent inspection position.

One deck owns the gesture. The Windows host suspends hover reentry notifications while that gesture holds native mouse capture. The deck continues to receive GPUI mouse events and schedules retained GPU updates. Other decks in Follow mode share its frame, and the compact strip reads a coalesced transport position. The transport keeps the originating queue occurrence during audition. In Track the gesture stays within the playing chapter's contiguous queue sequence, and Repeat One keeps it to the chapter. In Full CD it reaches every authored chapter that continues the playing chapter in the same file, whatever the queue holds, and a click there seeks too. On release the transport reuses the destination's occurrence when it continues the playing entry in queue order, and otherwise inserts one new occurrence after the playing entry. Escape and cancellation never change the queue. The compact Full CD waveform seeks every chapter of the program, including other files and sample rates; the detailed waveform shows and scrubs one physical file at a time. The transport checks the displayed occurrence, seek and queue revisions, the physical file, and the CUE sheet at admission and again before committing. Losing pointer capture, removing the panel, or hiding the window cancels the gesture. Seek, playback, and queue commands take control before executing.

Kira 0.12.6 clamps streaming playback to a nonnegative rate. Disqro installs a single audition sound on the existing Kira mixer per output device. A worker reuses the `Recording` decoder through the same source reader as waveform analysis. Six resident and three in-flight stereo PCM pages of 16,384 frames use 1.125 MiB of sample storage. The callback samples the most recent motion from atomics and exchanges preallocated pages through bounded queues. It interpolates decoded samples in either direction, uses short fades on reversals and changes of position, and settles to silence after the sampled movement. The visual target remains exact at wide zoom, where audition speed is capped at eight times source rate and the sound samples near the requested position.

Ordinary playback pauses for the audition. Release commits the target once. A previously playing recording starts its normal forward renderer at that sample with a short fade-in. A previously paused recording stays paused. The audition sound publishes zero listening progress. Only committed blocks from the ordinary renderer, including any pause ramp, can accrue listening credit. Returning to the same queue entry keeps its listening occurrence. Crossing into another entry on release closes the previous occurrence, and the new entry can earn credit when its ordinary renderer actually advances.

## Preserved qualification

These measurements predate the production integration. [Original scrubbing revision](https://github.com/maikeleckelboom/disqro/blob/c2f31c659fbbcbfa664ed707bcce9cf84803ec94/docs/waveform-scrubbing.md) · [Stroke production baseline](https://github.com/maikeleckelboom/disqro/blob/b55bdf7f39d0f111632c539d286c60be6f53994f/docs/waveform-final-hardening.md)

The deterministic mixer test decodes a synthetic stereo WAV, verifies actual rendered PCM across a page boundary, reverses direction twice, checks fractional interpolation at half speed, and verifies the return to digital silence. The transport test crosses adjacent CUE chapters in both directions, exercises stale revisions and competing queue edits, and checks that scrub movement earns no listening history.

The private Windows harness generates a synthetic five-minute CUE image, runs on a private desktop, and mutes the master output. Both passes verified stationary seek, out-of-pane release, paused CUE crossing, playing reverse release, Escape, cancellation on minimize, both zoom limits, and independent inspection. The 32-sample view moved four source frames. The 120-second view moved about 440,900 source frames while the other deck held its inspection position.

| Two-deck release run | Pointer moves | Whole-window scene builds during grabs | Pointer to retained draw, p95 | Pointer to first scrub audio callback, max | Scrub movements rendered |
| --- | ---: | ---: | ---: | ---: | ---: |
| Pass 1 | 469 | 92 | 5.33 ms | 8.97 ms | 389 |
| Pass 2 | 469 | 90 | 5.35 ms | 9.34 ms | 389 |

The visual measure pairs pointer targets with retained deck draws. The audio measure times the first callback that renders PCM for each observed movement. In each pass, 15 waveform analysis pages completed in trace intervals that also rendered scrub audio. The audition used one decoder open across all gestures. Ordinary playback opened four voices for the scripted playback starts and returns from scrubbing. Both passes had zero decode, device, or waveform GPU errors.

The same release binary passed 103 frozen Stroke views and 45 additional music views on D3D11. A separate two-deck host run recovered from injected device recreation and the expected target allocation error. Locked debug and optimized test suites passed with 221 tests each. Strict all-feature Clippy and formatting checks passed. Physical endpoint latency and audible feel remain to be qualified on an authorized desktop and audio run.

```powershell
cargo test --locked --features qualification scrub
cargo build --release --locked --features qualification
python -B tools/qualifyWaveformScrub.py target/release/disqro.exe .local/waveform-scrub-qualification
```

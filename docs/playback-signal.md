# Playback signal and stereo levels

The L and R bars beneath the volume slider measure the samples Disqro sends through Kira’s main mixer. The fill shows RMS, the bright marker shows sample peak, and the dim marker holds the highest sample peak from the last second. The red end cap means a channel reached or exceeded digital full scale during that second. Hover for channel readings in dBFS. **Workspace → Show Stereo Levels** toggles the display and saves the preference.

## Observation point

| Stage | What happens |
| --- | --- |
| File and demux | The transport opens the source and selects the default audio track. Symphonia probes the container and reads timestamped packets. Recording identity, MP4 edits, Opus pre-skip and CUE boundaries belong to the source timeline. |
| Decode and PCM | On Kira’s decoder thread, `Recording::decode` converts each decoded block to interleaved f32, trims to the requested source frames and builds stereo Kira frames. Mono feeds both channels. Opus applies its header gain. The waveform worker has its own decoder and cache. |
| Streaming voice | Kira feeds a 16,384-source-frame ring. The renderer interpolates at the output rate, applies voice fade and volume, then panning. Disqro’s slider and mute control that voice gain. |
| Mix and tap | Kira sums routed tracks and voices. Its main track invokes effects in insertion order. Disqro installs one read-only observer here, after summation. |
| Main output | Kira applies its main-track gain (unity in Disqro), clamps L/R to ±1 and writes f32 output. Mono output averages L and R. Outputs with more than two channels receive zero in the remaining channels. |
| Windows | CPAL opens the default device through WASAPI shared mode. System volume, the shared engine’s mix, format conversion, endpoint processing and physical output are downstream of the tap. |

The pinned Kira implementation defines the ordering. The two-voice test adds 0.8 and a half-volume 0.8 on the left and measures 1.2 upstream of a −12 dB main gain. The streaming CUE test compares rendered stereo samples around the chapter join against the original PCM. [Streaming voice](https://github.com/tesselode/kira/blob/b0325713c2cf090f980b46246505509f838d48f5/crates/kira/src/sound/streaming/sound.rs) · [Main track](https://github.com/tesselode/kira/blob/b0325713c2cf090f980b46246505509f838d48f5/crates/kira/src/track/main.rs) · [Output renderer](https://github.com/tesselode/kira/blob/b0325713c2cf090f980b46246505509f838d48f5/crates/kira/src/backend/renderer.rs)

## Measurement and publication

At the Kira effect boundary, `SignalTap::observe` accepts an immutable frame slice. The levels reducer tests its own atomic subscriber count and accumulates into fixed-size arrays. The stereo capture writer tests a second, independent subscriber count. An idle tap skips both sample loops.

At each output rate the bucket size is the nearest whole frame to 25 ms. Four consecutive buckets give approximately 100 ms of RMS and sample peak, updated every bucket. At 48 kHz that is exactly 4,800 frames with a 1,200-frame hop. At 44.1 kHz it is 4,412 frames with a 1,103-frame hop. Startup uses the available completed buckets and their actual frame count. An incomplete bucket carries across callbacks. A new voice, a new output rate or a first subscriber starts fresh. Contiguous CUE chapters keep the same voice and measurement history.

Each channel accumulates squares in f64. RMS is the square root of their mean and sample peak is the largest absolute sample. Forty peak buckets supply a rolling one-second hold on the audio clock, so brief overloads survive a missed UI refresh. Non-finite samples contribute zero energy and peak to the affected channel, retaining their frame in the denominator. Their original bit patterns pass through the effect. The UI converts amplitude using `20 × log10(amplitude)`. Silence measures −∞ dBFS. The display spans −60 to 0 dBFS and applies immediate attack and a 36 dB/s visual release. Numerical readings retain the measured values, including positive dBFS. These are sample-domain measurements. True peak needs oversampling and LUFS needs its own weighting and integration.

One writer publishes the six f32 levels in three atomic stereo pairs, with a measurement epoch and an even/odd version. Sequentially consistent atomic operations establish a single ordering. A reader takes one bounded snapshot and can skip a refresh that overlaps a write. All readers share the latest publication and can run at different rates. Reference counting, subscription changes and GPUI work run outside sample processing.

The meter uses the existing visible transport updates (up to 25 Hz) and repaints on half-decibel display changes. Pausing or stopping clears the display. Muting follows the gain tween to silence. Closing the meter or minimizing the window releases its subscription. The waveform retains its own playhead and analysis invalidation schedule. The accessible window path renders its descendants directly.

## Extending the tap

An additional level display can hold its own `SignalMonitor::subscribe()` reader and call `latest()` at its own cadence. Dropping it releases demand. Measurements that require PCM can use the existing bounded stereo capture and analysis worker. Spectrum panels share this worker and own their display settings. [Capture, calibration and presentation](realtime-spectrum.md)

## Reliability observations

- File open, probe, seek and lazy device initialization run on the transport worker. Their latency can delay command completion. Active seeks replace the voice and reopen the recording. Adjacent pending seeks coalesce. Paused seeks defer output until resume.
- A full decode ring makes Kira’s scheduler sleep. At high source rates the same ring covers less time (about 371 ms at 44.1 kHz or 43 ms at 384 kHz). A gap between decoder calls can therefore mean the buffer was full. Kira can emit zeros when the streaming voice lacks frames and exposes no fill gauge through this API.
- Contiguous CUE chapters share a voice. Separate-file transitions depend on the transport’s approximately 40 ms polling interval and the next open. This mission makes no physical-file gaplessness guarantee.
- Kira checks the default device and handles stream changes on a separate manager thread. Initial output failures reach the transport. Recovery includes upstream restart paths that can fail. Hardware disconnection and driver-specific behavior remain separate qualification targets.
- Setting `DISQRO_PLAYBACK_TRACE` to a **new** writable path enables the existing one-second CSV. Decode time, packet-call gaps, voice starts, accepted commands, position stalls, Kira renderer CPU and device errors help localize contention. Two appended columns cover the maximum absolute error between consecutive voice-position and wall-clock deltas, and handled-error queue losses reported by Kira. Position samples are callback-quantized. A pause or replacement voice resets the comparison. Kira’s CPU ratios time rendering within a callback and travel through a bounded queue. These observations cannot count all endpoint underruns.

## Windows qualification

See the [qualification results](playback-qualification.md) for the baseline, native screenshots, measurement fixtures and remaining limits.

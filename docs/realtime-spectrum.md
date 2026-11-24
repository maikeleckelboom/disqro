# Real-time spectrum and stereo measurements

Open **Workspace → New Spectrum** in the title bar. Use the sliders button inside a Spectrum panel to choose its channels, presentation, frequency range, resolution and refresh rate. In **Workspace → Customize Layout**, split a tab group or drag a tab to another group. Each instance saves its settings with its dock placement. The thin gray trace or horizontal marker shows the held peak.

## Signal path and ownership

```text
Symphonia Recording → Kira streaming voices → stereo main mix → main gain → clamp → WASAPI
                                                   │
                                              shared &[Frame]
                                                   ├─ Mission 13 RMS and peaks
                                                   └─ demand → SPSC blocks → analysis worker
                                                                                ├─ FFT by resolution
                                                                                └─ latest reading per panel → GPUI
```

The tap observes Kira's main-track effect input after voice gain. System volume, endpoint processing, buffering and the DAC are downstream. The transport still owns the output device and decoder. The waveform worker reads source pages for Track and Full CD projections. Playback spectrum analysis reads the live mix and opens no source files. [Playback path](playback-signal.md) · [Waveform architecture](musical-timeline.md)

| Owner | Work and lifetime |
|---|---|
| Kira effect builder | Allocates one single-producer/single-consumer ring with 256 slots. The existing Kira dependency already uses the pinned real-time ring library. |
| Audio effect | Borrows immutable stereo frames. With PCM demand, it assembles 128-frame blocks and tries one nonblocking push per block. With no PCM readers it tests one atomic and returns to the original levels path. Sample processing performs no allocation, locks, I/O, timestamps or GPUI calls. Optional trace counters use atomics. |
| Transport and Kira | Replacing a voice or changing the output sample rate advances the PCM epoch. Joining contiguous CUE chapters within one voice preserves it. Starting the first PCM observation opens a fresh epoch. Levels has independent subscribers and measurement history. |
| Analysis worker | Starts on the first active panel. Drains a bounded snapshot of the ring and keeps the latest 32,768 contiguous frames in a mirrored history. It blocks on control messages when idle. It caches FFT plans, both complex outputs, bins and scratch by transform length. Clients due on the same sample window and resolution share both FFTs and stereo statistics. |
| GPUI | A panel subscribes while playing, displayed in its tab group and visible to the window system. A capacity-one channel replaces an unread result. Only the plot entity receives measurement notifications. Panel-setting changes alone mark the workspace dirty. Cached axis text survives signal updates and resizing. Standard GPUI strokes draw the two traces and quads draw bands, markers and guides. |

At 48 kHz the capture ring covers 683 ms. The payload occupies 256 KiB plus block headers. The mirror occupies 512 KiB. The window-length choices bound the number of cached plans. A full ring discards the incoming observation and keeps advancing the frame sequence. The consumer rejects observations more than 50 ms behind the capture head or with a superseded epoch. A gap clears its window and display history. It waits for a full contiguous window before publishing again. An interruption with no incoming blocks clears the display after 150 ms. Queue growth and reader speed cannot delay the producer. [Capture](../src/capture.rs) · [Worker and publication](../src/spectrumWorker.rs)

## What the numbers mean

The default trace measures mean stereo line power, `(Pleft + Pright) / 2`. Two identical full-scale sines and two polarity-inverted full-scale sines both read 0 dBFS at their bin center. With one channel silent the combined reading falls by 3.01 dB. Channel selection also supports L, R, mid `(L+R)/2` and side `(L−R)/2`. Mid and side use complex FFT coefficients, preserving phase relationships.

For the periodic Blackman window `w`, an interior bin's peak-equivalent squared amplitude is `4 |X[k]|² / (Σw)²`. DC and Nyquist use a factor of one. The trace converts power with `10 log10(P)`. A narrow log cell interpolates between measured bin powers. A wider unsmoothed cell retains its strongest bin so a treble tone cannot disappear between plotted points. Optional fractional-octave smoothing averages power across fractional bin widths. Display tilt adds 3 or 6 dB/octave about 1 kHz to the trace after measurement. A flat, unsmoothed trace is the reference for sine calibration.

Band energy uses the single-sided sum of `|X[k]|² / (N Σw²)`, doubled at interior bins, apportioned by bin overlap. A full-scale in-band sine reads −3.01 dBFS RMS. Sub, bass, low mid, mid, high mid and high span 20–60, 60–250, 250–500, 500–2,000, 2,000–6,000 and 6,000–20,000 Hz. The chosen range and Nyquist clip each interval. These unweighted band energies exclude DC. Loudness would require perceptual weighting and integration.

The stereo footer measures the unwindowed samples over the same time interval. Correlation is `ΣLR / √(ΣL² ΣR²)`. Negative correlation indicates net phase opposition. Mono is the percentage of average channel power retained after equal-gain fold-down, `100 M² / (M² + S²)`, using mean-square mid and side energies. Identical channels retain 100%, inverted channels 0%, and a single active channel 50%. Correlation is undefined with either channel silent. Digital silence has no retained-power ratio. A dash marks an undefined reading. [DSP implementation](../src/spectrum.rs) · [Calibration fixtures](../src/spectrumTests.rs)

## Resolution, time and rendering

The target window durations are 85, 170 and 340 ms. The nearest power of two is selected at the actual output rate, bounded to 1,024–32,768 frames. At 48 kHz these give 4,096, 8,192 and 16,384 frames with 11.72, 5.86 and 2.93 Hz bin spacing. The standard Blackman window centers energy about 85 ms behind its newest sample. Worker scheduling, the audio device and rendering contribute additional delay. The sequence identifies samples at the tap and carries no hardware presentation timestamp.

The trace uses 256 log-spaced points. A bass-only range spreads 20–500 Hz across the plot. Increasing the point count or panel width cannot resolve tones inside the window's main lobe. A half-bin sine has approximately 1.1 dB of scalloping loss. Narrower octave smoothing approaches the available FFT resolution. Each panel applies its own 12 ms attack, linear dB release and optional peak hold using elapsed **sample time**. The plotted floor is −90 dBFS. Output above digital full scale remains measurable and the chart tops out at +6 dBFS.

Panels publish at 15, 30 or 60 Hz. Integer subdivisions of a shared clock permit independent rates while reusing transforms. The UI can resize and process input between spectrum updates. During sustained digital silence the worker still drains new input, skips the FFT, lets the trace and peaks decay, and stops publishing once they reach the floor. Pause, stop, tab deactivation, close and minimize release capture demand. [Panel lifecycle](../src/panels/spectrum.rs) · [Native drawing](../src/panels/spectrumPlot.rs)

## Pawse source comparison

The reference was inspected at the requested revision. Disqro uses its own capture, spectral projection, worker scheduling and dock integration. The Blackman-window strategy, band boundaries and calibrated-fixture approach draw on Pawse. Its MIT notice is retained with the reference attribution. [Upstream revision](https://github.com/popovpsk/pawse/tree/546aed00ea2a36734a507f2c5b66fac91d7b182b) · [License notice](licenses/pawse.txt)

| Pawse source | Finding and decision |
|---|---|
| [audio_output/tap.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/audio_output/src/tap.rs) | Pawse averages channels into an atomic ring and estimates the heard sample using output latency. Disqro keeps the stereo pair in an SPSC block with an epoch, output rate and frame sequence. Kira supplies the mixing boundary. |
| [spectrum/lib.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/spectrum/src/lib.rs) | Pawse caches a real FFT, compensates coherent gain, averages fractional-octave power and integrates zones. Disqro adds complex mid/side analysis, explicit DC/Nyquist treatment and separate peak and RMS conventions. Flat tilt is the default and strong narrow treble lines survive projection. |
| [spectrum_view/mod.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/pawse/src/spectrum_view/mod.rs) and [paint.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/pawse/src/spectrum_view/paint.rs) | Pawse adapts point count and idle scheduling, builds meshes for filled or stroked curves, and uses quads for rules. Disqro moves FFT work off the view thread, gates work on dock and native visibility, caches shaped labels and caps trace geometry. The band view uses quads throughout. |
| [bit_perfect.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/audio_output/src/bit_perfect.rs) | Pawse distinguishes exclusive output, volume, mute, sample-rate mismatch, sample precision and processing. Disqro asserts bit preservation across its observer. WASAPI shared-mode endpoint integrity remains a separate measurement question. |
| [equalizer/processor.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/equalizer/src/processor.rs) | Pawse separates control generations, prepared processing state and the UI. Disqro adopts prepared worker state and keeps the audio effect read-only. |
| [loudness/meter.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/loudness/src/meter.rs) and [tests.rs](https://github.com/popovpsk/pawse/blob/546aed00ea2a36734a507f2c5b66fac91d7b182b/crates/loudness/src/tests.rs) | Pawse's weighted, gated loudness meter tests known signals across rates. Disqro uses generated signals with explicit reference levels for bin amplitude, band mean square, silence, phase, leakage and time response. |

## Engineering review

The default targets musical observation, with an explicit tradeoff between bass resolution and event timing. A hardware-synchronized readout would need endpoint presentation timestamps. A spectrum of a short transient depends on window placement. The peak trace holds sampled analysis frames, so a slow client cannot guarantee capture of every transient. Mission 13's sample-peak history remains available for that purpose.

Stereo statistics are reusable independently of FFT execution. A dedicated stereo scope is deferred. The two current styles expose the different physical measurements directly. Future visualizations can share the prepared stereo window and use the same visibility and publication contracts. Native performance, screenshots and integration checks are in the [Windows qualification report](spectrum-qualification.md).

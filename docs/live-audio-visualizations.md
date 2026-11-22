# Reading the live mix

Choose **+ → New Spectrogram** or **+ → New Stereo Field**. Both open as ordinary workspace tabs. Use **Workspace → Customize Layout** to split or drag them beside the Music Browser. Each tab saves its own settings through its sliders button.

## Spectrogram

Frequency runs upward and elapsed audio time runs from left to right. The right edge follows the newest completed analysis window. The default view covers 20 Hz–20 kHz on a logarithmic axis and 30 seconds of audio. Choose a linear axis, a bass range ending at 500 Hz, or 15 and 60 second histories. The upper frequency is capped at the output Nyquist frequency.

Brightness follows sine-peak-equivalent dBFS. A bin-centered sine with a peak amplitude of 0.5 reads approximately −6 dBFS on its channel. Stereo power averages left and right squared line amplitudes. Left, right, mid and side are available independently. Mid and side use half the sum and half the difference. The intensity floor and ceiling adjust the color scale without changing the measurements. Ember and Marine both progress monotonically in luminance.

The window choice controls how long an event occupies the analysis window. At 48 kHz the transient, balanced and tonal windows use 1,024, 2,048 and 4,096 samples. The longer window distinguishes nearby tones and spans more of each attack. Each transform advances by one quarter of its window. Balanced analysis produces 93.75 columns per second at 48 kHz, including when the panel presents at 15 Hz. The Blackman window, coherent-gain normalization and single-sided FFT scaling come from Spectrum. A frequency row takes the strongest enclosed FFT line. Narrower rows interpolate neighboring line powers. These are line levels, measured before any Spectrum smoothing, tilt, release or peak hold.

**Freeze history** holds the displayed snapshot and releases that instance’s capture demand. **Resume live history** starts a fresh time interval. A seek, lost capture block, replacement voice or output-rate change starts a new history. An uninterrupted CUE chapter change within a voice keeps the history. Restart restores the interpretation and layout and opens a fresh live view.

## Waterfall

Choose **Presentation → Waterfall** in Spectrogram settings. The front ridge is the latest completed window. Frequency increases to the right, height spans the selected dBFS floor and ceiling, and older windows recede upward and to the right. The oldest-time label describes the full selected depth. Empty positions remain empty while the history fills.

Historical ridges sample fixed positions on the audio clock. The renderer moves those ridges together between sampling instants. Their faces occlude older lines. The bright leading edge marks the current window. Historical edges use measured level for color intensity and recede in contrast with age. The flat view retains every hop for inspecting attacks between ridges.

## Stereo Field

In the default mid/side view, positive mid points up and positive side points right. Identical channels concentrate vertically. Inverted channels concentrate horizontally. A left-only signal lies on the upward-right diagonal and a right-only signal on the upward-left diagonal. In XY view, left is horizontal and right is vertical. One unit corresponds to full scale at 1× gain.

Point density deposits actual stereo pairs with bilinear coverage. Vector trace joins successive pairs with bounded line sampling. The gain control magnifies the display. Off-scale samples are omitted and break the trace. Digital zero deposits no light. Occupancy decays according to a selectable 80, 240 or 720 ms half-life, measured in audio time.

The readouts describe the latest 50 ms of available PCM. Let $L$, $R$, $M$ and $S$ denote mean-square energies on those channels.

| Readout | Interpretation |
| --- | --- |
| Corr | Normalized, zero-lag cross-power. Identical channels give +1 and inverted channels give −1. A channel with zero energy leaves correlation undefined. |
| Mono | Retained energy after folding to mono, expressed as $100M/(M+S)$ percent. |
| Balance | Magnitude and direction of $(R-L)/(R+L)$, expressed as a percentage. Center means equal channel energy. |
| S/M | Side energy relative to mid energy, in dB. A pure mid or side signal reaches the corresponding infinite limit. |

Silence gives a dash for undefined values. These short-window readings change throughout a recording. A presented Stereo Field with statistics enabled takes over the stereo readout from Spectrum. Hiding those statistics brings back Spectrum’s own summary.

## Signal and rendering ownership

1. Kira’s existing main-track observer borrows the post-voice-gain mix. The callback copies paired frames into the preallocated 256-block capture ring only while a subscription is live. A block contains 128 stereo pairs, a capture epoch, a starting frame and an output rate.
2. The existing analysis thread drains at most one ringful per pass. Spectrum, spectrogram and field consumers share this thread and its mirrored PCM window. The UI owns one registration per live lease. A capacity-one wake coalesces configuration changes. Closing the final panel releases capture demand and the worker sleeps.
3. FFT scratch and window plans are reused by length. Equal windows ending at the same frame share a transform across consumers. Spectrograms with matching channels, range, mapping and resolution share projected history. DSP runs at the audio hop and results use 256 rows quantized in half-dB steps. Full tiles of 32 columns are immutable. Only the partial tile needs copy-on-write when a reader holds a snapshot. At 48 kHz the balanced 60-second history stores about 1.37 MiB of level codes plus tile overhead.
4. A field accumulator tracks occupied cells up to 2048 per side for Point density and 1024 per side for Vector trace. Signal time determines decay. Matching field clients share accumulation and publication. The field requests its stereo statistics independently and needs no FFT. Each consumer’s capacity-one mailbox holds the latest displayable result.
5. The dock controls presentation, window visibility and playback demand. Each active plot is a GPUI entity with its own invalidation boundary. Completed spectrogram tiles stay in the GPUI image atlas. On compact plots, maximum pooling in exact hop buckets preserves short events when several columns share a physical pixel. Waterfall caches the composited historical ridges and draws the latest ridge separately. Stereo Field maps the occupied cells to the plot’s physical pixels up to the grid size, keeping the strongest cell where several map to one pixel. It uploads a cropped image with transparent empty pixels. Resizing uses the existing signal history. Cache eviction releases the corresponding atlas entries.

The qualification build sets exact-zero main gain after the observer before creating CPAL output. The opt-in build feature and launch flag are independent of saved workspace data. Normal launch uses the normal output graph. The Windows measurements, captures and native verification results are in the [qualification report](visualization-qualification.md).

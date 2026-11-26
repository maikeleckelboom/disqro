# Serato Studio waveform procedure

Use the installed Serato Studio only after the owner writes `START SERATO RESEARCH`. Record the installed version, Windows display resolution, reported scale, physical viewport width, control mode, pitch/key/tempo or synchronization settings, zoom mechanism and visible time bounds before collecting evidence. Keep the normal Windows and application settings. Use an isolated unsaved project and generated media copies. Keep recordings and lossless originals local. Do not export project or stem files beside the owner's recordings.

## Capture order

| Priority | Source and fixed region | Purpose and sequence |
| --- | --- | --- |
| 1 | Generated 24-second PCM control. Use the isolated impulse and the attack followed by its bass tail. | Import while stopped. Record first overview, first usable detail and any partial state. Show the whole source, then 8 s, about 2 s, 250 ms, 20 ms and the closest supported view. Keep the event centered while changing scale, reverse each input sequence, and measure event placement against the time bounds. Record observed limits instead of assigning an unsupported zoom span. |
| 2 | Same fixture. Use the three equal-amplitude tones, their mixture, the sweep, clipped block, quiet block and stereo opposition. | Capture matched detail at two scales and one overview. Check band dominance, mixed colors, edge symmetry, clipping, low-level information, silence and antiphase visibility. Keep measured support and display hypotheses separate. |
| 3 | Dense hardcore or hardstyle selection and a 60–80-minute continuous CD image. Mark an isolated kick, dense drop, bass tail, breakdown and a known CUE join. | Observe overview readiness and seek before completion. Walk one kick from CD overview through phrase, beat, transient and closest scales. Repeat seek and fast zoom reversal during playback. Use the CD image's authored CUE times for cross-app registration whether or not Studio reads the sheet. |
| 4 | Representative electronic, guitar, acoustic, orchestral, ambient and voice excerpts. | Choose a small set that covers techno/house, DnB, industrial/uptempo or hardstyle, rock/metal, transients in exposed acoustic material, quiet ambient or speech, and high-dynamic-range orchestral changes. Reuse suitable historical captures with provenance. Choose one distinguishing event per recording and use two scales. |
| 5 | Short generated or already authorized excerpt copied into the scratch area. | If stem controls are available, collect an original/active/muted comparison with the same viewport. Separate first initialization from subsequent toggles. Record changes to geometry, color and muted regions. |

For input behavior, try both keyboard and wheel. Compare wheel input near 25%, 50% and 75% of the detail width while paused, then repeat playing. Record anchor motion, the first visible response, any transient movement, temporarily blurry or absent detail, and whether each adjustment changes the visible times continuously or by increments. Use the sample-deck overview to seek to the same event before each run. Where equipment permits, collect a synchronized present trace during this short sequence. A screen recording can show relative behavior at its recorded cadence. Record that cadence with the evidence.

Generate the stereo PCM16 control at 44.1 kHz with `python tools/waveform_control.py .local/waveform-mission/fixtures/control.wav`. The 70, 880 and 6,300 Hz blocks start at 1, 2 and 3 seconds. The mixture and sweep start at 4 and 5 seconds. The isolated impulse is at frame 308,737 (7 seconds + 37 frames). The bass/treble attack starts at 9 seconds, clipped tone at 11, quiet tone at 13, antiphase at 15, and split-channel tones at 17. Repeated attacks and dense texture run from 19 through 23 seconds. Intervening gaps contain exact zeros.

## Evidence record

For each PNG or motion trace, save a row with:

```text
capture_id,app,version,source_id,source_hash,sample_rate,source_frame_start,source_frame_end,event_frame,viewport_width_px,viewport_height_px,resolution,scale,display_hz,transport_state,settings,input_or_stage,capture_filename,observation,hypothesis
```

Use local aliases or fixture IDs in shared summaries. If bounds cannot be read precisely, mark them as estimated with the registration method and error. Keep a full lossless original and make any analytical crops separately. Record current application state before interaction and return to the previous project or leave the scratch project unsaved. Preserve any user-owned state already open.

## Matched Disqro qualification

Replay the same source region and comparable physical density in the pinned Disqro baseline, retained Dekzer references where practical, and the selected Disqro candidate. Check frame position numerically against the original PCM even when the display looks nearly identical. Compare peak preservation, signed shape and stereo handling, frequency readability, pending coverage and stability under zoom and pan.

Measure warm and cold-derived-cache runs separately. Record first nonempty geometry, first useful region, whole-source completion, detailed-region readiness, runtime CPU/GPU and private/working-set peaks. Apply paused and playing scenarios, one and multiple waveform views, and concurrent library work. Keep headless CPU, GPUI frame, GPU timestamp, compositor present and physical input-to-display observations as separate clocks. Report sample counts, median, p95, p99, maximum and missed/late counts at 8.33 ms and, where measurable, 4.17 ms.

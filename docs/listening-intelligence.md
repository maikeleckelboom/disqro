# Listening Intelligence

Mission 22 builds on the production workspace, including contextual reveal, independent Music Browsers, CUE transport, column pinning and visualization. The final hardening pass continues draft PR 22 and preserves the initial qualification evidence. [Production review](production-consolidation.md)

The queue continues to represent the playback plan and its traversed entries. Its existing panel is now labeled Queue History. Listening History records playback occurrences independently of that plan. Library opens Recently Played in the active Music Browser. The Workspace menu opens Listening History and Listening Statistics as dockable views.

## Playback accounting

The Kira streaming sound publishes its consumed source cursor after each render block. The transport drains timestamped observations and hands checkpoints to a separate writer. An immutable voice starts at its prepared cursor, including after a seek. The renderer can reconcile Kira's stale final cursor when it observes a natural ring drain and the decoder has supplied the full contiguous frame count. An expected duration or a stopped handle alone supplies no credit. A short or invalid decode earns only observed progression. Muted playback earns credit when rendering progresses. The cursor measures progression at the software renderer.

A pause and resume retain the occurrence. Ordinary seeks within the same recording retain it and credit subsequent progression from the new cursor. Replayed sections add listening time to the same occurrence. An explicit restart, repeat iteration or track change creates another occurrence when the new voice progresses. Automatic chapter progression divides credit at exact authored CUE offsets, including when consecutive chapters share one decoder. Moving the current playback target to a different chapter ends the previous occurrence. Natural EOF, stop, replacement, source failure, output failure and shutdown have separate end reasons.

One occurrence earns one play at the smaller of 30 seconds and half the logical recording duration. An unknown duration uses 30 seconds until the duration is verified. Qualification is latched. Shorter listens appear in history and count toward listening time. Recently Played and Last Played use the instant the newest qualifying occurrence first qualified. Plays and Listening Time use numeric sorting, and Last Played sorts by its timestamp.

UTC timestamps use a wall-clock anchor with a monotonic clock for the run. Each render block records its timestamp before the transport can observe it, so delayed transport polling does not shift ordinary activity or a stall across midnight. Commands that edit a contiguous CUE program use the same completed-block cursor for queue state and credit. Seek, stop and replacement close the renderer gate and let the transport drain an in-flight block for up to 40 ms. A callback stalled beyond that bound produces a notice and credit remains at the last published cursor. A voice holds up to 8,192 timestamped render blocks. After overflow it can still reconcile consumed totals from the published cursor, while the missing interval's placement is approximate. Natural completion is dated at the last credited activity. Local calendar ranges are calculated from individual local midnights, including 23-hour and 25-hour days. UTC minute buckets preserve splits across contemporary time-zone offsets even if the user views their history in another zone. Qualification belongs to its own instant. History's date filters refer to occurrence start.

## Identity and metadata

Listening checkpoints bind to the catalog’s logical recording IDs. An unindexed recording can be adopted without indexing its containing collection. CUE chapters retain their source file, sheet, chapter number and frame slice. A continuous file without chapters is one recording. Verified catalog relocation keeps the same recording ID, including on reattachment when the former path is absent and the native object and revision match. It invalidates listening locators and refreshes affected folder listings in all open Music Browsers. A delayed first checkpoint can bind after a verified move when the same native file and revision, plus the CUE sheet and frame slice where applicable, resolve to a single current track. History keeps its metadata snapshot and plays from the current location when available. Unresolved occurrences retain their snapshots and listening time and are counted separately.

Artist statistics group the full track-artist credit after trimming and case normalization. Composite credits stay together and missing artist tags are excluded from artist counts. Collection indexing preserves multi-disc release evidence with each member's recording identity. That evidence survives detachment, reattachment, and metadata edits. Indexing can reconcile an earlier standalone attribution when the exact recording now has authored release evidence. Conflicting established release assignments are preserved. Detached discs with no verified collection grouping retain their individual album or CUE identities. Loose files without release evidence are excluded from release counts. History retains recorded labels. All-time recording rankings can show the current title and artist for a catalog identity.

## Durability and local data ownership

Schema 5 extends the existing SQLite library with authored release assignments. The migration takes a backup of older libraries and runs transactionally. The occurrence and its UTC minute totals are the history record. Triggers maintain recording, artist and release totals in the same transaction. The recording projection supports future rules over play counts, last qualification and cumulative listening time alongside the catalog's existing metadata.

The writer saves the first progress, qualification, approximately every four seconds during playback, and at pause, seek and termination boundaries. A sequence number makes retries idempotent. The writer keeps up to 4,096 admitted checkpoints, including the active retry, and 16 queued storage commands. It retries the oldest failed checkpoint in order with a one-second delay following an error. The transport never waits on SQLite.

When the checkpoint budget fills, the last admitted checkpoint closes its occurrence with a capture-suspended reason where applicable. Disqro reports that new activity is excluded while capture is suspended. It retains all admitted activity in memory until the database accepts it, resumes capture at a new occurrence after recovery, and reports the recovery. Pending memory is lost on forced termination. Graceful shutdown asks the writer to drain and offers to keep Disqro open if saving failed. Startup recovery closes unfinished durable occurrences at their last saved activity. One process owns the SQLite capture writer. A second process can read the same library, buffers its own checkpoints under the same budget, and retries ownership until the first releases it. Open views receive live invalidation from their own application's commits.

Deleting a listen updates the dependent totals, rankings and Recently Played. The History menu's clear action confirms removal of local occurrences and their projections. A deletion follows all earlier admitted checkpoints. When those writes are blocked, the deletion returns an error and can be retried. A deleted active occurrence stays suppressed for that occurrence's remaining playback. The next replay or recording begins a new occurrence after new progress. Favorites, playlists, queue and workspace state use their existing persistence.

## Statistics

History and Statistics have separate SQLite workers. Statistics cancels superseded or hidden queries and publishes results with their period and ranking metric. Controls remain available while a replacement query is loading. A history deletion cancels the current calculation as soon as the writer's committed change reaches the catalog. Passive refreshes are coalesced on a ten-second schedule while Statistics is visible. Relative date ranges are recalculated on refresh. The workspace opens one Statistics dock. Each view owns its own query ticket and preferences.

The chart shows daily bins for the 7-day and 30-day ranges. Longer spans use labeled intervals of 7, 28, 91, or 364 days, increasing to longer intervals as needed. The tooltip shows the inclusive dates and the listening duration to the stored precision. The rank action reveals an available recording in the active Music Browser. Historical rankings still include unavailable recordings.

## Automated verification

- Locked suite on Rust 1.99.1 on Windows passed 196 tests with 10 opt-in tests.
- Formatting, Clippy with warnings denied, and locked debug and release builds passed.
- Deterministic coverage includes qualification thresholds, cursor discontinuities, pause and stall accounting, CUE edits crossing a renderer boundary, natural completion and truncation, delayed transport at a calendar boundary, replay, blocked ownership and bounded recovery, deletion, relocation after a delayed first checkpoint, and authored releases across detachment, metadata edits and reattachment. The Amsterdam time-zone check covers the spring 23-hour and autumn 25-hour days.
- Three optimized opt-in real-device audio tests passed. They exercised contiguous CUE playback, failed-source recovery and separate-file joins. In an interleaved comparison of 36 joins per build, both medians were 20.00 ms. The reviewed build ranged from 9.33 to 30.67 ms, and the hardening build ranged from 9.33 to 41.33 ms. The maxima differ by one observed buffer interval. These observations leave the separate-file tail latency and physical audibility open for the endpoint run. [Interleaved join measurements](evidence/listening-intelligence/separate-file-joins.csv)

## Native Windows verification

Qualification used isolated Windows profiles on a private desktop. The application used the real CPAL output device with qualification muting after the internal PCM observer. The Dark theme was inspected at 144 DPI in 1,240 × 780 and 720 × 460 logical-pixel windows, and in a three-pane workspace. A native DPI-change message exercised 96 DPI.

| Check | Observed result |
| --- | --- |
| Selection and queue admission | Selecting and queueing a 10-second WAV produced zero occurrences. Playback passed the 5-second threshold and appeared in Recently Played. |
| Playback, pause and restore | Paused playback retained its credited duration. Graceful exit saved a shutdown end reason. Relaunch restored the dock and history without crediting the paused queue. |
| Recently Played and columns | An unmodified click from Statistics reused the already open Music Browser. The three optional listening columns were enabled in the track table and sorted by Plays. |
| History and clearing | Removed a single listen after relaunch. Cancel and confirm were exercised for full history clearing during playback. Later checkpoints did not recreate the removed occurrence. |
| CUE and hard termination | Two chapters in one WAV image each received exactly 6.000 seconds and one play. In the final hard-kill run, 4.005 seconds survived and recovery marked the occurrence interrupted. Its last durable activity preceded termination by 2.137 seconds. |
| Indexed multi-disc release and repeat | Two recording IDs shared one release key. Two iterations of Repeat One produced two qualified occurrences for the second recording. The compilation credited each chapter's own performer. |
| Verified source relocation | Two simultaneously open folder Music Browsers updated the path, Plays, Last Played and Listening Time after a library rescan. The saved recording ID and credit were identical before and after the rename. Playback had already stopped. [Native evidence](evidence/listening-intelligence/relocation-two-browsers.webp) |
| Populated schema 3 profile | The initial native upgrade preserved three likes, a four-entry playlist and the restored queue without recording any history. The schema 5 concurrent migration test verifies that a populated schema 3 fixture creates one backup and retains its playlist. |
| Concurrent views | Browsed and filtered music with two browser instances while History displayed the synthetic catalog and Statistics switched ranges. A real 180-second WAV progressed through qualification during the workload. |
| Statistics navigation | Switched the ranking metric and date range while History paginated, then hid and reopened Statistics. Revealing an available ranked CUE track reused the open browser. The chart tooltip showed the date and exact listening duration. |

### Native screenshots

- [Recently Played](evidence/listening-intelligence/recently-played.webp) and [optional columns](evidence/listening-intelligence/listening-columns.webp)
- [Listening History](evidence/listening-intelligence/history.webp), [empty state](evidence/listening-intelligence/history-empty.webp) and [clear confirmation](evidence/listening-intelligence/clear-confirmation.webp)
- [Listening Statistics](evidence/listening-intelligence/statistics.webp), [compact dock](evidence/listening-intelligence/statistics-compact.webp), [96 DPI](evidence/listening-intelligence/statistics-96-dpi.webp), [empty statistics](evidence/listening-intelligence/statistics-empty.webp) and [exact chart tooltip](evidence/listening-intelligence/statistics-tooltip.webp)
- [Dense history and statistics](evidence/listening-intelligence/large-history.webp) and [Music Browser beside Statistics](evidence/listening-intelligence/concurrent-browser-and-statistics.webp), using a synthetic historical catalog
- [Indexed multi-disc playback and repeat](evidence/listening-intelligence/indexed-release.webp)

The isolated *Blue Hour Sessions* fixture uses fictional Mara Voss Quartet metadata, a generated cover and a synthetic WAV. The full and compact Statistics screenshots use this fixture. The album cover was generated with ImageGen from a prompt for a tungsten-lit jazz stage with a saxophone, an upright bass and midnight-blue curtains.

## Scale measurements

The optimized Windows fixture on a Ryzen 7 8845HS contains 100,000 catalog recordings and 1,000,000 occurrences distributed across five years. It holds 2,000,000 sparse UTC minute rows and occupies 350.1 MiB after checkpointing. The hardening run measured interactive queries while local builds were also running.

| Operation | Initial qualification | Hardening |
| --- | ---: | ---: |
| Recently Played, 100,000 ordered keys | 396.1 ms | 626.9 ms |
| Sort 100,000 music rows by Plays | 91.2 ms | 198.3 ms |
| First History page | 3.8 ms | 7.9 ms |
| History cursor page at 500,000 | 3.8 ms | 7.9 ms |
| Statistics, 7 days | 73.6 ms | 159.6 ms |
| Statistics, 30 days | 513.3 ms | 696.2 ms |
| Statistics, all time | 660.3 ms | 640.5 ms |
| History cursor page during all-time Statistics | — | 8.1 ms |
| Hydrate five Music Browser rows during all-time Statistics | — | 0.4 ms |
| Statistics, all time, during both interactive reads | — | 600.4 ms |
| Adopt unindexed recording and persist qualifying checkpoint | 9.3 ms | 7.1 ms |
| Incremental checkpoint | 1.2 ms | 0.9 ms |

The fixture reconciled to 1,000,000 qualified plays, 25,000 listening hours, 100,000 recordings, 3,000 artist credits and 6,000 releases. The history view keeps one page of 100 occurrences. The browser holds ordered keys and hydrates visible tracks. The statistics reader processes bounded calendar ranges and uses materialized totals for all-time ranking. Totals and compact time-series results are published to the dock.

Run `cargo test --locked --release listening_scale -- --ignored --nocapture` to reproduce the database measurements. Set `DISQRO_LISTENING_SCALE_DIR` to an existing scratch directory to keep a copy of that fixture for the native workload.

## Audio measurements

A quiet-system optimized run exercised the same large-history and browsing workload for 30 active diagnostic windows. It measured **2,976 callback samples, zero device or decode errors, and one unchanged-position tick**. Maximum reported callback load was 0.9% of the buffer budget. Maximum cursor-delta deviation was 8.787 ms. Peak private committed memory was 280.3 MiB and peak working set was 159.2 MiB. [Optimized workload trace](evidence/listening-intelligence/windows-release-load.csv)

A debug stress run queried the million-occurrence profile, browsed two Music Browsers and ran alongside a local compiler build. Its 31 active diagnostic windows included 3,002 callback samples, zero reported device or decode errors and one unchanged-position tick. Peak private committed memory was 294.5 MiB and peak working set was 185.3 MiB. One window measured a callback load of 8.109 times its buffer budget and a 49.39 ms cursor-delta deviation. [Debug workload trace](evidence/listening-intelligence/windows-debug-load.csv)

The hardening comparison used the same three-pane workload for each build. History paginated, Statistics switched ranges and metrics, two Music Browsers filtered tracks, and CPAL rendered a synthetic 180-second WAV. The compiler ran for 24.87 seconds during the final debug comparison.

| Build and load | Active windows | Callback samples | Peak callback budget use | Max cursor-delta deviation | Peak private / working set |
| --- | ---: | ---: | ---: | ---: | ---: |
| Optimized, quiet | 33 | 3,267 | 2.3% | 17.46 ms | 318.1 / 169.6 MiB |
| Debug, quiet | 34 | 3,315 | 9.7% | 12.45 ms | 345.2 / 217.8 MiB |
| Debug, local compiler | 35 | 3,421 | 8.0% | 12.95 ms | 319.0 / 186.3 MiB |

Each run reported zero device, discarded device, or decoder errors and one unchanged-position tick. Kira measures elapsed wall time inside the render callback, so the ratio includes scheduler delays. The quiet debug peak exceeded the optimized peak. Adding a concurrent compiler build did not reproduce the earlier 8.109× spike. These samples cannot establish its cause or rule out scheduling outliers. [Optimized trace](evidence/listening-intelligence/hardening-release-load.csv) · [Quiet debug trace](evidence/listening-intelligence/hardening-debug-load.csv) · [Compiler-load debug trace](evidence/listening-intelligence/hardening-debug-compiler-load.csv)

The final optimized run repeated the three-pane workload after the ranked navigation change. It measured 3,265 callback samples across 33 active windows, a 1.2% peak callback budget ratio, a 9.672 ms maximum cursor-delta deviation, 279.7 MiB peak private commitment and a 135.1 MiB peak working set. It reported one unchanged-position tick and zero device, discarded device, or decoder errors. History paginated and the final all-time Statistics result was observed within 2.312 seconds of the native input sequence, including automation and accessibility tree overhead. [Final optimized trace](evidence/listening-intelligence/hardening-final-release-load.csv)

The load traces describe one Windows machine. Callback timing, cursor progression and device errors describe software behavior. Physical audibility, endpoint unplug and replug, and downstream hardware underruns require separate evidence.

## Findings ledger

| Classification | Finding | Disposition |
| --- | --- | --- |
| Confirmed defects | Verified moves left the old listening locator and a stale folder listing in each open browser. | The catalog refreshes the listening projection and reloads affected folder views. Two open folder browsers rendered the new path with the same recording ID and totals after playback ended. |
| Confirmed defect | Detached discs could fall back to different release keys after their registered collection was removed. | Authored membership is persisted with the recording IDs. Deterministic tests cover detachment, post-detachment listening, reattachment, retagging, and a separate edition with matching tags. |
| Corrected risk | The transport used the declared playback limit for a stopped voice and assigned elapsed intervals at poll time. | The renderer publishes consumed frame progression and per-block time. Exact natural EOF requires evidence that the ring drained and the decoder supplied its full contiguous range. A truncated WAV test verifies that the advertised remainder receives no credit. |
| Concern did not reproduce | Whether selection, queue admission, seeking or stalls can create listens or credit unrendered ranges. | Admission creates no occurrence. Deterministic cursor and lifecycle tests verify paused or unchanged progression gives no credit, seeking preserves the occurrence, and explicit replay creates a new one. |
| Confirmed defect | Repeated writes could grow the pending queue indefinitely and delete acknowledgements could leave an active query behind old results. | The bounded policy retains admitted checkpoints in order, surfaces storage errors and capture suspension, and invalidates running reads on committed deletion. The ownership and injected write-failure tests check bounded recovery, ordering, idempotence and erase behavior. |
| Confirmed defect | Asynchronous reranking could render the previous result with the new metric. | Each result carries its period and metric. Selection, deletion, hiding and superseding queries cancel the old ticket. Ordinary History queries use a separate worker. |
| Measured results | Native renderer, views and scale. | The Windows test results, workload traces and native screenshots are listed above. The 8,192-block timing buffer and 40 ms final-drain boundary bound the work of an exceptionally delayed transport. |
| Remaining limitations | Physical output, hardware underruns, and storage media exhaustion. | The Windows audio trace could not start under the available account. SQLite locks, writer ownership and write errors were exercised using isolated databases. A physically full disk, audible endpoint continuity and endpoint unplug and replug remain owner checks. |

## Owner-run endpoint qualification

1. Build with `.\build.ps1 -Release`. Open an elevated PowerShell for system tracing and check that `wpr -status` reports no existing recording. Use an ordinary desktop session for the player.
2. In the player's PowerShell, set `DISQRO_PROFILE_DIR` to a new temporary directory and `DISQRO_PLAYBACK_TRACE` to a new CSV path in it. Open the optimized player normally and use a known 48 kHz stereo recording or a continuous CUE image with at least three chapters. Use a headphone or speaker endpoint at a comfortable volume. If using a qualification build, start it without `--qualify-silent`.
3. From the elevated shell, run `wpr -start Audio -filemode`. Start playback, open History and Statistics beside a Music Browser and switch chart ranges, ranking modes, history pages and library filters for at least 60 seconds. Listen for gaps through the joins. Where available, record the analog output with an independent recorder.
4. Record the time, device model, driver, Windows build, active endpoint, format and sample rate. Physically unplug the active USB device or analog endpoint while playing, wait for Windows to settle, and reconnect it. Record status, history end reason, playback position, whether other output took over, and whether Play can start again on the recovered endpoint. Verify the disconnected interval gives no credit when the renderer reports no progress.
5. Run `wpr -stop Disqro-audio.etl`. Analyze the Audio Glitches and CPU scheduling events in Windows Performance Analyzer and correlate them with the CSV and unplug times. For a hardware underrun claim, attach the endpoint's driver or hardware counters where available. Document the listener's observations and any analog capture independently from the software measurements.

On this host, `wpr -profiles` included `Audio`. Starting it failed with Windows system performance policy error `0xc5585011`.

The isolated profile's `library.sqlite3` is available for read-only inspection. Query `listening_occurrences` for `started_at`, `activity_at`, `ended_at`, `reason`, and `credited_us`. The timestamp and duration fields are in microseconds.

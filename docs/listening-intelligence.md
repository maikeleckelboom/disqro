# Listening Intelligence

Mission 22 builds on the production workspace, including contextual reveal, independent Music Browsers, CUE transport, column pinning and visualization. Draft PR 22 includes renderer accounting, local listening history and statistics, and the final corrections to renderer timing and ranked navigation. [Production review](production-consolidation.md)

The queue continues to represent the playback plan and its traversed entries. Its existing panel is now labeled Queue History. Listening History records playback occurrences independently of that plan. Library opens Recently Played in the active Music Browser. The Workspace menu opens Listening History and Listening Statistics as dockable views.

## Playback accounting

The Kira streaming sound publishes its consumed source cursor after each render block. The transport drains timestamped observations and hands checkpoints to a separate writer. An immutable voice starts at its prepared cursor, including after a seek. The renderer can reconcile Kira's stale final cursor when it observes a natural ring drain and the decoder has supplied the full contiguous frame count without error. A decoder failure permanently disqualifies that tail reconciliation, even if the decoded extent appears complete. A short or invalid decode earns only observed progression. Muted playback earns credit when rendering progresses. The cursor measures progression at the software renderer.

A pause and resume retain the occurrence. Ordinary seeks within the same recording retain it and credit subsequent progression from the new cursor. Replayed sections add listening time to the same occurrence. An explicit restart, repeat iteration or track change creates another occurrence when the new voice progresses. Automatic chapter progression divides credit at exact authored CUE offsets, including when consecutive chapters share one decoder. Moving the current playback target to a different chapter ends the previous occurrence. Natural EOF, stop, replacement, source failure, output failure and shutdown have separate end reasons.

One occurrence earns one play at the smaller of 30 seconds and half the logical recording duration. An unknown duration uses 30 seconds until the duration is verified. Qualification is latched. Shorter listens appear in history and count toward listening time. Recently Played and Last Played use the instant the newest qualifying occurrence first qualified. Plays and Listening Time use numeric sorting, and Last Played sorts by its timestamp.

UTC timestamps use a wall-clock anchor with a monotonic clock for the run. Each render block queues its source interval with its timestamp, then publishes the consumed cursor. The transport snapshots that cursor and drains only committed blocks. Repeated or regressive observations leave the activity timeline unchanged. Delayed transport polling therefore preserves ordinary block attribution across minute and local midnight boundaries.

Commands that edit a contiguous CUE program use the same completed-block cursor for queue state and credit. Seek, stop and replacement close the renderer gate and let the transport drain an in-flight block for up to 40 ms. A callback stalled beyond that bound produces a notice and credit remains at the last published cursor. A voice holds up to 8,192 timestamped blocks. In an overflow, the transport credits only the missing duration through the committed cursor or the active program limit. It estimates a gap against the next recorded block when available, or extends the prior credited edge for a trailing gap. That placement can shift minute or date attribution. A shortened CUE program leaves partially credited blocks in the ring, so restoring the program preserves their timing. The ring provides the timing for recorded blocks and the cursor preserves consumed totals.

Natural completion is dated at the last credited activity. Local calendar ranges use individual local midnights, including 23-hour and 25-hour days. UTC minute buckets preserve splits across contemporary time-zone offsets even if the user views their history in another zone. Qualification belongs to its own instant. History's date filters refer to occurrence start.

## Identity and metadata

Listening checkpoints bind to the catalog’s logical recording IDs. An unindexed recording can be adopted without indexing its containing collection. CUE chapters retain their source file, sheet, chapter number and frame slice. A continuous file without chapters is one recording. Verified catalog relocation keeps the same recording ID, including on reattachment when the former path is absent and the native object and revision match. It invalidates listening locators and refreshes affected folder listings in all open Music Browsers. A delayed first checkpoint can bind after a verified move when the same native file and revision, plus the CUE sheet and frame slice where applicable, resolve to a single current track. History keeps its metadata snapshot and plays from the current location when available. Unresolved occurrences retain their snapshots and listening time and are counted separately.

Artist statistics group the full track-artist credit after trimming and case normalization. Composite credits stay together and missing artist tags are excluded from artist counts. Collection indexing preserves multi-disc release evidence with each member's recording identity. That evidence survives detachment, reattachment, and metadata edits. Indexing can reconcile an earlier standalone attribution when the exact recording now has authored release evidence. Conflicting established release assignments are preserved. Detached discs with no verified collection grouping retain their individual album or CUE identities. Loose files without release evidence are excluded from release counts. History retains recorded labels. All-time recording rankings can show the current title and artist for a catalog identity.

## Durability and local data ownership

Schema 5 is the authoritative listening schema. Fresh profiles and upgrades from production install the same listening definitions in one transaction. Upgrades back up the production library and check its foreign keys before commit. Profiles already at schema 5 open directly. The obsolete development schema 4 is refused with instructions to keep the existing database and use a fresh profile or explicitly reset a disposable development database. Initialization leaves databases and music files intact when it refuses a version.

An occurrence stores its recording link, metadata snapshot, activity and qualification timestamps, end state and checkpoint sequence. Sparse UTC minute rows keep calendar attribution independent of the occurrence start. Triggers maintain recording totals and materialized artist and release rankings in the checkpoint transaction. Authored release evidence stays with each track through catalog detachment. Indexes support History pagination, dated bins and qualifications, recency, open-occurrence recovery, track totals and verified relocation.

The writer saves the first progress, qualification, approximately every four seconds during playback, and at pause, seek and termination boundaries. A sequence number makes retries idempotent. The writer keeps up to 4,096 admitted checkpoints, including the active retry, and 16 queued storage commands. It retries the oldest failed checkpoint in order with a one-second delay following an error. The transport never waits on SQLite.

When the checkpoint budget fills, the last admitted checkpoint closes its occurrence with a capture-suspended reason where applicable. Disqro reports that new activity is excluded while capture is suspended. It retains all admitted activity in memory until the database accepts it, resumes capture at a new occurrence after recovery, and reports the recovery. Pending memory is lost on forced termination. Graceful shutdown asks the writer to drain and offers to keep Disqro open if saving failed. Startup recovery closes unfinished durable occurrences at their last saved activity. One process owns the SQLite capture writer. A second process can read the same library, buffers its own checkpoints under the same budget, and retries ownership until the first releases it. Open views receive live invalidation from their own application's commits.

Deleting a listen updates the dependent totals, rankings and Recently Played. The History menu's clear action confirms removal of local occurrences and their projections. A deletion follows all earlier admitted checkpoints. When those writes are blocked, the deletion returns an error and can be retried. A deleted active occurrence stays suppressed for that occurrence's remaining playback. The next replay or recording begins a new occurrence after new progress. Favorites, playlists, queue and workspace state use their existing persistence.

## Statistics

History and Statistics have separate SQLite workers. Statistics cancels superseded or hidden queries and publishes results with their period and ranking metric. Controls remain available while a replacement query is loading. A history deletion cancels the current calculation as soon as the writer's committed change reaches the catalog. Passive refreshes are coalesced on a ten-second schedule while Statistics is visible. Relative date ranges are recalculated on refresh. The workspace opens one Statistics dock. Each view owns its own query ticket and preferences.

The chart shows daily bins for the 7-day and 30-day ranges. Longer spans use labeled intervals of 7, 28, 91, or 364 days, increasing to longer intervals as needed. The tooltip shows the inclusive dates and the listening duration to the stored precision. The rank action reserves the active Music Browser's navigation generation before querying the recording. Results can change only the original browser while that generation and browser are still current. A newer navigation, another active browser or closure makes both results and errors obsolete. A successful lookup reveals the existing pane in its existing dock placement. Historical rankings still include unavailable recordings.

## Automated verification

- Locked suite on Rust 1.99.1 on Windows passed 199 tests with 11 opt-in tests.
- Formatting, Clippy with warnings denied, and locked debug and release builds passed.
- The delayed-renderer fixture holds the actual publication path between queuing the second block and committing its cursor. Repeated and regressive observations leave the first block's timestamp intact. Both blocks contribute all 20 ms to the day before local midnight. A one-slot ring fixture verifies interior and trailing overflow, a split across minutes, recovery of exact timing for a later block and a single 50 ms total. A third case restores a shortened CUE program and checks that pending block timestamps still split credit across the minute boundary.
- Other deterministic coverage includes pause, stalls, seeks, CUE edits crossing a renderer boundary, natural completion and truncation, replay, ordered checkpoint retries, bounded recovery, deletion, verified relocation and authored release identity. The Amsterdam time-zone check covers the spring 23-hour and autumn 25-hour days.
- The opt-in native GPUI regression controls lookup completion around newer navigation, active-browser switches, out-of-order results and target closure. It verifies successful navigation and notification for an unavailable recording. Run it with `cargo test --locked delayed_ranked_reveal -- --ignored` in a Windows desktop session.
- The optimized audio suite passed 22 tests. The FFmpeg codec check and three opt-in CPAL checks also passed, covering contiguous CUE playback, failed-source recovery and separate-file joins. The final join observations were 20.00 ms, 20.00 ms and 9.33 ms below the PCM threshold. [Optimized test results](evidence/listening-intelligence/final-optimized-checks.txt)
- In the earlier interleaved comparison of 36 joins per build, both medians were 20.00 ms. The reviewed build ranged from 9.33 to 30.67 ms, and the hardening build ranged from 9.33 to 41.33 ms. The maxima differ by one observed buffer interval. These observations leave the separate-file tail latency and physical audibility open for the endpoint run. [Interleaved join measurements](evidence/listening-intelligence/separate-file-joins.csv)

## Native Windows verification

Qualification used isolated Windows profiles on a private desktop. The application used the real CPAL output device with qualification muting after the internal PCM observer. The final pass repeated Statistics reveal and unavailability, paused playback and restart, history removal, production migration, two-process writer recovery, contiguous CUE playback and recovery after forced termination. [Native test results](evidence/listening-intelligence/final-native-checks.txt)

The Dark theme was inspected at 144 DPI in 1,240 × 780 and 720 × 460 logical-pixel windows, and in a three-pane workspace. A native DPI-change message exercised 96 DPI.

| Check | Observed result |
| --- | --- |
| Selection and queue admission | Selecting and queueing a 10-second WAV produced zero occurrences. Playback passed the 5-second threshold and appeared in Recently Played. |
| Playback, pause and restore | Paused playback retained its credited duration. Graceful exit saved a shutdown end reason. Relaunch restored the dock and history without crediting the paused queue. |
| Recently Played and columns | An unmodified click from Statistics reused the already open Music Browser. The three optional listening columns were enabled in the track table and sorted by Plays. |
| History and clearing | Removed a single listen after relaunch. Cancel and confirm were exercised for full history clearing during playback. Later checkpoints did not recreate the removed occurrence. |
| Deletion with both docks visible | Cleared an active listen while History and Statistics were presented. Both views emptied, and the same occurrence remained suppressed across the next checkpoint while audio continued to render. |
| CUE and hard termination | Two chapters in one WAV image each received exactly 6.000 seconds and one play. In the final forced-termination run, 4.005 seconds survived and recovery marked the occurrence interrupted. Its last durable activity preceded termination by 2.229 seconds. |
| Indexed multi-disc release and repeat | Two recording IDs shared one release key. Two iterations of Repeat One produced two qualified occurrences for the second recording. The compilation credited each chapter's own performer. |
| Verified source relocation | Two simultaneously open folder Music Browsers updated the path, Plays, Last Played and Listening Time after a library rescan. The saved recording ID and credit were identical before and after the rename. Playback had already stopped. [Native evidence](evidence/listening-intelligence/relocation-two-browsers.webp) |
| Populated schema 3 profile | The native upgrade to schema 5 preserved three likes and four entries in one playlist. It backed up the schema 3 library and left history empty. The concurrent startup test verifies that multiple readers can open during the upgrade and share one completed migration. |
| Concurrent views | Browsed and filtered music with two browser instances while History displayed the synthetic catalog and Statistics switched ranges. A real 180-second WAV progressed through qualification during the workload. |
| Competing writer processes | A second optimized player continued rendering while the first held the writer lock. Its new activity remained unsaved until the first quit. After ownership recovered the second persisted one buffered occurrence with 8.77 seconds of credit and quit gracefully. |
| Statistics navigation | Switched the ranking metric and date range while History paginated, then hid and reopened Statistics. Revealing an available ranked CUE track reused the open browser. With the source temporarily moved, the same action showed an unavailable-recording notification and left Statistics open. The chart tooltip showed the date and exact listening duration. |

### Native screenshots

- [Recently Played](evidence/listening-intelligence/recently-played.webp) and [optional columns](evidence/listening-intelligence/listening-columns.webp)
- [Listening History](evidence/listening-intelligence/history.webp), [empty state](evidence/listening-intelligence/history-empty.webp) and [clear confirmation](evidence/listening-intelligence/clear-confirmation.webp)
- [Listening Statistics](evidence/listening-intelligence/statistics.webp), [compact dock](evidence/listening-intelligence/statistics-compact.webp), [96 DPI](evidence/listening-intelligence/statistics-96-dpi.webp), [empty statistics](evidence/listening-intelligence/statistics-empty.webp) and [exact chart tooltip](evidence/listening-intelligence/statistics-tooltip.webp)
- [Dense history and statistics](evidence/listening-intelligence/large-history.webp) and [Music Browser beside Statistics](evidence/listening-intelligence/concurrent-browser-and-statistics.webp), using a synthetic historical catalog
- [Indexed multi-disc playback and repeat](evidence/listening-intelligence/indexed-release.webp)
- [Unavailable ranked recording](evidence/listening-intelligence/unavailable-ranked-recording.webp)

The isolated *Blue Hour Sessions* fixture uses fictional Mara Voss Quartet metadata, a generated cover and a synthetic WAV. The full and compact Statistics screenshots use this fixture. The album cover was generated with ImageGen from a prompt for a tungsten-lit jazz stage with a saxophone, an upright bass and midnight-blue curtains.

## Scale measurements

The optimized Windows fixture on a Ryzen 7 8845HS contains 100,000 catalog recordings and 1,000,000 occurrences distributed across five years. It holds 2,000,000 sparse UTC minute rows and occupies 350.1 MiB after checkpointing. The hardening run measured interactive queries while local builds were also running. The final revalidation completed the same query sequence. [Initial qualification](https://github.com/maikeleckelboom/disqro/blob/a9576db4881c3262c515713558ab732a3da0b756/docs/listening-intelligence.md#scale-measurements) · [Hardening scale output](evidence/listening-intelligence/hardening-listening-scale.txt) · [Final scale output](evidence/listening-intelligence/final-listening-scale.txt)

| Operation | Hardening | Final |
| --- | ---: | ---: |
| Recently Played, 100,000 ordered keys | 626.9 ms | 515.1 ms |
| Sort 100,000 music rows by Plays | 198.3 ms | 175.7 ms |
| First History page | 7.9 ms | 7.9 ms |
| History cursor page at 500,000 | 7.9 ms | 6.4 ms |
| Statistics, 7 days | 159.6 ms | 116.4 ms |
| Statistics, 30 days | 696.2 ms | 580.4 ms |
| Statistics, all time | 640.5 ms | 523.0 ms |
| History cursor page during all-time Statistics | 8.1 ms | 8.3 ms |
| Hydrate five Music Browser rows during all-time Statistics | 0.4 ms | 0.5 ms |
| Statistics, all time, during both interactive reads | 600.4 ms | 569.1 ms |
| Adopt unindexed recording and persist qualifying checkpoint | 7.1 ms | 14.7 ms |
| Incremental checkpoint | 0.9 ms | 1.2 ms |

The fixture reconciled to 1,000,000 qualified plays, 25,000 listening hours, 100,000 recordings, 3,000 artist credits and 6,000 releases. The history view keeps one page of 100 occurrences. The browser holds ordered keys and hydrates visible tracks. The statistics reader processes bounded calendar ranges and uses materialized totals for all-time ranking. Totals and compact time-series results are published to the dock.

Run `cargo test --locked --release listening_scale -- --ignored --nocapture` to reproduce the database measurements. Set `DISQRO_LISTENING_SCALE_DIR` to an existing scratch directory to keep a copy of that fixture for the native workload.

## Audio measurements

The initial quiet-system optimized trace contains 31 active diagnostic windows. Across the full CSV it recorded **3,074 callback samples, zero reported device or decode errors, and one unchanged-position tick**. Maximum reported callback load was 0.9% of the buffer budget. Maximum cursor-delta deviation was 8.787 ms. [Optimized workload trace](evidence/listening-intelligence/windows-release-load.csv)

A debug stress run queried the million-occurrence profile, browsed two Music Browsers and ran alongside a local compiler build. The full CSV covers 32 active diagnostic windows, 3,091 callback samples, zero reported device or decode errors and two unchanged-position ticks. One window measured a callback load of 8.109 times its buffer budget and a 49.39 ms cursor-delta deviation. [Debug workload trace](evidence/listening-intelligence/windows-debug-load.csv)

The hardening comparison used the same three-pane workload for each build. History paginated, Statistics switched ranges and metrics, two Music Browsers filtered tracks, and CPAL rendered a synthetic 180-second WAV. The compiler load was applied during the third run.

| Build and load | Active windows | Callback samples | Peak callback budget use | Max cursor-delta deviation |
| --- | ---: | ---: | ---: | ---: |
| Optimized, quiet | 33 | 3,267 | 2.3% | 17.46 ms |
| Debug, quiet | 34 | 3,315 | 9.7% | 12.45 ms |
| Debug, local compiler | 35 | 3,421 | 8.0% | 12.95 ms |

Each run reported zero device, discarded device, or decoder errors and one unchanged-position tick. Kira measures elapsed wall time inside the render callback, so the ratio includes scheduler delays. The quiet debug peak exceeded the optimized peak. Adding a concurrent compiler build did not reproduce the earlier 8.109× spike. These samples cannot establish its cause or rule out scheduling outliers. [Optimized trace](evidence/listening-intelligence/hardening-release-load.csv) · [Quiet debug trace](evidence/listening-intelligence/hardening-debug-load.csv) · [Compiler-load debug trace](evidence/listening-intelligence/hardening-debug-compiler-load.csv)

The reviewed optimized run measured 3,265 callback samples across 33 active windows, 1.2% peak callback budget use and 9.672 ms maximum cursor-delta deviation. It reported one unchanged-position tick and zero device, discarded device or decoder errors. The 1.8% figure came from a separate intermediate run with 3,250 callback samples across 33 windows and 8.695 ms maximum cursor-delta deviation. [Reviewed optimized trace](evidence/listening-intelligence/hardening-final-release-load.csv) · [Intermediate optimized trace](evidence/listening-intelligence/hardening-intermediate-release-load.csv)

The final paired run used the exact reviewed head between two corrected runs, with the same copied million-occurrence fixture and native input sequence. Median and 95th percentile summarize each diagnostic window's peak callback ratio. Each run had one unchanged-position tick and zero reported device, discarded device or decoder errors. [Corrected A trace](evidence/listening-intelligence/final-load-a.csv) · [Reviewed head trace](evidence/listening-intelligence/reviewed-paired-load.csv) · [Corrected B trace](evidence/listening-intelligence/final-load-b.csv) · [Native workload output](evidence/listening-intelligence/paired-workload-reports.txt)

| Run | Active windows | Callback samples | Median / p95 / peak budget use | Max cursor-delta deviation |
| --- | ---: | ---: | ---: | ---: |
| Corrected A | 34 | 3,368 | 0.4% / 1.7% / 1.9% | 15.746 ms |
| Reviewed head | 33 | 3,260 | 0.4% / 2.4% / 2.6% | 9.989 ms |
| Corrected B | 33 | 3,270 | 0.3% / 3.0% / 3.2% | 8.770 ms |

Callback peaks and cursor deviations varied across runs. The corrected runs kept similar median callback load and peaked at 3.2% of the buffer budget. Each run qualified playback while History paginated and Statistics recalculated as browser filters changed. In the two-process test, capture persisted 8.77 seconds in a single buffered occurrence after writer ownership became available. The deterministic timing tests isolate the same accounting from UI polling entirely. [Writer ownership evidence](evidence/listening-intelligence/final-native-checks.txt)

The load traces describe one Windows machine. Callback timing, cursor progression and device errors describe software behavior. Physical audibility, endpoint unplug and replug, and downstream hardware underruns require separate evidence.

## Findings ledger

| Classification | Finding | Disposition |
| --- | --- | --- |
| Confirmed defects | Verified moves left the old listening locator and a stale folder listing in each open browser. | The catalog refreshes the listening projection and reloads affected folder views. Two open folder browsers rendered the new path with the same recording ID and totals after playback ended. |
| Confirmed defect | Detached discs could fall back to different release keys after their registered collection was removed. | Authored membership is persisted with the recording IDs. Deterministic tests cover detachment, post-detachment listening, reattachment, retagging, and a separate edition with matching tags. |
| Corrected risk | The transport used the declared playback limit for a stopped voice and assigned elapsed intervals at poll time. | The renderer publishes consumed frame progression and per-block time. Exact natural EOF requires a ring drain, the full contiguous decoded range and a clean decoder. Regression cases cover truncation and an earlier decoder failure with a complete extent. |
| Confirmed defect | Publishing a timestamp separately from the consumed cursor let an unchanged observation cross a minute or midnight. | Renderer timing comes from committed ring entries. The cursor bounds credit and overflow estimates. Zero and regressive observations leave the timeline unchanged. |
| Confirmed defect | A delayed ranked lookup could redirect to the newly active browser or supersede a newer route. | The originating browser reserves its existing navigation generation. The original browser's intent guards both pane reveal and error delivery. |
| Verified invariant | Selection, queue admission, seeking and stalls alone earn zero credit. | Deterministic cursor and lifecycle tests verify paused or unchanged progression gives zero credit, seeking preserves the occurrence, and explicit replay creates a new one. |
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

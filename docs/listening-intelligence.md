# Listening Intelligence

Mission 22 starts from production `main` at `dfd8c25f25d5f56c0c2b0800349d99702f24ece1` (unified breadcrumb navigation). The local tree was clean and matched the fetched remote. This includes the contextual reveal, independent Music Browsers, CUE transport, column pinning and visualization work listed in the [production review](production-consolidation.md).

The queue continues to represent the playback plan and its traversed entries. Its existing panel is now labeled Queue History. Listening History records playback occurrences independently of that plan. Library opens Recently Played in the active Music Browser. The Workspace menu opens Listening History and Listening Statistics as dockable views.

## Playback accounting

The Kira streaming sound publishes its consumed source cursor, with an additional publication after each render block. The callback uses atomics and the existing renderer. The transport samples progression and hands checkpoints to a separate writer. An immutable voice starts at its prepared cursor, including after a seek. Elapsed time, queue state and decoded lookahead supply no credit. Muted playback earns credit when rendering progresses. The cursor measures software output progression and carries no evidence of physical audibility.

A pause and resume retain the occurrence. Ordinary seeks within the same recording retain it and credit subsequent progression from the new cursor. Replayed sections add listening time to the same occurrence. An explicit restart, repeat iteration or track change creates another occurrence when the new voice progresses. Automatic chapter progression divides credit at exact authored CUE offsets, including when consecutive chapters share one decoder. Moving the current playback target to a different chapter ends the previous occurrence. Natural EOF, stop, replacement, source failure, output failure and shutdown have separate end reasons.

One occurrence earns one play at the smaller of 30 seconds and half the logical recording duration. An unknown duration uses 30 seconds until the duration is verified. Qualification is latched. Shorter listens appear in history and count toward listening time. Recently Played and Last Played use the instant the newest qualifying occurrence first qualified. Plays and Listening Time use numeric sorting, and Last Played sorts by its timestamp.

UTC timestamps use a wall-clock anchor with a monotonic clock for the run. Duration comes from the renderer cursor. The transport places each observed increment on the UTC timeline, normally with a 40 ms sampling cadence. At a voice boundary it drains a final in-flight block with a bounded wait. Local calendar ranges are calculated from individual local midnights, including 23-hour and 25-hour days. UTC minute buckets preserve splits across contemporary time-zone offsets even if the user views their history in another zone. Qualification belongs to its own instant. History's date filters refer to occurrence start.

## Identity and metadata

Listening checkpoints bind to the catalog’s existing logical recording IDs. An unindexed recording can be adopted without indexing its containing collection. CUE chapters retain their source file, sheet, chapter number and frame slice. A continuous file without chapters is one recording. History references protect catalog identities during collection removal. Verified catalog relocation keeps the same recording identity. History retains the metadata snapshot and can play from the current catalog location when the recording is available. When source identity cannot be verified at the first checkpoint, the occurrence still keeps its snapshot and listening time. Independent unresolved snapshots are counted separately.

Artist statistics group the full track-artist credit after case and whitespace normalization. Composite credits stay together and missing artist tags are excluded from artist counts. Release statistics use the catalog's authored multi-disc grouping when available, or the distinct catalog album or CUE identity. Detached discs from an unindexed multi-disc set may have separate album identities. Loose files without an album or sheet are excluded from release counts. Historical groupings use recorded metadata. All-time recording rankings can show the current title and artist for a catalog identity.

## Durability and local data ownership

Schema 4 extends the existing SQLite library. The migration takes a backup of older libraries and runs transactionally. The occurrence and its UTC minute totals are the history record. Triggers maintain recording, artist and release totals in the same transaction. The recording projection supports future rules over play counts, last qualification and cumulative listening time alongside the catalog's existing metadata.

The writer persists the first progress, qualification, approximately every four seconds during playback, and at pause and termination boundaries. A sequence number makes retries idempotent. The writer retries failed writes in order while playback continues. Recovery closes unfinished occurrences at their last durable activity. Graceful shutdown drains playback and waits for the writer. One process owns capture for a given library file. A second process using the same file can read history and receives a storage notice while the capture writer waits for ownership.

Deleting a listen updates the dependent totals, rankings and Recently Played. The confirmation in Listening History clears the local occurrences and their projections. A deleted active occurrence stays suppressed for that occurrence's remaining playback. The next replay or recording begins a new occurrence after new progress. Favorites, playlists, queue and workspace state use their existing persistence.

## Automated verification

- Locked suite on Rust 1.99.1 on Windows: **190 passed**, 10 opt-in.
- Formatting, Clippy with warnings denied, and locked debug and release builds passed.
- Deterministic coverage includes qualification thresholds, cursor discontinuities, pause and stall accounting, CUE boundaries, replay, write failure and retry, deletion and recovery, stable IDs, and the midnight split between duration and qualification. An Amsterdam system-time-zone run checked the spring 23-hour and autumn 25-hour days.
- Three opt-in real-device audio tests passed. They exercised contiguous CUE playback, failed-source recovery and separate-file transitions. The latter observed 12.00, 9.33 and 9.33 ms below the test's PCM threshold at the three joins.

## Native Windows verification

Qualification used isolated Windows profiles on a private desktop. The application used the real CPAL output device with qualification muting after the internal PCM observer. The current Dark theme was inspected at 144 DPI, including a 720 × 460 logical-pixel window. A native DPI-change message exercised 96 DPI.

| Check | Observed result |
| --- | --- |
| Selection and queue admission | Selecting and queueing a 10-second WAV produced zero occurrences. Playback passed the 5-second threshold and appeared in Recently Played. |
| Playback, pause and restore | Paused playback retained its credited duration. Graceful exit saved a shutdown end reason. Relaunch restored the dock and history without crediting the paused queue. |
| Recently Played and columns | An unmodified click from Statistics reused the already open Music Browser. The three optional listening columns were enabled in the track table and sorted by Plays. |
| History and clearing | Removed a single listen after relaunch. Cancel and confirm were exercised for full history clearing during playback. Later checkpoints did not recreate the removed occurrence. |
| CUE and hard termination | Two chapters in one WAV image received 6.000 seconds each and one play each. In a hard-kill run, 4.062 seconds survived and recovery marked the occurrence interrupted. Its last durable activity preceded termination by 2.202 seconds. |
| Indexed multi-disc release and repeat | The two discs kept distinct recording IDs and shared one release key. Two iterations of Repeat One produced two qualified occurrences for the second recording. The compilation credited each chapter's own performer. |
| Populated schema 3 profile | Upgraded a copied fixture and produced one schema backup. Three likes, one playlist and its four entries survived. The restored queue produced zero history. |
| Concurrent views | Browsed and filtered music with two browser instances while History displayed the synthetic catalog and Statistics switched ranges. A real 180-second WAV progressed through qualification during the workload. |

### Native screenshots

- [Recently Played](evidence/listening-intelligence/recently-played.webp) and [optional columns](evidence/listening-intelligence/listening-columns.webp)
- [Listening History](evidence/listening-intelligence/history.webp), [empty state](evidence/listening-intelligence/history-empty.webp) and [clear confirmation](evidence/listening-intelligence/clear-confirmation.webp)
- [Listening Statistics](evidence/listening-intelligence/statistics.webp), [compact dock](evidence/listening-intelligence/statistics-compact.webp) and [96 DPI](evidence/listening-intelligence/statistics-96-dpi.webp)
- [Dense history and statistics](evidence/listening-intelligence/large-history.webp) and [Music Browser beside Statistics](evidence/listening-intelligence/concurrent-browser-and-statistics.webp), using a synthetic historical catalog
- [Indexed multi-disc playback and repeat](evidence/listening-intelligence/indexed-release.webp)

## Scale measurements

One optimized Windows run on a Ryzen 7 8845HS created 100,000 catalog recordings and 1,000,000 occurrences distributed across five years. The fixture held 2,000,000 sparse UTC minute rows. It occupied 350.1 MiB after checkpointing. This sample includes the catalog, occurrence snapshots, indexes and projections.

| Operation | Elapsed |
| --- | ---: |
| Recently Played, 100,000 ordered keys | 396.1 ms |
| Sort 100,000 music rows by Plays | 91.2 ms |
| First history page | 3.8 ms |
| History cursor page halfway through | 3.8 ms |
| Statistics, 7 days | 73.6 ms |
| Statistics, 30 days | 513.3 ms |
| Statistics, all time | 660.3 ms |
| Adopt unindexed recording and persist qualifying checkpoint | 9.3 ms |
| Incremental checkpoint | 1.2 ms |

The fixture reconciled to 1,000,000 qualified plays, 25,000 listening hours, 100,000 recordings, 3,000 artist credits and 6,000 releases. The history view keeps one page of 100 occurrences. The browser holds ordered keys and hydrates visible tracks. The statistics reader processes bounded calendar ranges and uses materialized totals for all-time ranking. Totals and compact time-series results are published to the dock.

## Audio measurements

A quiet-system optimized run exercised the same large-history and browsing workload for 30 active diagnostic windows. It measured **2,976 callback samples, zero device or decode errors, and one unchanged-position tick**. Maximum reported callback load was 0.9% of the buffer budget. Maximum cursor-delta deviation was 8.787 ms. Peak private committed memory was 280.3 MiB and peak working set was 159.2 MiB. [Optimized workload trace](evidence/listening-intelligence/windows-release-load.csv)

A debug stress run queried the million-occurrence profile, browsed two Music Browsers and ran alongside a local compiler build. Its 31 active diagnostic windows included 3,002 callback samples, zero reported device or decode errors and one unchanged-position tick. Peak private committed memory was 294.5 MiB and peak working set was 185.3 MiB. One window measured a callback load of 8.109 times its buffer budget and a 49.39 ms cursor-delta deviation. [Debug workload trace](evidence/listening-intelligence/windows-debug-load.csv)

Physical audibility, hardware underrun counters and a physical endpoint disconnect and reconnect remain unverified. The load traces describe these runs on one Windows machine. Listening Statistics caps the all-time chart at 42 groups for long histories. The 7-day and 30-day views show daily bars.

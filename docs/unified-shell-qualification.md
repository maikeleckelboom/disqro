# Mission 11 Windows qualification

The optimized Windows player was exercised with Win32 input, Windows UI Automation and isolated profiles. The listening fixture held 126 catalog tracks, six likes, five ratings and two playlists. The display ran at 144 DPI (150% scaling).

## Workspace and navigation

| Workflow | Observed result |
| --- | --- |
| Ordinary listening | All Music, Liked Tracks, a registered collection and Techno reused the same browser ID. The address field remained in the title bar. [Default workspace](evidence/unified-shell/default.webp) · [Library detail](evidence/unified-shell/library.webp) |
| Physical and virtual paths | Ctrl+L selected the physical address. A quoted path opened the folder. An invalid path and Escape preserved the destination. Up, Back and Forward visited the expected folders. Ctrl+L in Techno opened an empty editor. [Folder](evidence/unified-shell/folder.webp) · [Playlist](evidence/unified-shell/playlist.webp) |
| Independent browsers | Clicking each split table immediately updated the accessible title-bar breadcrumb. Each browser kept its own filter and history through Back, hiding the filter, and restart. An expanded filter reopened with its text. [Split browsers](evidence/unified-shell/browsers.webp) · [Single browser with filter](evidence/unified-shell/filter.webp) |
| Music organization | Three selected tracks copied to Techno in order. Repeating one copy produced a distinct playlist occurrence. Source entries, recording identities, source-file hashes and queue entries were preserved. [Native drag](evidence/unified-shell/transfer-held.webp) |
| Sidebars | Hiding a side widened the table. Restoring it recovered the measured width. Both stacked pairs hid together and reopened after restart with the same tab IDs and divider positions. [Stacked](evidence/unified-shell/stacked.webp) · [Both hidden](evidence/unified-shell/hidden.webp) |
| Window sizes | The shell retained its address, search action and sidebar controls at 720 × 460, 900 × 600, 1240 × 780 and 1540 × 940 logical pixels. The four sizes were also exercised with two browsers, vertical stacks and both sidebars hidden. [Narrow window](evidence/unified-shell/narrow.webp) |
| Mission 10 profile migration | An isolated copy of a saved two-browser workspace reopened with the same pane IDs, tab topology, active recipient, locations and histories. Catalog track, collection, playlist-entry, like and rating records matched before and after migration. |
| Windows title bar | Native hit testing identified the draggable caption and the interactive address and search controls. Windows moved, maximized, restored and minimized the window. |

## Search and scale

The overlay returned grouped title, artist, album and playlist matches. Opening a result selected content in the existing browser and preserved the queue. Explicit Play resolved the selected CUE source, sheet, chapter and frame range. Escape preserved the original browser location. [Search results](evidence/unified-shell/search.webp)

The existing six-figure fixture generated 1,011 CUE sheets with 99 chapters each. After indexing the 100,089 tracks into an on-disk SQLite database, the optimized test measured the following first pages. Each measurement covers one search on an already indexed catalog. UI input debounce adds 65 ms.

| Query | Returned rows | Worker query time |
| --- | ---: | ---: |
| Volume 777 | 6 | 79 ms |
| Track 99 | 5 | 37 ms |
| Track | 5 | 141 ms |

The optimized app opened the same fixture, displayed matching recordings and the album, and opened the next results page. A selective album lookup initially took 45 seconds in a debug run. Its query plan used the collection index for each disc. Directing the correlated lookups to the existing album index reduced the same debug query to 90 ms. [Native large-catalog results](evidence/unified-shell/large-search.webp)

Reproduce the scale measurement with the pinned Rust toolchain.

```powershell
cargo test --release --locked six_figure_collection_projection -- --ignored --nocapture
```

To reopen the measured catalog in the native player, set `DISQRO_SCALE_FIXTURES` to an existing test directory before running the test. It retains a new, isolated profile beneath that directory and prints its path. Launch the release player with `DISQRO_PROFILE_DIR` set to that path.

## Playback and coverage

During continuous muted stereo FLAC playback, the driver navigated, searched, resized and toggled both sidebars. The final interaction interval recorded 701 output callbacks, zero decode errors, zero device errors and zero unchanged-position ticks. These diagnostics cover callback and decoder behavior. Audible output and additional display scaling factors remain unverified.

The merged Mission 10 and Mission 11 release builds played the same 44.1 kHz stereo FLAC at zero volume, at 1240 × 780. Each run used a fresh profile, six seconds of warm-up and a 15-second process-CPU sample. The first four runs used Mission 10, 11, 11, 10. The repeat reversed that order.

| Build | CPU range (% of one logical core) | Mean CPU | Private memory at sample end | Output callbacks |
| --- | ---: | ---: | ---: | ---: |
| Mission 10 | 5.73–10.31% | 7.76% | 118.8–120.7 MiB | 6,000 |
| Mission 11 | 6.98–9.79% | 8.59% | 119.4–120.4 MiB | 5,907 |

All eight intervals recorded zero decode errors, device errors and unchanged-position ticks. CPU varied between runs on this host. [Per-run Windows counters](evidence/unified-shell/playback.csv)

The visual comparison used the written Wake design direction and existing Mission 10 screenshots. A separate Wake reference image was unavailable.

Formatting, warnings-denied all-target Clippy and the standard test suite passed. The optimized suite passed 117 tests including the encoded-format fixture. The six-figure fixture passed separately.

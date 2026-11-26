# Unified Music Browser qualification

The Music Browser keeps its breadcrumb, navigation history, local filter, and connected view tabs in a single row. The allocated pane width determines whether the row shows labels, icons, a view picker, or the panel menu. Library destinations precede the Collections and Playlists overviews and their entries.

Tracks and Releases use the saved folder scope. A registered collection initially includes its indexed descendants. A location initially shows its own folder. Tree loads a branch when its disclosure opens, and filtering keeps the ancestors of matches in open branches. Select a row and use Right or Left to expand, collapse, or move to its child or parent. Enter opens a destination or plays a track. The context menu can open a destination in another tab or split.

Favorites uses the binary heart. The Favorite column is available in track tables and automatically omitted in Favorites. Favorites starts with the browser's current track columns and saves its own column edits. Use **Remove from Favorites** on one row or a selection. Star ratings stay independent.

## Windows qualification

| Check | Result |
| --- | --- |
| Full locked Rust test suite | 234 passed, 13 ignored |
| Favorites | The Favorite header appears in Tracks and is absent from Favorites. Removing two selected favorites preserved ratings and the saved Tracks layout. |
| Six-figure indexed playback snapshot | 100,001 sorted recordings resolved |
| Formatting and Clippy | Passed with warnings denied |
| Silent native journey | The collection stayed selected through disclosure changes. Restart and Back/Forward restored the open branch. Playback continued through the change from Tree to Releases. |
| Windows split layouts | Five combinations of width and filter state. Both browsers saved their own view, destination, and filter. |
| Live Windows resize | Across 15 successively narrower widths, the Favorites breadcrumb folded its ancestor once and the active filter stayed applied. |
| Native navigation and columns | Both Library overviews opened. Album pinning and visibility survived restart and reset to their defaults. A drive root started in single-folder scope. |

The native fixture used a 20-chapter CUE with one FLAC image, a second disc with three audio files, and loose recordings. The physical branch check counted exactly 20 playable chapters from the FLAC image. The release, folder, and playback context checks also ran against isolated catalog and workspace files.

## Native captures

- [Library sidebar](evidence/unified-music-browser/library-sidebar.png)
- [Favorites](evidence/unified-music-browser/favorites.png) and [Favorite column in Tracks](evidence/unified-music-browser/favorite-header.png)
- [Releases](evidence/unified-music-browser/releases.png)
- [Expanded Tree](evidence/unified-music-browser/expanded-tree.png)
- [Tree restored after restart](evidence/unified-music-browser/restored-tree.png)
- [Wide toolbar](evidence/unified-music-browser/toolbar-wide.png), [medium](evidence/unified-music-browser/toolbar-medium.png), [narrow](evidence/unified-music-browser/toolbar-narrow.png), [narrow with an active filter](evidence/unified-music-browser/toolbar-filtered-narrow.png), and [extremely narrow](evidence/unified-music-browser/toolbar-extreme.png)
- [Favorites at 1,100](evidence/unified-music-browser/favorites-resize-1100.png), [1,065](evidence/unified-music-browser/favorites-resize-1065.png), and [1,030 logical window pixels](evidence/unified-music-browser/favorites-resize-1030.png), with the [full resize trace](evidence/unified-music-browser/live-resize.json)
- [Native journey result](evidence/unified-music-browser/result.json) and [navigation and columns](evidence/unified-music-browser/navigation-and-columns.json)

Build the manual-test executable with `./build.ps1 -Release`. For the silent journey, build with `cargo build --locked --release --features qualification` and compile the test fixture with `cargo test --locked --no-run`. Pass the release executable, Rust test executable, and a fresh output directory to `tools/qualifyMusicBrowser.py`. Set `DISQRO_BROWSER_FLAC_IMAGE` to a 45-second FLAC file to run the real-FLAC fixture.

The tested source revision is `e023cdc29d3343647dc3e0f8829ddcb1cd4102ba`. The precision baseline is `d62c37826dacaf82a15d436db3bb51b96fa2b6b3`, with Waveform Native pinned to `91ca08c4009a650c0c2281d76cfe4c5c65cff8a8`. Private dependency authentication remains an external distribution gate.

# Interaction hardening and native qualification

Disqro draft PR #42 stacks on #41, from `fix/interaction-hardening` into `feat/library-destinations`. The most consequential change is that visiting an expanded folder no longer makes its payload permanently resident. The same 40,960-folder workload peaked at 43,520 resident nodes on PR #41 and 17,408 here. [Draft PR](https://github.com/maikeleckelboom/disqro/pull/42)

## Baselines and custody

| Revision | Commit | Tree |
| --- | --- | --- |
| Disqro PR #41, integrated starting point | `4d5e6f3852fcc905c0429a1a49eef7c616520be1` | `6234f617b54b0e32d817a40aa9c67993724a7af2` |
| Disqro PR #34, earlier presentation baseline | `b938507308cae86579b4476dde81d73f16ff787e` | `309fb99bd32510d527d2b216b6b6b01b4a574756` |
| Waveform Native and Windows GPUI patch | `3d9a8f5149483b051df7f0219bf1cbaf4c4c2555` | `dcbf3d613e8bd837f1d34795c87462214cc302ad` |

The continuation has its own worktree. The existing worktrees, PRs, engine visibility, credentials, dependency revisions and personal data were preserved. Every automated player interaction used an isolated fixture profile and an undisplayed Win32 desktop, with the silent qualification build. The headless native replay consumes frozen measurement files. The production executable was built separately. The delivery record and PR description identify the final Git tree and executable digest.

## Architecture and regression coverage

| Concern | Owner and change | Evidence |
| --- | --- | --- |
| Expanded residency | [Tree](../src/browser/tree.rs) owns expansion markers and compact ancestry separately from node payload. Eviction records the logical span, drops the page and reconciles its unreachable child branches. At most four page requests run per Tree. | 40,960 expanded folders including empty and nested folders, 36,864 indexed releases with sampled two-disc child branches, backward reload, stale work rejection. |
| Ordered reveal | [Reader](../src/browser/tree/reader.rs) locates the target page on workers. [Reveal](../src/browser/tree/reveal.rs) installs ordinary sibling pages and cursor intervals. [Tree table](../src/browser/tree/table.rs) keeps a pending GPUI scroll request attached to the same item across projections. | Late-page target, immediate siblings and rank before and after additional pages. Physical CUE beyond the first page, catalog hierarchy, private-window delayed navigation and browser destruction. |
| Recursive intent | Tree persists a marker's intermediate ancestor chain. Physical folder membership includes its collection. Subtree commands can remove unloaded exceptions after a restart. | Indexed collection → release → disc exception, unrelated subtree, restart, recursive expansion, collapse and renewed paging. |
| Sort and filter | [Ordered result](../src/ordered_result.rs) builds a disposable, indexed SQLite rank table on a worker. Equal sort keys have deterministic tie breakers. Filtered Tree pages retain counts and reload payload by logical span. [Descendant search](../src/browser/tree/descendants.rs) streams all indexed matching folder ancestors into a disk set and annotates each incoming page on a worker. | 45,067-row broad filter, late identity lookup, forward and backward pages, collapsed matches beyond 4,096 folders, more than 256 CUE sheets and filenames sharing stems. |
| Artists and Releases | [Shelf paging](../src/collection/paging.rs) uses the same rank/identity index. A cancellable read-only catalog connection builds it outside the write mailbox. A release is ordered as a family and its members keep their authored order. The initial shelf contains at most three 256-row pages. | 40,960 distinct artists and 40,960 distinct releases. Four 256-row reads in both directions, with identity lookup at each position. Active CUE playback while opening both shelves and a physical Tree. |
| Personal and derived state | [Cache](../src/cache.rs) opens a versioned, disposable database with current table definitions. The old database is left in place. Waveform callers now use the engine directly for measurement and rendering, while Disqro owns its source adaptation. | Old database, recording identity, ratings, favorites, listening totals, queue and workspace preservation tested in temporary profiles. Corrupt measurement data rebuilds in the current cache. |
| Engineering policy | [AGENTS.md](../AGENTS.md) and [Windows CI](../.github/workflows/windows.yml) specify ownership, platform qualification, personal-data handling, language conventions, naming and review. | Locked formatting, naming, American English, comparator unit test and all-feature Clippy. Existing runtime dependency pins preserved. |

## Process memory and scale

Windows x64 debug test processes ran one fixture at a time. Working set is the Windows process peak observed during execution. Private memory is the highest sample at 25 ms intervals. Both include the test process, database connections and temporary allocations. Timing includes fixture setup and teardown unless the column names a particular operation. Other machine activity was uncontrolled.

| Fixture | Resident nodes / child and root branches at sampled peak | Root page descriptors | Peak working set | Sampled private | Wall time |
| --- | ---: | ---: | ---: | ---: | ---: |
| PR #41, 40,960 expanded folders | 43,520 / 40,961 | 10 | 95.23 MiB | 88.52 MiB | 3.05 s |
| New Tree, same 40,960 folders and nested children | 17,408 / 16,385 | 10 | 58.57 MiB | 47.97 MiB | 4.25 s |
| New Tree, 36,864 indexed releases | 18,432 / 3,073 | 9 | 79.80 MiB | 69.75 MiB | 1.54 s |
| Distinct artist and release shelves | — | — | 31.70 MiB | 19.33 MiB | 4.14 s |
| 100,001 indexed recordings | — | — | 55.23 MiB | 42.50 MiB | 5.74 s |
| 100,089 physical CUE recordings | — | — | 126.49 MiB | 112.66 MiB | 86.67 s |

The PR #41 comparison used an archived checkout with the same batched fixture injected for measurement. The new folder fixture also checks backward reload after the forward peak. One recursive intent covered each run. Both new Tree fixtures had zero outstanding requests at the reconciled samples and no persisted ancestry links for their root-wide expansion. The node count excludes objects reachable through a release's disc list, projected rows and in-flight buffers. Process memory includes them. These experiments do not establish a simultaneous four-request memory peak or individual allocator costs.

In the sampled shelf process, building ranks took 817 ms for artists and 2.063 s for releases. Four page reads plus four identity seeks took 11.1 and 25.5 ms respectively. The 100,001-recording fixture resolved its sorted snapshot in 933 ms. In the physical CUE fixture, discovery took 2.493 s, grouping 80 ms and the recording query 85 ms. Reproduce the fixtures with `cargo test --locked <test-name> -- --ignored --nocapture` for opt-in tests. The expanded folder and release fixtures run in the default suite. [Tree fixtures](../src/browser/tree/tests.rs) · [Catalog fixtures](../src/store/tests.rs) · [Physical discovery fixture](../src/discovery/tests.rs)

## Framework conformance

The versioned design and coding guides informed the review. The implemented contracts were checked against gpui-kit/gpui-base 0.10.0 and gpui-fast 0.1.17. In the pinned `elements/div.rs`, `tab_index` enables focusability. `tab_stop` alone does not. Explicit handles retain their own tab-stop setting. The pinned Kit tree wraps virtual items in a positional element. Disqro gives the owned controls stable domain IDs and tracks the focused handle across that wrapper. [Design guide](https://gpui-kit.com/versions/v0.10.0/docs/design-guides/) · [Coding guide](https://gpui-kit.com/versions/v0.10.0/docs/coding-guides/) · [Kit source](https://github.com/longbridge/gpui-kit)

| Surface or responsibility | Source review and design decision | Verified behavior and limits |
| --- | --- | --- |
| Library destinations, collection and playlist rows, section headers | [Navigation](../src/panels/nav.rs) supplies button role, name, selected state, keyboard focus, outline and semantic colors. The parent heading and its disclosure and add actions have distinct owners. | Windows UI Automation exposed all five destinations, individual fixture collection/playlist rows, both headings and their focusability. Native Tab/Shift+Tab and Enter/Space changed destinations. Windows did not expose SelectionItemPattern on these button roles, so selected state was verified at the GPUI property and visible highlight only. |
| Locations, Tree and disclosure | [Locations](../src/panels/locations.rs), [shared disclosure](../src/disclosure.rs) and [Tree table](../src/browser/tree/table.rs) use path/TreeId keys. Positional wrappers use independently tracked weak focus handles. The shared control sets AccessKit's disabled flag and has a dedicated keyboard context. | UI Automation reported the offline fixture as disabled and unfocusable, and the live disclosures as expandable. Native insertion, reorder, activation and removal were tested under positional row wrappers. Individual wrapper geometry and keyboard outcomes were sampled, without a screen-reader speech test. |
| Breadcrumb and artwork | [Breadcrumb](../src/location/breadcrumb.rs) owns menu focus, keyboard navigation, escape and restoration. [Player](../src/player.rs) gives artwork and transport distinct names and key contexts. The compact seek target retains its pointer affordance. | Browser journeys cover independent splits, actual recording reveal, cleared filter history, resize and restore. Exhaustive focus transitions for every artwork and breadcrumb permutation remain unmeasured. |
| Track tables, ratings and repeated occurrences | [Table](../src/table.rs) owns table/row-key mapping and click propagation. The favorite button exposes native toggled state. [Browser navigation](../src/browser/navigation.rs) carries a recording locator or an exact playlist occurrence and rejects superseded results. | Ratings survive Favorites removal. Delayed-result tests cover competing browser intents and closed browsers. The pinned Kit table retains its virtual row wrappers. |
| Materials and geometry | [Theme](../src/theme.rs), shared navigation dimensions and the 20-logical-pixel disclosure own common compact geometry. Table rows and panel gutters remain intentionally different from navigation rows. Native waveform sampling and retained texture extents use physical pixels. | Matched 144-DPI screenshots and named-control rectangles in normal, narrow and wide layouts. A 96-DPI compatibility process was measured separately. No native 96/192-DPI monitors were available for this run. |
| Entity lifetimes and rendering | [Workspace](../src/workspace.rs) owns pane event subscriptions. Locations owns its expansion subscription, availability task and per-panel observer. Browser/page jobs own cancellation tickets and publish through weak entities. [Waveform panels](../src/panels/waveform.rs) and the compact cursor share a frame pump. | The lifecycle test closes two detailed decks while playback continues, checks zero deck clients dispatched and zero deck paints, and verifies continued compact draws. No comprehensive allocator or listener-count soak was performed for every visualizer. |

## Native and build evidence

- The locked suite passed **259 tests** with **15 opt-in tests skipped**. Separate runs passed the 100,001-recording store, 100,089-recording physical catalog, 40,960-artist/release shelf, delayed browser result and native focus-identity tests. Formatting, spelling, naming, comparator regression, all-target/all-feature warnings-denied Clippy and locked Windows debug/release builds passed.
- **Native rendering passed 103/103**. All **206 PNG and geometry files** matched PR #41's SHA-256 values. The gate reads local frozen facts and replay parameters. A unit test independently rejects a changed image and missing geometry. The obsolete renderer host, replay adapters, rejected-renderer build and tolerance configuration were removed. [Reproduction](waveform-visual-recovery.md)
- The 144-DPI private desktop produced 1,842 × 1,161 client captures. All pixels matched PR #41 in the normal, narrow, wide and collapsed overviews. All 14 named control rectangles matched in each sidebar width. Native activation and a focus outline are visible in the Artists capture. [Before](evidence/interaction-hardening/before-144.png) · [After](evidence/interaction-hardening/after-144.png) · [Narrow](evidence/interaction-hardening/narrow-144.png) · [Wide](evidence/interaction-hardening/wide-144.png) · [Keyboard focus](evidence/interaction-hardening/artists-keyboard-144.png)
- With the same six-second fixture, device width and deck configuration, PR #41 and this build each produced **1,446 compact draws** while playing, zero while paused and zero while hidden. The final run paired 1,440 compact/detail frames. The largest cursor difference was 4.55 × 10⁻¹⁴ pixels. Other host workloads varied during qualification.
- The private catalog journey kept the CUE queue playing through 40,960 artist and release summaries and a Tree with 4,400 physical folders. Artist and release navigation took 1.50 s and 2.17 s including UI Automation. The ordinary continuation journey reached the end of 4,200 folders and 4,300 recordings. Split browsers, restored filters and listening-state preservation passed. The sidebar journey initially missed the double-click interval while launching a second UI Automation process between clicks. It passed twice after both clicks used the previously resolved point. [Catalog playback fixture](../tools/qualify_catalog_playback.py)
- Silent scrub hold, movement, cancellation and release, CUE bounds, four transport journeys and exact inspection passed. An injected device failure recovered while paused. The lifecycle trace initially counted the shared compact-frame request after the decks closed. On both PR #41 and this build the remaining client was the compact strip. The corrected assertion checks per-deck dispatch count and detailed paints, while requiring the visible compact strip to keep drawing. The final trace measured zero minimized frames, zero post-close deck dispatches or paints, and 163 continuing compact draws. [Frame ownership](../src/panels/waveform.rs) · [Measured evidence](evidence/interaction-hardening/measurements.json)
- The two-deck sweep reported **9,388,044 bytes** of peak resident plus reserved waveform evidence across 36 measured phases, 0 dropped trace records and 0 qualification errors. This figure measures the evidence arena, separate from total process memory.
- The production build used the locked release profile with default features on `x86_64-pc-windows-msvc`, rustc 1.99.1. Its PE machine is `0x8664`, size **50,582,016 bytes**, SHA-256 `3f982e2c4f7cb8f38dc2d15a1c224deee78de40df0c269d5c4c1cafe94c68189`. The separate qualification build is `8ed64f9af2b460019732e243596463a267e5b363f3744df92a7ae3385b81709c`.

The synthetic window-DPI transitions in the deck lifecycle harness exercise resource reconstruction. They do not establish monitor DPI or physical display cadence. Pointer-to-audio measurements end at the silent callback. Physical listening quality, presented-frame pacing and detailed waveform microjudder remain outside these measurements.

## Reproducing the scale checks

~~~powershell
cargo test --locked expanded_pages_release_empty_and_nested_branches_and_reload_backward -- --nocapture
cargo test --locked expanded_indexed_releases_reload_their_authored_discs_after_eviction -- --nocapture
cargo test --locked independent_large_artist_and_release_shelves_page_in_both_directions -- --ignored --nocapture
cargo test --locked six_figure_listening_snapshot -- --ignored --nocapture
python -B tools/qualify_catalog_playback.py <qualification-exe> <test-exe> <new-output-directory>
~~~

The native folder and catalog journey creates its own audio, database and workspace under the new output directory. The waveform replay uses the frozen local facts pack. [Native waveform qualification](waveform-visual-recovery.md)

## Remaining engineering limits

- Physical folder search covers open branches and labels its scope. Native filesystem enumeration does not provide an indexed ordering. Initial multi-page index construction and an unsorted seek may reread directory entries. Once a sorted snapshot exists, page movement uses indexed ranks. Construction latency on remote filesystems remains unmeasured.
- A released page retains one resume descriptor and its logical spans. Metadata therefore scales with visited pages. The resident node cap does not separately cap the byte size of a single authored CUE or release family. Shelf construction retains one family while assigning its ranks.
- An old workspace that omitted intermediate ancestry cannot supply the missing information on its own. New saves retain complete known ancestry. No personal data conversion ran. The inert serialized toolbar preference still round-trips because it belongs to saved workspace data.
- Sorting and reveal rely on the current source generation. Concurrent external filesystem changes at every gap between directory reads, arbitrary pathological depth and every combination of filter, sort and late prefetch were not exhaustively qualified. The model regression checks the target rank and siblings after prefetch. The native journey covers ordinary viewport restoration. A device-pixel scroll-offset assertion for every deep and evicted case remains open.
- Hosted Windows verification cannot fetch the pinned private platform dependency. Private repository access and distribution policy remain owner decisions.

## Filename and module inventory

The following inventory covers moved or removed first-party code and qualification tools relative to PR #41. Nested modules now resolve through Rust's ordinary module lookup. Persisted panel keys and serialized identifiers retain their original spelling.

<details>
<summary>Show the filename inventory</summary>

| PR #41 | This branch |
| --- | --- |
| `src/audioClock.rs` | `src/audio/clock.rs` |
| `src/audioScrub.rs` | `src/audio/scrub.rs` |
| `src/audioTests.rs` | `src/audio/tests.rs` |
| `src/browser/navigationTests.rs` | `src/browser/navigation/native_tests.rs` |
| `src/browser/treeTests.rs` | `src/browser/tree/tests.rs` |
| `src/discoveryTests.rs` | `src/discovery/tests.rs` |
| `src/levelMeter.rs` | `src/level_meter.rs` |
| `src/panels/fieldGeometry.rs` | `src/panels/instruments/plot/field_geometry.rs` |
| `src/panels/instrumentPlot.rs` | `src/panels/instruments/plot.rs` |
| `src/panels/spectrumPlot.rs` | `src/panels/spectrum/plot.rs` |
| `src/panels/waterfall.rs` | `src/panels/instruments/plot/waterfall.rs` |
| `src/panels/waveformInspection.rs` | `src/panels/waveform/inspection.rs` |
| `src/panels/waveformInspectionJourney.rs` | `src/panels/waveform/inspection_journey.rs` |
| `src/panels/waveformScrub.rs` | `src/panels/waveform/scrub.rs` |
| `src/panels/waveformZoom.rs` | `src/panels/waveform/zoom.rs` |
| `src/player_cursor.rs` | `src/player/cursor.rs` |
| `src/player_wave.rs` | `src/player/wave.rs` |
| `src/signalTests.rs` | `src/signal/tests.rs` |
| `src/spectralHistory.rs` | `src/spectral_history.rs` |
| `src/spectrumTests.rs` | `src/spectrum/tests.rs` |
| `src/spectrumWorker.rs` | `src/spectrum_worker.rs` |
| `src/stereoField.rs` | `src/stereo_field.rs` |
| `src/stereoFieldTests.rs` | `src/stereo_field/tests.rs` |
| `src/store/listeningTests.rs` | `src/store/listening/tests.rs` |
| `src/timeline_tests.rs` | `src/timeline/tests.rs` |
| `src/visualStreams.rs` | `src/spectrum_worker/visual_streams.rs` |
| `src/visualizationTests.rs` | `src/spectrum_worker/visualization_tests.rs` |
| `src/waveform.rs` | `src/waveform_source.rs` |
| `src/waveformBaselineTests.rs` | `src/analysis/baseline_tests.rs` |
| `src/waveformDetail.rs` | `src/waveform_detail.rs` |
| `src/waveformPortAnalysis.rs` | Removed |
| `src/waveformPortQualification.rs` | `src/waveform_qualification.rs` |
| `src/waveformRender.rs` | Removed |
| `tools/analyzeWaveformForeground.py` | `tools/analyze_waveform_foreground.py` |
| `tools/buildFieldReference.py` | `tools/build_field_reference.py` |
| `tools/captureFieldReference.py` | `tools/capture_field_reference.py` |
| `tools/captureInterface.py` | `tools/capture_interface.py` |
| `tools/captureTimelineBoundary.py` | `tools/capture_timeline_boundary.py` |
| `tools/captureWaveformForeground.py` | `tools/capture_waveform_foreground.py` |
| `tools/copyLibraryToCurrentSchema.py` | `tools/copy_library_to_current_schema.py` |
| `tools/privateDesktop.py` | `tools/private_desktop.py` |
| `tools/qualifyCompactPlayhead.py` | `tools/qualify_compact_playhead.py` |
| `tools/qualifyLibrarySidebar.py` | `tools/qualify_library_sidebar.py` |
| `tools/qualifyMusicBrowser.py` | `tools/qualify_music_browser.py` |
| `tools/qualifySpectrum.py` | `tools/qualify_spectrum.py` |
| `tools/qualifyTreeContinuation.py` | `tools/qualify_tree_continuation.py` |
| `tools/qualifyWaveform.py` | `tools/qualify_waveform.py` |
| `tools/qualifyWaveformInspection.py` | `tools/qualify_waveform_inspection.py` |
| `tools/qualifyWaveformLoading.py` | `tools/qualify_waveform_loading.py` |
| `tools/qualifyWaveformMotion.py` | `tools/qualify_waveform_motion.py` |
| `tools/qualifyWaveformParity.py` | `tools/qualify_waveform_parity.py (native-only rewrite)` |
| `tools/qualifyWaveformScrub.py` | `tools/qualify_waveform_scrub.py` |
| `tools/qualifyWaveformTransport.py` | `tools/qualify_waveform_transport.py` |
| `tools/waveformControl.py` | `tools/waveform_control.py` |
| `tools/waveformParity/electronHost.cts` | Removed |
| `tools/waveformParity/historicalAnalysis.rs` | Removed |
| `tools/waveformParity/historicalOctave.rs` | Removed |
| `tools/waveformParity/legacyRenderer.ts` | Removed |
| `tools/waveformParity/referenceAnalysis.rs` | Removed |
| `tools/waveformParity/rejected.rs` | Removed |
| `tools/waveformParity/renderer.ts` | Removed |
| `tools/waveformParity/tolerances.json` | Removed |

</details>

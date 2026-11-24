# Production consolidation review

## Source inventory

Verified against the remote on November 24, 2026. All five open PRs had a successful Windows verification job and zero submitted reviews. PR #13 was already merged. The candidate starts from the existing production history.

| Source | Branch | Original tip | Git tree | Merge base with production |
| --- | --- | --- | --- | --- |
| Production / #13 | `main` | `44703121b977b1059170d4a2b98ae110f99dda41` | `09cad16b55ebd81350cd8963582a7e88b5f11f56` | `44703121b977b1059170d4a2b98ae110f99dda41` |
| #13 | `codex/playback-signal-monitor` | `44703121b977b1059170d4a2b98ae110f99dda41` | `09cad16b55ebd81350cd8963582a7e88b5f11f56` | `44703121b977b1059170d4a2b98ae110f99dda41` |
| #11 | `feat/unified-navigation-calm-workspace` | `3aa9ab197537d9e02b84e6df09108f72df03d1c2` | `c58810145d970b23e6061ca4f0327738dd5ccb7b` | `89da41d99bed159af28f793ac41f1058079c0631` |
| #12 | `feat/coherent-workspace-design` | `37232cab9432eed7578cb704cc54cd1200bb61c4` | `7f0084621d87df0cc691918b8bdc4a94859f191d` | `89da41d99bed159af28f793ac41f1058079c0631` |
| #14 | `codex/native-spectrum` | `8f94e23860de36f87ee38c4ebdc444ffa87ca123` | `67ea29c05c14b22ea222bd32cd9098bd9cc24cbd` | `44703121b977b1059170d4a2b98ae110f99dda41` |
| #15 | `codex/advanced-audio-visualization` | `82c8860f134864f29e64040220e6c2792dea55a0` | `cbbebdc588a0ec60301b3239a8b5d9c9bdfaaab2` | `44703121b977b1059170d4a2b98ae110f99dda41` |
| #16 | `codex/browser-navigation` | `3e382210f64dbfa037923436aef19d2e47c03092` | `30004019e8ddc50a4bbeb7842c4a5f46ecd96d03` | `44703121b977b1059170d4a2b98ae110f99dda41` |

PR #12 contains #11 and PR #15 contains #14. Each stack's internal merge base is its lower PR tip. The two audio/browser lines meet at production. The older workspace line meets both at the earlier modular-workspace commit shown in the table. The detached scratch worktree at `b2182d006df12d2ef7117ffbc80a19acbdaa3ed6` (tree `2d887d92677be3bf79511d35839cc7544f9ca705`) is an ancestor of production. All five pre-existing worktrees were clean. The desktop chat inventory showed this consolidation as the only active Disqro chat. The main, browser and audio worktrees also contain ignored local material, including native captures, fixtures and qualification scripts. Before making the candidate, a verified Git bundle archived all seven branch refs and their complete ancestry. The candidate has its own worktree.

## Integration decisions

| Area | Retained or adapted | Rejected or superseded |
| --- | --- | --- |
| Audio, #14 and #15 | The first integration commit has the exact tree of #15. One demand-counted stereo capture feeds the calibrated Spectrum, time histories and physical-pixel Stereo Field. Separate instances own their settings. The callback writes fixed-size blocks to a bounded ring. Gaps reset analysis. RMS and sample peaks retain their original observer. The renderer tests, silent harness and source screenshots travel with the code. | The intermediate fixed-resolution field renderers remain available in the source bundle and comparative evidence. Only the final renderer ships. |
| Navigation, #16 | The three files shared with audio composed cleanly. Browser entities own history, scroll/selection bookmarks, exact chapter targets and ordered traversal. The store's album structure and the physical folder traversal resolve CD and release neighbors. Explicit single-track playback, continue-CD and reveal preserve queue semantics. | The older restore, address-history and shelf-ordering implementations would replace newer behavior. The candidate uses the same #16 route transition for indexed track search as for an exact reveal. |
| Search and tables, #11 and #12 | The transactional FTS migration builds indexes for existing rows and maintains them through edits. Ctrl+K opens ranked cross-library results. Ctrl+F stays in its browser. Track tables share width between text fields, keep configurable pins and group cover art with the disc. The Favorite glyph takes a compact default. Existing custom widths, hidden fields, order and pins survive migration. | The older album table projection and alternate shelf delegate omit #16's directory rows and change which authored disc can be selected. The production shelf keeps its ordered release and child disc rows, directory links and viewport restoration. |
| Browser header and panels | One header combines the #16 address and ancestor path, local history and sequence buttons with the #12 view and filter controls. The panel menu contains the page and instance actions. The global Workspace menu creates all registered instruments. Tabs appear for shared groups and during customization. A single workspace pass sends presentation changes for active tabs in visible groups. Full-page search suspends their demand until the dock returns. | Abandoned navigation rails, repeated toolbars, the second breadcrumb algorithm, and the older global history controls are omitted. Library and playlist directory entry points stay as in #16. Historical design galleries are linked below. |
| Player and queue | The centered transport, timeline lane, source-aware waveform labels and queue button coexist with the stereo meter and #16's Now Playing actions. Queue clicks reach the workspace even after a panel menu closes. A docked Queue sharing a group with other types can be revealed or closed individually. | The old player layout and its redundant queue controls are omitted. The live source, CUE timing, meter subscription and analyzer services continue to use the audio stack. |

The source reports include calibrated inputs, measurements and prior qualification gaps. The 4K field reduced peak plot-paint time in the original comparison while increasing worker CPU and worst publication-to-paint latency. Two fields also increased UI-thread work. Bounds on ring, history and raster sizes limit storage. Whole-window redraws can still follow the fastest active instrument. Physical display scan-out and endpoint underrun counts require additional instrumentation.

## Candidate qualification on Windows

Formatting, Clippy with warnings denied on all targets, the regular locked suite (166 passed) and locked debug and release builds passed. All five opt-in tests passed. They exercised codec decoding and seek, transport and contiguous chapters, signal-observer cost, a 100,089-track library and ten minutes of real-device playback with silent output. The three indexed searches on the scale fixture took 40–153 ms. The soak measured 14,710 non-silent internal reads and 14,354 spectra over 600 seconds.

The native player ran on a private Windows desktop with isolated profiles. The qualification flag silenced the downstream fader before opening the endpoint. The observer continued to receive full-amplitude PCM. The deterministic signal-path test checked zero output samples with positive internal peaks. The qualification players were closed after the runs.

| Native check | Observed outcome |
| --- | --- |
| Library, Locations, collections and playlists | Migrated a populated v2 catalog, found Disc 2 Chapter 30 through the global index, pinned Title, played the single chapter and revealed the exact CUE target in Locations. Created a playlist, traversed adjacent discs and releases, and went Back while the playback queue kept its identity. [Pinned table](evidence/production-consolidation/pinned-table.webp) · [Exact chapter](evidence/production-consolidation/cue-chapter.webp) |
| Workspace and transport | Split two browsers, added and tabbed instruments, toggled Queue from the player after customization, opened History and restored the layout after relaunch. Clicking the waveform sought to 130.44 s. [Restored workspace](evidence/production-consolidation/restored-workspace.webp) |
| Live instruments | Exercised Spectrum, Spectrogram, Waterfall and two independent Stereo Fields. Distinct response settings and statistics switches survived relaunch. The activity run switched waveform perspective 13 times during playback. [Running workspace](evidence/production-consolidation/live-workspace.webp) · [Waterfall](evidence/production-consolidation/waterfall.webp) · [Independent fields](evidence/production-consolidation/independent-fields.webp) |
| Demand and resizing | Capture stopped behind full-page search and returned to 48,000 frames/s on dismissal. Hidden tabs and minimization stopped capture and analysis. Combined instruments sustained 172 resizes over 15 s. No diagnostic phase recorded capture drops or device/decode errors. [Full-page search](evidence/production-consolidation/full-search.webp) · [Search trace](evidence/production-consolidation/search-capture.csv) · [Scenario measurements](evidence/production-consolidation/native-scenarios.csv) |
| Existing profiles | Opened isolated copies of both a production v2 profile and a #12 v3 profile, then compared saved pane IDs and order, queue entry IDs, root identities and personal tables. The production catalog upgraded to v3. The #12 copy kept its three playlists, entries, ratings and likes. Routine scan generation and timestamps can advance. The older experimental shelf's column preferences have no counterpart on the retained #16 directory shelf. |

Screenshots were inspected from the running application at 144 DPI (150%). The 3840 × 2160 field run used a native window on that display. It measured 83.19% of one core on the UI thread, 5.23% on the worker, and 299.82 MiB peak private memory with two fields. This single laptop run demonstrates the cost of that configuration. Physical monitor transitions, scan-out latency, hardware underrun counters and audible listening remain unqualified.

## Original qualification evidence

- PR #11. [Connected workspace](https://github.com/maikeleckelboom/disqro/blob/3aa9ab197537d9e02b84e6df09108f72df03d1c2/docs/mission-11-connected-workspace.md)
- PR #12. [Workspace comparisons](https://github.com/maikeleckelboom/disqro/blob/37232cab9432eed7578cb704cc54cd1200bb61c4/docs/coherent-workspace.md)
- PR #14. [Spectrum calibration and Windows runs](https://github.com/maikeleckelboom/disqro/blob/8f94e23860de36f87ee38c4ebdc444ffa87ca123/docs/spectrum-qualification.md)
- PR #15. [Live instrument qualification](https://github.com/maikeleckelboom/disqro/blob/82c8860f134864f29e64040220e6c2792dea55a0/docs/visualization-qualification.md)
- PR #15. [Final physical-pixel Stereo Field comparisons](https://github.com/maikeleckelboom/disqro/blob/82c8860f134864f29e64040220e6c2792dea55a0/docs/stereo-field-rendering.md)
- PR #16. [Browser navigation and exact CUE reveals](https://github.com/maikeleckelboom/disqro/blob/3e382210f64dbfa037923436aef19d2e47c03092/docs/cd-navigation.md)

## Production approval and cleanup

Record the candidate commit, tree and base in the candidate PR. Approval applies to that tree. Re-fetch both refs immediately before promotion. If the base or candidate changed, refresh the review. Fast-forward production to the approved linear candidate through the repository's permitted workflow, then verify the production tree and its own Windows CI.

After production verification, close #15 before #14 and #12 before #11. Close #16 against the navigation accounting above. #13 is already merged. Replace branch-based URLs in the historical PR descriptions with their verified source-tip URLs before removing their branches. The original PR heads and full history are recoverable from the bundle. Recheck every checkout and its ignored files, consult the owner of the detached scratch checkout, and keep worktrees containing unique local evidence. The primary browser checkout must be moved to an appropriate retained branch before removing its local branch.

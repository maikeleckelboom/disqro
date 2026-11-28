# Interface foundations engineering review

**November 28, 2026 · Windows x64 · Hold integration.** The corrected browser and dock journeys pass. A supported dock layout can still make browser navigation controls unreachable with the pointer. The next development mission should keep the current location identifiable and the controls usable through a divider drag and a restart in a narrow split.

## Recovered state

The feature worktree had 13 implementation commits and four review corrections above the common base. Its only uncommitted change corrected the Releases destination in the Music Browser journey. The last optimized qualification executable preceded the Python capture-helper commit. Both interrupted journeys had written results by recovery. No qualification worker was still running. The main checkout had an unrelated Python cache, the Waveform Mission 4 branch had its own worktree, and a separate production player was running.

## Native findings

| Finding | Evidence and disposition |
| --- | --- |
| Music Browser reveal | The first run selected Releases **view** inside the fixture collection. Its saved route correctly remained the collection, which failed the test’s destination assertion. Clicking Releases in the Library sidebar satisfied the original route and selected-release assertions. The recovered second run also verified Tree expansion, Back/Forward, restart, independent splits, Favorites removal with ratings preserved, five width cases and 15 live-resize steps. [Completed browser checks](evidence/design-review/music-browser-checks.json) |
| Dock interactions | The first probe reported 16 of 19 checks true. It collapsed same-title browsers by name and sent the intended bottom drop into Kit’s horizontal edge zone. It also credited a reorder whose end state already existed. A replacement probe counts duplicate titles, checks distinct persisted IDs, requires an actual swap and drops at the measured horizontal center near the lower edge. A release four logical pixels from the divider missed the tab target. Releasing within the first quarter of the tab verified reordering. |
| Queue restoration | Hiding Queue in a group shared with Listening Statistics and showing it again reversed the tab order. Reopening now inserts before the saved following instance when that instance remains open. Older records still locate their group through their saved neighbors. At 144 DPI all ten checks passed, including the two browsers, both toggles, reorder, bottom split, playback after the edits, and restart with the same split axes, IDs, active indices and visible geometry within two device pixels. [Dock results](evidence/design-review/dock-checks.json) · [Final split](evidence/design-review/restored-queue-split.png) |
| Keyboard and playback | Earlier private journeys recorded Enter on Filter, Escape to dismiss it, Space on View, Enter on the panel menu and Right Arrow from Queue to Listening Statistics. The original dock journey also recorded cross-group moves, moving the last panel from a group, the active-tab rule and an independent divider drag. Its focused-element query after a pointer click returned an empty name. Playback state and the analyzer image were observed at checkpoints. Sample-accurate audio continuity during every drag was outside these measurements. |
| Contrast | Faint text on the canvas’s selection and hover tints measured 3.66:1 and 4.47:1. Raising the shared faint-text token brought those to 4.71:1 and 5.75:1. A test checks relative luminance on all five plain surfaces and both browser row states. These ratios describe the specified backgrounds. [W3C contrast calculation](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html) |

The dock follow-up used the 26-track fixture, silent output and a 1440 × 900 logical window at 150%. The cropped image is 2142 × 1341 device pixels. One-second process samples from the corrected journey observed a peak working set of 279 MiB. This is a workload measurement from the development qualification build. The earlier interface journeys completed at native 144 DPI and simulated 96 DPI on the same display. Physical 96 DPI and 120 DPI remain unverified.

## Dock constraints and architecture

With a view picker, the fixed toolbar geometry takes 154 logical pixels. The minimum picker takes 78 and the current location requests 132. Below **364 px** the location gives up space. At 310 px only 78 px remain for the entire breadcrumb, including its disclosure controls. Below **232 px** even the fixed controls and minimum picker cannot fit. Right alignment clips Back first. The saved divider sequence shows the label disappearing and then the left controls clipping. [Native divider captures](evidence/design-review/toolbar-drag.webp)

In the pinned `gpui-base` 0.10.0, `resizable/panel.rs` accepts a per-slot size range and the drag solver respects it. `dock/dock_area.rs::render_node` creates split slots with the default 100 px minimum. Its panel and rendering interfaces expose no hook for propagating a content minimum. A minimum on the toolbar’s own element cannot constrain that parent slot.

Keep placement, tab order and resizing under Kit’s dock. The sizing mission should distinguish preferred starting widths from hard operating minimums. Resolve tab groups using their largest member minimum and nested splits using the sum along a split and the maximum across it, accounting for chrome. Decide what remains reachable when the window cannot contain the combined minimums. Acceptance must cover restoration from a larger display as well as live dragging. An upstream API or a dependency patch requires a separate decision.

The reviewed changes retain the shared Materials palette, browser-owned navigation state and existing virtualized Tree and tables. The saved allocation correction in `theme::tabular()` uses one lazily initialized feature list. The audio change in the original diff corrected a diagnostic string. The engine dependency and Windows presentation patch retained their pins. [Design specification](design-foundations.md)

## Navigation and capture evidence

Two saved optimized batches alternated baseline and candidate. Each fixture had 40,960 synthetic catalog tracks plus 26 seeded tracks and 4,400 folders. Of 28 launches, one candidate missed Pause and one baseline timed out in UI Automation. Pairing by batch, launch and action, and requiring an internal navigation sample in both builds, yielded 81 pairs. The median paired candidate-minus-baseline time to rows was −0.14 ms. Cold Artists was slower in this sample. Cold Releases and the third Artists visit were faster. No folder-open action yielded a matched pair.

| Time to rows, median ms | Pairs | Baseline | Candidate |
| --- | ---: | ---: | ---: |
| First Artists visit | 11 | 353 | 524 |
| First Releases visit | 12 | 1,058 | 1,027 |
| Tree view of the place | 11 | 3 | 2 |
| Second / third Artists visit | 11 / 12 | 406 / 519 | 486 / 446 |
| Second / third Releases visit | 12 / 12 | 923 / 943 | 901 / 964 |

The slowest matched observations were 2.13 s baseline and 2.40 s candidate. The largest foreground timer lateness among completed launches was 266 ms baseline and 135 ms candidate. These measurements support no systematic regression in the sampled routes. They do not establish equivalent latency on each route. The separate catalog journey retained playback and reached Artists, Releases and Tree.

The capture archive contains ten launches at each burst scale. Every launch reported playing and exited normally. In one native five-frame resize burst, the grayscale edge scores were 14.31, 4.96, 14.50, 14.40 and 14.50. The second capture lost most dock content while nearby samples contained it. The matching frame and DXGI traces establish renderer activity. The private desktop cannot independently show which image reached a physical display. PrintWindow interacting with composed surfaces is a supported explanation, and the original session-long symptom remains unproven. Diagnostics now save a single raw sample by default. Static workspace references can explicitly select the highest-detail of three. [Full and partial native captures](evidence/design-review/capture-comparison.png)

## Validation and integration decision

- The historical aggregate had **257 passed, one timed out, 18 ignored**. The existing audio drain test hit its three-second deadline. Four exact runs on the same binary passed in 0.95–0.97 s of test time. Its implementation predates this branch. The cause of that particular timeout remains undetermined.
- The original 256-to-258 increase is consistent with the source diff. Two toolbar tests for superseded folding behavior were replaced by two stable-composition tests. Two tab visibility and rename tests were added. Recovery added the contrast test.
- On the corrected Rust source, the focused contrast and profile round-trip tests passed. **`cargo test --locked` passed 259 with 18 ignored.** Formatting, all-target all-feature warnings-denied Clippy, source naming and American English checks passed. The capture helper also saved the expected pixels for raw and three-sample modes against the archived burst.
- The recovery build used the locked development profile with the qualification feature. Optimized performance and production build evidence belongs to the earlier revisions. The current branch still needs its production build and milestone checks when the toolbar issue is closed. Hosted CI, integration and release are separate from this engineering review.

Defer the later Tree presentation, Tracks and Releases refinements, player and analyzer design work to focused missions under the specification. The [handoff](design-review-handoff.md) identifies the approved branch, current checkpoint, raw evidence and next action.

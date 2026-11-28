# Interface foundations handoff

**Branch** `feat/design-foundations` in `C:\dev\disqro\.local\design-foundations`. **Reviewed revision** `30df98f`. The following documentation commit contains the report and evidence. Resolve its exact ID with `git log -1 --format=%H -- docs/design-review-handoff.md`. The delivery record in the local evidence directory also identifies the branch tip and tree.

**Objective and decision.** Recovery and engineering review are complete. Hold integration until pointer navigation and the current location remain usable in a constrained browser. The design specification and the mission types in the repository instructions guide later sessions. [Review](design-foundations-review.md) · [Product direction](design-foundations.md) · [Workflow and architectural invariants](../AGENTS.md)

**Completed.** Recovered all 17 commits and the pending Releases selector correction. Verified the completed browser journey. Fixed mixed-group Queue restoration and faint row text, reproduced tab reordering and bottom splitting, and distinguished raw diagnostic screenshots from selected reference images. The corrected dock journey passed all ten checks. The locked Rust suite passed 259 tests with 18 ignored. Formatting, Clippy, source naming and spelling passed.

**Evidence.** Portable checks and images are under [docs/evidence/design-review](evidence/design-review/). Raw local evidence is in `C:\dev\disqro\.local\stabilization`.

| Question | Local evidence |
| --- | --- |
| Browser reveal, resize, catalog | `qual/music-browser/qualification-error.log`, `qual/music-browser-2/`, `qual/catalog/`, `qual/interface-{100,native}/` |
| Dock behavior and memory | `dock-1/`, `recovery-dock-before/`, `recovery-dock-after/`, `recovery-reorder-probe/`, `recovery-dock-reviewed/`, `recovery-dock-memory.csv` |
| Tests and builds | `wt-tests.log`, `recovery-audio-check.json`, `recovery-locked-tests.log`, `recovery-clippy.log`, `head-qual.log`, `recovery-qual-build.log` |
| Navigation, rendering and constraints | `perf-{a,b}/results.json` and their per-launch traces, `render-burst-{100,native}/`, `dock-constraints.md` |
| Original visual review, keyboard and divider journey | Under `%LOCALAPPDATA%\Temp\claude\8bd1a44b42ac05d0\e9d0aff9-8fe5-43be-9db7-ca76f559af98\scratchpad`, see `gate2/index.html` and `cap/final/` |

**Known failures and gaps.** The toolbar can lose its breadcrumb below 364 logical pixels and clip controls below 232. The original audio timeout passed focused repetition and the final suite. The intermittent capture discrepancy is documented. Physical 100% and 125% scaling, a current production build and hosted integration checks remain open.

**Exact next action.** Start a Development mission to keep a narrow Music Browser operable. Use the saved divider sequence and the locked dock source to specify what happens when a restored split cannot fit its panels. Agree on that behavior before changing a toolkit dependency. Validate one browser beside a sibling through dragging, shrinking and restart. Defer Tree, player and analyzer redesign until their own missions.

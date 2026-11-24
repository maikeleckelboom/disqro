# Disqro native UX audit

The audit compared the verified production baseline from PR #19 with the candidate on Windows 11 Pro build 26300. The generated library included loose WAVs, duplicate playlist occurrences, nested folders, and a two-disc release with separate CUE images. The larger fixture contained 1,032 indexed recordings. Each playback run used the qualification build, a private Windows desktop, and a separate profile. The qualification graph muted the main output before WASAPI initialization and left the PCM observer upstream of that fader.

## Findings register

### P1 · Duplicate playlist occurrences

**Reproduction** Create a playlist ordered A, B, A, D. Double-click the second A, select B once, then click the Now Playing artwork. Repeat after closing and reopening the app.

**Expected and actual** The second A should own the playing indicator and be the row selected on reveal. Production marked both A rows and revealed the first one. The queue was already starting at the second A, so the visual feedback disagreed with the transport.

**Root cause and correction** Queue construction retained the recording after resolving the playlist row but dropped the occurrence ID. The queue now stores both the playlist and entry IDs for playlist activations and additions. The browser uses those IDs for its marker and reveal, including **Show in Playlist** from Queue and History. Revealing a filtered-out row uses the browser's existing filter recovery and Back behavior. If the source occurrence has been removed, the action reports that and the recording can still be located through Library or Locations.

**Verification** The native playback selection and marker agreed on entry 3. After a paused restart, the artwork and menu both revealed entry 3 again. Deterministic coverage checks distinct IDs for the same recording in a sorted playlist, preservation through queue edits and serialization, and recording matching after restoration. [Production reveal](evidence/ux-audit/playlist-before.webp) · [Corrected reveal](evidence/ux-audit/playlist-after.webp) · [Restored reveal](evidence/ux-audit/playlist-restored.webp)

### P1 · Queue reorder can cross the playing track

**Reproduction** Start the folder sequence A, B, C, D and advance to B. Select the current or first upcoming row and try to move it across the playback boundary with a drag or Alt+Arrow.

**Expected and actual** Reordering upcoming tracks should leave the current item and its predecessors in place. In production, the shared move operation accepted positions on either side of the current entry. A queue reorder could send upcoming recordings to History without playing them.

**Root cause and correction** Keyboard and drag handlers sent indices in the combined History and Queue list. The transport now checks that both the moving entry and its destination belong to the upcoming list. Drag targets cannot use the current row. Drop destinations use queue IDs, so an intervening advance cannot repurpose an old row index.

**Verification** The candidate kept B current and retained C and D after attempts at the boundary. The queue test also checks a valid upcoming reorder and rejects an attempt to move a History item with the reorder command.

### P1 · Removing the final current entry brings History back into Queue

**Reproduction** Play A and advance to B. Run **Clear Up Next** in Queue. Remove B with **Remove from Queue**.

**Expected and actual** Queue should be empty and History should still show A. Production cleared the current position and displayed A as upcoming again, available to play from the transport.

**Root cause and correction** With no current entry, the queue treated all saved entries as upcoming. It now preserves the boundary when the current entry is removed and stores that boundary in the same snapshot.

**Verification** Queue remained empty after removal and A remained in History across restart. The regression test also covers scheduling A next and then clearing its historical occurrence. [Production Queue](evidence/ux-audit/last-removal-before.webp) · [Corrected Queue](evidence/ux-audit/last-removal-after.webp)

### P2 · Play Next removes the source row from History

**Reproduction** Play A, advance to B, open History and choose **Play Next** on A.

**Expected and actual** A should go next and remain findable in History. Production moved the existing entry to Queue and removed its History row.

**Root cause and correction** The action called the same move operation as drag reorder. The transport now copies a History occurrence into an independent queue entry and promotes an existing upcoming entry in place. Its playlist identity and CUE context survive the copy. An explicit promotion retains priority when Shuffle is switched off.

**Verification** Production changed A, B, C, D to B, A, C, D. The candidate kept A in History and queued a separate A after B. [Production History](evidence/ux-audit/history-before.webp) · [Corrected History](evidence/ux-audit/history-after.webp)

### P2 · Listening context disappears when browsing elsewhere

**Reproduction** Start from the middle of Liked Tracks, open a collection, then inspect Queue or the Now Playing actions.

**Expected and actual** The listener should be able to check which list started the sequence. Production preserved the queue order, but neither menu stated where the sequence started.

**Root cause and correction** The persistent queue only stored the entries and transport state. The existing menus can now show the starting playlist, Liked Tracks, a registered collection, All Music or the global search query. The wording describes the starting list even if the listener manually adds tracks later. The search result play button's tooltip also describes starting from search results.

**Verification** Starting playback from Liked Tracks and opening the collection preserved the three-entry snapshot. The source appeared in the candidate menu. [Now Playing source](evidence/ux-audit/listening-source.webp)

### P2 · Queue and History rows are hard to operate with the keyboard

**Reproduction** Click a Queue row and try to reach an adjacent row with Down before using Enter. Inspect the focused list in Windows UI Automation.

**Expected and actual** The keyboard should be able to select another entry without a pointer click. Production required a pointer selection and did not expose the virtual rows as named list options.

**Root cause and correction** Queue handled activation, deletion and reorder shortcuts, but lacked selection movement and accessible row state. Up and Down now move the selection in the displayed order and scroll it into view. List options expose titles and identify the current entry's playback state.

**Verification** On the candidate, Down then Enter selected and played C from the native Queue. Windows UI Automation exposed the row names, including the paused state after restoration.

### P2 · Debug builds panic while laying out selected tabs

**Reproduction** Build with `cargo build --locked --features qualification`. Launch a fresh profile on a private desktop with `--qualify-silent` and show the collection view tabs. The selected waveform perspective used the same styling pattern.

**Expected and actual** The selected tab should render and respond to input in debug. Production exited with `hover style already set` at `gpui-pre 0.3.12`, `elements/div.rs:867`. The full backtrace ends in `gpui_base::button::Button::render` when it applies the second hover style.

**Root cause and correction** Disqro registered a direct hover style on a GPUI Kit button whose renderer already owns hover state. The selected controls now use the built-in selection state and their existing background.

**Verification** The debug candidate opened the same private-desktop fixture and activated the Spectrum tab. The release browser and waveform perspective controls rendered in the multi-disc journey.

## Windows qualification

| Journey | Native checks |
| --- | --- |
| Find and play | Started with an empty generated folder, opened the fixture, indexed it using the browser menu, started playback from global search, Liked Tracks and the second duplicate playlist entry. |
| Browse a collection | Used Albums / CDs and the local filter to find the release, opened CD 1, navigated to the next CD and back, then went up to the release. Browsing left the playback sequence alone. |
| Manage the sequence | Used Play Next, Add to Queue, History replay, shuffle and repeat, single-track playback, a missing-file skip, Queue keyboard selection, a boundary reorder, Clear Up Next and final-current removal. Restarted with a History-only snapshot and with a duplicate playlist occurrence. |
| Organize music | Liked and unliked an indexed recording, applied and changed its rating, created and renamed a playlist, dragged the same recording in twice, removed one occurrence and deleted the playlist. The indexed recording and its rating survived. |
| Customize the workspace | Opened a second browser and two independent Spectrum panels, hid and restored the sidebar, resized to 720 × 460 logical pixels, and reopened the saved arrangement. Also exercised same-group and cross-group reordering, a split drop and the overflow switcher. |
| Control playback | Played an individual CD, started the authored release sequence, stepped across its CD boundary, switched between Track and Full CD, sought through the waveform, and toggled mute. |

The collection indexed in approximately 0.94–1.11 s on this laptop. The title search action became observable in 0.56–0.69 s, including UI Automation polling. The release-mode catalog fixture resolved 100,001 ordered recordings in about 139 ms. That measurement starts after key generation and ends before opening an audio source. These single-machine observations were sufficient to exercise the large-list paths, with deeper rendering and indexing work left outside the audit.

The final large-fixture run recorded 7,029 audio callbacks with zero device or decode errors. The deterministic silence check verified normal-amplitude captured PCM and zero samples after the main fader.

`cargo fmt --all -- --check`, warnings-denied Clippy for all targets with `qualification`, locked Windows builds, and the deterministic suite were run. The suite passed 178 tests with eight opt-in tests skipped. The six-figure listening snapshot was run separately.

## Remaining work and limits

- History still represents the prefix of the current listening sequence. Starting at a later album or playlist row can therefore show earlier rows that were never heard. A listening log would need an explicit policy for Previous, replay, and retention across queue replacements.
- The source label describes the sequence's starting point. It uses the name at activation, and it does not infer an origin for older snapshots, manually assembled queues or physical folders.
- Silent qualification suppresses the Windows media-session integration. System media buttons and the tray were outside this native run. The deterministic command-order tests still exercised the transport path they share.
- Playback and organization ran against generated fixtures. One initial idle launch followed the app's Windows Music fallback. Subsequent testing used the isolated fixture paths. Real collection metadata and physical audio output were outside this audit.

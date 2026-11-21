# Browsing a music collection

Open the collection root from Locations, the address bar or the folder picker, then choose **Include Subfolders**. A folder that holds only subfolders offers it in the list; the **Subfolders** button in the Music Browser toolbar always offers it. Disqro remembers the choice for that folder and every folder inside it, and starts showing discs and tracks as it finds them. Its parent and neighbouring folders keep showing only their own recordings. Choosing it again inside a collection turns it off for that branch alone.

- **Albums / CDs** shows the artwork, title, year, track count, duration, format and relative folder for each disc. Double-click a disc or press Enter to see its tracks. The button above the table returns to the discs.
- **All Tracks** groups the customizable table by CD. Double-click a CD header to open its page. Click a column heading to sort across the collection. Ctrl+F searches titles, artists, albums and paths. Combine terms such as `artist:gizmo format:flac` or `album:thunderdome`.
- **Queue a selection** by Ctrl-clicking tracks or Shift-clicking a range, then choosing Play Next or Add to Queue from the selection’s context menu. Entries follow the displayed order. Open Containing Folder selects the same chapter in its original folder. Browsing, searching and refreshing leave the playing queue independent.

![Albums and discs at the Thunderdome collection root](evidence/thunderdome-discs.png)

## What counts as a disc

A CUE program owns its authored track sequence and source-frame boundaries. For ordinary tagged files, the grouping uses the containing folder, album and disc number. Untagged files fall back to the folder. Album names alone never merge editions across folders. Disc folders named CD1 or Disc 2 supply a disc number when interpreting a sheet. The Albums / CDs list shows one row per disc.

The walker checks each referenced source on every visit. A missing file in a multi-file sheet takes only its own tracks out of the playable results. An equivalent available sheet shares a program when the entire sequence agrees on canonical physical sources, numbers, boundaries, titles, artists and albums. Covered audio images disappear from the flat list even when their sheets are discovered in another directory. Recorded lead-ins remain playable. A rescan can restore missing sources.

## Ownership and storage

- Two discovery workers allow a newer local request to proceed while a previous filesystem call is blocked. They share a single pending request slot. Each scan publishes through a bounded channel. Navigating closes the old receiver and invalidates its generation. An iterative walk skips linked directories and Windows reparse points. Network or filesystem calls already inside the operating system can take time to return.
- The browser publishes playable records before audio-header enrichment. It sends additions, replacements and removals. Filtered and sorted tables share immutable track allocations with queued entries. Virtualization limits rendering to the visible rows. Album artwork is requested on demand and reduced before upload to the smallest of a few thumbnail sizes, from 64 to 256 device pixels, that covers its drawn size.
- The versioned SQLite database under the profile’s `Cache` directory holds rebuildable data. Source-header records and parsed CUE programs are checked against file size and modification time. A transaction replaces an entire sheet and its indexed chapters. Folder enumeration and source availability remain authoritative. Browsing works when the cache is unavailable.
- Waveform overviews use the physical source and exact CUE range, file revision and cache-format version. Equivalent ranges can reuse an analysis across sheets. Adjacent disjoint chapters still have individual overviews. The player holds its current overview in memory.
- The profile uses atomic replacement, a process-level file lock and ordered background commits. Quit obtains a queue snapshot after pending transport commands, then waits for the final commit. Recovery preserves corrupt bytes and reports their location. A second process uses a named session file if it cannot acquire the primary workspace.
- The transport keeps FIFO ordering across queue edits and control actions. Adjacent seeks or volume updates coalesce in their pending slot. A seek targets the queue entry displayed when issued. The 64-slot queue returns an error on saturation and the UI can report it. Zero volume uses Kira’s exact silence value.

## Windows qualification, 19 November 2026

The optimized x64 application ran on Windows 11 Pro build 26300, Ryzen 7 8845HS, at 150% scaling. The local Thunderdome archive contained 211 subdirectories, 108 FLAC images totalling 60.86 GiB and 214 CUE sheets. An independent count found 108 available programs and 106 sheets referencing absent rips. The available programs specified **2,086 numbered tracks and 37 recorded lead-ins**. Disqro displayed all **2,123** in **108 discs**.

| Observation | Result |
| --- | ---: |
| First 20 tracks applied to the table, empty Disqro cache | 27.9 ms |
| First captured window with disc rows, from process launch | 391 ms |
| Full discovery and header enrichment, empty Disqro cache | 392 ms |
| Full refresh during playback | 89 ms |
| New process reopening the same root with the populated cache | 88 ms |
| Working set at discovery completion / process peak to that point | 104.2 / 106.4 MiB |
| Largest sampled window-message response during startup and discovery | 17.4 ms across 15 probes |
| Native table scroll on the playing collection | 16 distinct table captures in 16 scroll steps |

These are individual desktop runs. Windows filesystem caching remained enabled. The table and completion timers begin at the browse request. The first captured row includes process launch and capture overhead. Window-message probes measure UI-thread dispatch. Scroll samples measure changes in rendered content. Frame pacing and physical display latency need separate measurements.

From the root, opening *Thunderdome IV – The Original Dreamteam (CD1)* exposed 13 numbered tracks. Playing track 7, *Boomerang (Koala mix)*, selected DJ Buzz Fuzz, the disc artwork and its own waveform. While it played, a refresh returned 2,123 rows, switching off subfolders returned zero, and turning them back on restored the collection. Searching `artist:gizmo format:flac` found 25 tracks across discs. The table sorted those results by title. The player and queue remained on *Boomerang*. [CUE playback](evidence/thunderdome-cue-playback.png) · [Filtered tracks](evidence/thunderdome-filtered-tracks.png)

After a restart the original 13-entry queue returned paused at 7.452 seconds. Ctrl-selecting two tracks and using Add to Queue produced 15 saved entries. Open Containing Folder revealed the selected CUE chapter. A shell integration check removed only Disqro’s icon, delivered Windows’ registered TaskbarCreated message and verified the restored icon rectangle. A real Explorer process restart remains unqualified.

The Rust suite passed 33 default tests. Both opt-in tests passed separately. FFmpeg fixtures exercised seven encodings, including sample comparisons for the lossless cases. In an optimized synthetic walk, 1,011 fixture discs produced 100,089 logical tracks in 843 ms. Grouping the resulting records took 32 ms and a two-field filter took 22 ms. This fixture measures scanner and projection costs with placeholder audio files. The Windows UI and memory measurements above used real FLAC images.

To repeat the desktop timings, set `DISQRO_TRACE_DISCOVERY=1`, choose a fresh `DISQRO_PROFILE_DIR`, and launch the release executable on a collection root. The trace on standard error reports first table publication and scan completion. Reopen with the same profile to measure reuse. Run the opt-in checks individually with these commands.

```powershell
cargo test --release --locked six_figure_collection_projection -- --ignored --nocapture
cargo test --release --locked encoded_formats_open_decode_and_seek -- --ignored --nocapture
```

## Folder browsing, 20 November 2026

The same machine ran optimized x64 builds of the previous and the current browser with isolated profiles and muted playback. The listener's Music folder holds 10 recordings of its own, the Thunderdome archive, other albums, a DJ application's sample packs and another program's test recordings: 4,302 playable entries in all.

Opening Music used to walk every subfolder, because including subfolders was one switch for all folders and on by default. The list grew to 4,302 entries, sample packs and test recordings included. The tree showed Music among the home folder's other folders. Now Music lists its 10 recordings, and the tree starts at Music. An earlier profile keeps its Albums / CDs choice; the old global switch is not carried over.

| Observation | Previous browser | Current browser |
| --- | --- | --- |
| Music opened with an empty cache | 4,302 entries, complete in 1.05 s | 10 entries, complete in 74 ms |
| A row selected, then the list scrolled while Music with subfolders was discovered | The next batch scrolled back to the selected row | The list stayed where it was scrolled; only cell text filled in |
| Thunderdome as Albums / CDs, empty cache | Discs found later were sorted in above earlier ones for about 550 ms | Discs were added below in folder order; only times filled in |
| A folder opened from the tree | The tree was rebuilt around its parent; the row under the pointer changed | The tree stayed; the clicked row kept its place |
| Longest interface work per batch, Thunderdome as Albums / CDs | 2.0 ms | 1.1–1.3 ms |
| Longest interface work per batch, Music with subfolders as Albums / CDs | 3.5–4.0 ms | 2.4–3.4 ms |

Each figure comes from two or three runs, with Windows file caching enabled. `DISQRO_TRACE_DISCOVERY=1` now also reports the number of batches and the interface time spent applying them. Rebuilding the list for each batch takes well under a frame at these sizes, so the list is still projected in full rather than patched row by row.

Interaction checks, driven through window messages:

- **Tree:** the arrow expanded *Music Collection* without leaving Music. The name opened *VA – Thunderdome – CD Collection* in the same row, and a second click there changed nothing. Back, Forward and Parent moved between `# 1-22`, the archive and Music with the expanded branches kept. Parent from Music anchored the tree at `C:`, and choosing the Music Place anchored it at Music again. Four tree clicks within 80 ms ended on the last folder clicked, with breadcrumbs, highlight and list in agreement.
- **Typed paths:** a path inside a dot-folder opened its three recordings, and the tree showed the way to it.
- **Scope:** the archive offered **Include Subfolders** in its empty list and then showed 108 discs and 2,123 entries in natural folder order. `# 1-22` inherited the choice with 44 discs, and Music kept its own 10 recordings. In a fixture, an album with `CD1` and `CD2` showed two discs; its artist folder still offered the choice, and `CD1` listed its tracks.
- **Content:** a fixture with text files, a playlist file, `.git`, `.cache` and a folder with the hidden attribute listed only recordings and visible folders. A zero-byte WAV and a text file named `.mp3` were dimmed with a warning; an untagged WAV was not.
- **During discovery:** a search narrowed the list as it grew and restored all 4,302 entries when cleared. With Title sorted and the list scrolled, the top row stayed in place while about 3,000 more entries were sorted in around it. Albums / CDs and All Tracks switched while discovery continued. Choosing the `C:` Place cancelled the scan. Refresh, and Include Subfolders on Music, left the tree unchanged in 20 and 43 sampled frames.
- **Library:** the archive added to the library showed 108 discs and 2,123 entries in its collection. A like and a three-star rating on two CUE chapters, a new playlist and playback from the album all worked. Reveal in Folder opened the chapter's `CD1` folder with the chapter selected and both marks shown, while playback continued.

[Folder without recordings](evidence/folder-without-recordings.png) · [Folder as a collection](evidence/folder-as-collection.png) · [Reveal in Folder](evidence/reveal-in-folder.png) · [Unreadable recordings](evidence/unreadable-recordings.png)

## Release boundary

Playback qualification here used application volume zero. The observed transport continued advancing during discovery. Audible continuity, buffer underruns on a physical output, device-loss recovery, cross-file gapless playback, network-drive latency and sample-accurate AAC seeking remain open.

The browser still holds the selected collection’s track references in memory. A paged global library and verified relocation of moved sources are the next storage milestones. Persistent identities currently use paths and authored CUE segments. User-owned playlists and ratings need durable identities and an explicit relocation policy. Waveform table cells, shared multiresolution analysis across disjoint chapters and independent visualizer panels remain future work.

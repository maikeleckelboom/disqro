# Browsing and continuing a CD

**All Tracks** groups a recursive folder or indexed collection by disc. Double-click a section header, or select it and press Enter, to open the CD. Search keeps the headers of matching discs. Clicking a column heading sorts across the collection. The third click restores the grouped order. **Albums / CDs** remains the disc overview.

![The collection between two CDs](evidence/cd-sections.png)

## Listening controls

| Action | Result |
| --- | --- |
| Double-click a chapter, including a filtered or globally sorted result | Start at that chapter in the complete authored CD sequence. Previous reaches earlier chapters and Up Next contains the remainder of that CD. |
| Double-click a loose file | Play the single file. Album tags, a disc tag, a CUE program or a numbered CD folder supply disc context. Opening a disc page also explicitly selects that scope. |
| Choose **Play CD** from a section menu or CD page menu | Start the whole CD at the beginning, including a recorded lead-in. Disc playback uses authored order and clears shuffle and repeat-one. |
| Choose **Play All** or **Add All to Queue** from a collection | Play or append the whole displayed track sequence. A search limits these actions to the results. |
| Double-click a playlist occurrence | Play from that occurrence in the displayed playlist sequence. Repeated occurrences retain independent queue entries. |
| Click the player artwork | Open the playing CD and select the exact chapter. The artwork menu also offers **Show in Collection**. |
| Choose **Show Track on CD** from a queue entry | Reveal that entry’s chapter. The playback indicator stays on the active recording. |
| Choose **Continue This CD** from the artwork menu or the playing CD’s panel menu | Place the remaining chapters immediately after the current occurrence. Current position, play/pause state and history are preserved. |

Continue reuses the first available queued occurrence of each required chapter and creates only missing entries. All other upcoming entries keep their relative order, including intentional repeats. If the continuation is already correct, a second invocation leaves the queue revision unchanged. When there are chapters to continue, the operation disables shuffle and repeat-one. Saved playlists keep their entries and ordering.

![Two search matches on different CDs while one plays](evidence/cd-search-playback.png)

## Ownership and navigation

The folder scan supplies the shared track objects used by the overview, table and player. A CUE sheet identifies a program across physical files. Ordinary albums use folder, album and disc number. Sections retain discovery order, CUE tracks retain authored order, and tagged file albums use track numbers with stable ties. The table interleaves fixed-height section headers with track rows through an index of header offsets. Selection, ratings, likes, drag targets and paging still use track or playlist occurrence keys. Only rendered headers request thumbnails.

Indexed sections and ordered keys come from one SQLite read snapshot. Explicit column sorting removes the headers. Opening a track resolves the CD independently of the visible filter and viewport. Queue edits still go through the transport. It can remap a contiguous run inside the decoder range already open, including restoring a previously cleared continuation. A boundary outside that range requires a subsequent decoder open.

Back and Forward retain the page, search, presentation, sort, primary selection and scroll offset. Moving between a collection and its discs can reuse its scan. Reveal uses the current recursive folder when it contains the browsed item, the owning indexed collection for a registered identity, and the nearest enabled collection ancestor or containing folder for unregistered music. Live indexed tracks can follow verified relocation by database ID. Saved queues retain their original file and chapter locators across restarts.

![Queue chapter nine selected while chapter seven plays](evidence/cd-queue-reveal.png)

## Windows qualification, 20 November 2026

The optimized Windows application ran with isolated profiles and application volume zero. The real Thunderdome archive displayed **108 discs and 2,123 logical tracks**. The separately populated SQLite index returned the same counts.

- Double-clicking *Boomerang (Koala mix)* in grouped search results, on its dedicated CD page, and in indexed results built a 13-entry queue. Chapter 7 played with *Dominatin* and five later chapters up next.
- Returning from a CD restored the previous scrolled section and selection. Back also restored the two-disc search for `title:boomerang`. Clicking the artwork from a different CD selected chapter 7. Revealing *God* from the queue selected chapter 9 while chapter 7 played.
- Clearing Up Next and choosing **Continue This CD** kept queue entry 7 active. The saved transport position advanced from 11.512 to 13.372 seconds and the six remaining chapter titles returned in order.
- The playing chapter retained its like and three stars in Liked Tracks. A two-occurrence playlist started at its second entry. Continuing the CD kept that occurrence active, appended the six chapters and left the saved playlist at two occurrences.
- A separate fixture combined a multi-file CUE with a recorded lead-in, a second CD of individual WAV files and a loose folder. The two CD track starts produced three and two queue entries respectively. A loose file played alone. Explicit Play All included all eight entries.
- In one final release run with an empty application cache, discovery published its first 19 tracks at 12.6 ms and completed in 302 ms. Applying six batches took 8.8 ms total on the interface thread, with a 4.8 ms maximum. These are application timers with Windows filesystem caching enabled.

Formatting, warnings-denied Clippy and all 60 default tests passed locally. Tests cover alternate sheets, missing and replaced sources, verified moves, recorded lead-ins, multi-file CUE ordering, playlist duplicates and decoder-range restoration. An in-memory renderer also compared stereo samples across a CUE boundary. The two opt-in codec and scale campaigns were outside this run. Physical output, audible transitions and gapless playback between different files remain unqualified.

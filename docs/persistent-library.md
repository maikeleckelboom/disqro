# Persistent library and personal collections

## Listening controls

- Open a folder and choose **Add Folder to Library** from the Recordings panel. Choose its sidebar entry to browse the index. **Albums / CDs** opens the disc list. Open a disc to browse its CUE chapters. The address bar, Back and Forward follow indexed locations.
- Ctrl+F searches the current view. Terms include `artist:`, `album:`, `format:`, `liked:yes` and `rating:4` (four or five stars). Click a heading to cycle through ascending, descending and original order.
- Click a heart or a star to edit one track. Alt+L toggles likes on the selection. Alt+1 through Alt+5 set its rating, and Alt+0 clears it. Liked Tracks includes saved favorites whose sources are missing.
- Ctrl+N creates a playlist. Drag tracks onto its sidebar entry or choose **Add to Playlist** from their context menu. Ctrl-click or Shift-click selects multiple rows. Shift+Up and Shift+Down extend a range from the keyboard.
- Playlists preserve repeated occurrences. In the original, unfiltered order, drag entries before another entry or use Ctrl+Shift+Up and Ctrl+Shift+Down. Delete removes selected occurrences. Browsing and editing a playlist leave the playback queue independent.
- Double-click plays from the selected track. For large indexed views, this queues up to 5,000 surrounding entries. **Add All to Queue** explicitly requests the whole view. A root's context menu offers rescan and removal.

## Storage and identity

The permanent `library.sqlite3` lives alongside the workspace profile. Parsed sheets, audio headers, artwork-related data and waveform analysis can be rebuilt independently. Foreign keys protect tracks referenced by likes, ratings or playlist entries. Removing a root detaches personal tracks. Removing music from disk leaves its personal entries in place.

A physical audio source and its CUE sheet each have a file ID. A standalone track is anchored to its audio file. A chapter is anchored to its sheet and authored number, with a separate part for the recorded lead-in. Source IDs and CD-frame bounds describe playback. Tagged albums include the folder and disc number in their grouping. Playlist entries have independent IDs so two occurrences of the same track can be moved separately.

The index treats an established path as continuous across edits. A relocation requires an absent old path, matching native volume and file ID, size, modification time, corroborating content sample and a unique candidate on both sides. The sample covers the first 64 KiB of audio or the full supported sheet. An uncertain copy or cross-volume move gets a fresh identity. Personal data remains attached to the original. If a different recording replaces a file at the same path between scans, the library cannot reconstruct that history.

Each index pass uses a separate connection and a root generation. Each batch checks ownership under the SQLite write lock. A retired scan cannot publish new availability after root removal or a competing scan. A completed walk marks absent paths missing and protects unreadable areas. Header reads take place outside library write transactions. Schema upgrades back up the database and advance the version in the same transaction as the migration. A future or corrupt database produces an error.

## Paging and ownership

The browser owns navigation and search. The table delegate owns the ordered keys, painted rows and selection. The catalog sends personal edits to SQLite and projects the resulting marks and playlist summaries into GPUI. The audio engine consumes independent track snapshots.

Each search or sort materializes one 64-bit key per matching row, with stable tie breakers. Metadata is fetched around the viewport with 64 rows of prefetch. The table trims its cache after 4,096 full rows. The SQLite worker retains the latest pending read per view and up to 128 pending requests in total. Running superseded SQL reads have a progress handler. Views also check the generation before applying an asynchronous result. Personal writes retain FIFO order and shutdown drains accepted edits.

The filesystem walk and album grouping still require memory proportional to the collection being scanned. Substring searches examine the candidate set. Rendering a track page does not materialize the entire library. Explicitly queuing an entire library does.

## Windows verification

An isolated debug profile at volume zero opened a standalone WAV folder and the Thunderdome archive. Registering the archive produced 108 discs and 2,123 logical tracks. One run recorded 309 ms between root registration and the initial index commit. This was a warm-filesystem observation on the real archive. It excludes later tag enrichment and screen presentation.

Opening *The Original Dreamteam (CD1)* displayed 13 tracks. Searching for `artist:buzz` returned three. Sorting by title put *Boomerang (Koala mix)* first. Clicking its heart and fourth star left playback stopped. Two CUE chapters and a standalone WAV were dragged into *Listening set*. Moving the WAV to the first position preserved all three entries. The context menu queued *Boomerang*, and double-clicking started its transport and waveform. Browsing Liked Tracks and navigating back preserved playback. Restart restored the playlist order, four stars, favorite, registered root and paused queue position.

![Mixed playlist during muted CUE playback](evidence/persistent-playlist.png)

Temporary filesystem tests cover verified same-volume relocation, ambiguous copies, missing and restored sources, root removal during a scan, competing scans and migration rollback. This qualification did not measure audible output, removable-drive latency or six-figure indexed-library performance.

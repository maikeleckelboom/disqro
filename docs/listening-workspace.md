# Listening workspace

Disqro opens a folder or a CUE album directly. Browsing never changes what is playing: the queue retains the tracks independently of the browser, so you can open other folders, sort tables or rearrange panels while an album continues.

![Default Windows workspace with an album playing](evidence/modular-workspace/corrections/tabbed-1240.webp)

## Workspace

The default layout places Library and Locations on the left, Music Browser in the middle, and Queue and History on the right. Spectrum, Spectrogram (including Waterfall) and Stereo Field can share any group. The player stays below the dock.

- **Navigation.** Each browser has Back, Forward, Up, its address, a local filter and view controls. The current CD or release also has previous and next actions. Clicking a browser makes it the destination for Library and Locations. Ctrl+T opens another browser at that view. **Open Beside** uses an adjacent split when there is room.
- **Library search.** Ctrl+K searches the indexed catalog across tracks, artists, albums, releases, collections and playlists. Selecting a track reveals its catalog identity. Its play button plays the track. Ctrl+F filters the current browser.
- **Tabs and splits.** Groups with several panels show tabs. Choose **Workspace → Customize Layout** to show single-panel drag handles, move a panel, or use **Split Right** or **Split Down**. Choose **Done** to lock the arrangement. A tab's context menu offers **Stack Vertically** or **Group as Tabs** for eligible sidebar pairs.
- **Opening and closing.** Workspace lists the registered panel types. **Reopen Closed Panel** uses the saved instance and surviving companions. Closing all panels leaves **Open a Panel** above the player.
- **Queue.** Use the button beside volume or Ctrl+Alt+Right to toggle the playback sidebar. In a group shared with other panel types it acts on Queue alone.
- **Restoration.** The profile saves the dock arrangement, sidebar visibility, player height, browser history, table preferences and visualization settings. **Undo Layout Change** restores the preceding arrangement during the session.

### Adding music by dragging

- Drag a folder from Locations onto the Library tab, or onto the Library panel, to add it to the library. The folder is indexed where it is; nothing is moved or copied.
- Drag a track or selection onto a playlist in Library or into an open playlist, including its empty space. The drop preview names the destination and track count. A drop appends new references in selection order. Each repeated occurrence gets its own entry. To reorder within a playlist in its natural order, drop its entries on the row they should precede.
- When Library is behind another tab, rest over its tab while dragging to reveal the playlist entries.
- With Library and Locations side by side, drag directly from one to the other.
- Dragging a track or folder never moves a panel, and dragging a tab never adds music.

## Controls

| Action | Control |
| --- | --- |
| Edit the folder path | Ctrl+L, Alt+D or F4, or click the address bar |
| Open a folder | Ctrl+O, a Place or folder in Locations, or a breadcrumb |
| Back, Forward, Parent folder | Alt+Left (or Backspace), Alt+Right, Alt+Up, or the mouse's back and forward buttons |
| Find tracks in the current collection | Ctrl+F |
| Play from a track | Double-click, Enter, or **Play** in its context menu |
| Queue a track | **Play Next** or **Add to Queue** in its context menu; **Add All to Queue** in the panel menu |
| Play or pause | Space outside a text field, the media keys, or the tray menu |
| Previous and next track | Ctrl+Left and Ctrl+Right, the media keys, or the tray menu |
| Seek | Click or drag the waveform |
| Volume | Click, drag or scroll the volume bar; click the speaker to mute |
| Show or hide columns | Right-click a column header |
| Sort | Click a column header: ascending, descending, folder order |
| Reorder the queue | Drag an entry in Queue, or Alt+Up and Alt+Down; Delete removes it; Enter plays it |
| Open or close a tab | Ctrl+T opens a Music Browser; Ctrl+W closes the panel you are working in |
| Quit | Ctrl+Q, the workspace menu, or **Quit Disqro** in the tray menu |

### Address bar

In a folder, click an ancestor to navigate there. Click the empty part of the bar or press Ctrl+L to edit the selected absolute path. The context menu offers **Copy Address**. Library and CD pages show their own hierarchy, such as **Playlists › Techno**. Ctrl+L opens an empty **Go to folder or file** field from these views.

Enter accepts a drive path such as `C:\Music\Albums`, a UNC path such as `\\server\share\Music`, a quoted path or a path containing `%VARIABLES%`. A valid audio file or CUE sheet opens its folder with the recording selected. Validation runs off the interface thread. An invalid entry leaves the current view and history intact. Escape returns to the same browser's breadcrumbs.

### Library and Locations

**Library** aligns All Music, registered collections, Liked Tracks and individual playlists in one list. **Locations** shows the filesystem hierarchy under each Place. New profiles and **Reset Places to Defaults** start with C: under its Windows display name, then Downloads, Music and additional available drives. Saved Places retain their order and labels. Library and Locations each keep their own scroll position and selection.

- Click a Place or folder name to open it. Click the arrow to open or close a branch without leaving the current folder.
- The Place menu offers **Open**, **Open in New Tab**, **Open Beside**, **Add to Library**, **Rename Shortcut** and **Remove from Places**.
- Right-click the Locations tab or the space below the Places: **Add Folder to Places…**, **Restore Missing Default Places**, **Reset Places to Defaults…**.
- A folder's context menu offers the same three opening options, plus **Add to Places** and **Add to Library**.
- Drag a Place onto another to move it there, or below the last row to move it to the end.
- Right-click the Library tab, its empty space or a collection for **Add Music Folder…** and **New Playlist**. Collections, Liked Tracks and playlists offer both **Open in New Tab** and **Open Beside**. The **+** beside Playlists creates a playlist. Entries are always visible beneath the heading and scroll with Library.

Removing a shortcut never touches its folder. Restoring adds back only the defaults you removed, at the end, and leaves your own shortcuts, names and order alone. Reset replaces the whole list after a confirmation. A drive that is not attached disappears until it returns; a custom Place whose folder is missing stays, dimmed.

A registered collection and a Place can point at the same folder. The Library entry opens the index; the Location opens the folder on disk.

### Folders in Locations

- The open folder is shown under one Place: the one you chose it from while you stay inside it, otherwise the outermost Place other than a drive that holds it, then its drive. A folder outside every Place appears under a dimmed entry for its drive or share until you return to a Place.
- Opening a folder reveals the ancestors needed to show its row. The row and Enter navigate. Its chevron and the Right and Left keys control expansion.
- Back, Forward, browser switching and restart preserve the branches you opened or closed. A folder under a manually collapsed ancestor can stay hidden while its content is open in the browser.
- Each Place keeps its own open folders, and they are remembered across restarts. Folders are listed off the interface thread, one listing task per Place, so a sleeping network drive holds up only its own branch.
- Folders whose names start with a dot, such as `.git` or `.cache`, and hidden or system folders are left out of the tree and of subfolder discovery. Typing the path of one in the address bar still opens it, and the tree shows the way to it.
- Folders and files are ordered as Explorer orders them, so `CD 2` comes before `CD 10`.

### Queue and History

**Queue** shows the current entry, marked as playing or paused, followed by what plays after it. **History** shows what played before it, most recent first; double-click an entry to play it again, or use its menu to play it next, remove it or show it on its CD. They are separate panels with their own selection and scroll position, and switching, splitting or closing them never changes the queue. **Clear Up Next**, Shuffle and Repeat are in the Queue menu, **Clear History** in the History menu. Shuffle reorders only what is still to come and switching it off restores that order. Repeat cycles Off, All and One. Previous restarts a track after three seconds and otherwise steps back through History. A CUE chapter is an ordinary entry: a chapter from one disc, a single file from another folder and a chapter from a second disc queue and play in that order with their own artwork, titles and boundaries. Consecutive chapters of the same disc image still share one decoder, so an album plays without gaps between its chapters. The queue, the current entry and the position are saved shortly after each change and on exit; the next launch restores them paused.

### Tray and background playback

Disqro shows its compact icon in the notification area while it runs. Click it to bring the window back. Its menu offers Show Disqro, Play or Pause, Previous, Next, Show Queue and Quit Disqro. The minimize button and taskbar use normal Windows minimize and restore. Choose **Hide to Tray** from the workspace menu to hide the window explicitly. Enable **Close to Tray** to hide it when closing. By default, closing the window quits Disqro.

Quit Disqro is always in the tray menu and the workspace menu. While the window is hidden, playback, the queue, the tray and the media keys keep working; the interface stops drawing and waveform analysis waits until the window returns.

### Windows media controls

Disqro registers with the System Media Transport Controls. The Windows media overlay shows the title, artist, album, artwork and timeline of the current track, including CUE chapters, and its Play, Pause, Previous, Next and seek controls drive the same transport as the window and the tray.

## Appearance

Disqro Dark is a neutral graphite theme: the title bar and the navigation panels sit on `#141416`, tables on `#1B1B1E`, the player on `#202024`, with `#2A2A2F` separators and a steel-blue `#91AFCB` accent reserved for selection, focus and what is playing. The waveform has its own colours so it never tints the interface.

The theme lives in `src/theme.rs` as a small set of materials (chrome, canvas, raised, overlay, lines, three text levels, accent, hover and selection states, and the waveform colours) mapped onto GPUI Kit's theme. Kit components and Disqro's own views read the same values. A Light or Glass appearance adds another set of materials, and for Glass a compositor backdrop behind the chrome surfaces, without a separate frontend.

## Playback and analysis

The transport streams local mono and stereo recordings through Kira and the default Windows audio output. Symphonia decodes WAV, AIFF, FLAC, MP3, AAC and ALAC in M4A, and Ogg Vorbis. Ogg Opus uses the bundled libopus adapter. FFmpeg is only needed to generate and compare optional test fixtures.

CUE times use 75 ticks per second. The next track's INDEX 01 ends the current track in the same source. Recorded pregaps remain playable and a recorded lead-in before the first track is accessible. When the queue changes while a disc is playing, the running decoder keeps only the chapters that still follow in the queue and stops where the next queued entry differs. Synthesized gaps, mixed-mode discs, complex MP4 edit lists, and surround playback are outside this build.

The player reads embedded artwork or a neighbouring cover image. The waveform keeps the minimum and maximum of both channels per bin, so antiphase audio stays visible, and the RMS level inside that envelope, which shows the structure of heavily limited masters. A cancellable background job writes up to 1,536 bins per overview. The versioned SQLite cache checks source path, size, modification time and CUE range before reusing an overview. [Collection storage and verification](recursive-collections.md)

On a normal close, and shortly after any change to the queue or Places, Disqro saves the workspace with each panel's own state, the volume, queue, Places, window preferences, and the column layouts and view new browsers start with, in `%LOCALAPPDATA%\Disqro\workspace-v1.json`. Set `DISQRO_PROFILE_DIR` to use an isolated profile. The dock layout is version 3; a layout of another version is replaced by the default workspace.

## Engineering decisions

- Panels are instances of registered types (`src/panels.rs`). A type names itself, its icon, whether it exists once or many times, where it opens, and how to build an instance from saved state. An instance has a stable identity and saves its own state into the dock layout. GPUI Kit's dock owns the layout tree, dragging, splitting and persistence, and rebuilds instances through its panel registry. The workspace creates, places, closes and reopens instances without knowing their types, so a new panel type, such as a visualizer, is a new module rather than a change to the workspace.
- Shared state is not owned by any panel: the catalog, the player and queue, and the Places and folder tree live outside the dock. Closing Library, Locations or Queue loses nothing, and every Music Browser reads the same catalog while keeping its own view.
- GPUI Kit supplies each group's tabs, active panel, order and drag operations. A browser instance owns its address editor and search input. Moving it between groups moves those controls with it.
- Each group renders its selected panel. Cached drawing isolates playback updates from the dock. When Windows assistive technology requests an accessibility tree, the dock and visible panels render their controls into that tree.
- One transport owns playback and the queue on its own thread. The window, the tray and the Windows media controls all send it the same commands, so they cannot disagree about what is playing.
- Redraws follow what is visible. The player redraws for position only when the playhead moves at least three quarters of a pixel or the clock changes a second, instead of on every 40 ms transport tick. While the window is hidden the transport stops waking the interface for position updates.
- GPUI Kit's default "show while scrolling" scrollbars kept requesting animation frames for the folder tree, costing about 4% of a core in an idle window. Disqro uses hover scrollbars, which Windows 11 uses as well.
- Controls inside the title bar are marked as occluding so Windows does not treat them as the caption.
- The tray icon lives on a hidden window owned by the interface thread, created with the Win32 API from the `windows` crate that GPUI already depends on. No second UI toolkit is involved. The media session uses `ISystemMediaTransportControlsInterop` on the same window.
- The read-only Dekzer review covered its playback crate, native player integration, CUE handling, player qualification, and waveform research. Disqro adopted source-frame addressing, decoder preroll, a buffered boundary between decoding and output, and sample-oracle checks. It has its own implementation and no Dekzer source or runtime dependency.

## Historical Mission 10 proposal for navigation and search

Place a shared navigation row directly below the title bar, with Back, Forward, Up, editable address or semantic breadcrumbs, and **Search Library**. The active Music Browser owns the location and history shown in that row. Switching browsers updates the controls to reflect that browser's context.

**Search Library** (Ctrl+K) opens an immediate results overlay for tracks, albums/CDs, artists and playlists across the indexed library. Query the index asynchronously and preserve recording and CUE chapter identities. Searching keeps the existing tabs and dock arrangement. **Filter This View** (Ctrl+F) stays in each browser's contextual toolbar and retains that browser's query.

Carry forward the app-icon title bar, Scope-inspired content tabs, connected utility tabs, lightweight view controls, persistent player and Disqro Dark styling. The shared controls and global search are scheduled for the dedicated navigation mission.

## Sidebar and navigation qualification

The optimized Windows player was exercised at 150% display scaling with Win32 input and Windows UI Automation. The address and local search fields share the original 28-logical-pixel height. Local search uses a borderless field. [Default workspace at 1240 × 780](evidence/modular-workspace/corrections/tabbed-1240.webp) · [Compact workspace at 720 × 460](evidence/modular-workspace/corrections/tabbed-720.webp)

| Check | Observed result |
| --- | --- |
| Sidebar geometry | At 720 × 460, 900 × 600, 1240 × 780 and 1540 × 940, the central table measured 640, 790, 1098 and 1370 device pixels wide in both arrangements. At 1240 × 780 its position and height also matched exactly after stacking both sidebars. |
| Sidebar recovery | Both stack orders worked while locked. Each divider resized and restored across restart within three logical pixels. Dragging a tab onto its sibling header reconnected the pair. Dragging to a pane edge reordered it. Close, reopen, Undo and Restore Default Layout preserved the sibling and the player. [Library and Queue above](evidence/modular-workspace/corrections/stacked-library-queue.webp) · [Locations and History above](evidence/modular-workspace/corrections/stacked-locations-history.webp) · [Compact stacked panes](evidence/modular-workspace/corrections/stacked-720.webp) |
| Utility headers | Continuous window and sidebar-divider sweeps kept each header 34 logical pixels tall. Compact icons retained their accessible names and Left/Right selection. After grouping four utilities in a narrow sidebar, the menu exposed all four and switched to a hidden tab. [Overflow menu](evidence/modular-workspace/corrections/overflow-menu.webp) |
| Folder tree and Places | Row clicks and Enter opened folders. Chevron and Left/Right changed disclosure independently. Nested navigation revealed ancestors. History, tab switching and restart preserved manual disclosure. Fresh and reset Places used the Windows name for C:, then Downloads and Music. Restoring a removed default appended it. Scrolling through 60 folders left the adjacent 47-playlist Library pane in place, and vice versa. |
| Addresses | Physical folders supplied their selected absolute path. All Music, Liked Tracks, a playlist, an indexed collection, a release, an indexed CD and a folder-backed CUE page supplied semantic breadcrumbs and an empty Ctrl+L field. Invalid entries and Escape preserved the view. Switching between two side-by-side browsers selected each browser’s own address context. [Playlist breadcrumbs](evidence/modular-workspace/corrections/playlist-breadcrumb.webp) |
| Empty playlists | The playlist uses the Library music-note icon. The instruction names the **Add to Playlist** menu and centers each line inside its own browser pane. [Compact split with an empty playlist](evidence/modular-workspace/corrections/empty-playlist-compact.webp) |
| Playlists and playback | Create, rename, delete, copying three separate CUE chapters, copying into a populated playlist and repeating an occurrence all passed. The source files, likes, ratings and queue retained their values. During sidebar layout changes the muted player issued 3,468 audio callbacks with zero device or decode errors. |

The locked release suite passed all 117 tests, including FFmpeg fixtures and the six-figure collection test. Formatting, warnings-denied Clippy, the standard debug tests and the Windows build script also passed. The captured release executable has SHA-256 `b3ad7244f6106cff150ceb18d578ceb5879197602d73fc3c184d63921fac7e54`. Audio qualification used transport telemetry. [Release test output](evidence/modular-workspace/corrections/release-tests.txt) · [Native check results](evidence/modular-workspace/corrections/windows-checks.txt)

## Final Mission 10 Windows qualification

The previously reviewed optimized Windows executable has SHA-256 `6f76a31e1dc6e8d8386b372634fa31cdd1e7d1193330a01c3175342b698daecd`. The locked release suite passed all 111 tests, including the Windows junction fixture, missing-source playlist copies, browser history persistence, FFmpeg fixtures and the six-figure collection test. Formatting, warnings-denied Clippy and the locked release build passed. [Release test output](evidence/modular-workspace/final/test-suite.txt)

Native input and accessibility checks ran at 150% Windows scaling against an isolated copy of a 126-track collection. The following captures show the application during that run. [Default listening view](evidence/modular-workspace/final/default.webp) · [Three selected tracks over an empty Techno playlist](evidence/modular-workspace/final/playlist-transfer.webp) · [Customize Layout with Done visible](evidence/modular-workspace/final/customize-layout.webp)

| Check | Observed in the optimized application |
| --- | --- |
| Navigation and sizing | Liked Tracks → Techno reused the same browser. Open in New Tab created a second tab. Open Beside tabbed at 720 × 460 and 900 × 600, then split at 1240 × 780 into tables measuring 362 and 361 logical pixels. Also checked the default view at 1540 × 940. |
| Music transfers | Dragging three selected CUE chapters into an empty Techno browser appended the displayed recordings in order. Dropping one again onto a populated row appended a distinct occurrence. Copying from Techno into another playlist preserved Techno's occurrences. Source file digests, likes, ratings and queue entries matched the pre-transfer values. Tracks and folders also crossed the utility tabs, including indexing a folder in Library. |
| Layout safety and recovery | Dragging a locked tab left its group intact. A splitter resized while locked. Customize Layout enabled splitting, reordering and moving between groups. Done locked them again. Undo restored the group and instance identities after rearrangement and reset. Reopening after a close restored the original arrangement. Closing all panels kept playback running and the panel picker accessible. |
| Browser independence and restart | Clicks in vertically stacked Techno and Liked Tracks directed subsequent Library navigation to the chosen browser. Scrolling one of two side-by-side tables left the other in place. On restart, both browsers restored their locations, search text, Back and Forward destinations, active recipient and grouping. The layout reopened locked. A panel closed before quitting reopened with its previous ID, tab group and search. The library, playlists, favorites, ratings and saved queue survived restart. [Native check results](evidence/modular-workspace/final/windows-checks.txt) |
| Playback during interaction | The two muted sessions recorded 8,648 and 5,124 callbacks across navigation, panel edits, resizing and empty-workspace recovery. Each had zero decode or device errors and one unchanged-position tick in a sample containing a transport command. These traces measure transport behavior. Audible output on speakers or headphones remains unverified. [First session](evidence/modular-workspace/final/playback.csv) · [After restart](evidence/modular-workspace/final/playback-after-restart.csv) |

### Production playback comparison

Both optimized executables played the same 44.1 kHz stereo FLAC at zero application volume in a visible 1240 × 780 window. Each run used a fresh isolated profile and six seconds of warm-up after audio callbacks began. Windows process counters measured CPU over the following 15 seconds and private memory at the end. Run order was production, Mission 10, Mission 10, production.

| Build | CPU, percent of one logical core | Private memory | Audio callbacks |
| --- | --- | --- | --- |
| Production baseline | 11.14%, 10.42% | 118.3, 119.1 MiB | 1,502, 1,499 |
| Final Mission 10 | 6.98%, 6.15% | 117.4, 117.5 MiB | 1,504, 1,498 |

All four samples recorded zero device errors, decode errors and unchanged-position ticks. The production binary had SHA-256 `0c2ea99199946ebafe2964ae0639935de0b500f8fdf230b4f87dd87a5ea84427` and was built from the requested production baseline. These measurements cover foreground playback of one file on this Windows machine. Library indexing, startup and complex arrangements were outside this comparison. [Per-run measurements](evidence/modular-workspace/final/comparison.csv)

## Earlier Mission 10 Windows qualification

The optimized Windows executable was built from `f5e1cac859b5bbe474f8860d1f12aaeae0b1b45c` plus the working-tree refinement. Its SHA-256 is `75446b6b04551183db28beba548bed0b9bf3b13813b4bf5afb6c67bc8f368684`. Screenshots below are lossless window captures at 150% Windows scaling.

| Check | Observed result |
| --- | --- |
| Default navigation | Clicking Liked Tracks and then Techno kept the same browser instance ID and the original three dock groups. [Techno in the original browser](evidence/modular-workspace/refined/same-browser-techno.webp) |
| Explicit second browser | **Open in New Tab** on Liked Tracks added a second ID to the same content group. Library and Queue retained their selections. [Local content tabs](evidence/modular-workspace/refined/browser-tabs.webp) |
| Split navigation and restoration | Splitting the browsers placed a breadcrumb and search above each table. Back and Forward in the upper pane changed only that browser. The two search fields kept different values. Two successive restarts restored both IDs, locations, searches, the active recipient and dock geometry to within one logical pixel. [Techno and Liked Tracks in separate panes](evidence/modular-workspace/refined/scoped-vertical-browsers.webp) |
| Window sizes | Tested at requested window sizes of [720 × 460](evidence/modular-workspace/refined/player-720.webp), [900 × 600](evidence/modular-workspace/refined/player-900.webp), [1240 × 780](evidence/modular-workspace/refined/player-1240.webp) and [1540 × 940](evidence/modular-workspace/refined/player-1540.webp). The table measured 427, 527, 732 and 913 logical pixels wide, respectively. Narrow utility headers stacked each icon over its label. |
| Utility docking and drops | A track dragged over inactive Library revealed its playlist target and created one entry. A folder hovered over Library and dropped into the panel produced an indexed track. A second folder dropped on the Library header from a separate Locations pane also indexed. Both pairs split and re-tabbed. [Library and Locations](evidence/modular-workspace/refined/split-library-locations.webp) · [Queue and History](evidence/modular-workspace/refined/split-queue-history.webp) |
| Playlist operations | Created, renamed and deleted a playlist through the Windows UI. Added the same recording twice and moved the final occurrence to the start. Checked the order by occurrence ID in SQLite. |
| Panel lifecycle | Hid and restored a browser toolbar, closed and reopened History, then closed every panel. The player and the empty-workspace picker remained available. Opening a browser and restoring the default layout recovered the three groups. |
| Existing profile | Used a copy of a version-2 profile with the old playlist disclosure set to closed. The new geometry opened with both playlists visible. Verified the original folder, expanded folders, Places, column preferences, volume, playback position, queue order, all 126 track IDs and the contents of the likes, ratings, playlists and playlist-entry tables. |
| Playback during interaction | Resized the window and a dock splitter, switched tabs and dragged panels and music during muted playback. The 196-second trace recorded 18,959 callbacks, zero device or decode errors and one unchanged-position tick in a sample that also contained a transport command. Physical audio quality was outside this measurement. [Playback trace](evidence/modular-workspace/refined/playback.csv) |

Formatting, warnings-denied Clippy and the locked optimized build passed in this earlier run. Running the complete suite including the FFmpeg and six-figure collection tests produced **108 passes and one fixture setup failure**. Windows returned access denied when `directory_junction_is_not_descended` invoked `mklink /j`. Repeating the fixture in a writable directory produced the same denial. At the time, the production-baseline build had exited 101 during its CMake step, leaving the performance comparison unverified.

## Earlier listening-workspace verification

Measured on a Ryzen 7 8845HS running Windows 11 at 150% display scaling with the optimized build, using isolated profiles with the application volume at zero. CPU is the share of one logical core averaged over 12 seconds; GPU is the sum of the process's GPU engine counters.

| State | CPU | GPU | Private memory |
| --- | --- | --- | --- |
| Before this change, window visible, playing | 16.2% | 4.1% | 122 MB |
| Window visible, idle, nothing played | 1.6% | 0% | 109 MB |
| Window visible, playing a FLAC disc | 4.0% | 0% | 113 MB |
| Hidden in the tray, playing | 1.1% | 0% | 114 MB |

The first row is this branch before the redraw throttle and the scrollbar change; the earlier build drew its seek slider on every transport tick in the same way.

Checked in the running application:

- The before and after layouts at 1240×780 and 900×600. [Before](evidence/before-refinement.png), [after](evidence/disqro-dark.png).
- Address bar: click and Ctrl+L select the whole path; a quoted path opens; a missing path shows its message and keeps the folder; Escape restores the breadcrumbs; Back, Forward, Parent and Backspace follow the history. [Missing path](evidence/address-error.png).
- Places: Add to Places from the tree, drag to reorder, remove, restore one missing default, reset after confirmation, and the saved order. [Places menu](evidence/places-menu.png).
- Queue: Play Next for track 7 of the second disc of *Thunderdome IV*, Add to Queue for a single FLAC from another folder, Add to Queue for track 11 of *The Megamix Of Thunderdome 1-5!*, a drag to reorder, and playback across those entries by seeking near each end. Each entry kept its title, artist, album, artwork and duration. After a restart the queue returned paused at the same entry and position. [Queue after playing across sources](evidence/queue-mixed-sources.png).
- Tray: Hide to Tray removed the window while playback continued, and activating the icon restored it.
- Media controls: through the WinRT session manager, Disqro's session reported title, artist, album, a thumbnail, playback status and timeline. Pause and Skip Next sent through Windows paused the player and advanced to the next queue entry.
- The earlier checks still pass: a 13-track FLAC+CUE disc that also carries an alternate sheet for an absent WAV rip shows 13 tracks, and a three-chapter CUE across two WAV files advances through both boundaries. [Four native transport captures in playback order](evidence/cue-transport.png)

Not yet verified: audible output on physical speakers or headphones (the runs used application volume zero), physical media keys (the session was driven through the Windows API rather than a keyboard), the tray menu opened with a real right-click, scaling factors other than 150%, device loss, gapless joins between separate files, and sample-accurate AAC seeking.

For repeatable audio checks, run `cargo test --release --locked` with CMake available. With FFmpeg on PATH, use `cargo test --release --locked -- --include-ignored --nocapture` to exercise the encoded fixtures as well.

## Not in this build

Future work includes output device selection, loudness normalization, zoomable waveforms and additional visualizer panel types.

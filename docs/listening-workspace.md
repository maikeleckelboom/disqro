# Listening workspace

Disqro opens a folder or a CUE album directly. Browsing never changes what is playing: the queue retains the tracks independently of the browser, so you can open other folders, sort tables or rearrange panels while an album continues.

![Disqro Dark with an album playing](evidence/disqro-dark.png)

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
| Reorder the queue | Drag an entry, or Alt+Up and Alt+Down; Delete removes it; Enter plays it |
| Quit | Ctrl+Q, the workspace menu, or **Quit Disqro** in the tray menu |

### Address bar

The current folder appears as breadcrumbs. Click a crumb to open that folder. Click the empty part of the bar, or press Ctrl+L, to edit the full path with everything selected. Type or paste a drive path such as `C:\Music\Albums`, a UNC path such as `\\server\share\Music`, a path in quotes, or one with `%VARIABLES%`. Enter opens it; Escape restores the breadcrumbs. A path that does not exist shows a short message in the bar and leaves the current folder as it was. Paths are checked off the interface thread, so a sleeping network share cannot stall the window. A path to an audio file or CUE sheet opens its folder with that track selected. Right-click the bar to copy the address.

### Library and Locations

The sidebar has two parts. **Library** holds the indexed collections, Liked Tracks and playlists. **Locations** holds the Places, and each Place opens in place to show its own folders: Music expands to the folders inside Music, so the sidebar never shows Music twice. Disqro starts with Music and Downloads, resolved through Windows Known Folders, and the drives present on the machine.

- Click a Place or folder name to open it. Click the arrow to open or close a branch without leaving the current folder.
- Right-click a Place: **Open**, **Add to Library**, **Rename Shortcut**, **Remove from Places**.
- Right-click the Locations heading or the space below the Places: **Add Folder to Places…**, **Restore Missing Default Places**, **Reset Places to Defaults…**.
- Right-click a folder: **Open**, **Add to Places**, **Add to Library**.
- Drag a Place onto another to move it there, onto the heading to move it to the top, or below the last row to move it to the end.
- Right-click the Library heading, or a collection, for **Add Music Folder…** and **New Playlist**. The **+** on Playlists creates a playlist; the arrow beside it closes a long list.

Removing a shortcut never touches its folder. Restoring adds back only the defaults you removed, at the end, and leaves your own shortcuts, names and order alone. Reset replaces the whole list after a confirmation. A drive that is not attached disappears until it returns; a custom Place whose folder is missing stays, dimmed. Opening a Place in a new tab is not available yet: the workspace has one folder view.

A registered collection and a Place can point at the same folder. The Library entry opens the index; the Location opens the folder on disk.

### Folders in Locations

- The open folder is shown under one Place: the one you chose it from while you stay inside it, otherwise the outermost Place other than a drive that holds it, then its drive. A folder outside every Place appears under a dimmed entry for its drive or share until you return to a Place.
- Opening a folder expands the way down to it and the folder itself, never its neighbours or the discs inside a large collection. Nothing collapses because you navigated; the rest of the tree, its scroll position and the row under the pointer stay where they were.
- Back, Forward, Parent, breadcrumbs and typed paths keep the highlighted folder, the breadcrumbs and the list in agreement. While a library view is shown, the tree keeps the last folder in view without highlighting it.
- Each Place keeps its own open folders, and they are remembered across restarts with whether Playlists is open. Folders are listed off the interface thread, one listing task per Place, so a sleeping network drive holds up only its own branch.
- Folders whose names start with a dot, such as `.git` or `.cache`, and hidden or system folders are left out of the tree and of subfolder discovery. Typing the path of one in the address bar still opens it, and the tree shows the way to it.
- Folders and files are ordered as Explorer orders them, so `CD 2` comes before `CD 10`.

### Queue

The Queue panel labels the current track as playing, paused, finished or stopped. **Up Next** shows upcoming entries. **History** can be expanded to see what played in this session. Shuffle reorders only what is still to come and switching it off restores that order. Repeat cycles Off, All and One. Previous restarts a track after three seconds and otherwise steps back through History. A CUE chapter is an ordinary entry: a chapter from one disc, a single file from another folder and a chapter from a second disc queue and play in that order with their own artwork, titles and boundaries. Consecutive chapters of the same disc image still share one decoder, so an album plays without gaps between its chapters. The queue, the current entry and the position are saved shortly after each change and on exit; the next launch restores them paused.

### Tray and background playback

Disqro shows its compact icon in the notification area while it runs. Click it to bring the window back. Its menu offers Show Disqro, Play or Pause, Previous, Next, Show Queue and Quit Disqro. The minimize button and taskbar use normal Windows minimize and restore. Choose **Hide to Tray** from the workspace menu to hide the window explicitly. Enable **Close to Tray** to hide it when closing. By default, closing the window quits Disqro.

Quit Disqro is always in the tray menu and the workspace menu. While the window is hidden, playback, the queue, the tray and the media keys keep working; the interface stops drawing and waveform analysis waits until the window returns.

### Windows media controls

Disqro registers with the System Media Transport Controls. The Windows media overlay shows the title, artist, album, artwork and timeline of the current track, including CUE chapters, and its Play, Pause, Previous, Next and seek controls drive the same transport as the window and the tray.

## Appearance

Disqro Dark is a neutral graphite theme: the title bar and sidebars sit on `#141416`, tables on `#1B1B1E`, the player on `#202024`, with `#2A2A2F` separators and a steel-blue `#91AFCB` accent reserved for selection, focus and what is playing. The waveform has its own colours so it never tints the interface.

The theme lives in `src/theme.rs` as a small set of materials (chrome, canvas, raised, overlay, lines, three text levels, accent, hover and selection states, and the waveform colours) mapped onto GPUI Kit's theme. Kit components and Disqro's own views read the same values. A Light or Glass appearance adds another set of materials, and for Glass a compositor backdrop behind the chrome surfaces, without a separate frontend.

## Playback and analysis

The transport streams local mono and stereo recordings through Kira and the default Windows audio output. Symphonia decodes WAV, AIFF, FLAC, MP3, AAC and ALAC in M4A, and Ogg Vorbis. Ogg Opus uses the bundled libopus adapter. FFmpeg is only needed to generate and compare optional test fixtures.

CUE times use 75 ticks per second. The next track's INDEX 01 ends the current track in the same source. Recorded pregaps remain playable and a recorded lead-in before the first track is accessible. When the queue changes while a disc is playing, the running decoder keeps only the chapters that still follow in the queue and stops where the next queued entry differs. Synthesized gaps, mixed-mode discs, complex MP4 edit lists, and surround playback are outside this build.

The player reads embedded artwork or a neighbouring cover image. The waveform keeps the minimum and maximum of both channels per bin, so antiphase audio stays visible, and the RMS level inside that envelope, which shows the structure of heavily limited masters. A cancellable background job writes up to 1,536 bins per overview. The versioned SQLite cache checks source path, size, modification time and CUE range before reusing an overview. [Collection storage and verification](recursive-collections.md)

On a normal close, and shortly after any change to the queue or Places, Disqro saves the folder, volume, queue, Places, window preferences, dock layout and column layout in `%LOCALAPPDATA%\Disqro\workspace-v1.json`. Set `DISQRO_PROFILE_DIR` to use an isolated profile. The dock layout format moved to version 2 for the Queue panel, so a layout saved by the first build is replaced once by the new default.

## Engineering decisions

- One transport owns playback and the queue on its own thread. The window, the tray and the Windows media controls all send it the same commands, so they cannot disagree about what is playing.
- Redraws follow what is visible. The player redraws for position only when the playhead moves at least three quarters of a pixel or the clock changes a second, instead of on every 40 ms transport tick. While the window is hidden the transport stops waking the interface for position updates.
- GPUI Kit's default "show while scrolling" scrollbars kept requesting animation frames for the folder tree, costing about 4% of a core in an idle window. Disqro uses hover scrollbars, which Windows 11 uses as well.
- Controls inside the title bar are marked as occluding so Windows does not treat them as the caption.
- The tray icon lives on a hidden window owned by the interface thread, created with the Win32 API from the `windows` crate that GPUI already depends on. No second UI toolkit is involved. The media session uses `ISystemMediaTransportControlsInterop` on the same window.
- The read-only Dekzer review covered its playback crate, native player integration, CUE handling, player qualification, and waveform research. Disqro adopted source-frame addressing, decoder preroll, a buffered boundary between decoding and output, and sample-oracle checks. It has its own implementation and no Dekzer source or runtime dependency.

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

Output device selection, ReplayGain-style loudness normalization, a persistent listening history beyond the session queue, a sleep timer, path autocompletion, Places in new tabs, the Light and Glass appearances, and the library features on the roadmap (paged global library, playlists, favorites, ratings, artwork and waveform cells, zoomable waveforms and visualizer panels).

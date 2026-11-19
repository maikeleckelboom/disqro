# Listening workspace

The first Windows build plays a folder or a CUE album directly. Open a folder to browse silently. Open an audio file or CUE sheet from the command line to start listening. Playback follows the table order at the moment you start it. You can then browse another folder or rearrange the workspace while the album continues.

## Controls

| Action | Control |
| --- | --- |
| Open a folder | Ctrl+O |
| Find a title, artist, or album in this folder | Ctrl+F |
| Play from a track | Double-click, Enter on the selected row, or **Play from here** in its context menu |
| Play or pause | Space outside a text field |
| Seek | Click or drag the waveform slider |
| Restart or go back one track | Previous restarts after three seconds, otherwise it goes to the preceding track |
| Return to the previous folder | Alt+Left |
| Recover a closed panel | Use the workspace menu beside **Open folder** |

Drag panel tabs and splitters to rearrange the workspace. Drag a column header to move it, drag its edge to change width, or click its sort control. On a normal window close, Disqro saves the folder, volume, dock layout, and column widths and order in `%LOCALAPPDATA%\Disqro\workspace-v1.json`. Set `DISQRO_PROFILE_DIR` to use an isolated profile.

## Playback and analysis

The transport streams local mono and stereo recordings through Kira and the default Windows audio output. Symphonia decodes WAV, AIFF, FLAC, MP3, AAC and ALAC in M4A, and Ogg Vorbis. Ogg Opus uses the bundled libopus adapter. FFmpeg is only needed to generate and compare optional test fixtures.

CUE times use 75 ticks per second. The next track's INDEX 01 ends the current track in the same source. Recorded pregaps remain playable and a recorded lead-in before the first track is accessible. Adjacent chapters share a running decoder. Changing files opens the next recording on the transport worker. Synthesized gaps, mixed-mode discs, complex MP4 edit lists, and surround playback are outside this build.

The player reads embedded artwork or a neighboring cover image. The seek overview accumulates signed peaks from both channels over the selected track. A cancellable background job writes up to 1,536 peak pairs per overview. The 32 cache slots live in the profile and check source path, size, modification time, and CUE range before reuse.

## Engineering decisions

- The read-only Dekzer review covered its playback crate, native player integration, CUE handling, player qualification, and waveform research. Disqro adopted source-frame addressing, decoder preroll, a buffered boundary between decoding and output, and sample-oracle checks. It has its own implementation and no Dekzer source or runtime dependency.
- GPUI Kit 0.10 owns the window Root, Dock, DataTable, Tree, menus, chrome, and icons. The pinned Kit distribution uses GPUI Pre. The supplied brand SVGs are preserved and the executable embeds a multiresolution Windows icon. [GPUI Kit components](https://gpui-kit.com/component) and [Dock](https://gpui-kit.com/component/dock)
- Folder rows appear before metadata enrichment. The current playback queue has its own snapshot. A persistent collection needs stable identities for physical recordings and CUE tracks before it can own favorites, ratings, or playlists.

## Windows verification

On a Ryzen 7 8845HS Windows machine at 150% display scaling, one optimized launch of a full-disc CUE produced the first nonblank window capture at 0.545 seconds and the first completed waveform cache file at 0.747 seconds from process creation. The application cache was empty and Windows had recently read the recording. These are observed times for one launch. The executable measured 30.9 MiB.

- Opened an actual full-disc FLAC via its CUE sheet. An earlier build displayed 26 rows because the album also contained an alternate sheet for a missing WAV rip. The corrected build showed 13 tracks with real artwork, track durations, and a seekable waveform. Next, seek, pause, column rearrangement, and sidebar resizing were exercised in the native window.
- Ran a three-chapter CUE across two generated WAV files. The native player advanced through both chapter boundaries and stopped at the end of the last file. A separate 48 kHz renderer capture matched 1,024 stereo frames spanning the same-file chapter boundary against the source samples. [Four native transport captures in playback order](evidence/cue-transport.png)
- Reopened a saved workspace with a narrower sidebar and Artist moved before Title. Both changes were visible after restart. Windows UI Automation exposed the restored table order and the names of the transport, seek, and volume controls. The executable contains seven icon sizes from 16 through 256 pixels.
- Compared WAV seek positions with known sample values at 44.1, 48, and 96 kHz. Seven additional FFmpeg encodings of a one-second stereo recording each yielded exactly 48,000 frames. FLAC, AIFF, and ALAC seek windows matched sequential decoding exactly. Full sequential Opus output differed from FFmpeg by less than 0.000001 at peak on this fixture. AAC seek windows differed by up to 0.147 on this fixture, so sample-equivalent lossy seeking remains open.
- The Windows device ran with application volume at zero. Physical listening, device-loss recovery, large-directory latency, and gapless joins across separate recordings still need qualification. The player overview has a single resolution. Library-wide indexing, ratings, favorites, playlists, visualizer panels, and an installer remain future work.

For repeatable audio checks, run `cargo test --release --locked` with CMake available. With FFmpeg on PATH, use `cargo test --release --locked -- --include-ignored --nocapture` to exercise the encoded fixtures as well.

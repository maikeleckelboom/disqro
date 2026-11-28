# Disqro

**A fast, native music player built for the listening experience.**

Disqro is an open-minded approach to local music playback. Built with Rust, GPUI, and GPUI Kit, it combines a powerful
music library, first-class CUE/CD support, detailed waveforms, and modular audio visualizers in a lightweight,
customizable desktop application.

The goal is simple: **excellent music playback and library management, with new ways to explore and experience your
music.**

For day-to-day changes, use the incremental development build. [Windows development workflow and timings](docs/development-builds.md)

## Run the Windows player

Build on Windows x64 with Rustup, Visual Studio C++ Build Tools, the Windows SDK, and CMake. The repository pins Rust and the dependency graph.

```powershell
.\build.ps1 -Release
.\target\release\disqro.exe
```

Open a folder from Locations, the address bar (**Ctrl+L**), or **Ctrl+O**. Every Place in Locations opens in place to show its own folders. Double-click a track to start listening, or right-click it to play it next or add it to the queue. A folder, an audio file, or a CUE sheet can also be passed as the first argument. Opening a file or sheet starts playback.

A folder shows the recordings it holds, and Locations shows where it sits within its Place. To browse an archive such as a CD collection as a whole, choose **Include subfolders** in the Music Browser’s view menu. Disqro remembers the choice for that folder and the folders inside it. A collection shows its albums, discs and individual CUE tracks as they appear. Switch among **Tracks**, **Releases**, and **Tree**, or filter the current view with Ctrl+F. Ctrl-click or Shift-click to queue a selection. The player keeps its queue while you browse.

**Tracks** groups recursive collections by CD, and **Releases** lists the CDs of a multi-disc release beneath it. Double-click a track to play from its position in the current list. Favorites, folders, playlists and filtered library views keep their displayed order. CD and release pages follow authored chapter and disc order. **Play CD** plays one CD, **Play Release** every CD of its release in order, and **Play Collection** the whole collection; each stops at its own end unless you queued more. Click the player artwork to return to the playing chapter or loose recording. Its menu offers **Show in Library**, **Show in Locations** and, when the recording has a CD context, **Continue This CD**. [CD navigation, queue behavior and Windows screenshots](docs/cd-navigation.md) · [Releases and Locations](docs/releases.md)

Use **Ctrl+K** for indexed library search and **Ctrl+F** to filter the active browser. Each browser keeps its own history, selection and scroll position. **Previous CD** and **Next CD** follow authored order. The panel menu also offers release navigation and the track context menu offers **Play Only This Track**.

The Windows workspace holds Music Browsers, Library, Locations, Queue and Queue History side by side or stacked, with the player along the bottom. Every group shows its tabs. Choose **Customize Layout** to move tabs freely between groups or split them, then **Done** to lock the arrangement. The Queue control beside volume shows or hides the playback sidebar. The workspace includes an editable address bar, native tray and media controls, artwork and waveform seeking. It remembers every panel, the queue, volume and columns. [Collection controls and measurements](docs/recursive-collections.md) · [Listening workspace and playback coverage](docs/listening-workspace.md)

In the bottom player, switch between **Track** and **Full CD** with the waveform control or **Alt+W**. Both views follow the same running playback position. [Waveform perspectives and Windows verification](docs/musical-timeline.md)

The L/R meter beneath the volume slider shows stereo RMS, sample peak and one-second peak hold during playback. Choose **Show Stereo Levels** in the Workspace menu to hide or restore it. [Signal path and meter reference](docs/playback-signal.md)

Choose **Workspace → New Spectrum** to analyze the live stereo mix. The sliders button selects a calibrated trace or band RMS, stereo or individual channels, frequency range, resolution and response. Open another instance to compare settings side by side. [Spectrum and stereo measurements](docs/realtime-spectrum.md)

Choose **Workspace → New Spectrogram** for a scrolling frequency history. Freeze a passage or select **Presentation → Waterfall** to read it as layered spectral profiles. **Workspace → New Stereo Field** shows the paired stereo samples with selectable XY and mid/side axes. Each panel remembers its settings. [Live audio visualizations](docs/live-audio-visualizations.md)

## Persistent library and personal collections

Open a music folder and choose **Add Folder to Library** in the Music Browser menu, or drag the folder from Locations onto Library. Its indexed albums and tracks appear in Library. Like a CUE chapter, give it up to five stars, or drag it to a playlist. A playlist can mix chapters and individual files from any folder. [Library controls, storage policy and Windows verification](docs/persistent-library.md)

**Recently Played** in Library shows recordings with a qualified play. Open **Listening History** or **Listening Statistics** from the Workspace menu to inspect individual listens or activity over time. Right-click a track-table heading to show **Plays**, **Last Played**, or **Listening Time**. Listening History also lets you remove individual listens or clear the local history. [Listening accounting and Windows qualification](docs/listening-intelligence.md)

## Product direction

- **Native and fast** — Lightweight, responsive, and designed for large music collections.
- **Advanced music library** — Virtualized tables with customizable columns, artwork thumbnails, cached waveform
  previews, sorting, filtering, and layouts.
- **First-class CUE support** — Browse and play individual tracks from single-file and multi-file CD images without
  splitting the audio.
- **Music organization** — Playlists, favorites, ratings, and track metadata, including individual CUE tracks.
- **Detailed waveforms** — GPU-rendered Stroke in the compact player and the centered Waveform deck. Zoom directly from the musical waveform into the decoded stereo samples. Shift+wheel or scroll horizontally to inspect another position, or choose **Follow playback** to return to the playhead. Press the detailed Waveform to hold the recording like a record, move to audition forward or backward, and release to play on; press Escape to cancel. The compact waveform seeks with a click. [Waveform scrubbing](docs/waveform-scrubbing.md) · [Waveform architecture and qualification](docs/waveform-native-gpu.md)
- **Modular visualizers** — Waveforms, spectrum analyzers, spectrograms, and experimental music visualizations. Open
  multiple simultaneously or create new ones over time.
- **Customizable workspace** — GPUI Kit Dock with movable, resizable panels, saved layouts, and native application
  chrome.
- **Clean interface** — Minimal visual clutter, contextual menus, and powerful keyboard interactions.
- **Local-first** — Your music and library data remain under your control.

## Technology direction

- **Rust** — Native application and audio processing.
- **GPUI + GPUI Kit** — Interface, components, icons, and docking system.
- **SQLite** — Local music library, metadata, playlists, and preferences.
- **Waveform Native** — Source-accurate waveform analysis and the native Stroke renderer. [Waveform Native in Disqro](docs/waveform-native.md)
- **Modular visualization engine** — Extensible audio-driven visualizations sharing playback and analysis data.

## Philosophy

Disqro puts the fundamentals first: reliable playback, speed, accurate music representation, and an enjoyable interface.

Visualizers are more than decoration. They should reveal something meaningful about the music while remaining beautiful,
responsive, and optional.

**Simple when you want it. Powerful when you need it.**

## Project Status

Disqro is in early development, targeting Windows first. The listening workspace is available from source. The workspace includes recursive browsing, a persistent indexed library, playlists, likes and ratings. The player includes a seekable track or full-CD waveform and independent Spectrum, Spectrogram, Waterfall and Stereo Field panels. [Integration review and native qualification](docs/production-consolidation.md)

# Disqro

**A fast, native music player built for the listening experience.**

Disqro is an open-minded approach to local music playback. Built with Rust, GPUI, and GPUI Kit, it combines a powerful
music library, first-class CUE/CD support, detailed waveforms, and modular audio visualizers in a lightweight,
customizable desktop application.

The goal is simple: **excellent music playback and library management, with new ways to explore and experience your
music.**

## Run the Windows player

Build on Windows x64 with Rustup, Visual Studio C++ Build Tools, the Windows SDK, and CMake. The repository pins Rust and the dependency graph.

```powershell
.\build.ps1 -Release
.\target\release\disqro.exe
```

Open a folder from Places, the folder tree, the address bar (**Ctrl+L**), or **Ctrl+O**. Double-click a track to start listening, or right-click it to play it next or add it to the queue. A folder, an audio file, or a CUE sheet can also be passed as the first argument. Opening a file or sheet starts playback.

A folder shows the recordings it holds, and the folder tree shows where it sits within its Place. To browse an archive such as a CD collection as a whole, choose **Include Subfolders**: the folder offers it when it holds only subfolders, and the Recordings panel menu always has it. Disqro remembers the choice for that folder and the folders inside it. A collection shows its albums, discs and individual CUE tracks as they appear. Switch between **Albums / CDs** and **All Tracks**, or search nested music with Ctrl+F. Ctrl-click or Shift-click to queue a selection. The player keeps its queue while you browse.

**All Tracks** groups recursive collections by CD. Double-click a chapter to play within that CD, or use **Play All** for the collection. Click the player artwork to return to the playing chapter or loose recording. Its menu offers **Show in Collection** and, when the recording has a CD context, **Continue This CD**. [CD navigation, queue behavior and Windows screenshots](docs/cd-navigation.md)

The Windows workspace includes an editable address bar, Places, native tray and media controls, real artwork, and waveform seeking. It remembers the queue, folder, volume, collection view, columns and dock layout. [Collection controls, real Thunderdome measurements and screenshots](docs/recursive-collections.md) · [Listening workspace and playback coverage](docs/listening-workspace.md)

## Persistent library and personal collections

Open a music folder and choose **Add Folder to Library** in the Recordings panel menu. Its indexed albums and tracks appear in the sidebar. Like a CUE chapter, give it up to five stars, or drag it to a playlist. A playlist can mix chapters and individual files from any folder. [Library controls, storage policy and Windows verification](docs/persistent-library.md)

## Product direction

- **Native and fast** — Lightweight, responsive, and designed for large music collections.
- **Advanced music library** — Virtualized tables with customizable columns, artwork thumbnails, cached waveform
  previews, sorting, filtering, and layouts.
- **First-class CUE support** — Browse and play individual tracks from single-file and multi-file CD images without
  splitting the audio.
- **Music organization** — Playlists, favorites, ratings, and track metadata, including individual CUE tracks.
- **Detailed waveforms** — High-quality, cached, interactive waveforms with smooth, continuous zoom.
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
- **Modular visualization engine** — Extensible audio-driven visualizations sharing playback and analysis data.

## Philosophy

Disqro puts the fundamentals first: reliable playback, speed, accurate music representation, and an enjoyable interface.

Visualizers are more than decoration. They should reveal something meaningful about the music while remaining beautiful,
responsive, and optional.

**Simple when you want it. Powerful when you need it.**

## Project Status

Disqro is in early development, targeting Windows first. The listening workspace is available from source. The workspace includes recursive browsing, a persistent indexed library, playlists, likes and ratings. Zoomable waveforms and independent visualizer panels are future milestones.

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

Choose **Open folder** or press **Ctrl+O**. Double-click a track to start listening. A folder, an audio file, or a CUE sheet can also be passed as the first argument. Opening a file or sheet starts playback.

The first build browses folders, reads tags and cover art, plays individual CUE tracks, and lets you seek on a waveform built from the recording. It remembers your workspace, table columns, folder, and volume. See the [listening workspace notes](docs/listening-workspace.md) for format coverage, shortcuts, and verification.

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

Disqro is in early development, targeting Windows first. The listening workspace is available from source. The indexed library, playlists, favorites, ratings, zoomable waveforms, and independent visualizer panels are future milestones.

# Playback interaction and Music Browser

## Music Browser

Artists, Releases, and the existing Library destinations share the Music Browser and its table. An artist's Tracks view lists recordings with that performer credit. Its Releases view includes all discs of a release when the artist performed on any disc or was explicitly credited as the album artist. Global Releases includes indexed albums and authored programs, including compilations and multi-disc releases. Loose tracks remain available through All Music, Collections, and Locations.

The scanner reads album-artist tags and the CUE sheet's performer separately from per-track credits. A release credit stays empty if the scan finds conflicting credits. The catalog keeps stable recording IDs and derives release membership once per collection scan. The Artists directory and global search share artist IDs and a performer index. [Persistent library](persistent-library.md)

Go to Playing Track resolves a recording or playlist occurrence against the current unfiltered scope. The active presentation decides whether to select a recording, a release, or a Tree node. A filter that hides the target is cleared and Back restores the prior filter. Tree resolves the target's lineage by catalog identity or by a targeted CUE or file page. Its regular paging continues around the revealed branch. Reveal uses the browser's navigation request token and leaves transport state and queue order alone.

## Tree and playback

Tree paging keeps released rows in runs, remembers explicit expansion and subtree intent, and retains filter matches without keeping the full batch. A file continuation uses the complete filename so CUE sheets and audio files with a shared stem do not share a resume key. The configured resident target is 32,768 nodes. The per-branch paging policy, outstanding requests, and compressed page entries are distinct from this count. Whole-branch sorting is offered up to 24,576 rows. Larger branches show their source order and label that state.

The compact cursor and detail deck sample the same presented audio frame. The compact cursor repaints a retained GPU texture and re-creates it if the window renderer's D3D11 device changes. A detail drag publishes displacement from the grip. The transport applies it to the actual held frame even if pointer movement preceded the hold acknowledgment. The frozen rendering comparisons and the reachable-boundary screenshots cover different contracts. [Waveform scrubbing](waveform-scrubbing.md)

## Windows qualification

Use a locked Windows x64 build. The tools below create their own profiles and private desktops. Each capture directory must be new. Keep the qualification executable separate from the production executable.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets --features qualification -- -D warnings
cargo test --locked
cargo test --locked six_figure_listening_snapshot -- --ignored --nocapture
cargo build --locked --release --features qualification
python -B tools/capture_interface.py <qualification-exe> <test-exe> <capture-directory>
python -B tools/qualify_music_browser.py <qualification-exe> <test-exe> <capture-directory>
python -B tools/qualify_compact_playhead.py <qualification-exe> <capture-directory> --deck
python -B tools/qualify_waveform_scrub.py <qualification-exe> <capture-directory>
```

Native screenshots record their measured window DPI. Compare playback traces under matched fixture, backend, and window conditions. Private-desktop draw cadence measures application behavior. Physical display cadence and audible acceptance require a foreground session. [Windows presentation qualification](playback-presentation.md)

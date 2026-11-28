# Design foundations

Disqro's interface draws its measurements and colors from one place, [`src/theme.rs`](../src/theme.rs). `Materials` holds the colors and `install` maps them onto GPUI Kit's theme, so Kit components and Disqro's own views share one source. The `size` module holds the geometry and type scale. A surface takes its measurements from there instead of choosing its own, so headers, rows and controls line up across panels that sit side by side.

## Comfortable density

Comfortable is the default. It is generous enough for long listening sessions, while a dense library still fits.

| Role | Token | Logical px |
| --- | --- | ---: |
| Body text: rows, tabs, menus | `TEXT` | 14 |
| Secondary text: artists, durations, status | `TEXT_SMALL` | 13 |
| Column headings, section and axis labels | `LABEL` | 12 |
| Prominent single titles, such as Now Playing | `TITLE` | 16 |
| Title bar | `TITLE_BAR` | 46 |
| Tab strips and panel headers | `HEADER` | 36 |
| Content toolbars, such as the Music Browser's | `BAR` | 42 |
| Table column headings | `HEADING` | 32 |
| Table, tree, queue and Locations rows | `ROW` | 32 |
| Library destinations | `NAV_ROW` | 34 |
| Standard control | `CONTROL` | 32 |
| Icon button in a toolbar or header | `ICON_BUTTON` | 30 |
| Icons | `ICON`, `ICON_SMALL` | 16, 14 |
| Content inset, group gap, section gap | `INSET`, `GAP`, `SECTION` | 12, 8, 16 |
| Controls, rows and selection; tabs | `RADIUS`, `TAB_RADIUS` | 6 |

Durations, counts, levels and times use tabular figures (`theme::tabular`), so they line up in columns.

Analyzer and Waveform panels build their header row from [`panels::header`](../src/panels/header.rs), which fixes its height, inset, title and detail styles.

## Color roles

Text has three roles: `text` for primary labels, `text_muted` for secondary metadata, and `text_faint` for column headings, axes and other quiet labels. Faint text keeps at least 4.5:1 contrast against every surface it sits on, so quiet never means unreadable.

The surfaces stay neutral graphite: `chrome` frames the window, `tab_well` recesses behind tabs, `canvas` holds content and `raised` holds the player. Artwork and the musical visualizations supply the color. The play button is the one control with a finish of its own, `Materials::silver`. It is brushed metal that lights a ring in the accent color while playing.

## Qualification

`tools/capture_reference_workspaces.py` opens the Listening, Library and Analysis reference workspaces on a private desktop with a seeded fixture library, generated artwork and a synthetic musical CD image. It captures them at 1440 × 900 and 1100 × 720 while the fixture plays silently, and records which Music Browser toolbar controls exist through a continuous resize.

```powershell
python tools/capture_reference_workspaces.py <disqro.exe> <test.exe> <output>
```

The player must be built with the `qualification` feature.

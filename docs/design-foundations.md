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

Analyzer and Waveform panels build their header row from [`panels::header`](../src/panels/header.rs), which fixes its height, inset, summary and detail styles.

## Workspace chrome

Every dock group shows its tab strip, whether it holds one panel or several. Tabs are everyday workspace chrome, not a sign that the layout is being customized: Customize Layout and Done change only whether panels can be rearranged freely. A tab names its panel, so a panel's own header never repeats that name. It summarizes the panel's settings and holds its controls instead.

Tabs distinguish four states. The selected tab takes its panel's surface and opens into it. The selected tab of the panel the listener is working in also carries an accent rule along its top edge. Hover lightens an unselected tab, and keyboard focus outlines the tab inside a border it always reserves.

## Music Browser toolbar

The toolbar has one composition at every width:

```text
Back  Forward  Breadcrumb ………………  Filter  View ▾  More
```

Width changes only the breadcrumb. It first folds its earliest ancestors into an Earlier locations menu, then truncates the current location at its end. Back, Forward, Filter, the view picker and the panel menu keep their order, labels and sizes at every width. The view picker has one width for every view, so choosing a view never moves its neighbors. It also holds the folder scope wherever a folder offers one. Up and CD and release stepping are in the panel menu and keep their shortcuts.

Filter opens a row below the toolbar, so the field never takes space from the breadcrumb or moves a control. The row stays while a filter hides rows, and Escape or its close button clears and closes it.

## Truncation

Names and titles end in an ellipsis. Middle truncation is kept for folder paths and other identifiers whose ends both matter.

## Color roles

Text has three roles: `text` for primary labels, `text_muted` for secondary metadata, and `text_faint` for column headings, axes and other quiet labels. Faint text keeps at least 4.5:1 contrast against every surface it sits on, so quiet never means unreadable.

The surfaces stay neutral graphite: `chrome` frames the window, `tab_well` recesses behind tabs, `canvas` holds content and `raised` holds the player. The player's timeline lane, `wave.lane`, is the same black the waveform renderer clears to, so the waveform has no visible plate. Artwork and the musical visualizations supply the color. The play button is the one control with a finish of its own, `Materials::silver`: satin metal with machined rim highlights, which lights a thin ring in the accent color while playing.

## Qualification

`tools/capture_reference_workspaces.py` opens the Listening, Library and Analysis reference workspaces on a private desktop with a seeded fixture library, generated artwork and a synthetic musical CD image. It captures them at 1440 × 900 and 1100 × 720 while the fixture plays silently. At the display's own scale it also records the Music Browser toolbar through a continuous window resize and a continuous divider drag, takes the Analysis workspace through Customize Layout and Done, records the keyboard focus order, and checks that playback continues.

```powershell
python tools/capture_reference_workspaces.py <disqro.exe> <test.exe> <output>
python tools/capture_reference_workspaces.py <disqro.exe> <test.exe> <output> --scale 100 --journeys listening,library,analysis
```

`--scale 100` runs the player DPI-unaware, so it lays out at 96 DPI whatever the display's scale; the tool sizes the window in the display's pixels and keeps only the player's own surface. Other scales need the display itself set to them. The player must be built with the `qualification` feature.

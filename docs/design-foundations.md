# Design foundations

Use this specification to scope individual interface missions. Comfortable density, permanent tabs and one browser toolbar composition are approved foundations. The browsing, player and secondary-panel directions below guide subsequent work. Their implementation status and open decisions live in the [review handoff](design-review-handoff.md).

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

### Panel sizing

Each panel type declares its operating width and content height. The dock adds the 36 px tab strip once per group. Starting sizes and saved divider positions are preferences. Dragging can give space to a neighbor down to these operating sizes.

| Panel | Minimum width × height including tabs, logical px | Space reserved for |
| --- | --- | --- |
| Music Browser | 364 × 178 | History, interactive location, Filter, View and menu, with room for an open filter row, heading and recording |
| Library / Locations | 168 × 140 / 160 × 140 | Destination or folder label, icon, disclosure and multiple rows |
| Queue / Queue History | 176 × 132 | Title and duration in a compact list |
| Listening History / Listening Statistics | 272 × 276 / 224 × 200 | Wrapped period controls, search or rankings and result content |
| Spectrogram / Spectrum / Stereo Field | 208 × 164 / 224 × 164 / 176 × 164 | Settings and measurement header above the plot |
| Waveform | 256 × 144 | Zoom, Follow playback and the waveform lane |

A group reserves the largest width and height of its visible members, including inactive tabs. A horizontal split adds child widths and takes the tallest child. A vertical split adds heights and takes the widest child. Handles overlay their shared edges. All-hidden groups occupy zero space.

When the window cannot fit the resulting minimum, scroll the dock horizontally or vertically. Scrollbars appear on the overflowing axes. The title bar and full-width player stay in place. Widening or raising the window makes the panels visible together again. Selecting a tab never changes the group's minimum. Saving in a constrained window retains the preferred proportions, so restoring the larger window also restores its divider positions.

## Music Browser toolbar

The toolbar has one composition at every width:

```text
Back  Forward  Breadcrumb ………………  Filter  View ▾  More
```

Give available width to the breadcrumb. Fold its earliest ancestors into Earlier locations, then truncate the current location at its end. The picker reserves the same width across view choices. As space tightens it can shorten its label to protect the current location. Preserve the control order and access to navigation, filtering and views. Folder scope belongs in the view picker wherever a folder offers one. Up and CD and release stepping are in the panel menu and keep their shortcuts. Reserve 132 logical pixels for the location and at least 78 for the picker. Together with the four icon buttons, insets and gaps, the browser needs 364 logical pixels across. A tab that shares its group keeps that space when selected.

Filter opens a row below the toolbar, so the field never takes space from the breadcrumb or moves a control. The row stays while a filter hides rows, and Escape or its close button clears and closes it.

## Truncation

Names and titles end in an ellipsis. Middle truncation is kept for folder paths and other identifiers whose ends both matter.

## Color roles

Use `text` for primary labels, `text_muted` for secondary metadata and `text_faint` for column headings, axes and quiet labels. Target at least 4.5:1 for small text. On the dark canvas, faint text measures 6.46:1 at rest, 5.75:1 on the hover tint and 4.71:1 on the selection tint.

The surfaces stay neutral graphite: `chrome` frames the window, `tab_well` recesses behind tabs, `canvas` holds content and `raised` holds the player. The player's timeline lane, `wave.lane`, is the same black the waveform renderer clears to, so the waveform has no visible plate. Artwork and the musical visualizations supply the color. The play button is the one control with a finish of its own, `Materials::silver`: satin metal with machined rim highlights, which lights a thin ring in the accent color while playing.

## Browsing direction

- **Tree** gives the available width to the item name in a minimal hierarchical list. Use a disclosure for expandable rows, restrained indentation and contextual counts or durations. Remove permanent column headers and repetitive metadata. Keep lazy loading, virtualization, keyboard and range selection, drag and drop, expansion, exact reveal, scroll restoration and independent browser history. In Library, simplify verified musical relationships. In Locations, show the physical hierarchy. Matching titles alone cannot establish identity.
- **Tracks** provides a flat track table by default, with optional grouping. Preserve configured column order, widths, pinning, sorting, filtering, selection, contextual playback order and each browser’s saved choices.
- **Releases** centers artwork, release and artist identity, contextual playback actions and a clear disc-to-track hierarchy. Multi-disc releases and CUE chapters keep stable navigation and playback identities. [Browser contracts](unified-music-browser.md) · [Releases](releases.md)

## Workspace and player direction

Listening, Library preparation and Analysis are reference arrangements of the same flexible workspace. Keep groups, splits, multiple independent panel instances, drag targets, keyboard focus and layout restoration available in each. Keep navigation and queue controls reachable while the player spans the full client width.

Give artwork, track identity, Favorite and primary transport clear positions. Keep secondary transport, volume and optional meters accessible. The horizontal player waveform can use the entire window width with either sidebar open. Docked waveforms have independent presentation settings and share the musical clock and analysis data. Future vertical orientation belongs in coordinate mapping and presentation. Preserve source-accurate seeking, pointer-anchored zoom, Track and Full CD scope, continuous playback and queue occurrence identity.

Use list-oriented Queue and History views, readable statistics, and common header, axis and control roles for analyzers. Keep playback, DSP, asynchronous loading, cache ownership and personal-library persistence under their existing domain owners. [Workspace architecture](listening-workspace.md) · [Musical timeline](musical-timeline.md)

## Qualification

`tools/capture_reference_workspaces.py` opens the Listening, Library and Analysis reference workspaces on a private desktop with a seeded fixture library, generated artwork and a synthetic musical CD image. It captures them at 1440 × 900 and 1100 × 720 while the fixture plays silently. At the display's own scale it also records the Music Browser toolbar through a continuous window resize and a continuous divider drag, takes the Analysis workspace through Customize Layout and Done, records the keyboard focus order, and checks that playback continues.

```powershell
python tools/capture_reference_workspaces.py <disqro.exe> <test.exe> <output>
python tools/capture_reference_workspaces.py <disqro.exe> <test.exe> <output> --scale 100 --journeys listening,library,analysis
```

`--scale 100` runs the player DPI-unaware, so it lays out at 96 DPI whatever the display's scale; the tool sizes the window in the display's pixels and keeps only the player's own surface. Other scales need the display itself set to them. The player must be built with the `qualification` feature.

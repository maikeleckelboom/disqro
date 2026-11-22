# Connected workspace qualification

**Native acceptance is pending.** The owner was using the Windows desktop during implementation. The committed [default](evidence/design-review/default.webp), [split browser](evidence/design-review/browsers.webp), [narrow window](evidence/design-review/narrow.webp) and [held drag](evidence/design-review/drag.webp) captures document the preceding interface. The comparison images show two earlier revisions. Use their right-hand images as the baseline for this change.

## What changed

Each GPUI Kit dock group draws the identities of its visible panels. Selected tabs extend to the content edge in the panel's surface color. Utility panes use the sidebar surface. Music Browsers use the table canvas. The browser icon identifies the shell's navigation target, and the focus outline identifies the keyboard target.

The old tab strip placed its expanding drop target directly after the tabs. That pushed the entire browser control group to the right. The selected browser now places its view choices and count beside the tabs. The flexible area **after the count** accepts a tab dropped at the end of the group and anchors Filter at the right edge. It retains at least 8 logical pixels for dropping. Utility-only rows use the existing space after their tabs. Release and CD pages keep their parent links inside the browser.

Each group measures its own labels and reserves editing room when fitting sibling tabs. Opening Filter therefore keeps the same tab widths and switcher while the controls fit inline. The count needs room for its full text. As space decreases, it gives way to the labeled view menu and compact filter button. When the active identity and essential controls need separate rows, the contextual row starts at the left. Regaining 8 logical pixels beyond the wrap threshold returns it to one row. In the tightest splits a short **View** menu also offers **Include subfolders** where applicable, leaving room for the current query.

Back, Forward and Up follow the last active browser. Ctrl+K opens indexed Library search. Ctrl+F opens that browser's filter. Ctrl+L or **Workspace → Go to Folder or File** opens a temporary editor in the title bar. A physical folder supplies its selected path. A virtual destination supplies an empty field. **Copy Address** is available on a physical folder tab's context menu.

## Noninteractive results

For the header refinement on Windows, `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings` passed. The locked test run passed 123 tests, including five header boundary tests. Two opt-in cases were skipped. Both debug and optimized builds succeeded. The optimized executable was built separately from the owner's running copy. Live focus, dragging, pixel alignment and restart checks await native qualification.

The preceding connected-tab revision passed 120 default tests and both opt-in tests, covering FFmpeg decode/seek and the 100,089-track fixture.

## Windows verification procedure

After the owner authorizes native testing, use the optimized build and set `DISQRO_PROFILE_DIR` to a disposable test directory containing a fixture catalog with a physical folder, a playlist and a multi-disc release. Record the build, Windows display scale, logical window size and fixture used. Keep the same fixture and scale for before/after captures. The historical comparison was made at 150% scale.

Capture genuine Windows screenshots at **720 × 460, 900 × 600, 1240 × 780** and a larger available size. Include the full dock boundaries and first content row. Compare the collection's view-to-count spacing and trailing Filter against the [committed album overview](evidence/mission-11-final/albums-before.jpg). Use the supplied connected-header screenshot to compare the position of the flexible space. Capture split, utility and drag states against the corresponding baselines above.

| Scenario | Interaction and expected evidence |
| --- | --- |
| Default shell | At 1240 × 780, capture one browser with its destination, view controls, filter and first table heading. Check the selected tab meets the content edge and the shell shows Library search. Toggle each sidebar independently. |
| Tabbed browsers | Open Liked Tracks, Serato Demo Tracks and Hardcore in one group. Give two tabs different filters. Switch by click and arrow key, drag to reorder, rename a tab and restore its name. Capture the tab group and an active filter. Check each count and table against its own query. |
| Split and mixed groups | Open Beside, then split vertically through Customize Layout. Select a control or tab in each group by mouse and keyboard. Use Ctrl+F, Ctrl+L and history navigation to identify their target. Place a utility panel with a browser, switch to the utility panel, and check which controls disappear. Close the last browser and use Open Music Browser to recover. |
| View, count and Filter | Switch a collection between All Tracks and Albums / CDs. Compare the count to the list. In a physical folder toggle Include subfolders and confirm the selector follows the contents. Check the active tab joins the panel, the views follow the last visible tab, and the flexible area is immediately before Filter. |
| Resize with keyboard focus | With two browsers side by side, enter different queries. Narrow one group one logical pixel at a time around each transition. The count should disappear whole, the selected view should remain reachable and the field should retain its query and caret. Reverse direction several times at the wrap boundary and pause after each step. Check the row count settles, the other browser keeps its selection, and Tab, arrow keys, Enter, Ctrl+F and Escape still reach the intended controls. Repeat while a view choice has focus. |
| Append drop | With three tabs, drag the first tab to the space before Filter. Check it lands last. Repeat with a tab from the other group, with the field open near the width threshold, and on the spare space in a stacked identity row. Check the held preview, resulting order and Undo Layout Change. |
| Physical path entry | Capture Ctrl+L with the actual folder path selected. Enter a quoted path, an environment-variable path, a drive root, a reachable UNC share, an audio file and a CUE sheet. Verify the resulting folder or file selection. Try a missing path and an unsupported file. Read the error and correct the input. Escape should return focus to the list and preserve history. |
| Virtual path entry | From a collection, playlist, release and CD, open the editor from the menu and keyboard. Capture the blank field with “Enter a folder or file path…”. Cancel. Submit a slow path, switch browser before it resolves, and check both histories for unwanted navigation. |
| Release hierarchy | Open a release from its collection and then CD 2. Use its parent links, Back, Forward and Up in both browsers. Check the destination and restored selection. Ordinary playlists and collections need only their local identity. |
| Utilities and layout | Capture Library/Locations and Queue/History as tabs. Drag each pair into a vertical stack, reverse the order, regroup and undo. Capture a held preview and a lone stacked header. Hide and restore both sidebars. Verify their arrangements survive. |
| Global search | From a filtered browser use Ctrl+K. Capture grouped catalog suggestions, open full results, then dismiss and revisit the local filter. Compare both browsers' queries. |
| Narrow window and caption | At 720 × 460 repeat with each sidebar visible, hidden and stacked. Resize a split across the wrapping threshold with its filter open. Check focus, full destination tooltips, the panel switcher and view selector. Drag and double-click the brand and free caption space. Verify editor, buttons and search receive client clicks. Capture the narrow layout and caption movement. |
| Albums and persistence | Capture Albums / CDs with the independent Cover column. Resize, hide and reorder it. Resize the window and inspect header-to-row alignment and the right edge. Restart the fixture profile and compare column settings, tab order, split positions, histories, filters, sidebar visibility and destinations. |
| Playback | Play a known CUE chapter while filtering, switching tabs and changing layout. Compare source identity, queue order and transport position. Record muted callback diagnostics separately from any authorized audible listening check. |

Save the new native captures separately from the baselines. Record observed results and any failures beside the captures before granting visual acceptance.

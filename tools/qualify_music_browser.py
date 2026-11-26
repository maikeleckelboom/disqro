"""Qualify Music Browser interactions in a silent, private Windows desktop.

The fixture uses a fresh indexed library and workspace. All input is sent
only to the window created by this run.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time
import traceback

from private_desktop import run
from qualify_spectrum import click_control, control_centers, find_window, screenshot
from ctypes import wintypes as W


def panel(name, identity, **state):
    return dict(panel_name="disqro." + name, children=[], info=dict(panel=dict(id=identity, state=state)))


def tabs(*children):
    return dict(panel_name="TabPanel", children=children, info=dict(tabs=dict(active_index=0)))


def profile(root, width, mode="tracks", query=""):
    primary = tabs(panel("browser", 1, current=dict(location=dict(Library=root), mode=mode, query=query)))
    children = [tabs(panel("library", 2), panel("locations", 3)), primary]
    sizes = [240, width]
    if width < 950:
        children.append(tabs(panel("browser", 4, current=dict(location="Liked", mode="tracks", query=""))))
        sizes.append(1000 - width)
    center = dict(panel_name="StackPanel", children=children, info=dict(stack=dict(axis=0, sizes=sizes)))
    return dict(layout=dict(version=3, center=center), workspace=dict(active=1), window=dict(close_to_tray=False))


def wait_for(process, label=None, min_y=None, timeout=12, *, min_x=None):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline and process.poll() is None:
        hwnd = find_window(process.pid)
        if hwnd and label is None:
            return hwnd
        if hwnd:
            try:
                return hwnd, control_centers(hwnd, [label], min_y=min_y, min_x=min_x)[0]
            except subprocess.CalledProcessError as exc:
                last_error = exc
        time.sleep(0.1)
    raise RuntimeError(f"Native window did not show {label!r}") from last_error


def click(process, label, **bounds):
    hwnd, point = wait_for(process, label, **bounds)
    click_control(hwnd, point)
    return hwnd


def close(process):
    hwnd = find_window(process.pid)
    if hwnd:
        C.windll.user32.PostMessageW(hwnd, 0x0010, 0, 0)
    try:
        process.wait(10)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(5)
        raise RuntimeError("Disqro did not close and save the fixture workspace")


def launch(exe, output, settings=None):
    if settings is not None:
        (output / "workspace-v1.json").write_text(json.dumps(settings), encoding="utf-8")
    with (output / "native.log").open("ab") as log:
        return subprocess.Popen([exe, "--qualify-silent"], env=dict(os.environ, DISQRO_PROFILE_DIR=str(output)),
                                stdout=log, stderr=log)


def browser_states(profile_path):
    def walk(node):
        if node["panel_name"] == "disqro.browser":
            yield node["info"]["panel"]
        for child in node["children"]:
            yield from walk(child)
    workspace = json.loads(profile_path.read_text(encoding="utf-8"))
    return {panel["id"]: panel["state"] for panel in walk(workspace["layout"]["center"])}


def right_click(hwnd, point):
    C.windll.user32.ScreenToClient(hwnd, C.byref(point))
    position = (point.y << 16) | (point.x & 0xffff)
    for message, keys in [(0x200, 0), (0x204, 2), (0x205, 0)]:
        C.windll.user32.SendMessageW(hwnd, message, keys, position)


def favorites_journey(exe, output):
    workspace_file = output / "workspace-v1.json"
    settings = profile(None, 1000)
    pane = settings["layout"]["center"]["children"][1]["children"][0]["info"]["panel"]["state"]
    title = dict(field="Title", width=405, pin="None")
    favorite = dict(field="Favorite", width=96, pin="Right")
    mixed = dict(columns=[title, dict(field="Artist", width=140, pin="None"), favorite], adaptive=False)
    pane["layouts"] = dict(version=3, disc=dict(columns=[title]), mixed=mixed, playlist=dict(columns=[title]))
    process = launch(exe, output, settings)
    try:
        hwnd, _ = wait_for(process, "Favorite")
        screenshot(hwnd, output / "favorite-header.png")
        click(process, "Favorites")
        wait_for(process, "Opening Notes")
        screenshot(hwnd, output / "favorites.png")
    finally:
        close(process)
    saved = browser_states(workspace_file)[1]
    assert saved["current"]["location"] == "Liked", saved["current"]
    assert saved["layouts"]["mixed"] == mixed, saved["layouts"]["mixed"]
    assert saved["layouts"]["favorites"]["columns"] == mixed["columns"][:-1]
    assert not saved["layouts"]["favorites"].get("hidden")

    with sqlite3.connect(output / "library.sqlite3") as db:
        selected = list(db.execute("SELECT t.id, t.title FROM likes l JOIN tracks t ON t.id = l.track_id ORDER BY t.id"))
        assert len(selected) == 2, selected
        ratings = list(db.execute("SELECT track_id, stars FROM ratings"))

    workspace = json.loads(workspace_file.read_text(encoding="utf-8"))
    def set_selection(node):
        if node["panel_name"] == "disqro.browser":
            current = node["info"]["panel"]["state"]["current"]
            if current["location"] == "Liked":
                first = dict(Key=selected[0][0])
                current["bookmark"] = dict(marked=dict(
                    anchor=first, cursor=first, marked=[dict(Key=key) for key, _ in selected]))
        for child in node["children"]:
            set_selection(child)
    set_selection(workspace["layout"]["center"])
    process = launch(exe, output, workspace)
    try:
        hwnd, point = wait_for(process, selected[0][1])
        right_click(hwnd, point)
        click(process, "Remove from Favorites")
        wait_for(process, "No favorites yet. Add a track from its menu or with Alt+L.")
        click(process, "All Music")
        wait_for(process, "Favorite")
    finally:
        close(process)

    with sqlite3.connect(output / "library.sqlite3") as db:
        assert not list(db.execute("SELECT track_id FROM likes"))
        assert list(db.execute("SELECT track_id, stars FROM ratings")) == ratings
    assert browser_states(workspace_file)[1]["layouts"]["mixed"] == mixed


def toolbar_geometry(hwnd):
    script = r"""
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$ErrorActionPreference = 'Stop'
$root = [Windows.Automation.AutomationElement]::FromHandle([IntPtr][long]$env:DISQRO_RESIZE_WINDOW)
foreach ($label in @("Location", "Earlier locations", "Filter this view")) {
    $match = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $label)
    foreach ($entry in $root.FindAll([Windows.Automation.TreeScope]::Descendants, $match)) {
        $bounds = $entry.Current.BoundingRectangle
        if ($bounds.Width -gt 0 -and $bounds.Height -gt 0) {
            "{0}|{1}|{2}" -f $label, [int]$bounds.X, [int]$bounds.Width
        }
    }
}
"""
    probe = subprocess.run(
        ["powershell", "-NoProfile", "-Command", script],
        env=dict(os.environ, DISQRO_RESIZE_WINDOW=str(hwnd)),
        creationflags=subprocess.CREATE_NO_WINDOW, timeout=15, text=True, capture_output=True,
    )
    if probe.returncode:
        raise RuntimeError(probe.stderr.strip())
    entries = [line.split("|") for line in probe.stdout.splitlines()]
    location = max((entry for entry in entries if entry[0] == "Location"), key=lambda entry: int(entry[1]))
    x, width = map(int, location[1:3])
    earlier = any(entry[0] == "Earlier locations" and int(entry[1]) >= x for entry in entries)
    filters = [entry for entry in entries if entry[0] == "Filter this view" and int(entry[1]) >= x]
    filter_entry = max(filters, key=lambda entry: int(entry[2])) if filters else None
    return dict(address_width=width, earlier_locations=earlier,
                filter_width=int(filter_entry[2]) if filter_entry else 0)


def resize_journey(exe, output, root):
    settings = profile(root, 510)
    center = settings["layout"]["center"]
    primary, sibling = center["children"][1:]
    primary["children"][0]["info"]["panel"]["state"]["current"].update(location="Liked", query="Opening")
    sibling["children"][0]["info"]["panel"]["state"]["current"]["location"] = {"Library": None}
    center["children"][1:] = [sibling, primary]
    center["info"]["stack"]["sizes"] = [240, 490, 510]
    process = launch(exe, output, settings)
    try:
        hwnd, _ = wait_for(process, "Opening Notes")
        rect = W.RECT()
        assert C.windll.user32.GetWindowRect(hwnd, C.byref(rect))
        scale = C.windll.user32.GetDpiForWindow(hwnd) / 96
        height = rect.bottom - rect.top
        states = []
        for width in range(1240, 749, -35):
            assert C.windll.user32.SetWindowPos(hwnd, 0, 0, 0, round(width * scale), height, 0x414)
            # Let the native size notification reach the dock and the UIA tree.
            time.sleep(0.12)
            state = toolbar_geometry(hwnd)
            states.append(dict(window_width=width, **state))
            if width in (1100, 1065, 1030):
                screenshot(hwnd, output / f"favorites-resize-{width}.png")
        widths = [s["address_width"] for s in states]
        folded = [s["earlier_locations"] for s in states]
        assert all(after <= before + 1 for before, after in zip(widths, widths[1:])), states
        assert any(folded) and not all(folded) and folded == sorted(folded), states
        inline = [s["filter_width"] > 0 for s in states]
        assert inline[0] and inline == sorted(inline, reverse=True), states
        (output / "live-resize.json").write_text(json.dumps(states, indent=2), encoding="utf-8")
    finally:
        close(process)
    assert browser_states(output / "workspace-v1.json")[1]["current"]["query"] == "Opening"


def journey(exe, test_exe, output):
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(output)), check=True)
    root = int((output / "reference-root.txt").read_text(encoding="ascii"))
    process = launch(exe, output, profile(root, 1000))
    try:
        hwnd, _ = wait_for(process, "Browse Collections")
        wait_for(process, "Browse Playlists")
        wait_for(process, "Tracks")
        screenshot(hwnd, output / "library-sidebar.png")

        _, sidebar = wait_for(process, "Browse Collections")
        click(process, "Releases", min_x=sidebar.x + 180)
        wait_for(process, "Album / Disc")
        time.sleep(0.25)
        screenshot(hwnd, output / "releases.png")

        click(process, "Tree")
        click(process, "Expand Live Set")
        click(process, "Expand Live Set")
        click(process, "Expand CD 1")
        wait_for(process, "Collapse CD 1")
        time.sleep(0.25)
        screenshot(hwnd, output / "expanded-tree.png")
        click(process, "Collapse CD 1")
        wait_for(process, "Expand CD 1")
        click(process, "Expand CD 1")
        wait_for(process, "Collapse CD 1")
    finally:
        close(process)

    saved = browser_states(output / "workspace-v1.json")[1]
    assert saved["current"]["location"] == {"Library": root}
    assert saved["current"]["mode"] == "tree"
    expanded = saved["current"]["tree_expanded"]
    assert len(expanded) == 3, expanded
    assert not saved.get("back") and not saved.get("forward"), "Expanding a branch changed navigation history"

    process = launch(exe, output)
    try:
        hwnd, _ = wait_for(process, "Collapse CD 1")
        screenshot(hwnd, output / "restored-tree.png")
        hwnd, disc = wait_for(process, "CD 1")
        click_control(hwnd, disc)
        click_control(hwnd, disc)
        wait_for(process, "Recordings")
        hwnd, back = wait_for(process, "Back · Reference Library · Alt+Left")
        click_control(hwnd, back)
        wait_for(process, "Collapse CD 1")
        dpi = C.windll.user32.GetDpiForWindow(hwnd)
        forward = W.POINT(back.x + round(27 * dpi / 96), back.y)
        click_control(hwnd, forward)
        wait_for(process, "Recordings")
        click_control(hwnd, back)
        wait_for(process, "Collapse CD 1")
        hwnd, chapter = wait_for(process, "Chapter 01")
        click_control(hwnd, chapter)
        click_control(hwnd, chapter)
        wait_for(process, "Pause")
        click(process, "Releases")
        wait_for(process, "Album / Disc")
        click(process, "Go to Playing Track")
        wait_for(process, "Pause")
        screenshot(hwnd, output / "reveal-release.png")
    finally:
        close(process)

    previous = browser_states(output / "workspace-v1.json")[1]["current"]
    assert previous["location"] == "Releases", previous
    assert "Release" in previous["bookmark"]["selected"], previous
    saved_queue = json.loads((output / "workspace-v1.json").read_text(encoding="utf-8"))["queue"]
    queue = saved_queue["queue"]
    playing = queue["entries"][queue["current"]]["track"]
    chapter = playing["disc"]
    with sqlite3.connect(output / "library.sqlite3") as db:
        target_id, = db.execute(
            """SELECT t.id FROM tracks t JOIN files a ON a.id = t.anchor
               JOIN files s ON s.id = t.source WHERE a.path_key = ? AND s.path_key = ?
               AND t.part = ? AND t.first_tick = ? AND t.last_tick IS ? AND t.present""",
            (os.path.normpath(chapter["sheet"]).lower(), os.path.normpath(playing["source"]).lower(),
             chapter["number"], chapter["start"], chapter.get("end")),
        ).fetchone()
    tree = profile(root, 1000, "tree", "hide all recordings")
    tree["queue"] = saved_queue
    process = launch(exe, output, tree)
    try:
        hwnd = click(process, "Go to Playing Track")
        wait_for(process, playing["title"])
        screenshot(hwnd, output / "reveal-tree.png")
    finally:
        close(process)
    restored = browser_states(output / "workspace-v1.json")[1]
    assert restored["current"]["location"] == {"Library": root}, restored
    assert restored["current"]["mode"] == "tree" and not restored["current"].get("query"), restored
    assert restored["current"]["bookmark"]["selected"]["Tree"] == {"IndexedTrack": target_id}, restored
    assert restored["back"][-1]["query"] == "hide all recordings", restored
    after = json.loads((output / "workspace-v1.json").read_text(encoding="utf-8"))["queue"]["queue"]
    assert after["entries"] == queue["entries"] and after["current"] == queue["current"]

    for name, width, label, query in [
        ("wide", 760, "Releases", ""),
        ("medium", 505, "Releases", ""),
        ("narrow", 375, "Choose view · Releases", ""),
        ("filtered-narrow", 418, "Choose view · Releases", "Chapter 02"),
        ("extreme", 240, "Panel menu", "Chapter 02"),
    ]:
        process = launch(exe, output, profile(root, width, "albums", query))
        try:
            hwnd, _ = wait_for(process, label)
            screenshot(hwnd, output / f"toolbar-{name}.png")
        finally:
            close(process)
        saved = browser_states(output / "workspace-v1.json")
        assert saved[1]["current"]["mode"] == "albums"
        assert saved[1]["current"]["query"] == query
        assert saved[4]["current"]["location"] == "Liked"
        assert saved[4]["current"]["mode"] == "tracks"
    resize_journey(exe, output, root)
    favorites_journey(exe, output)
    (output / "result.json").write_text(json.dumps(dict(expanded_nodes=len(expanded),
        route_preserved=True, restart_restored=True, back_forward_restored=True, playback_continued=True,
        reveal_release=True, reveal_tree_clears_filter_with_history=True,
        independent_splits=True, responsive_cases=5,
        favorites_column_suppressed=True, multi_remove_preserved_ratings=True, live_resize=True), indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("test_exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            journey(args.exe, args.test_exe, args.output)
        except BaseException:
            output = args.output if args.output.is_dir() else args.output.parent
            (output / "qualification-error.log").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.test_exe),
             str(args.output), "--worker"], 225)

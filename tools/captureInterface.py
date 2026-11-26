"""Capture Disqro's navigation surfaces in a silent, private Windows desktop.

Seeds a fresh fixture library and workspace, opens Library, Locations and a
Music Browser side by side, and saves native captures of their rows,
disclosures and hover states. Every input goes only to the window this run
creates; playback stays silent and the listener's own profile is untouched.

    python tools/captureInterface.py <disqro.exe> <test.exe> <output> [--scale 100|native]
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
from ctypes import wintypes as W

from privateDesktop import run
from qualifyMusicBrowser import close, panel, tabs, wait_for
from qualifySpectrum import click_control, find_window, screenshot

user32 = C.windll.user32


def control_rects(hwnd, labels, min_y=None, *, focus=False):
    """Find the first control named each label and optionally focus it."""
    script = r"""
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [Windows.Automation.AutomationElement]::FromHandle([IntPtr]HWND)
foreach ($label in (ConvertFrom-Json $env:DISQRO_CONTROL_LABELS)) {
    $match = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $label)
    $found = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $match)
    $element = $null
    for ($i=0; $i -lt $found.Count; $i++) {
        $candidate = $found.Item($i)
        if (-not $env:DISQRO_CONTROL_MIN_Y -or $candidate.Current.BoundingRectangle.Y -ge [int]$env:DISQRO_CONTROL_MIN_Y) {
            $element = $candidate
            break
        }
    }
    if ($null -eq $element) { throw "Missing native control $label" }
    if ($env:DISQRO_CONTROL_FOCUS -eq '1') {
        if (-not $element.Current.IsKeyboardFocusable) { throw "Cannot focus $label" }
        $element.SetFocus()
    }
    $r = $element.Current.BoundingRectangle
    Write-Output ([int]$r.X)
    Write-Output ([int]$r.Y)
    Write-Output ([int]$r.Width)
    Write-Output ([int]$r.Height)
}
"""
    output = subprocess.check_output(
        ["powershell", "-NoProfile", "-Command", script.replace("HWND", str(hwnd))],
        env=dict(os.environ, DISQRO_CONTROL_LABELS=json.dumps(labels),
                 DISQRO_CONTROL_MIN_Y="" if min_y is None else str(min_y),
                 DISQRO_CONTROL_FOCUS="1" if focus else "0"),
        creationflags=subprocess.CREATE_NO_WINDOW, timeout=20)
    values = list(map(int, output.split()))
    return [tuple(values[i:i + 4]) for i in range(0, len(values), 4)]


def try_rect(hwnd, label, min_y=None):
    try:
        return control_rects(hwnd, [label], min_y)[0]
    except subprocess.CalledProcessError:
        return None


def hover(hwnd, x, y):
    p = W.POINT(x, y)
    user32.ScreenToClient(hwnd, C.byref(p))
    user32.SendMessageW(hwnd, 0x200, 0, (p.y << 16) | (p.x & 0xffff))


def keyboard_disclosures(process, hwnd):
    """Focused disclosure keys toggle the branch without leaving the Tree view."""
    for before, after, key in (
        ("Collapse Collections", "Expand Collections", 0x20),
        ("Expand Collections", "Collapse Collections", 0x0D),
        ("Collapse Reference Library", "Expand Reference Library", 0x0D),
        ("Expand Reference Library", "Collapse Reference Library", 0x20),
    ):
        control_rects(hwnd, [before], focus=True)
        user32.PostMessageW(hwnd, 0x0100, key, 0)
        user32.PostMessageW(hwnd, 0x0101, key, 0xC0000000)
        wait_for(process, after)
        wait_for(process, "Tree")


def resize(hwnd, width, height):
    dpi = user32.GetDpiForWindow(hwnd) or 96
    user32.SetWindowPos(hwnd, None, 40, 40, round(width * dpi / 96), round(height * dpi / 96), 0x0014)
    return dpi


def layout(root, sidebar, browser_width, mode="tree", expanded=None, location=None):
    current = dict(location=location if location is not None else dict(Library=None), mode=mode, query="")
    if expanded:
        current["tree_expanded"] = expanded
    sidebar_column = dict(panel_name="StackPanel",
                          children=[tabs(panel("library", 2)), tabs(panel("locations", 3))],
                          info=dict(stack=dict(axis=1, sizes=[360, 300])))
    browser = tabs(panel("browser", 1, current=current))
    center = dict(panel_name="StackPanel", children=[sidebar_column, browser],
                  info=dict(stack=dict(axis=0, sizes=[sidebar, browser_width])))
    return dict(volume=0.0, layout=dict(version=3, center=center), workspace=dict(active=1),
                window=dict(close_to_tray=False))


def launch(exe, output, settings, scale):
    (output / "workspace-v1.json").write_text(json.dumps(settings), encoding="utf-8")
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(output))
    if scale == "100":
        # Windows renders a DPI-unaware process at 96 DPI.
        env["__COMPAT_LAYER"] = "DPIUNAWARE"
    with (output / "native.log").open("ab") as log:
        return subprocess.Popen([exe, "--qualify-silent"], env=env, stdout=log, stderr=log)


def capture_states(process, output, prefix, results):
    hwnd, _ = wait_for(process, "Browse Collections")
    dpi = resize(hwnd, 1240, 780)
    time.sleep(0.8)
    results[f"{prefix}dpi"] = dpi
    screenshot(hwnd, output / f"{prefix}overview.png")
    rects = {}
    for name in ["Collapse Collections", "Collapse Playlists", "Browse Collections", "Browse Playlists",
                 "Add Music Folder…", "New Playlist…", "Reference Library", "Evening Session",
                 "All Music", "Artists", "Releases", "Favorites", "Recently Played",
                 "Collapse Reference Library", "Collapse Live Set"]:
        rect = try_rect(hwnd, name)
        if rect:
            rects[name] = rect
    results[f"{prefix}rects"] = rects
    for name, file in [("Collapse Collections", "library-chevron-hover"),
                       ("Collapse Reference Library", "tree-chevron-hover"),
                       ("Collapse Live Set", "tree-chevron-hover-nested")]:
        rect = rects.get(name)
        if rect:
            hover(hwnd, rect[0] + rect[2] // 2, rect[1] + rect[3] // 2)
            time.sleep(0.3)
            screenshot(hwnd, output / f"{prefix}{file}.png")
    heading = rects.get("Browse Collections")
    if heading:
        hover(hwnd, heading[0] + heading[2] - round(60 * dpi / 96), heading[1] + heading[3] // 2)
        time.sleep(0.3)
        screenshot(hwnd, output / f"{prefix}library-heading-hover.png")
    # A Locations place opened beside the others.
    places = [label for label in ("Music", "Home", "Desktop", "Documents") if try_rect(hwnd, label)]
    results[f"{prefix}places"] = places
    if places:
        rect = try_rect(hwnd, "Expand " + places[0]) or try_rect(hwnd, places[0])
        if rect:
            x = rect[0] + rect[2] // 2 if rect[2] < 40 * dpi / 96 else rect[0] - round(30 * dpi / 96)
            hover(hwnd, x, rect[1] + rect[3] // 2)
            time.sleep(0.3)
            screenshot(hwnd, output / f"{prefix}locations-chevron-hover.png")
    hover(hwnd, 5, 5)
    if not prefix:
        keyboard_disclosures(process, hwnd)
        results["keyboard_disclosures"] = True
    # Collapsed sections.
    for name in ["Collapse Collections", "Collapse Playlists"]:
        rect = try_rect(hwnd, name)
        if rect:
            click_control(hwnd, W.POINT(rect[0] + rect[2] // 2, rect[1] + rect[3] // 2))
            time.sleep(0.3)
    hover(hwnd, 5, 5)
    time.sleep(0.4)
    screenshot(hwnd, output / f"{prefix}collapsed.png")
    return hwnd


def capture_destinations(exe, output, scale, root):
    settings = layout(root, 236, 1004, mode="tracks", location=dict(Library=root))
    process = launch(exe, output, settings, scale)
    try:
        hwnd, _ = wait_for(process, "Opening Notes")
        resize(hwnd, 1240, 780)
        for destination, content in [
            ("Artists", "Various Artists"), ("Releases", "Album Artist"),
        ]:
            hwnd, point = wait_for(process, destination)
            click_control(hwnd, point)
            wait_for(process, content)
            screenshot(hwnd, output / f"{destination.lower()}.png")
        hwnd, point = wait_for(process, "Artists")
        click_control(hwnd, point)
        hwnd, point = wait_for(process, "Various Artists")
        click_control(hwnd, W.POINT(point.x, point.y))
        click_control(hwnd, point)
        wait_for(process, "Tracks")
        screenshot(hwnd, output / "album-artist-tracks.png")
        rects = control_rects(hwnd, ["Releases"], min_y=0)
        # The toolbar's presentation is to the right of the sidebar label.
        title, _, width, _ = rects[0]
        hwnd, point = wait_for(process, "Releases", min_x=title + width)
        click_control(hwnd, point)
        wait_for(process, "Live Set")
        screenshot(hwnd, output / "album-artist-releases.png")
    finally:
        close(process)


def journey(exe, test_exe, output, scale):
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(output)), check=True,
                   stdout=subprocess.DEVNULL)
    root = int((output / "reference-root.txt").read_text(encoding="ascii"))
    release = str(output / "Reference Library" / "Live Set").lower()
    expanded = [dict(Collection=root), dict(Folder=[root, release])]
    results = {}
    for prefix, sidebar in [("", 260), ("narrow-", 170), ("wide-", 340)]:
        process = launch(exe, output, layout(root, sidebar, 1240 - sidebar, expanded=expanded), scale)
        try:
            capture_states(process, output, prefix, results)
        finally:
            close(process)
    capture_destinations(exe, output, scale, root)
    (output / "result.json").write_text(json.dumps(results, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("test_exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--scale", choices=["100", "native"], default="native")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            journey(args.exe, args.test_exe, args.output, args.scale)
        except BaseException:
            output = args.output if args.output.is_dir() else args.output.parent
            (output / "capture-error.log").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.test_exe),
             str(args.output), "--scale", args.scale, "--worker"], 300)

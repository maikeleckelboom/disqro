"""Qualify the Library sidebar sections in a silent, private Windows desktop.

The fixture uses a fresh indexed library and workspace. All input is sent
only to the window created by this run.
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

from private_desktop import run
from qualify_music_browser import browser_states, close, launch, profile, wait_for
from qualify_spectrum import click_control, control_centers, find_window, screenshot
from ctypes import wintypes as W


def library_state(profile_path):
    def walk(node):
        if node["panel_name"] == "disqro.library":
            yield node["info"]["panel"]["state"]
        for child in node["children"]:
            yield from walk(child)
    workspace = json.loads(profile_path.read_text(encoding="utf-8"))
    return next(walk(workspace["layout"]["center"]))


def present(process, label):
    hwnd = find_window(process.pid)
    try:
        control_centers(hwnd, [label])
        return True
    except subprocess.CalledProcessError:
        return False


def hover(hwnd, target):
    p = W.POINT(target.x, target.y)
    C.windll.user32.ScreenToClient(hwnd, C.byref(p))
    C.windll.user32.SendMessageW(hwnd, 0x200, 0, (p.y << 16) | (p.x & 0xffff))


def location(output):
    return browser_states(output / "workspace-v1.json")[1]["current"]["location"]


def journey(exe, test_exe, output):
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(output)), check=True)
    root = int((output / "reference-root.txt").read_text(encoding="ascii"))
    results = {}
    process = launch(exe, output, profile(root, 1000))
    try:
        hwnd, heading = wait_for(process, "Browse Collections")
        wait_for(process, "Collapse Playlists")
        time.sleep(0.3)
        screenshot(hwnd, output / "sidebar.png")
        dpi = C.windll.user32.GetDpiForWindow(hwnd)
        # The heading's remaining horizontal area, well right of its label.
        hwnd, chevron = wait_for(process, "Collapse Collections")
        _, add = wait_for(process, "Add Music Folder…")
        empty = W.POINT((heading.x + add.x) // 2 + round(20 * dpi / 96), heading.y)
        hover(hwnd, empty)
        time.sleep(0.25)
        screenshot(hwnd, output / "heading-hover.png")
        hover(hwnd, chevron)
        time.sleep(0.25)
        screenshot(hwnd, output / "chevron-hover.png")

        # Start playback, then open an overview: the section stays expanded.
        click_control(hwnd, wait_for(process, "Chapter 01")[1])
        click_control(hwnd, wait_for(process, "Chapter 01")[1])
        wait_for(process, "Pause")
        click_control(hwnd, empty)
        wait_for(process, "Collapse Collections")
        time.sleep(0.4)
        screenshot(hwnd, output / "overview-selected.png")
        assert present(process, "Pause"), "Opening an overview interrupted playback"

        # The chevron collapses without navigating or stopping playback.
        click_control(hwnd, wait_for(process, "Collapse Collections")[1])
        wait_for(process, "Expand Collections")
        assert present(process, "Pause"), "Collapsing interrupted playback"
        click_control(hwnd, wait_for(process, "Collapse Playlists")[1])
        wait_for(process, "Expand Playlists")
        assert not present(process, "Evening Session"), "A collapsed section kept its rows"
        time.sleep(0.3)
        screenshot(hwnd, output / "collapsed.png")
    finally:
        close(process)
    saved = library_state(output / "workspace-v1.json")
    assert saved.get("collections_closed") and saved.get("playlists_closed"), saved
    assert location(output) == "Collections", location(output)
    browser = browser_states(output / "workspace-v1.json")[1]
    results["history_after_collapse"] = len(browser.get("back", []))

    # Restart restores both sections.
    process = launch(exe, output)
    try:
        hwnd, _ = wait_for(process, "Expand Collections")
        wait_for(process, "Expand Playlists")
        time.sleep(0.3)
        screenshot(hwnd, output / "restored.png")
        click_control(hwnd, wait_for(process, "Expand Collections")[1])
        wait_for(process, "Collapse Collections")
        hwnd, heading = wait_for(process, "Browse Collections")
        click_control(hwnd, wait_for(process, "Expand Playlists")[1])
        wait_for(process, "Collapse Playlists")
        # A selected child is distinct from a selected overview.
        click_control(hwnd, wait_for(process, "Reference Library", min_y=heading.y)[1])
        time.sleep(0.4)
        screenshot(hwnd, output / "child-selected.png")
    finally:
        close(process)
    saved = library_state(output / "workspace-v1.json")
    assert not saved.get("collections_closed") and not saved.get("playlists_closed"), saved
    assert location(output) == {"Library": root}, location(output)
    results.update(collapse_persisted=True, overview_without_toggle=True, playback_continued=True)
    (output / "result.json").write_text(json.dumps(results, indent=2), encoding="utf-8")


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
             str(args.output), "--worker"], 150)

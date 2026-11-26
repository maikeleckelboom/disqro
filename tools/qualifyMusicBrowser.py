"""Qualify Music Browser interactions in a silent, private Windows desktop.

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

from privateDesktop import run
from qualifySpectrum import click_control, control_centers, find_window, screenshot
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


def wait_for(process, label=None, min_y=None, timeout=12):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline and process.poll() is None:
        hwnd = find_window(process.pid)
        if hwnd and label is None:
            return hwnd
        if hwnd:
            try:
                return hwnd, control_centers(hwnd, [label], min_y=min_y)[0]
            except subprocess.CalledProcessError as exc:
                last_error = exc
        time.sleep(0.1)
    raise RuntimeError(f"Native window did not show {label!r}") from last_error


def click(process, label):
    hwnd, point = wait_for(process, label)
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


def journey(exe, test_exe, output):
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(output)), check=True)
    root = int((output / "reference-root.txt").read_text(encoding="ascii"))
    process = launch(exe, output, profile(root, 1000))
    try:
        hwnd, _ = wait_for(process, "Browse Collections")
        wait_for(process, "Browse Playlists")
        wait_for(process, "All Tracks")
        screenshot(hwnd, output / "library-sidebar.png")

        click(process, "Releases")
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
        wait_for(process, "Pause")
    finally:
        close(process)

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
    (output / "result.json").write_text(json.dumps(dict(expanded_nodes=len(expanded),
        route_preserved=True, restart_restored=True, back_forward_restored=True, playback_continued=True,
        independent_splits=True, responsive_cases=5), indent=2), encoding="utf-8")


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

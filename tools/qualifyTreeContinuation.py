"""Qualify Tree view continuation beyond one page in a silent, private desktop.

The fixture folder holds more subfolders and recordings than one page lists.
All input is sent only to the window created by this run.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import struct
import sys
import time
import traceback

from privateDesktop import run
from qualifyMusicBrowser import browser_states, close, launch, panel, tabs, wait_for
from qualifySpectrum import click_control, control_centers, find_window, screenshot
from ctypes import wintypes as W

FOLDERS = 4200
RECORDINGS = 4300


def silent_wav(path):
    # One silent stereo frame: the smallest recording the tag reader accepts.
    header = b"RIFF" + struct.pack("<I", 40) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 2, 8000, 32000, 4, 16)
    path.write_bytes(header + b"data" + struct.pack("<I", 4) + bytes(4))


def profile(folder, query="", location=None):
    location = location or dict(Folder=str(folder))
    browser = panel("browser", 1, current=dict(location=location, mode="tree", query=query))
    children = [tabs(panel("library", 2), panel("locations", 3)), tabs(browser)]
    center = dict(panel_name="StackPanel", children=children, info=dict(stack=dict(axis=0, sizes=[240, 1000])))
    return dict(layout=dict(version=3, center=center), workspace=dict(active=1), window=dict(close_to_tray=False))


def present(hwnd, label):
    try:
        control_centers(hwnd, [label])
        return True
    except Exception:
        return False


def scroll_to_end(hwnd, anchor):
    # Large wheel messages to this window; each moves several hundred rows.
    p = W.POINT(anchor.x, anchor.y)
    for _ in range(24):
        C.windll.user32.SendMessageW(hwnd, 0x20A, (-120 * 4000 & 0xFFFF) << 16, (p.y << 16) | (p.x & 0xFFFF))
        time.sleep(0.15)


def journey(exe, test_exe, output):
    output.mkdir(parents=True, exist_ok=False)
    folder = output / "Large Folder"
    folder.mkdir()
    for i in range(FOLDERS):
        (folder / f"Folder {i:04}").mkdir()
    for i in range(RECORDINGS):
        silent_wav(folder / f"Track {i:04}.wav")
    process = launch(exe, output, profile(folder))
    results = {}
    try:
        hwnd, first = wait_for(process, "Folder 0000", timeout=30)
        assert not present(hwnd, "Show more"), "Loading asked the listener for input"
        # Scrolling alone reaches every entry; batches load ahead of the viewport.
        started = time.monotonic()
        deadline = started + 60
        while not present(hwnd, "Track 4299"):
            assert time.monotonic() < deadline, "Scrolling did not reach the last entry"
            scroll_to_end(hwnd, first)
        results["last_entry_seconds"] = round(time.monotonic() - started, 3)
        assert not present(hwnd, "Show more")
        time.sleep(0.3)
        screenshot(hwnd, output / "last-entry.png")
    except BaseException:
        hwnd = find_window(process.pid)
        if hwnd:
            screenshot(hwnd, output / "failure.png")
        raise
    finally:
        close(process)
    saved = browser_states(output / "workspace-v1.json")[1]
    assert saved["current"]["mode"] == "tree", saved
    results.update(folders=FOLDERS, recordings=RECORDINGS, last_entry_reached=True)

    # A folder on disk filters what is open and says so; it never scans.
    process = launch(exe, output, profile(folder, query="Folder 0001"))
    try:
        # The scope is told by the filter control, not by table rows.
        hwnd, _ = wait_for(process, "Filter open folders · Unopened folders are not searched")
        wait_for(process, "Folder 0001")
        assert not present(hwnd, "Filtering open folders only")
        assert not present(hwnd, "Folder 0002")
        time.sleep(0.3)
        screenshot(hwnd, output / "physical-filter.png")
    finally:
        close(process)

    # An indexed collection keeps collapsed folders whose recordings match.
    indexed = output / "indexed"
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(indexed)), check=True)
    root = int((indexed / "reference-root.txt").read_text(encoding="ascii"))
    process = launch(exe, indexed, profile(None, query="Chapter 02", location=dict(Library=root)))
    try:
        hwnd, _ = wait_for(process, "Expand Live Set", timeout=20)
        assert not present(hwnd, "Opening Notes"), "A non-matching recording stayed visible"
        time.sleep(0.3)
        screenshot(hwnd, output / "indexed-filter.png")
    finally:
        close(process)
    results.update(filter_scope_in_control=True, indexed_descendant_match=True)
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
             str(args.output), "--worker"], 300)

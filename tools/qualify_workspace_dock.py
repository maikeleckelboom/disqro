"""Exercise mixed Queue tabs, duplication, reordering and a bottom split on a private desktop."""
import argparse
import json
from pathlib import Path
import sys
import traceback
from ctypes import wintypes as W

import capture_reference_workspaces as reference
from private_desktop import run
from qualify_music_browser import browser_states, click, close, launch, wait_for
from qualify_spectrum import screenshot


def visible_tabs(hwnd):
    # Multiple browser instances at the same destination have the same title.
    return sorted((t["name"], t["x"], t["y"], t["width"], t["height"])
                  for t in reference.named_controls(hwnd) if t["kind"] == "TabItem")


def tab(tabs, name):
    return next(t for t in tabs if t[0] == name)


def geometry_matches(before, after):
    return len(before) == len(after) and all(
        a[0] == b[0] and all(abs(x - y) <= 2 for x, y in zip(a[1:], b[1:]))
        for a, b in zip(before, after))


def center(t):
    return W.POINT(t[1] + t[3] // 2, t[2] + t[4] // 2)


def drag(hwnd, source, end):
    start = center(source)
    path = [W.POINT(round(start.x + (end.x - start.x) * i / 24),
                    round(start.y + (end.y - start.y) * i / 24)) for i in range(1, 25)]
    reference.drag(hwnd, start, path)


def layout_shape(profile):
    def visit(node):
        info = node["info"]
        if "panel" in info:
            return [node["panel_name"], info["panel"]["id"]]
        kind = (["split", info["stack"]["axis"]] if "stack" in info
                else ["tabs", info["tabs"]["active_index"]])
        return [*kind, [visit(child) for child in node["children"]]]

    workspace = json.loads((profile / "workspace-v1.json").read_text(encoding="utf-8"))
    return visit(workspace["layout"]["center"])


def journey(exe, test_exe, output):
    profile = output / "profile"
    root, expanded = reference.seed(test_exe, profile)
    checks = {}
    result = {"checks": checks}
    process = reference.launch(exe, profile, reference.workspaces(root, expanded)["analysis"], "native")
    try:
        hwnd, _ = wait_for(process, "Opening Notes")
        result["dpi"] = reference.resize(hwnd, 1440, 900)
        # The first chapter is visible above the nested divider at the operating height.
        reference.play(process, ["Chapter 01"])
        wait_for(process, "Pause")
        start = visible_tabs(hwnd)
        browser = tab(start, "Reference Library")
        point = center(browser)
        reference.mouse(hwnd, 0x204, point, 2)
        reference.mouse(hwnd, 0x205, point, 0)
        click(process, "Open in New Tab")
        before = visible_tabs(hwnd)
        checks["two visible browser instances"] = sum(t[0] == browser[0] for t in before) == 2

        click(process, "Hide left sidebar")
        click(process, "Hide Queue · Ctrl+Alt+Right")
        hidden = [t[0] for t in visible_tabs(hwnd)]
        checks["mixed group keeps its companion"] = (
            "Library" not in hidden and "Queue" not in hidden and "Listening Statistics" in hidden)
        click(process, "Show left sidebar")
        click(process, "Show Queue · Ctrl+Alt+Right")
        restored = visible_tabs(hwnd)
        result.update(before_hide=before, after_show=restored)
        checks["restored tab order and geometry"] = geometry_matches(before, restored)
        checks["playing after toggles"] = reference.playing(hwnd)

        click(process, "Workspace")
        click(process, "Customize Layout")
        wait_for(process, "Done")
        tabs = visible_tabs(hwnd)
        queue = tab(tabs, "Queue")
        statistics = tab(tabs, "Listening Statistics")
        # Move the right-hand tab before the left-hand one and require an actual swap.
        left, right = sorted((queue, statistics), key=lambda t: t[1])
        drag(hwnd, right, W.POINT(left[1] + left[3] // 4, center(left).y))
        tabs = visible_tabs(hwnd)
        checks["reordered within the group"] = tab(tabs, right[0])[1] < tab(tabs, left[0])[1]

        # The browser occupies the column from its first tab to Waveform and
        # ends above Queue's strip. Kit gives the side edges priority at corners.
        browser = tab(tabs, "Reference Library")
        wave = tab(tabs, "Waveform")
        queue = tab(tabs, "Queue")
        midpoint = (browser[1] + wave[1]) // 2
        drag(hwnd, queue, W.POINT(midpoint, queue[2] - round(12 * result["dpi"] / 96)))
        click(process, "Done")
        after = visible_tabs(hwnd)
        queue = tab(after, "Queue")
        checks["Queue split below browser and above its former companion"] = (
            abs(queue[1] - browser[1]) <= 2 and
            browser[2] < queue[2] < tab(after, "Listening Statistics")[2])
        checks["Done kept all instances"] = [t[0] for t in after] == [t[0] for t in before]
        checks["playing after dock edits"] = reference.playing(hwnd)
        screenshot(hwnd, output / "split-done.png")
    finally:
        close(process)

    result["layout"] = layout_shape(profile)
    browser_ids = sorted(browser_states(profile / "workspace-v1.json"))
    result["browser_ids"] = browser_ids
    checks["two visible browser instances"] &= len(browser_ids) == 2 and 1 in browser_ids
    process = launch(exe, profile)
    try:
        hwnd, _ = wait_for(process, "Opening Notes")
        reference.resize(hwnd, 1440, 900)
        checks["restart restored tab geometry"] = geometry_matches(after, visible_tabs(hwnd))
    finally:
        close(process)
    checks["restart restored split axes, tab order, selection and IDs"] = layout_shape(profile) == result["layout"]
    (output / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    failed = [name for name, passed in checks.items() if not passed]
    if failed:
        raise RuntimeError(", ".join(failed))


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
        except Exception:
            (args.output / "qualification-error.log").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.test_exe),
             str(args.output), "--worker"], 300)

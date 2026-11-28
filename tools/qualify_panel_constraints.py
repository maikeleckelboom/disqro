"""Verify dock slot ranges, navigation and a smaller restored workspace on Windows."""
import argparse
import json
import sys
import time
import traceback
from ctypes import wintypes as W
from pathlib import Path

import capture_reference_workspaces as native
from private_desktop import run
from qualify_music_browser import browser_states, close, launch, panel, tabs, wait_for
from qualify_spectrum import click_control
from qualify_workspace_dock import layout_shape


def control(rows, name, kind=None):
    return min((r for r in rows if r["name"].split(" · ")[0] == name
                and (kind is None or r["kind"] == kind)), key=lambda r: r["x"])


def point(row):
    return W.POINT(row["x"] + row["width"] // 2, row["y"] + row["height"] // 2)


def press_control(hwnd, name, kind=None):
    click_control(hwnd, point(control(native.named_controls(hwnd), name, kind)))
    time.sleep(0.15)


def sweep(hwnd, start, dx, dy, sample):
    path = [W.POINT(start.x + round(dx * n / 24), start.y + round(dy * n / 24))
            for n in range(1, 25)]
    native.drag(hwnd, start, path, lambda i, _: sample() if i in (3, 11, 19, 23) else None)


def pan_to_end(hwnd, viewport, scale, horizontal):
    x, y = viewport["x"], viewport["y"]
    width, height = viewport["width"], viewport["height"]
    inset = round(8 * scale)
    if horizontal:
        start = W.POINT(x + width // 3, y + height - inset)
        end = W.POINT(x + width - 3 * inset, start.y)
    else:
        start = W.POINT(x + width - inset, y + height // 3)
        end = W.POINT(start.x, y + height - 3 * inset)
    path = [W.POINT(round(start.x + (end.x - start.x) * n / 12),
                    round(start.y + (end.y - start.y) * n / 12)) for n in range(1, 13)]
    native.drag(hwnd, start, path)


def geometry(rows, scale):
    back = control(rows, "Back", "Button")
    menu = control(rows, "Panel menu", "Button")
    left = back["x"] - 6 * scale
    top = control(rows, "Queue", "TabItem")["y"]
    bottom = control(rows, "Listening History", "TabItem")["y"]
    return dict(x=left, y=top, width=(menu["x"] + menu["width"] - left) / scale + 6,
                height=(bottom - top) / scale)


def journey(exe, test_exe, out):
    profile = out / "profile"
    root, _ = native.seed(test_exe, profile)
    # An undersized saved sibling, a mixed group, and nested splits on both axes.
    primary = tabs(native.browser(1, root, "tracks"), panel("queue", 2))
    nested = native.split(1, [400, 280], primary, tabs(panel("listening-history", 3)))
    sibling = tabs(panel("browser", 4, current=dict(location="Liked", mode="tracks", query="Opening")))
    tree = native.split(0, [680, 100, 460], nested, sibling, tabs(panel("library", 5)))
    observations = {}
    def save(name, value):
        observations[name] = value
        (out / "results.json").write_text(json.dumps(observations, indent=2), encoding="utf-8")
        return value

    process = native.launch(exe, profile, tree, "native")
    try:
        hwnd = wait_for(process)
        dpi = save("dpi", native.resize(hwnd, 1440, 900))
        scale = dpi / 96
        time.sleep(0.6)
        save("initial_tabs", native.tabs_shown(hwnd))
        native.play(process, ["Chapter 01"])
        wait_for(process, "Pause")
        rows = native.named_controls(hwnd)
        start = geometry(rows, scale)
        sibling_width = (control(rows, "Library", "TabItem")["x"] - control(rows, "Favorites", "TabItem")["x"]) / scale
        assert save("restored_100_px_browser_width", sibling_width) >= 363
        normalized = lambda: geometry(native.named_controls(hwnd), scale)
        # The handle overlays the start of the second slot, just after the browser.
        samples = [start]
        sweep(hwnd, W.POINT(round(start["x"] + start["width"] * scale),
                            round(start["y"] + 110 * scale)), -round(620 * scale), 0,
              lambda: samples.append(normalized()))
        save("held_horizontal_drag", samples)
        assert start["width"] > samples[-1]["width"] + 25
        assert all(g["width"] >= 363 for g in samples) and samples[-1]["width"] <= 365
        g = samples[-1]
        heights = [g["height"]]
        sweep(hwnd, W.POINT(round(g["x"] + 130 * scale), round(g["y"] + g["height"] * scale)),
              0, -round(430 * scale), lambda: heights.append(normalized()["height"]))
        save("held_vertical_drag", heights)
        assert heights[0] > heights[-1] + 20 and all(h >= 177 for h in heights) and heights[-1] <= 179

        rows = native.named_controls(hwnd)
        names = ["Back", "Forward", "Location", "Filter this view", "View", "Panel menu"]
        controls = [control(rows, name, "Group" if name == "Location" else "Button") for name in names]
        save("constrained_toolbar", controls)
        assert all(a["x"] + a["width"] <= b["x"] + 1 for a, b in zip(controls, controls[1:]))
        assert all(c["width"] >= 29 * scale for c in controls)
        assert control(rows, "Location", "Group")["width"] >= 129 * scale
        native.capture(hwnd, out / "divider-limits.png")

        # Switching to the smaller panel keeps the browser's width reserved.
        press_control(hwnd, "Queue", "TabItem")
        rows = native.named_controls(hwnd)
        second_x = control(rows, "Favorites", "TabItem")["x"]
        sweep(hwnd, W.POINT(round(g["x"] + g["width"] * scale), round(g["y"] + 80 * scale)),
              -round(100 * scale), 0, lambda: None)
        assert abs(second_x - control(native.named_controls(hwnd), "Favorites", "TabItem")["x"]) <= 1
        press_control(hwnd, "Reference Library", "TabItem")
        rows = native.named_controls(hwnd)
        assert abs(second_x - control(rows, "Favorites", "TabItem")["x"]) <= 1
        press_control(hwnd, "Earlier locations", "Button")
        wait_for(process, "Locations")
        native.capture(hwnd, out / "ancestor-menu.png")
        press_control(hwnd, "Collections", "MenuItem")
        wait_for(process, "Back · Reference Library · Alt+Left")
        press_control(hwnd, "Back", "Button")
        wait_for(process, "Locations for Reference Library")
        press_control(hwnd, "Forward", "Button")
        wait_for(process, "Back · Reference Library · Alt+Left")
        press_control(hwnd, "Back", "Button")
        press_control(hwnd, "Filter this view", "Button")
        press_control(hwnd, "Clear and close filter", "Button")
        press_control(hwnd, "View", "Button")
        press_control(hwnd, "Tree")
        wait_for(process, "View · Tree")
        press_control(hwnd, "Panel menu", "Button")
        wait_for(process, "Open in New Tab")
        native.press(hwnd, 0x1b)
        save("constrained_navigation", ["ancestor selection", "Back", "Forward", "filter", "view", "panel menu"])

        native.resize(hwnd, 720, 460)
        time.sleep(0.4)
        compact = normalized()
        save("small_window", compact)
        assert abs(compact["width"] - 364) <= 1 and abs(compact["height"] - 178) <= 1
        assert native.playing(hwnd)
        native.capture(hwnd, out / "small-window.png")
        rows = native.named_controls(hwnd)
        viewport = control(rows, "Workspace panels", "Group")
        player = point(control(rows, "Pause", "Button"))
        pan_to_end(hwnd, viewport, scale, True)
        rows = native.named_controls(hwnd)
        library = control(rows, "Library", "TabItem")
        library_right = library["x"] + library["width"]
        assert viewport["x"] < library_right <= viewport["x"] + viewport["width"]
        press_control(hwnd, "Library", "TabItem")
        history_y = control(rows, "Listening History", "TabItem")["y"]
        pan_to_end(hwnd, viewport, scale, False)
        rows = native.named_controls(hwnd)
        shift = (history_y - control(rows, "Listening History", "TabItem")["y"]) / scale
        assert shift > 80
        after = point(control(rows, "Pause", "Button"))
        assert (after.x, after.y) == (player.x, player.y)
        save("scrollbars", dict(library_right=library_right, viewport=viewport,
                                vertical_travel=shift, player_position_unchanged=True))
        native.capture(hwnd, out / "small-window-scrolled.png")
        native.resize(hwnd, 1440, 900)
        time.sleep(0.4)
        roomy = normalized()
        save("before_small_window_save", roomy)
        assert abs(roomy["x"] - start["x"]) <= 1 and abs(roomy["y"] - start["y"]) <= 1
        assert abs(roomy["width"] - samples[-1]["width"]) <= 1
        assert abs(roomy["height"] - heights[-1]) <= 1
        native.resize(hwnd, 720, 460)
        time.sleep(0.4)
        compact = normalized()
        assert abs(compact["width"] - 364) <= 1 and abs(compact["height"] - 178) <= 1
    except Exception:
        if process.poll() is None:
            native.capture(hwnd, out / "failure.png")
            save("failure_controls", native.named_controls(hwnd))
        raise
    finally:
        close(process)

    previous = layout_shape(profile)
    saved_tree = json.loads((profile / "workspace-v1.json").read_text(encoding="utf-8"))["layout"]["center"]
    horizontal = saved_tree["info"]["stack"]["sizes"]
    vertical = saved_tree["children"][0]["info"]["stack"]["sizes"]
    save("persisted_sizes", dict(horizontal=horizontal, vertical=vertical))
    assert len(horizontal) == 3 and len(vertical) == 2 and min(*horizontal, *vertical) > 0
    states = browser_states(profile / "workspace-v1.json")
    assert states[1]["current"]["mode"] == "tree" and states[4]["current"]["query"] == "Opening"
    process = launch(exe, profile)
    try:
        hwnd, _ = wait_for(process, "View · Tree")
        native.resize(hwnd, 720, 460)
        time.sleep(0.4)
        restored = normalized()
        save("restart", restored)
        assert all(abs(compact[k] - restored[k]) <= 2 for k in compact)
        native.resize(hwnd, 1440, 900)
        time.sleep(0.4)
        regrown = save("roomy_after_restart", normalized())
        assert all(abs(roomy[k] - regrown[k]) <= 2 for k in roomy), (roomy, regrown)
    finally:
        close(process)
    assert layout_shape(profile) == previous
    states_after = browser_states(profile / "workspace-v1.json")
    assert all(states[i]["current"] == states_after[i]["current"] for i in states)
    save("restored_topology_and_browser_state", True)


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
            (args.output / "error.log").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.test_exe),
             str(args.output), "--worker"], 240)

"""Capture Disqro's reference workspaces in a silent, private Windows desktop.

Seeds a fresh fixture library with generated artwork and a synthetic musical
CD image, then opens the Listening, Library and Analysis reference
workspaces at an ordinary and a small window size and captures them while
the fixture plays silently. At the display's own scale it also measures the
Music Browser toolbar through a continuous window resize and a continuous
divider drag, takes the workspace through Customize Layout and Done, records
the keyboard focus order, and checks that playback continues throughout.
Every input goes only to the windows this run creates; the listener's own
profile is untouched.

`--scale 100` runs the player DPI-unaware, so it lays out at 96 DPI whatever
the display's scale.

    python tools/capture_reference_workspaces.py <disqro.exe> <test.exe> <output> [--scale 100|native]
"""
import argparse
import ctypes as C
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
import wave
from ctypes import wintypes as W

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from private_desktop import run
from qualify_music_browser import close, panel, tabs, wait_for
from qualify_spectrum import click_control, screenshot

user32 = C.windll.user32
RATE = 44100


def split(axis, sizes, *children):
    return dict(panel_name="StackPanel", children=list(children), info=dict(stack=dict(axis=axis, sizes=sizes)))


def browser(identity, root, mode, filtered=None, **current):
    state = dict(location=dict(Library=root), mode=mode, query=filtered or "")
    state.update(current)
    if filtered:
        return panel("browser", identity, current=state, filter_open=True)
    return panel("browser", identity, current=state)


def workspaces(root, expanded):
    """The three reference compositions, built from ordinary dock records."""
    listening = split(0, [250, 890, 300],
                      tabs(panel("library", 2)),
                      tabs(browser(1, root, "tracks")),
                      tabs(panel("queue", 3)))
    library = split(0, [250, 420, 770],
                    tabs(panel("library", 2), panel("locations", 3)),
                    tabs(browser(1, root, "tree", tree_expanded=expanded)),
                    split(1, [430, 300],
                          tabs(browser(4, root, "tracks", filtered="Chapter 1")),
                          tabs(browser(5, root, "albums"))))
    analysis = split(0, [250, 560, 630],
                     tabs(panel("library", 2)),
                     split(1, [470, 260],
                           tabs(browser(1, root, "tracks")),
                           tabs(panel("queue", 3), panel("listening-statistics", 4))),
                     split(1, [250, 480],
                           tabs(panel("waveform", 5)),
                           split(0, [330, 300],
                                 tabs(panel("spectrogram", 6), panel("spectrum", 7)),
                                 tabs(panel("stereo-field", 8)))))
    return dict(listening=listening, library=library, analysis=analysis)


def settings(center):
    # --qualify-silent mutes the output device itself. The player keeps a
    # normal volume so the level meter and analyzers receive the signal.
    return dict(volume=0.8, layout=dict(version=3, center=center), workspace=dict(active=1),
                window=dict(close_to_tray=False))


def synthetic_mix(seconds):
    """A 128 BPM arrangement with an intro, a drop and a breakdown, so the
    waveform, spectrum and stereo field show musical structure."""
    t = np.arange(int(seconds * RATE)) / RATE
    beat = 60 / 128
    phase = (t % beat) / beat
    bar = (t // (beat * 4)).astype(int)
    section = np.select([t < 8, t < 16, t < 32, t < 38], [0, 1, 2, 3], 4)
    kick_on = np.isin(section, [1, 2, 4])
    kick = np.sin(2 * np.pi * (48 + 90 * np.exp(-phase * 30)) * t) * np.exp(-phase * 9) * kick_on
    rng = np.random.default_rng(7)
    noise = rng.standard_normal(t.size)
    hat_phase = ((t + beat / 2) % beat) / beat
    hats = np.convolve(noise, [1, -1], "same") * np.exp(-hat_phase * 40) * 0.18 * (section != 3)
    notes = np.array([55, 55, 65.4, 49])[bar % 4]
    bass = np.tanh(3 * np.sin(2 * np.pi * notes * t)) * 0.22 * np.exp(-phase * 3) * np.isin(section, [2, 4])
    pad_l = sum(np.sin(2 * np.pi * f * t + i) for i, f in enumerate([220, 277.2, 329.6, 440]))
    pad_r = sum(np.sin(2 * np.pi * f * 1.003 * t + 2 * i) for i, f in enumerate([220, 277.2, 329.6, 440]))
    swell = 0.05 + 0.05 * np.sin(2 * np.pi * t / 16) ** 2
    lead = np.sin(2 * np.pi * 880 * t + 3 * np.sin(2 * np.pi * 5 * t)) * 0.06 * (section == 2) * (phase < 0.5)
    left = 0.8 * kick + hats + bass + pad_l * swell + lead
    right = 0.8 * kick + np.roll(hats, 40) + bass + pad_r * swell - lead * 0.4
    fade = np.clip(np.minimum(t, seconds - t) / 1.5, 0, 1)
    mix = np.stack([left, right], 1) * fade[:, None]
    return (mix / np.abs(mix).max() * 0.85 * 32767).astype("<i2")


def write_mix(path, seconds):
    with wave.open(str(path), "wb") as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes(synthetic_mix(seconds).tobytes())


def artwork(path, seed):
    """Abstract square artwork, so captures show covers without using any
    real release's art."""
    rng = np.random.default_rng(seed)
    size = 600
    hue = rng.random()
    base = Image.new("RGB", (size, size))
    draw = ImageDraw.Draw(base)
    for y in range(size):
        k = y / size
        r, g, b = (int(255 * (0.5 + 0.5 * math.cos(2 * math.pi * (hue + k * 0.25 + o)))) for o in (0, 0.33, 0.66))
        draw.line([(0, y), (size, y)], fill=(r // 2, g // 2, b // 2))
    for _ in range(5):
        x, y, radius = rng.integers(0, size, 2).tolist() + [int(rng.integers(60, 260))]
        tone = tuple(int(v) for v in rng.integers(40, 230, 3))
        draw.ellipse([x - radius, y - radius, x + radius, y + radius], fill=tone)
    base = base.filter(ImageFilter.GaussianBlur(18))
    base.save(path, quality=90)


def seed(test_exe, output):
    output.mkdir(parents=True, exist_ok=False)
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(output)), check=True,
                   stdout=subprocess.DEVNULL)
    library = output / "Reference Library"
    write_mix(library / "Live Set" / "CD 1" / "Source 01.wav", 45)
    for i, folder in enumerate([library / "Live Set", library / "Live Set" / "CD 1", library / "Live Set" / "CD 2",
                                library / "Archive", output / "Imported"]):
        artwork(folder / "cover.jpg", i + 3)
    root = int((output / "reference-root.txt").read_text(encoding="ascii"))
    release = str(library / "Live Set").lower()
    return root, [dict(Collection=root), dict(Folder=[root, release])]


SYSTEM_DPI = user32.GetDpiForSystem() or 96


def resize(hwnd, width, height, move=True):
    """Size the window so it lays out at `width` x `height` logical pixels.

    A DPI-unaware window (the 100% run) lays out at 96 DPI and Windows
    stretches it by the system scale, so it is sized in system pixels too.
    """
    dpi = user32.GetDpiForWindow(hwnd) or 96
    factor = max(dpi, SYSTEM_DPI) / 96
    flags = 0x0014 if move else 0x0416
    user32.SetWindowPos(hwnd, None, 40, 40, round(width * factor), round(height * factor), flags)
    return dpi


def capture(hwnd, path):
    """Save the window as the app drew it. A DPI-unaware window's own
    surface sits unstretched at the top left of the capture."""
    screenshot(hwnd, path)
    dpi = user32.GetDpiForWindow(hwnd) or 96
    if dpi < SYSTEM_DPI:
        image = Image.open(path)
        scale = dpi / SYSTEM_DPI
        image.crop((0, 0, round(image.width * scale), round(image.height * scale))).save(path)


def launch(exe, profile, center, scale):
    (profile / "workspace-v1.json").write_text(json.dumps(settings(center)), encoding="utf-8")
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(profile))
    if scale == "100":
        env["__COMPAT_LAYER"] = "DPIUNAWARE"
    with (profile / "native.log").open("ab") as log:
        return subprocess.Popen([exe, "--qualify-silent"], env=env, stdout=log, stderr=log)


UIA_PRELUDE = r"""
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [Windows.Automation.AutomationElement]::FromHandle([IntPtr][long]$env:DISQRO_WINDOW)
"""


def powershell(hwnd, script, **env):
    probe = subprocess.run(["powershell", "-NoProfile", "-Command", UIA_PRELUDE + script],
                           env=dict(os.environ, DISQRO_WINDOW=str(hwnd), **{k: str(v) for k, v in env.items()}),
                           creationflags=subprocess.CREATE_NO_WINDOW, timeout=30, text=True, encoding="utf-8",
                           capture_output=True)
    return probe.stdout.splitlines()


def named_controls(hwnd, top=-10**6, bottom=10**6):
    """Every named UIA element whose top edge lies in [top, bottom) screen pixels."""
    lines = powershell(hwnd, r"""
$all = $root.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
foreach ($e in $all) {
    $b = $e.Current.BoundingRectangle
    if ($e.Current.Name -and $b.Width -gt 0 -and $b.Y -ge [int]$env:DISQRO_TOP -and $b.Y -lt [int]$env:DISQRO_BOTTOM) {
        "{0}|{1}|{2}|{3}|{4}|{5}" -f $e.Current.Name, [int]$b.X, [int]$b.Y, [int]$b.Width, [int]$b.Height, $e.Current.ControlType.ProgrammaticName
    }
}
""", DISQRO_TOP=top, DISQRO_BOTTOM=bottom)
    rows = []
    for line in lines:
        name, x, y, width, height, kind = line.rsplit("|", 5)
        rows.append(dict(name=name, x=int(x), y=int(y), width=int(width), height=int(height),
                         kind=kind.replace("ControlType.", "")))
    return rows


def focused_name(hwnd):
    """The element of this window that holds keyboard focus."""
    lines = powershell(hwnd, r"""
$c = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::HasKeyboardFocusProperty, $true)
$f = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $c)
if ($f) { "{0}|{1}" -f $f.Current.Name, $f.Current.ControlType.ProgrammaticName }
""")
    return lines[0] if lines else ""


def tabs_shown(hwnd):
    return sorted(c["name"] for c in named_controls(hwnd) if c["kind"] == "TabItem")


def playing(hwnd):
    """Whether the play button offers Pause, which it does while playing."""
    return any(c["name"] == "Pause" and c["kind"] == "Button" for c in named_controls(hwnd))


def play(process, labels):
    """Double-click the first of `labels` the window shows, and repeat the
    fixture, so playback outlasts a long journey."""
    for label in labels:
        try:
            hwnd, point = wait_for(process, label, timeout=4)
        except RuntimeError:
            continue
        click_control(hwnd, point)
        click_control(hwnd, point)
        try:
            hwnd, point = wait_for(process, "Repeat off", timeout=3)
            click_control(hwnd, point)
        except RuntimeError:
            pass
        return label
    raise RuntimeError(f"None of {labels!r} could start playback")


def client_point(hwnd, x, y):
    """A point in logical client pixels, as a screen point."""
    dpi = user32.GetDpiForWindow(hwnd) or 96
    point = W.POINT(round(x * dpi / 96), round(y * dpi / 96))
    user32.ClientToScreen(hwnd, C.byref(point))
    return point


def mouse(hwnd, message, point, buttons):
    p = W.POINT(point.x, point.y)
    user32.ScreenToClient(hwnd, C.byref(p))
    user32.SendMessageW(hwnd, message, buttons, (p.y << 16) | (p.x & 0xffff))


def drag(hwnd, start, path, step=None, pause=0.03):
    """Press at `start`, move through `path` with the button held, release."""
    mouse(hwnd, 0x200, start, 0)
    mouse(hwnd, 0x201, start, 1)
    for i, point in enumerate(path):
        mouse(hwnd, 0x200, point, 1)
        time.sleep(pause)
        if step:
            step(i, point)
    mouse(hwnd, 0x202, path[-1] if path else start, 0)


def press(hwnd, key):
    user32.PostMessageW(hwnd, 0x0100, key, 0)
    user32.PostMessageW(hwnd, 0x0101, key, 0xC0000000)


def capture_workspace(exe, output, profile, name, center, scale, results):
    process = launch(exe, profile, center, scale)
    try:
        hwnd, _ = wait_for(process, "Opening Notes", timeout=20)
        for width, height in [(1440, 900), (1100, 720)]:
            dpi = resize(hwnd, width, height)
            results.setdefault("dpi", dpi)
            time.sleep(0.6)
            if width == 1440:
                results[f"{name}-played"] = play(process, ["Chapter 05", "Chapter 02", "Opening Notes"])
                time.sleep(4.0)
            else:
                time.sleep(1.5)
            capture(hwnd, output / f"{name}-{width}.png")
    finally:
        close(process)


def toolbar_state(hwnd, top, bottom):
    controls = named_controls(hwnd, top, bottom)
    return [dict(name=c["name"].split(" · ")[0], x=c["x"], width=c["width"]) for c in controls
            if c["kind"] in ("Button", "Group", "Edit", "Text", "Custom")]


def toolbar_journey(exe, output, profile, center, scale):
    """Shrink the window continuously and record the browser toolbar."""
    process = launch(exe, profile, center, scale)
    states = []
    try:
        hwnd, _ = wait_for(process, "Opening Notes", timeout=20)
        dpi = resize(hwnd, 1440, 900)
        time.sleep(0.8)
        rect = W.RECT()
        user32.GetWindowRect(hwnd, C.byref(rect))
        # The tab strip, then the toolbar.
        top = rect.top + round(82 * dpi / 96)
        bottom = top + round(42 * dpi / 96)
        frames = output / "toolbar"
        frames.mkdir(exist_ok=True)
        for width in range(1440, 779, -30):
            resize(hwnd, width, 900, move=False)
            time.sleep(0.25)
            states.append(dict(window_width=width, controls=toolbar_state(hwnd, top, bottom)))
            capture(hwnd, frames / f"{width}.png")
    finally:
        close(process)
    (output / "toolbar-journey.json").write_text(json.dumps(states, indent=2), encoding="utf-8")
    return states


def split_journey(exe, output, profile, center, results):
    """Drag the divider beside a browser continuously and record its toolbar."""
    process = launch(exe, profile, center, "native")
    states = []
    try:
        hwnd, _ = wait_for(process, "Opening Notes", timeout=20)
        dpi = resize(hwnd, 1440, 900)
        time.sleep(0.8)
        results["split-played"] = play(process, ["Chapter 05", "Chapter 02", "Opening Notes"])
        time.sleep(1.0)
        rect = W.RECT()
        user32.GetWindowRect(hwnd, C.byref(rect))
        top = rect.top + round(82 * dpi / 96)
        bottom = top + round(42 * dpi / 96)
        # The divider between the Tree browser (420) and the right column.
        x0 = 664
        frames = output / "split"
        frames.mkdir(exist_ok=True)
        path = [client_point(hwnd, x, 400) for x in range(x0, 200, -12)]

        def step(i, point):
            if i % 4 == 0:
                logical = x0 - 12 * (i + 1)
                states.append(dict(divider=logical, controls=toolbar_state(hwnd, top, bottom)))
                capture(hwnd, frames / f"{logical}.png")

        drag(hwnd, client_point(hwnd, x0, 400), path, step)
        time.sleep(0.5)
        results["split-playing-after"] = playing(hwnd)
    finally:
        close(process)
    (output / "split-journey.json").write_text(json.dumps(states, indent=2), encoding="utf-8")


def customize_journey(exe, output, profile, center, results):
    """Customize Layout, then Done: tabs, layout and playback carry through."""
    process = launch(exe, profile, center, "native")
    try:
        hwnd, _ = wait_for(process, "Opening Notes", timeout=20)
        resize(hwnd, 1440, 900)
        time.sleep(0.8)
        results["customize-played"] = play(process, ["Chapter 05", "Chapter 02", "Opening Notes"])
        time.sleep(1.0)
        results["tabs-before"] = tabs_shown(hwnd)
        capture(hwnd, output / "customize-before.png")
        hwnd, point = wait_for(process, "Workspace")
        click_control(hwnd, point)
        time.sleep(0.5)
        hwnd, point = wait_for(process, "Customize Layout")
        click_control(hwnd, point)
        time.sleep(0.8)
        results["tabs-customizing"] = tabs_shown(hwnd)
        capture(hwnd, output / "customize-editing.png")
        hwnd, point = wait_for(process, "Done")
        click_control(hwnd, point)
        time.sleep(0.8)
        results["tabs-after-done"] = tabs_shown(hwnd)
        results["playing-after-done"] = playing(hwnd)
        capture(hwnd, output / "customize-done.png")
    finally:
        close(process)


def focus(hwnd, name):
    """Give keyboard focus to the first control called `name`."""
    lines = powershell(hwnd, r"""
$c = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $env:DISQRO_NAME)
$e = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $c)
if ($e) { $e.SetFocus(); "ok" }
""", DISQRO_NAME=name)
    time.sleep(0.3)
    return bool(lines)


def selected_tab(hwnd, names):
    """Which of the tabs `names` is selected."""
    lines = powershell(hwnd, r"""
foreach ($name in (ConvertFrom-Json $env:DISQRO_NAMES)) {
    $c = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $name)
    $e = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $c)
    if ($e) {
        $p = $null
        if ($e.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern, [ref]$p) -and $p.Current.IsSelected) { $name }
    }
}
""", DISQRO_NAMES=json.dumps(names))
    return lines[0] if lines else None


def shows(hwnd, name):
    return any(c["name"] == name for c in named_controls(hwnd))


def keyboard_journey(exe, output, profile, center, results):
    """Operate the toolbar and tabs from the keyboard, and record the focus
    ring as Tab moves through the toolbar."""
    process = launch(exe, profile, center, "native")
    checks = {}
    try:
        hwnd, _ = wait_for(process, "Opening Notes", timeout=20)
        resize(hwnd, 1440, 900)
        time.sleep(0.8)
        frames = output / "focus"
        frames.mkdir(exist_ok=True)
        enter, space, escape, tab, right = 0x0D, 0x20, 0x1B, 0x09, 0x27

        focus(hwnd, "Filter this view")
        press(hwnd, enter)
        time.sleep(0.4)
        checks["Enter on Filter opens the filter row"] = shows(hwnd, "Clear and close filter")
        press(hwnd, escape)
        time.sleep(0.4)
        checks["Escape closes the filter row"] = not shows(hwnd, "Clear and close filter")

        focus(hwnd, "View · Tracks")
        press(hwnd, space)
        time.sleep(0.4)
        checks["Space on View opens the view menu"] = shows(hwnd, "Tree")
        capture(hwnd, frames / "view-menu.png")
        press(hwnd, escape)
        time.sleep(0.3)

        focus(hwnd, "Panel menu")
        press(hwnd, enter)
        time.sleep(0.4)
        checks["Enter on the panel menu opens it"] = shows(hwnd, "Shuffle All")
        press(hwnd, escape)
        time.sleep(0.3)

        # Through the toolbar with Tab, from Filter. Back and Forward take
        # focus only while they have history to go to.
        if focus(hwnd, "Filter this view"):
            capture(hwnd, frames / "00.png")
            for i in range(1, 7):
                press(hwnd, tab)
                time.sleep(0.25)
                capture(hwnd, frames / f"{i:02d}.png")
    finally:
        close(process)
    results["keyboard"] = checks


def tab_keyboard_journey(exe, output, profile, center, results):
    """Arrow keys move between the tabs of a group."""
    process = launch(exe, profile, center, "native")
    try:
        hwnd, _ = wait_for(process, "Opening Notes", timeout=20)
        resize(hwnd, 1440, 900)
        time.sleep(0.8)
        pair = ["Queue", "Listening Statistics"]
        before = selected_tab(hwnd, pair)
        focus(hwnd, before or "Queue")
        press(hwnd, 0x27)
        time.sleep(0.4)
        after = selected_tab(hwnd, pair)
        capture(hwnd, output / "focus" / "tab-arrow.png")
        results["tab-arrow"] = dict(before=before, after=after)
    finally:
        close(process)


JOURNEYS = ["listening", "library", "analysis", "toolbar", "split", "customize", "keyboard", "tabs"]


def journey(exe, test_exe, output, scale, chosen):
    profile = output / "profile"
    root, expanded = seed(test_exe, profile)
    results = {"system_dpi": SYSTEM_DPI}
    spaces = workspaces(root, expanded)
    for name, center in spaces.items():
        if name in chosen:
            capture_workspace(exe, output, profile, name, center, scale, results)
    # Interaction journeys address controls by their native coordinates.
    if scale == "native":
        if "toolbar" in chosen:
            toolbar_journey(exe, output, profile, spaces["listening"], scale)
        if "split" in chosen:
            split_journey(exe, output, profile, spaces["library"], results)
        if "customize" in chosen:
            customize_journey(exe, output, profile, spaces["analysis"], results)
        if "keyboard" in chosen:
            keyboard_journey(exe, output, profile, spaces["listening"], results)
        if "tabs" in chosen:
            tab_keyboard_journey(exe, output, profile, spaces["analysis"], results)
    (output / "result.json").write_text(json.dumps(results, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("test_exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--scale", choices=["100", "native"], default="native")
    parser.add_argument("--journeys", default=",".join(JOURNEYS),
                        help="Comma-separated subset of: " + ", ".join(JOURNEYS))
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            journey(args.exe, args.test_exe, args.output, args.scale, args.journeys.split(","))
        except BaseException:
            output = args.output if args.output.is_dir() else args.output.parent
            (output / "capture-error.log").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.test_exe),
             str(args.output), "--scale", args.scale, "--journeys", args.journeys, "--worker"], 900)

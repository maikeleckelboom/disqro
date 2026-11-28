"""Capture Disqro's reference workspaces in a silent, private Windows desktop.

Seeds a fresh fixture library with generated artwork and a synthetic musical
CD image, then opens the Listening, Library and Analysis reference
workspaces at an ordinary and a small window size and captures them while
the fixture plays silently. A continuous resize journey records which
Music Browser toolbar controls exist at each width, so changes in toolbar
composition are measured rather than eyeballed. Every input goes only to the
windows this run creates; the listener's own profile is untouched.

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
from qualify_spectrum import click_control, find_window, screenshot

user32 = C.windll.user32
RATE = 44100


def split(axis, sizes, *children):
    return dict(panel_name="StackPanel", children=list(children), info=dict(stack=dict(axis=axis, sizes=sizes)))


def browser(identity, root, mode, **current):
    state = dict(location=dict(Library=root), mode=mode, query="")
    state.update(current)
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
                          tabs(browser(4, root, "tracks")),
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


def resize(hwnd, width, height):
    dpi = user32.GetDpiForWindow(hwnd) or 96
    user32.SetWindowPos(hwnd, None, 40, 40, round(width * dpi / 96), round(height * dpi / 96), 0x0014)
    return dpi


def launch(exe, profile, center, scale):
    (profile / "workspace-v1.json").write_text(json.dumps(settings(center)), encoding="utf-8")
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(profile))
    if scale == "100":
        env["__COMPAT_LAYER"] = "DPIUNAWARE"
    with (profile / "native.log").open("ab") as log:
        return subprocess.Popen([exe, "--qualify-silent"], env=env, stdout=log, stderr=log)


def named_controls(hwnd, top, bottom):
    """Every named UIA element whose top edge lies in [top, bottom) screen pixels."""
    script = r"""
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [Windows.Automation.AutomationElement]::FromHandle([IntPtr][long]$env:DISQRO_WINDOW)
$all = $root.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
foreach ($e in $all) {
    $b = $e.Current.BoundingRectangle
    if ($e.Current.Name -and $b.Width -gt 0 -and $b.Y -ge [int]$env:DISQRO_TOP -and $b.Y -lt [int]$env:DISQRO_BOTTOM) {
        "{0}|{1}|{2}|{3}|{4}" -f $e.Current.Name, [int]$b.X, [int]$b.Y, [int]$b.Width, $e.Current.ControlType.ProgrammaticName
    }
}
"""
    probe = subprocess.run(["powershell", "-NoProfile", "-Command", script],
                           env=dict(os.environ, DISQRO_WINDOW=str(hwnd), DISQRO_TOP=str(top), DISQRO_BOTTOM=str(bottom)),
                           creationflags=subprocess.CREATE_NO_WINDOW, timeout=30, text=True, capture_output=True)
    rows = []
    for line in probe.stdout.splitlines():
        name, x, y, width, kind = line.rsplit("|", 4)
        rows.append(dict(name=name, x=int(x), y=int(y), width=int(width), kind=kind.replace("ControlType.", "")))
    return rows


def play(process, labels):
    """Double-click the first of `labels` the window shows."""
    for label in labels:
        try:
            hwnd, point = wait_for(process, label, timeout=4)
        except RuntimeError:
            continue
        click_control(hwnd, point)
        click_control(hwnd, point)
        return label
    raise RuntimeError(f"None of {labels!r} could start playback")


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
            screenshot(hwnd, output / f"{name}-{width}.png")
    finally:
        close(process)


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
        top = rect.top + round(40 * dpi / 96)
        bottom = top + round(46 * dpi / 96)
        frames = output / "toolbar"
        frames.mkdir(exist_ok=True)
        for width in range(1440, 779, -30):
            user32.SetWindowPos(hwnd, 0, 0, 0, round(width * dpi / 96), round(900 * dpi / 96), 0x416)
            time.sleep(0.25)
            controls = named_controls(hwnd, top, bottom)
            states.append(dict(window_width=width, controls=controls))
            screenshot(hwnd, frames / f"{width}.png")
    finally:
        close(process)
    (output / "toolbar-journey.json").write_text(json.dumps(states, indent=2), encoding="utf-8")
    return states


def journey(exe, test_exe, output, scale):
    profile = output / "profile"
    root, expanded = seed(test_exe, profile)
    results = {}
    for name, center in workspaces(root, expanded).items():
        capture_workspace(exe, output, profile, name, center, scale, results)
    toolbar_journey(exe, output, profile, workspaces(root, expanded)["listening"], scale)
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
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.test_exe),
             str(args.output), "--scale", args.scale, "--worker"], 600)

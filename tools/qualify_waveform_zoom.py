"""Capture the silent native deep-zoom journey on a private Win32 desktop.

Input is dispatched by the qualification build inside GPUI. The worker only
captures its own fixture window, without system-wide keyboard or mouse input.
"""
import argparse
import ctypes
from ctypes import wintypes
import struct
import json
import math
import os
import shutil
from pathlib import Path
import subprocess
import sys
import time
import traceback
import wave
from array import array

from PIL import Image, ImageChops

from private_desktop import run
from qualify_spectrum import find_window, screenshot
from qualify_waveform import settings


def client_offset(hwnd):
    origin = wintypes.POINT()
    if not ctypes.windll.user32.ClientToScreen(hwnd, ctypes.byref(origin)):
        raise ctypes.WinError()
    visible = wintypes.RECT()
    if ctypes.windll.dwmapi.DwmGetWindowAttribute(hwnd, 9, ctypes.byref(visible), ctypes.sizeof(visible)):
        if not ctypes.windll.user32.GetWindowRect(hwnd, ctypes.byref(visible)):
            raise ctypes.WinError()
    return origin.x - visible.left, origin.y - visible.top


def check_trace(folder, source, hwnd, axis):
    """Compare native trace locations with the two channels of the source WAV."""
    projection = json.loads((folder / "projection.json").read_text())
    assert projection["axis"].lower() == axis and projection["span"] == 32
    x, y, width, height = projection["bounds"]
    dx, dy = client_offset(hwnd)
    with Image.open(folder / "deep-zoom.png") as screenshot:
        # PrintWindow includes nonclient insets. Register the black GPU target
        # against surrounding panel chrome before comparing any audio pixels.
        img = screenshot.convert("RGB")
        x, y = x + dx, y + dy
        def boundary(start, pixel):
            candidates = [i for i in range(start - 12, start + 13)
                          if pixel(i - 1) != (0, 0, 0) and pixel(i) == (0, 0, 0)]
            assert len(candidates) == 1, (start, candidates)
            return candidates[0]
        left = boundary(x, lambda i: img.getpixel((i, y + height//12)))
        top = boundary(y, lambda i: img.getpixel((left + width//12, i)))
        rect = (left, top, left + width, top + height)
        image = img.crop(rect)
    first = projection["first"]
    # A segment whose first vertex lies outside the viewport can still cover
    # an edge pixel. Include its two endpoints before clipping to the target.
    start = max(0, math.floor(first) - 1)
    with wave.open(str(source), "rb") as pcm:
        assert pcm.getsampwidth() == 2 and pcm.getnchannels() == 2
        pcm.setpos(start)
        stereo = list(struct.iter_unpack("<hh", pcm.readframes(math.ceil(first + 32) - start + 2)))
    lines = [[], []]
    for index, amplitudes in enumerate(stereo, start):
        for line, amplitude in zip(lines, amplitudes):
            t = (index - first) / 32
            a = 0.5 - 0.46 * amplitude / 32768
            u, v = {"right": (t, a), "left": (1-t, a), "down": (1-a, t), "up": (1-a, 1-t)}[axis]
            line.append((u * width - 0.5, v * height - 0.5))
    background = image.getpixel((2, 2))
    rgb = ImageChops.difference(image, Image.new("RGB", image.size, background)).split()
    ink = ImageChops.lighter(ImageChops.lighter(rgb[0], rgb[1]), rgb[2]).point(lambda v: 255 if v > 28 else 0)
    # Measure Euclidean distance to the decoded polylines. A rasterized
    # thick-line mask can leave false gaps at sharp reversals.
    segments = []
    for line in lines:
        for (ax, ay), (bx, by) in zip(line, line[1:]):
            vx, vy = bx - ax, by - ay
            segments.append((ax, ay, vx, vy, 1 / (vx * vx + vy * vy)))

    def distance_squared(x, y, ax, ay, vx, vy, inverse_length_squared):
        rx, ry = x - ax, y - ay
        t = max(0., min(1., (rx * vx + ry * vy) * inverse_length_squared))
        return (rx - t * vx) ** 2 + (ry - t * vy) ** 2

    farthest_squared = 0.
    for index, bright in enumerate(ink.tobytes()):
        if not bright:
            continue
        col, row = index % width, index // width
        time_pixel, time_length = (col, width) if axis in ("right", "left") else (row, height)
        if abs(time_pixel + 0.5 - time_length * 0.5) <= 3:
            continue  # The fixed playhead is painted on top of the waveform.
        nearest = min(distance_squared(col, row, *segment) for segment in segments)
        farthest_squared = max(farthest_squared, nearest)
    max_error = math.sqrt(farthest_squared)
    probes = 0
    for line in lines:
        for a, b in zip(line, line[1:]):
            for fraction in (0., 0.5):
                col, row = (round(i + fraction * (j-i)) for i, j in zip(a, b))
                if 2 <= col < width-2 and 2 <= row < height-2:
                    assert ink.crop((col-2, row-2, col+3, row+3)).getbbox(), (axis, col, row)
                    probes += 1
    assert probes >= 116, probes
    # Includes antialias coverage and registration of the full-window capture.
    assert max_error <= 2.5, f"Trace ink {max_error:.3f} physical pixels from decoded PCM"
    return rect, {"dpi": round(projection["scale"] * 96), "axis": axis,
                  "decodedTraceProbes": probes, "maxTraceInkDistancePx": round(max_error, 4)}


def measure(exe, folder, band, control_wav, axis):
    folder.mkdir(parents=True, exist_ok=False)
    recordings = folder / "Signal studies"
    recordings.mkdir()
    source = recordings / "Stereo zoom.wav"
    if control_wav:
        shutil.copyfile(control_wav, source)
    else:
        rate = 48000
        second = array("h")
        frequencies = {"mid": (1847, 2333), "bass": (83, 127), "high": (7393, 8681)}[band]
        for i in range(rate):
            for frequency, phase in zip(frequencies, (0, 0.4)):
                second.append(round(21500 * math.sin(math.tau * i * frequency / rate + phase)))
        with wave.open(str(source), "wb") as out:
            out.setparams((2, 2, rate, 0, "NONE", ""))
            for _ in range(60):
                out.writeframesraw(second.tobytes())
    cue = recordings / "Stereo zoom.cue"
    # At 3.2 seconds the frozen control contains a 6.3 kHz mono tone.
    # Starting the CUE chapter here exercises coincident channel outlines.
    index, next_index = ("00:03:08", "00:18:00") if control_wav else ("00:00:03", "00:30:00")
    cue.write_text('FILE "Stereo zoom.wav" WAVE\n'
                   f'TRACK 01 AUDIO\nTITLE "Stereo reference"\nINDEX 01 {index}\n'
                   f'TRACK 02 AUDIO\nTITLE "Continuation"\nINDEX 01 {next_index}\n', encoding="ascii")
    profile = settings(cue, 1)
    def restore(node):
        if node["panel_name"] == "disqro.waveform":
            # The retired ruler key checks that older workspaces still load.
            node["info"]["panel"]["state"] = {"showSamplePoints": True, "showSampleRuler": True,
                "vertical": axis in ("up", "down"), "reverseTimeAxis": axis in ("left", "up")}
        for child in node["children"]:
            restore(child)
    restore(profile["layout"]["center"])
    (folder / "workspace-v1.json").write_text(json.dumps(profile), encoding="utf-8")
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(folder), DISQRO_ZOOM_JOURNEY=str(folder))
    if control_wav:
        env["DISQRO_ZOOM_MIN_FRAME"] = str(round(3.2 * 44100))
    captures = ("deep-zoom", "hover", "shift-hover", "panel-menu", "playback", "scrub", "resized")
    report = None
    log = folder / "journey.log"
    with log.open("w", encoding="utf-8") as stream:
        player = subprocess.Popen([exe, "--qualify-silent", cue], env=env, stdout=stream, stderr=stream)
        try:
            deadline = time.monotonic() + 100
            while time.monotonic() < deadline and player.poll() is None:
                records = log.read_text(encoding="utf-8", errors="replace").splitlines()
                if "zoom-complete" in records:
                    assert report and all((folder / (name + ".png")).is_file() for name in captures)
                    (folder / "result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
                    return
                if any(record.startswith("zoom-failed") for record in records):
                    raise RuntimeError(records[-1])
                for name in captures:
                    path = folder / (name + ".png")
                    if "capture " + name in records and not path.exists():
                        handle = find_window(player.pid)
                        if not handle:
                            continue
                        # Keep the in-process step waiting until its full PNG is available.
                        pending = folder / (name + ".pending.png")
                        screenshot(handle, pending)
                        if name == "panel-menu":
                            with Image.open(folder / "hover.png") as before, Image.open(pending) as after:
                                delta = ImageChops.difference(before.crop(rect), after.crop(rect)).convert("L")
                                if sum(delta.histogram()[16:]) < 500:
                                    continue
                        pending.replace(path)
                        if name == "deep-zoom":
                            rect, report = check_trace(folder, source, handle, axis)
                        if name in ("hover", "shift-hover"):
                            with Image.open(folder / "deep-zoom.png") as before, Image.open(path) as after:
                                region = (rect[0], max(0, rect[1] - round(report["dpi"] / 3)),
                                          rect[2], rect[3])
                                diff = ImageChops.difference(before.crop(region), after.crop(region))
                                assert diff.getbbox() is None, f"{name} changed the trace or panel header"
                time.sleep(0.05)
            raise RuntimeError("Deep-zoom journey did not complete. See " + str(log))
        finally:
            if player.poll() is None:
                player.terminate()
            player.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--control-wav", type=lambda p: Path(p).resolve(strict=True), help="Use Waveform Native's 24-second synthetic control WAV")
    parser.add_argument("--band", choices=("mid", "bass", "high"), default="mid")
    parser.add_argument("--axis", choices=("right", "left", "down", "up"), default="right")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            measure(args.exe, args.output, args.band, args.control_wav, args.axis)
        except Exception:
            if args.output.is_dir():
                (args.output / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        fixture = ["--control-wav", str(args.control_wav)] if args.control_wav else []
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.output), "--band", args.band, "--axis", args.axis, *fixture, "--worker"], 90)

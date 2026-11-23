"""Capture the real GPUI window on a private desktop with a two-second post-tap reference."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import wave

import numpy as np
import qualifySpectrum as q
from privateDesktop import run


# name, signal, axes, ink, physical window width/height, injected DPI (0 = actual)
CASES = [
    ("ridges", "ridges", "MidSide", "Density", 1860, 1170, 0),
    ("mono", "mono", "MidSide", "Density", 1860, 1170, 0),
    ("opposed", "opposed", "MidSide", "Density", 1860, 1170, 0),
    ("wide", "wide", "XY", "Density", 1860, 1170, 0),
    ("trace", "wide", "XY", "Trace", 1860, 1170, 0),
    ("music", "music", "MidSide", "Density", 1860, 1170, 0),
    ("transients", "transients", "XY", "Trace", 1860, 1170, 0),
    ("compact", "music", "MidSide", "Density", 1080, 690, 0),
    ("music-wide", "music-wide", "MidSide", "Density", 2560, 1440, 0),
    ("music-narrow", "music-narrow", "MidSide", "Density", 1860, 1170, 0),
    ("scale100", "wide", "XY", "Density", 1240, 780, 96),
    ("scale200-4k", "wide", "XY", "Density", 3840, 2160, 192),
]


def make_fixture(path, signal, music):
    t = np.arange(96000) / 48000
    phase = 2 * np.pi * t
    carrier = .47 * np.sin(97 * phase + .1) + .22 * np.sin(291 * phase)
    other = .42 * np.sin(193 * phase + .3) + .23 * np.sin(307 * phase)
    if signal.startswith("music"):
        raw = subprocess.check_output(["ffmpeg", "-v", "error", "-ss", {"music": "12", "music-wide": "24", "music-narrow": "48"}[signal], "-i", str(music),
                                      "-t", "2", "-ar", "48000", "-ac", "2", "-f", "s16le", "-"])
    else:
        right = {"ridges": carrier, "mono": carrier, "opposed": -carrier, "wide": other, "transients": other}[signal]
        pair = np.stack((carrier, right), axis=1)
        if signal == "ridges":
            side = ((np.arange(len(t)) // 137) % 3 - 1) * .008
            pair[:, 0] += side
            pair[:, 1] -= side
        if signal == "transients":
            pair *= np.exp(-70 * (t % .125))[:, None]
            pair[(t % .125) > .065] = 0
        raw = np.rint(pair * 32767).astype("<i2").tobytes()
    assert len(raw) == 96000 * 4
    with wave.open(str(path), "wb") as wav:
        wav.setparams((2, 2, 48000, 0, "NONE", ""))
        wav.writeframes(bytes(4 * 48000 * 4) + raw + bytes(14 * 48000 * 4))


def capture(args, case):
    name, signal, axes, ink, width, height, dpi = case
    folder = args.output / name
    folder.mkdir()
    source = folder / (signal + ".wav")
    make_fixture(source, signal, args.music)
    profile = q.profile("field", source)
    panel = profile["layout"]["center"]["children"][1]["children"][0]
    panel["info"]["panel"]["state"] = dict(axes=axes, ink=ink, statistics=False)
    (folder / "workspace-v1.json").write_text(json.dumps(profile))
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(folder), DISQRO_REFERENCE_DPI=str(dpi),
               DISQRO_PLAYBACK_TRACE=str(folder / "trace.csv"))
    player = subprocess.Popen([str(args.exe), "--qualify-silent", str(source)], env=env)
    hwnd = None
    try:
        deadline = time.monotonic() + 18
        while time.monotonic() < deadline:
            hwnd = q.find_window(player.pid)
            if hwnd:
                break
            if player.poll() is not None:
                raise RuntimeError(f"{name}: player exited early")
            time.sleep(.05)
        if dpi and hwnd:
            # The private DPI notification updates the platform window. Force a
            # subsequent WM_SIZE so GPUI refreshes its logical viewport as well.
            time.sleep(1.4)
            q.user32.SetWindowPos(hwnd, 0, 0, 0, width - 8, height, 0x414)
        if not hwnd or not q.user32.SetWindowPos(hwnd, 0, 0, 0, width, height, 0x414):
            raise RuntimeError("Private HWND resize failed")
        observed = folder / "observed.f32"
        while not observed.exists() or observed.stat().st_size != 96000 * 8:
            if time.monotonic() > deadline or player.poll() is not None:
                raise RuntimeError(f"{name}: incomplete internal reference")
            time.sleep(.05)
        time.sleep(.25)
        q.screenshot(hwnd, folder / "native.webp")
        with (folder / "trace.csv").open() as data:
            rows = list(csv.DictReader(data))
        if not rows or any(int(r[k]) for r in rows for k in ("capture_drops", "decode_errors", "device_errors")):
            raise RuntimeError("Invalid audio trace")
        scale, plot_x, plot_y, plot_side = map(float, (folder / "geometry.csv").read_text().split(","))
        want_scale = (dpi or q.user32.GetDpiForWindow(hwnd)) / 96
        if scale != want_scale:
            raise RuntimeError(f"{name}: GPUI scale {scale}, expected {want_scale}")
        return dict(case=name, gpui_scale=scale, plot_x=plot_x, plot_y=plot_y, plot_side=plot_side, hwnd_dpi=q.user32.GetDpiForWindow(hwnd), injected_dpi=dpi,
                    requested_width=width, requested_height=height,
                    pcm_sha256=hashlib.sha256(observed.read_bytes()).hexdigest())
    finally:
        if hwnd:
            q.user32.PostMessageW(hwnd, 0x10, 0, 0)
        try:
            player.wait(timeout=6)
        except subprocess.TimeoutExpired:
            player.terminate()
            player.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("exe", "music", "output"):
        parser.add_argument(key, type=lambda p: Path(p).resolve())
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--case", action="append", choices=[case[0] for case in CASES])
    args = parser.parse_args()
    if args.desktop_worker:
        import traceback
        try:
            with args.exe.open("rb") as binary:
                binary_sha256 = hashlib.file_digest(binary, "sha256").hexdigest()
            for case in CASES:
                if args.case and case[0] not in args.case:
                    continue
                row = dict(binary_sha256=binary_sha256, **capture(args, case))
                with (args.output / "captures.csv").open("a", newline="") as out:
                    writer = csv.DictWriter(out, fieldnames=row)
                    if not out.tell():
                        writer.writeheader()
                    writer.writerow(row)
        except Exception:
            (args.output / "failure.txt").write_text(traceback.format_exc())
            raise
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 240)

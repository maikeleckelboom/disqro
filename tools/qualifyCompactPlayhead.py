"""Measure the compact waveform playhead in a silent, private Windows desktop.

Plays a generated recording muted and records, per drawn frame, the device
x position of the compact playhead together with Player and strip renders.
It reports how often the playhead moves, its steps in device pixels, and
whether it stops while paused or hidden. With --deck it also opens a
detailed waveform, so both playheads run on one clock.

    python tools/qualifyCompactPlayhead.py <disqro.exe> <output> [--deck] [--seconds 6]
"""
import argparse
import ctypes as C
import json
import math
import os
from pathlib import Path
import statistics
import struct
import subprocess
import sys
import time
import traceback
import wave

from privateDesktop import run
from qualifySpectrum import find_window, profile, user32
from qualifyWaveform import counter_frequency, performance_counter, settings
from qualifyWaveformMotion import quantiles, real, records


def tone(path, seconds, rate=44100):
    frames = bytearray()
    for i in range(seconds * rate):
        value = int(9000 * math.sin(2 * math.pi * 220 * i / rate) * (0.4 + 0.6 * ((i // rate) % 4) / 3))
        frames += struct.pack("<hh", value, value)
    with wave.open(str(path), "wb") as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(bytes(frames))


def between(rows, start, end):
    return [row for row in rows if start <= row.qpc < end]


def agreement(rows, frames):
    """Device-pixel distance between the compact and detailed playheads
    drawn in the same frame, both mapped to the recording's length."""
    compact, deck = {}, {}
    for row in rows:
        if row.stage == "wave_compact_cursor" and row.frame:
            compact[row.frame] = real(row.value) / row.detail
        elif row.stage == "wave_playhead" and row.frame:
            deck[row.frame] = real(row.value) / frames
    width = next((row.detail for row in rows if row.stage == "wave_compact_cursor"), 0)
    shared = sorted(set(compact) & set(deck))
    return dict(shared_frames=len(shared),
                distance_px=quantiles([abs(compact[f] - deck[f]) * width for f in shared]))


def cursor_report(rows, frequency, seconds, duration):
    cursor = [row for row in rows if row.stage == "wave_compact_cursor"]
    xs = [real(row.value) for row in cursor]
    widths = sorted({row.detail for row in cursor})
    steps = [b - a for a, b in zip(xs, xs[1:])]
    moving = [step for step in steps if step]
    expected = widths[-1] / duration if widths and duration else None
    intervals = [(b.qpc - a.qpc) * 1000 / frequency for a, b in zip(cursor, cursor[1:])]
    return dict(
        draws=len(cursor),
        draws_per_second=round(len(cursor) / seconds, 1),
        device_widths=widths,
        backward_steps=sum(step < -1e-9 for step in steps),
        repeated_positions=len(steps) - len(moving),
        step_px=quantiles([abs(step) for step in moving]),
        whole_pixel_steps=sum(abs(step - round(step)) < 1e-6 for step in moving),
        subpixel_positions=sum(abs(x - round(x)) > 1e-3 for x in xs),
        speed_px_per_s=(round((xs[-1] - xs[0]) / ((cursor[-1].qpc - cursor[0].qpc) / frequency), 3)
                        if len(cursor) > 1 else None),
        expected_px_per_s=round(expected, 3) if expected else None,
        draw_interval_ms=quantiles(intervals),
        player_renders=sum(row.stage == "player_render" for row in rows),
        strip_renders=sum(row.stage == "wave_strip_render" for row in rows),
        deck_draws=sum(row.stage == "wave_motion" for row in rows),
        overview_paints=sum(row.stage in ("wave_compact_track", "wave_compact_disc") for row in rows),
    )


def press(hwnd, key):
    user32.PostMessageW(hwnd, 0x0100, key, 0)
    user32.PostMessageW(hwnd, 0x0101, key, 0xC0000000)


def measure(args):
    args.output.mkdir(parents=True, exist_ok=False)
    source = args.output / "Playhead Tone.wav"
    tone(source, args.duration)
    config = settings(source, 1) if args.deck else profile("baseline", source)
    config["volume"] = 0.0
    config["full_cd"] = False
    (args.output / "workspace-v1.json").write_text(json.dumps(config), encoding="utf-8")
    stages = args.output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PRESENTATION_STAGES=str(stages),
               DISQRO_PLAYBACK_TRACE=str(args.output / "trace.csv"))
    frequency = counter_frequency()
    phases = {}
    with (args.output / "player.log").open("x", encoding="utf-8") as log:
        process = subprocess.Popen([str(args.exe), "--qualify-silent", str(source)], env=env,
                                   stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            hwnd = None
            while time.monotonic() < deadline:
                hwnd = hwnd or find_window(process.pid)
                if hwnd and (any(row.stage == "wave_compact_cursor" for row in records(stages))
                             or args.baseline and time.monotonic() > deadline - 24):
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("the compact playhead never drew")
            time.sleep(1.0)
            start = performance_counter()
            time.sleep(args.seconds)
            phases["playing"] = (start, performance_counter())
            # Space pauses playback through the workspace's key binding.
            press(hwnd, 0x20)
            time.sleep(0.6)
            start = performance_counter()
            time.sleep(2.0)
            phases["paused"] = (start, performance_counter())
            press(hwnd, 0x20)
            time.sleep(0.6)
            user32.ShowWindow(hwnd, 6)  # Minimize.
            time.sleep(0.6)
            start = performance_counter()
            time.sleep(2.0)
            phases["hidden"] = (start, performance_counter())
            user32.ShowWindow(hwnd, 9)  # Restore.
            time.sleep(0.8)
            start = performance_counter()
            time.sleep(2.0)
            phases["resumed"] = (start, performance_counter())
        finally:
            if hwnd:
                user32.PostMessageW(hwnd, 0x0010, 0, 0)
            try:
                process.wait(10)
            except subprocess.TimeoutExpired:
                process.kill()
    rows = records(stages)
    report = {name: cursor_report(between(rows, a, b), frequency, (b - a) / frequency, args.duration)
              for name, (a, b) in phases.items()}
    if args.deck:
        report["agreement"] = agreement(between(rows, *phases["playing"]), args.duration * 44100)
    (args.output / "result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--deck", action="store_true")
    parser.add_argument("--baseline", action="store_true",
                        help="measure a build without the retained playhead")
    parser.add_argument("--seconds", type=float, default=6.0)
    parser.add_argument("--duration", type=int, default=90)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            measure(args)
        except BaseException:
            output = args.output if args.output.is_dir() else args.output.parent
            (output / "qualification-error.log").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        command = [sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.output),
                   "--seconds", str(args.seconds), "--duration", str(args.duration), "--worker"]
        if args.deck:
            command.append("--deck")
        if args.baseline:
            command.append("--baseline")
        run(command, 120)

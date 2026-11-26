"""Muted motion journey for the detailed Waveform on a private desktop.

Plays a recording with one following deck, zooms out to 120 s visible and
then in one wheel notch at a time, and holds each reported duration
(120.000, 78.846 and 51.805 s) and 8 s. Each retained deck draw records the
continuous device origin it shows (`wave_motion`), each frame records how
long after the scheduling compositor tick it sampled the renderer clock
(`wave_frame`), and the platform records its VSync waits. Builds without
`wave_motion` are measured from `wave_playhead` with the integer origin
they draw and the deck width from `--width`.

This is a hidden, muted qualification. It measures what the application
draws and when it samples; it is not a measurement of physical display
cadence.
"""

import argparse
import csv
import json
import os
from pathlib import Path
import statistics
import struct
import subprocess
import sys
import time

from privateDesktop import run
from qualifySpectrum import find_window, user32
from qualifyWaveform import counter_frequency, performance_counter, settings, wheel

# Wheel notches from the 8 s default: seven out clamp at 120 s, then one in
# per reported duration, then back to 8 s.
STEPS = [("120.000", [-120] * 7), ("78.846", [120]), ("51.805", [120]), ("8.000", [-120] * 2 + [120] * 6)]
DECK = (0.62, 0.3)


def real(bits):
    return struct.unpack("<d", struct.pack("<Q", bits))[0]


def records(path):
    if not path.is_file():
        return []
    with path.open(newline="", encoding="utf-8") as data:
        return [(int(row["qpc"]), int(row["ticks"]), row["stage"], int(row["value"]), int(row["detail"]))
                for row in csv.DictReader(data) if all(row.values())]


def quantiles(values):
    if not values:
        return None
    ordered = sorted(values)
    pick = lambda q: ordered[min(len(ordered) - 1, round(q * (len(ordered) - 1)))]
    return dict(p50=pick(0.5), p95=pick(0.95), p99=pick(0.99), max=ordered[-1])


def analyze(rows, start, end, frequency, width):
    window = [row for row in rows if start <= row[0] < end]
    seconds = (end - start) / frequency
    motion = [(row[0], real(row[3]), row[4]) for row in window if row[2] == "wave_motion"]
    if not motion:
        # Earlier builds draw the integer origin of the viewport's left edge.
        motion = [(row[0], round((real(row[3]) / real(row[4]) - 0.5) * width), width)
                  for row in window if row[2] == "wave_playhead"]
    origins = [origin for _, origin, _ in motion]
    steps = [b - a for a, b in zip(origins, origins[1:])]
    moving = [step for step in steps if step != 0]
    mean = statistics.fmean(steps) if steps else 0
    intervals = [(b[0] - a[0]) * 1000 / frequency for a, b in zip(motion, motion[1:])]
    lags = [row[4] / 1000 for row in window if row[2] == "wave_frame"]
    vsync = [row for row in window if row[2] == "vsync_wait"]
    return dict(
        seconds=round(seconds, 3),
        retained_draws_per_second=round(len(motion) / seconds, 1),
        distinct_origins_per_second=round((1 + len(moving)) / seconds, 1) if motion else 0,
        repeated_origins=len(steps) - len(moving),
        mean_step_px=mean,
        step_cv=(statistics.pstdev(steps) / abs(mean)) if len(steps) > 1 and mean else None,
        step_px=quantiles([abs(step) for step in steps]),
        draw_interval_ms=quantiles(intervals),
        sampling_lag_after_tick_ms=quantiles(lags),
        vsync_waits=len(vsync),
        compositor_ticks=sum(1 for row in vsync if row[3] == 1),
        vsync_wait_ms=quantiles([row[1] * 1000 / frequency for row in vsync]),
    )


def measure(args):
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "workspace-v1.json").write_text(json.dumps(settings(args.source, 1)), encoding="utf-8")
    stages = args.output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PRESENTATION_STAGES=str(stages),
               DISQRO_PLAYBACK_TRACE=str(args.output / "trace.csv"))
    frequency = counter_frequency()
    phases = []
    with (args.output / "player.log").open("x", encoding="utf-8") as log:
        process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)], env=env,
                                   stdout=log, stderr=log)
        hwnd = None
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                hwnd = hwnd or find_window(process.pid)
                if hwnd and any(row[2] in ("wave_motion", "wave_playhead") for row in records(stages)):
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("no retained deck draw")
            time.sleep(1.5)
            for label, notches in STEPS:
                for delta in notches:
                    wheel(hwnd, *DECK, delta)
                    time.sleep(0.12)
                time.sleep(1.0)
                start = performance_counter()
                time.sleep(args.seconds)
                phases.append((label, start, performance_counter()))
        finally:
            if hwnd:
                user32.PostMessageW(hwnd, 0x10, 0, 0)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait()
    rows = records(stages)
    report = dict(exe=str(args.exe), source=str(args.source), phases={
        label: analyze(rows, start, end, frequency, args.width) for label, start, end in phases})
    widths = {row[4] for row in rows if row[2] == "wave_motion"}
    report["deck_widths"] = sorted(widths)
    (args.output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("source", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("output", type=lambda path: Path(path).resolve())
    parser.add_argument("--seconds", type=float, default=4.0)
    parser.add_argument("--width", type=int, default=0,
                        help="device width of the deck, for builds without wave_motion")
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_worker:
        measure(args)
    else:
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 120)


if __name__ == "__main__":
    main()

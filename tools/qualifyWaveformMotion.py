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
import shutil
import statistics
import struct
import subprocess
import sys
import time

from privateDesktop import run
from qualifySpectrum import GpuTime, W, find_window, kernel, sample, user32
from qualifyWaveform import counter_frequency, gpu_memory, performance_counter, settings, wheel

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


def residency(rows, start, end, frequency):
    window = [row for row in rows if start <= row[0] < end]
    completed = [row for row in window if row[2] == "wave_evidence_completed"]
    footprint = {}
    for kind in ("geometry", "spectral", "pcm"):
        observations = [row for row in window if row[2] == "wave_residency_" + kind]
        exact_views = set()
        misses_after_exact = 0
        samples = 0
        misses = 0
        for _, _, _, exact, value in observations:
            view, missing = divmod(value, 1 << 32)
            samples += bool(exact or missing)
            misses += bool(missing)
            misses_after_exact += bool(missing and view in exact_views)
            if exact and not missing:
                exact_views.add(view)
        footprint[kind] = dict(
            samples=samples, misses=misses, misses_after_first_exact=misses_after_exact,
            panels_with_exact=len(exact_views),
        )
    return dict(
        footprint=footprint,
        demand_changes=sum(row[2] == "wave_evidence_demand" for row in window),
        page_requests=sum(row[2] == "wave_evidence_requested" for row in window),
        pages_consumed=sum(row[2] == "wave_evidence_consumed" for row in window),
        completed=len(completed),
        canceled=sum(bool(row[4] & 8) for row in completed),
        missed_forecast_deadlines=sum((row[4] & 3) == 3 for row in completed),
        speculative_completions=sum((row[4] & 5) == 1 for row in completed),
        service_time_ms=quantiles([row[1] * 1000 / frequency for row in completed if row[4] & 1]),
        resident_and_reserved_bytes=max((row[3] + row[4] for row in window
                                        if row[2] == "wave_evidence_bytes"), default=0),
        scenes=sum(row[2] == "wave_scene" for row in window),
        approximate_scenes=sum(row[2] == "wave_scene" and row[4] > 0 for row in window),
        scene_paints=sum(row[2] == "wave_deck_paint" and row[3] == 1 for row in window),
        successful_presents=sum(row[2] == "present" and row[4] == 0 for row in window),
        spans_frames=sorted({round(real(row[3]), 4) for row in window if row[2] == "wave_scene"}
                            | {round(real(row[4]), 4) for row in window if row[2] == "wave_playhead"}),
    )


def sweep():
    yield "wheel_00", [-120] * 7
    # 120 seconds, -0.42 log span per wheel step, clipped to 32 frames.
    for notch in range(1, 33):
        yield f"wheel_{notch:02}", [120]


def measure(args):
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "workspace-v1.json").write_text(json.dumps(settings(args.source, args.panels)), encoding="utf-8")
    if args.warm_from:
        shutil.copytree(args.warm_from / "Cache", args.output / "Cache")
    stages = args.output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PRESENTATION_STAGES=str(stages),
               DISQRO_PLAYBACK_TRACE=str(args.output / "trace.csv"))
    if args.reactive:
        env["DISQRO_RESIDENCY_REACTIVE"] = "1"
    frequency = counter_frequency()
    phases = []
    with (args.output / "player.log").open("x", encoding="utf-8") as log:
        process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)], env=env,
                                   stdout=log, stderr=log)
        hwnd = None
        thread = None
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                hwnd = hwnd or find_window(process.pid)
                if hwnd and any(row[2] in ("wave_motion", "wave_playhead") for row in records(stages)):
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("no retained deck draw")
            kernel.OpenThread.restype = W.HANDLE
            thread = W.HANDLE(kernel.OpenThread(0x0800, False, user32.GetWindowThreadProcessId(hwnd, None)))
            if not thread:
                raise RuntimeError("window thread is unavailable")
            gpu = GpuTime(process.pid)
            first = sample(process, thread)
            private_memory = [first[3]]
            time.sleep(1.5)
            decks = (DECK,) if args.panels == 1 else ((0.62, 0.26), (0.62, 0.65))
            for label, notches in sweep() if args.residency else STEPS:
                for delta in notches:
                    for deck in decks:
                        wheel(hwnd, *deck, delta)
                    time.sleep(0.12)
                time.sleep(1.0)
                start = performance_counter()
                time.sleep(args.seconds)
                phases.append((label, start, performance_counter()))
                private_memory.append(sample(process, thread)[3])
            if args.reverse:
                # Return the first deck to an envelope tier and repeatedly
                # pan backward through it without moving the playback clock.
                for _ in range(20):
                    wheel(hwnd, *decks[0], -120)
                time.sleep(1)
                start = performance_counter()
                for _ in range(60):
                    wheel(hwnd, *decks[0], -120, horizontal=True)
                    time.sleep(0.05)
                phases.append(("reverse_pan", start, performance_counter()))
            last = sample(process, thread)
            elapsed = last[0] - first[0]
            costs = dict(process_cpu_core_percent=100 * (last[1] - first[1]) / elapsed,
                         ui_cpu_core_percent=100 * (last[2] - first[2]) / elapsed,
                         peak_sampled_private_mib=max(private_memory + [last[3]]),
                         busiest_gpu_engine_percent=gpu.finish(), gpu_memory=gpu_memory(process.pid))
        finally:
            if thread:
                kernel.CloseHandle(thread)
            if hwnd:
                user32.PostMessageW(hwnd, 0x10, 0, 0)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait()
    rows = sorted(records(stages))
    report = dict(exe=str(args.exe), source=str(args.source), phases={
        label: analyze(rows, start, end, frequency, args.width) for label, start, end in phases})
    widths = {row[4] for row in rows if row[2] == "wave_motion"}
    report["deck_widths"] = sorted(widths)
    if args.residency:
        report["residency"] = {label: residency(rows, start, end, frequency)
                               for label, start, end in phases}
        report["costs"] = costs
        report["trace_records_dropped"] = sum(row[3] for row in rows if row[2] == "dropped")
    if args.reverse:
        _, start, end = phases[-1]
        positions = [real(row[3]) for row in rows
                     if start <= row[0] < end and row[2] == "wave_playhead" and real(row[4]) > 32.0]
        reverse_steps = sum(a > b for a, b in zip(positions, positions[1:]))
        report["reverse_pan"] = dict(observations=len(positions), backward_steps=reverse_steps,
                                     traveled_frames=positions[0] - positions[-1] if positions else 0)
        if reverse_steps < 10 or report["reverse_pan"]["traveled_frames"] <= 0:
            raise RuntimeError("The panning fixture did not move backward")
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
    parser.add_argument("--residency", action="store_true", help="Sweep the full wheel range and report page residency")
    parser.add_argument("--panels", type=int, choices=(1, 2), default=1)
    parser.add_argument("--reactive", action="store_true", help="Disable read-ahead in the qualification binary")
    parser.add_argument("--warm-from", type=Path, help="Copy the evidence cache from a completed run")
    parser.add_argument("--reverse", action="store_true", help="Pan the first deck backward after the wheel sweep")
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_worker:
        measure(args)
    else:
        timeout = max(120, 70 + (33 if args.residency else len(STEPS)) * (args.seconds + 1.2))
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], timeout)


if __name__ == "__main__":
    main()

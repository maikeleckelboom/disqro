"""Measure hidden, muted waveform motion or summarize an existing trace.

The silent journey tests exact spans and, with --residency, the wheel range
and evidence-tier boundaries. Application draws and successful Present calls
are distinct from observed foreground display updates.
"""

import argparse
from collections import defaultdict
import ctypes as C
import csv
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import struct
import subprocess
import sys
import time
import traceback
from typing import NamedTuple

from privateDesktop import run
from qualifySpectrum import (GpuTime, W, click_control, find_window,
                             kernel, sample, user32)
from qualifyWaveform import (counter_frequency, gpu_memory, performance_counter,
                             settings, wheel)

DECK = (0.62, 0.3)


class Record(NamedTuple):
    qpc: int
    ticks: int
    stage: str
    value: int
    detail: int
    frame: int = 0
    window: int = 0
    panel: int = 0


def real(bits):
    return struct.unpack("<d", struct.pack("<Q", bits))[0]


def signed(bits):
    return struct.unpack("<q", struct.pack("<Q", bits))[0]


def records(path):
    if not path.is_file():
        return []
    with path.open(newline="", encoding="utf-8") as data:
        return [Record(int(row["qpc"]), int(row["ticks"]), row["stage"],
                       int(row["value"]), int(row["detail"]), int(row.get("frame") or 0),
                       int(row.get("window") or 0), int(row.get("panel") or 0))
                for row in csv.DictReader(line for line in data if line.endswith("\n")) if all(row.get(key) for key in
                    ("qpc", "ticks", "stage", "value", "detail"))]


def quantiles(values):
    if not values:
        return None
    ordered = sorted(values)
    pick = lambda q: ordered[min(len(ordered) - 1, round(q * (len(ordered) - 1)))]
    return dict(p50=pick(0.5), p95=pick(0.95), p99=pick(0.99), max=ordered[-1])


def intervals_ms(qpc, frequency):
    return [(b - a) * 1000 / frequency for a, b in zip(qpc, qpc[1:])]


def analyze_panel(rows, frequency, seconds, width):
    motion = []
    frames = {}
    spans = set()
    for row in rows:
        if row.stage in ("wave_playhead", "wave_scene_playhead"):
            spans.add(round(real(row.detail), 5))
        if row.stage in ("wave_motion", "wave_scene_motion"):
            item = (row.qpc, real(row.value), row.detail)
            if row.frame:
                # A scene repaint can replace a retained texture in the same frame.
                frames[row.frame] = item
            else:
                motion.append(item)
    motion.extend(frames.values())
    motion.sort()
    if not motion and width:
        motion = [(row.qpc, round((real(row.value) / real(row.detail) - 0.5) * width), width)
                  for row in rows if row.stage == "wave_playhead"]
    origins = [origin for _, origin, _ in motion]
    steps = [b - a for a, b in zip(origins, origins[1:])]
    moving = [step for step in steps if step != 0]
    mean = statistics.fmean(steps) if steps else 0
    timings = {}
    for name in ("wave_projection", "wave_evidence_projection", "wave_retained_submit",
                 "wave_deck_paint", "wave_scene"):
        selected = [row.ticks * 1000 / frequency for row in rows if row.stage == name]
        if selected:
            timings[name] = {"count": len(selected), "ms": quantiles(selected)}
    clocks = [row for row in rows if row.stage == "wave_clock"]
    uploads = [row for row in rows if row.stage == "shared_device_update"]
    return dict(
        generated=len(motion),
        retained_draws=sum(row.stage == "wave_motion" for row in rows),
        generated_per_second=round(len(motion) / seconds, 1),
        distinct_origins_per_second=round((1 + len(moving)) / seconds, 1) if motion else 0,
        repeated_origins=len(steps) - len(moving),
        backward_steps=sum(step < 0 for step in steps),
        mean_step_px=mean,
        step_cv=statistics.pstdev(steps) / abs(mean) if len(steps) > 1 and mean else None,
        step_px=quantiles([abs(step) for step in steps]),
        draw_interval_ms=quantiles(intervals_ms([qpc for qpc, _, _ in motion], frequency)),
        source_spans_frames=sorted(spans),
        device_widths=sorted({item[2] for item in motion}),
        phase_error_abs_ms=quantiles([abs(signed(row.value)) / 1e6 for row in clocks]),
        prediction_ms=quantiles([row.detail / 1e6 for row in clocks]),
        projected_frames=sum(row.detail > 0 for row in clocks),
        total_upload_bytes=sum(row.value for row in uploads),
        source_upload_bytes=sum(row.detail for row in uploads),
        cpu_stages=timings,
    )


def analyze(rows, start, end, frequency, width):
    section = [row for row in rows if start <= row.qpc < end]
    seconds = (end - start) / frequency
    panels, windows, voices = defaultdict(list), defaultdict(list), defaultdict(list)
    for row in section:
        if row.panel or row.stage in ("wave_motion", "wave_playhead"):
            panels[(row.window, row.panel)].append(row)
        if row.window or (not row.frame and row.stage in ("present", "wave_frame", "window_frame", "gpui_draw")):
            windows[row.window].append(row)
        if row.stage in ("audio_callback", "audio_commit"):
            voices[row.value].append(row)
    panel_reports = {
        f"{window}:{panel}": analyze_panel(values, frequency, seconds, width)
        for (window, panel), values in panels.items()
    }
    window_reports = {}
    for window, values in windows.items():
        present = [row for row in values if row.stage == "present"]
        submitted = [row for row in present if row.detail == 0]
        completed = [row for row in values if row.stage == "gpu_frame_completed" and row.detail == 0]
        targets = {row.frame: row.qpc + signed(row.value) * frequency / 1e9
                   for row in values if row.stage == "wave_frame_target"}
        generated = {row.frame for row in values if row.frame and row.stage in
                     ("wave_motion", "wave_scene_motion")}
        missed_target = [max(0, (row.qpc - targets[row.frame]) * 1000 / frequency)
                         for row in submitted if row.frame in targets]
        window_reports[str(window)] = dict(
            generated_motion_frames=len(generated),
            generated_without_successful_present=len(generated - {row.frame for row in submitted}),
            present_calls=len(present),
            successful_present_calls=len(submitted),
            present_statuses=sorted({row.detail for row in present}),
            present_interval_ms=quantiles(intervals_ms([row.qpc for row in submitted], frequency)),
            present_return_past_intended_ms=quantiles(missed_target),
            worker_wake_to_dispatch_ms=quantiles(
                [row.value / 1e6 for row in values if row.stage == "wave_frame_signal"]
                or [row.detail / 1000 for row in values if row.stage == "wave_frame" and not row.frame]
            ),
            request_to_dispatch_ms=quantiles([row.ticks * 1000 / frequency for row in values
                                             if row.stage == "wave_request_to_dispatch"]),
            frame_callback_ms=quantiles([row.ticks * 1000 / frequency for row in values
                                         if row.stage == "wave_frame"]),
            gpu_execution_ms=quantiles([row.value / 1e6 for row in completed]),
            gpu_completion_poll_ms=quantiles([row.ticks * 1000 / frequency for row in completed]),
            gpu_completions=len(completed),
            gpu_queries_skipped=sum(row.stage == "gpu_query_skipped" for row in values),
            gpu_disjoint=sum(row.stage == "gpu_frame_completed" and row.detail != 0 for row in values),
        )
    waits = [row for row in section if row.stage in ("vsync_wait", "compositor_wait")]
    status = {0: "compositor", 1: "legacy_flush", 2: "legacy_unpaced",
              3: "occluded", 4: "timeout", 5: "failed"}
    outcomes = defaultdict(int)
    periods = []
    for row in waits:
        if row.stage == "compositor_wait":
            outcomes[status[row.value]] += 1
            periods.append(row.detail / 1e6)
        else:
            outcomes["legacy_trace_native" if row.value else "legacy_trace_fallback"] += 1
    audio = {}
    for voice, values in voices.items():
        callbacks = [row for row in values if row.stage == "audio_callback"]
        commits = [row for row in values if row.stage == "audio_commit"]
        cursor = [row.detail for row in commits]
        audio[str(voice)] = dict(
            callbacks=len(callbacks),
            callback_interval_ms=quantiles(intervals_ms([row.qpc for row in callbacks], frequency)),
            committed_blocks=len(commits),
            commit_latency_ms=quantiles([row.ticks * 1000 / frequency for row in commits]),
            source_frames_per_block=quantiles([b - a for a, b in zip(cursor, cursor[1:])]),
        )
    return dict(
        seconds=round(seconds, 3),
        panels=panel_reports,
        gpui_scene_draws=sum(row.stage == 'gpui_draw' for row in section),
        windows=window_reports,
        audio=audio,
        compositor_outcomes=dict(outcomes),
        compositor_period_ms=quantiles(periods),
        compositor_wait_ms=quantiles([row.ticks * 1000 / frequency for row in waits]),
        successful_short_compositor_waits=sum(row.stage == "compositor_wait" and row.value == 0
                                              and row.ticks * 1000 / frequency < 1 for row in waits),
        fallback_sleep_ms=quantiles([row.ticks * 1000 / frequency for row in section
                                     if row.stage == "vsync_fallback_sleep"]),
        trace_records_dropped=sum(row.value for row in section
                                  if row.stage in ("dropped", "audio_trace_dropped")),
    )


def display(path, start, end, frequency):
    if not path.is_file():
        return None
    chains = defaultdict(list)
    with path.open(newline="", encoding="utf-8") as data:
        for row in csv.DictReader(line for line in data if line.endswith("\n")):
            values = {key: int(value) for key, value in row.items() if value}
            if start <= values["qpc"] < end:
                chains[(values.get("window", 0), values.get("chain", 0))].append(values)
    result = {}
    for (window, chain), rows in chains.items():
        valid = [row for row in rows if row["foreground"] and row["statistics_status"] == 0
                 and row["displayed"] and row["present_refresh"] and row["sync_qpc"]]
        advances = [(a, b) for a, b in zip(valid, valid[1:]) if b["displayed"] > a["displayed"]
                    and b["present_refresh"] > a["present_refresh"]]
        refresh_periods = [(b["sync_qpc"] - a["sync_qpc"]) * 1000 /
                           (frequency * (b["sync_refresh"] - a["sync_refresh"]))
                           for a, b in zip(valid, valid[1:]) if b["sync_refresh"] > a["sync_refresh"]
                           and b["sync_qpc"] > a["sync_qpc"]]
        estimated_ms = statistics.median(refresh_periods) if refresh_periods else None
        result[f"{window}:{chain}"] = dict(
            calls=len(rows),
            foreground_dxgi_observations=len(valid),
            distinct_displayed_ids_observed=len({row["displayed"] for row in valid}),
            observed_display_interval_ms=quantiles([
                (b["present_refresh"] - a["present_refresh"]) * estimated_ms for a, b in advances
            ]) if estimated_ms else None,
            repeated_refresh_slots_between_consecutive_present_ids=sum(
                b["present_refresh"] - a["present_refresh"] - 1 for a, b in advances
                if b["displayed"] == a["displayed"] + 1
            ) if valid else None,
            present_id_gaps_unresolved=sum(b["displayed"] - a["displayed"] - 1 for a, b in advances)
            if valid else None,
        )
    return result


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
        for row in observations:
            exact = row.value
            view, missing = divmod(row.detail, 1 << 32)
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
    yield "wheel_00", [-120] * 32
    # 120 seconds, -0.42 log span per wheel step, clipped to 32 frames.
    for notch in range(1, 33):
        yield f"wheel_{notch:02}", [120]


def boundary_spans(width, rate):
    # Mirror the detail planner's tier selection to place test points on both
    # sides of each boundary at the physical width of this fixture.
    def raw_tier(span):
        close = min(1, max(0, math.log2(8 * rate / span)))
        columns = min(49_152, math.ceil(min(16_384, max(128., width * 10.)) * (1 + 3 * close)))
        return (span + columns - 2) // (columns - 1)

    maximum = 120 * rate
    tier = 1
    result = {}
    while tier < raw_tier(maximum):
        low, high = 32, maximum
        while high - low > 1:
            mid = (high + low) // 2
            if raw_tier(mid) <= tier:
                low = mid
            else:
                high = mid
        result[f"geometry_{tier}_below"] = low
        result[f"geometry_{tier}_above"] = high
        tier *= 2
    for label, threshold in [("pcm_begin", min(3 * width, 40_960)), ("pcm_complete", 0.75 * width)]:
        result[f"{label}_below"] = math.ceil(threshold) - 1
        result[f"{label}_above"] = math.ceil(threshold)
    return result


def zoom_controls(hwnd, panels, stages):
    rows = records(stages)
    layout = {row.panel: row for row in rows if row.stage == "wave_deck_layout"}
    if layout:
        positions = [W.POINT(row.value >> 32, row.value & 0xffff_ffff) for row in layout.values()]
    else:
        # The legacy single-deck fixture puts the panel against the right
        # window edge. Its title and zoom row are 40 and 32 logical pixels.
        client = W.RECT()
        user32.GetClientRect(hwnd, C.byref(client))
        scale = next(row.detail for row in rows if row.stage == "wave_deck_paint") / 96
        width = next(row.detail for row in rows if row.stage == "wave_motion")
        positions = [W.POINT(round(client.right - width + 30 * scale), round(56 * scale))]
    if len(positions) != panels:
        raise RuntimeError(f"Expected {panels} waveform zoom controls, observed {len(positions)}")
    positions.sort(key=lambda position: position.y)
    for position in positions:
        user32.ClientToScreen(hwnd, C.byref(position))
    return positions, next(iter(sorted(layout, key=lambda panel: layout[panel].value & 0xffff_ffff)), 0)


def exact_zoom(hwnd, target, positions):
    for position in positions:
        click_control(hwnd, position)
        time.sleep(0.1)
        for char in target:
            user32.SendMessageW(hwnd, 0x0102, ord(char), 1)  # WM_CHAR
        # GPUI handles WM_KEYDOWN in the message loop before dispatch.
        user32.PostMessageW(hwnd, 0x0100, 0x0D, 0x001c0001)  # Enter
        user32.PostMessageW(hwnd, 0x0101, 0x0D, 0xc01c0001)
        time.sleep(0.1)


def measure(args):
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "workspace-v1.json").write_text(json.dumps(settings(args.source, args.panels)), encoding="utf-8")
    if args.warm_from:
        shutil.copytree(args.warm_from / "Cache", args.output / "Cache")
    stages = args.output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PRESENTATION_STAGES=str(stages),
               DISQRO_PLAYBACK_TRACE=str(args.output / "trace.csv"),
               DISQRO_DXGI_TRACE=str(args.output / "dxgi.csv"))
    if args.gpu_timestamps:
        env["DISQRO_GPU_TIMESTAMPS"] = "1"
    if args.buffer_frames:
        env["DISQRO_QUALIFY_BUFFER_FRAMES"] = str(args.buffer_frames)
    if args.reactive:
        env["DISQRO_RESIDENCY_REACTIVE"] = "1"
    frequency = counter_frequency()
    phases = []
    with (args.output / "player.log").open("x", encoding="utf-8") as log:
        process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)], env=env,
                                   stdout=log, stderr=log)
        hwnd = None
        thread = None
        workers = []
        try:
            for _ in range(args.background_workers):
                workers.append(subprocess.Popen(
                    [sys.executable, "-B", "-c", "while True: sum(i*i for i in range(20000))"],
                    creationflags=subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS,
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                ))
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
            def hold(label):
                time.sleep(0.8)
                start = performance_counter()
                time.sleep(args.seconds)
                phases.append((label, start, performance_counter()))
                private_memory.append(sample(process, thread)[3])

            expected = {"8.000": 8 * args.source_rate}
            controls, first_panel = zoom_controls(hwnd, args.panels, stages)
            hold("8.000")
            for duration in (9., 6.):
                exact_zoom(hwnd, f"{duration:g} s", controls)
                expected[f"{duration:.3f}"] = round(duration * args.source_rate)
                hold(f"{duration:.3f}")
            if args.boundaries:
                widths = {row.detail for row in records(stages) if row.stage == "wave_motion"}
                if len(widths) != 1:
                    raise RuntimeError(f"Expected one shared physical deck width, got {sorted(widths)}")
                for label, span in boundary_spans(widths.pop(), args.source_rate).items():
                    exact_zoom(hwnd, f"{span} samples", controls)
                    expected[label] = span
                    hold(label)
            for label, notches in sweep() if args.residency else ():
                for delta in notches:
                    for deck in decks:
                        wheel(hwnd, *deck, delta)
                    time.sleep(0.12)
                hold(label)
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
            for worker in workers:
                worker.terminate()
                worker.wait()
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
        label: analyze(rows, start, end, frequency, args.width) for label, start, end in phases},
        private_desktop=True, qpc_frequency=frequency, costs=costs,
        source_rate=args.source_rate, background_workers=args.background_workers,
        requested_buffer_frames=args.buffer_frames,
        display={label: display(args.output / "dxgi.csv", start, end, frequency)
                 for label, start, end in phases},
        trace_records_dropped=sum(row.value for row in rows
                                  if row.stage in ("dropped", "audio_trace_dropped")))
    errors = []
    for label, span in expected.items():
        panels = report["phases"][label]["panels"]
        if len(panels) != args.panels:
            errors.append(f"{label}: expected {args.panels} identified decks, observed {list(panels)}")
        for panel, metrics in panels.items():
            if metrics["source_spans_frames"] != [span]:
                errors.append(f"{label} on deck {panel}: {metrics['source_spans_frames']} frames")
    if args.residency or args.boundaries:
        report["residency"] = {label: residency(rows, start, end, frequency)
                               for label, start, end in phases}
    if args.reverse:
        _, start, end = phases[-1]
        positions = [real(row[3]) for row in rows
                     if start <= row.qpc < end and row.stage == "wave_playhead"
                     and row.panel == first_panel and real(row.detail) > 32.0]
        reverse_steps = sum(a > b for a, b in zip(positions, positions[1:]))
        report["reverse_pan"] = dict(observations=len(positions), backward_steps=reverse_steps,
                                     traveled_frames=positions[0] - positions[-1] if positions else 0)
        if reverse_steps < 10 or report["reverse_pan"]["traveled_frames"] <= 0:
            errors.append("The panning fixture did not move backward")
    report["qualification_errors"] = errors
    (args.output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    if errors:
        raise RuntimeError("\n".join(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", nargs="?", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("source", nargs="?", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("output", nargs="?", type=lambda path: Path(path).resolve())
    parser.add_argument("--trace", type=Path, help="Summarize an existing stages CSV")
    parser.add_argument("--dxgi", type=Path, help="Pair an existing trace with the DXGI CSV")
    parser.add_argument("--source-rate", type=int, default=48_000)
    parser.add_argument("--boundaries", action="store_true", help="Exercise both sides of geometry and PCM boundaries")
    parser.add_argument("--background-workers", type=int, choices=range(5), default=0,
                        help="Number of competing CPU workers on the private desktop")
    parser.add_argument("--gpu-timestamps", action="store_true")
    parser.add_argument("--buffer-frames", type=int, help="Request a fixed output buffer in the silent build")
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
    if args.trace:
        rows = sorted(records(args.trace))
        if len(rows) < 2 or rows[0].qpc == rows[-1].qpc:
            parser.error("the trace contains no time interval")
        start, end = rows[0].qpc, rows[-1].qpc + 1
        frequency = counter_frequency()
        report = analyze(rows, start, end, frequency, args.width)
        if args.dxgi:
            report["display"] = display(args.dxgi, start, end, frequency)
        print(json.dumps(report, indent=2))
        return
    if not (args.exe and args.source and args.output):
        parser.error("provide exe, source and output for the silent journey")
    if args.desktop_worker:
        try:
            measure(args)
        except Exception:
            if args.output.is_dir():
                with (args.output / "failure.txt").open("x", encoding="utf-8") as failure:
                    traceback.print_exc(file=failure)
            raise
    else:
        phases = (33 if args.residency else 0) + (30 if args.boundaries else 0)
        timeout = max(120, 100 + phases * (args.seconds + 1.4))
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], timeout)
        print((args.output / "report.json").read_text(encoding="utf-8"))


if __name__ == "__main__":
    main()

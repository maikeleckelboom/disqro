"""Exercise the muted player privately or record foreground display events."""

import argparse
import csv
import ctypes as C
import json
import os
from pathlib import Path
import shutil
import statistics
import struct
import subprocess
import sys
import time
import traceback

from private_desktop import run
from qualify_spectrum import (GpuTime, W, click_control, control_centers, find_window, kernel,
                             profile, sample, screenshot, user32)


def settings(source, panels):
    config = profile("baseline", source)
    browser = config["layout"]["center"]

    def wave(identity):
        pane = dict(panel_name="disqro.waveform", children=[], info=dict(panel=dict(id=identity, state=None)))
        return dict(panel_name="TabPanel", children=[pane], info=dict(tabs=dict(active_index=0)))

    def split(axis, sizes, *children):
        return dict(panel_name="StackPanel", children=children, info=dict(stack=dict(axis=axis, sizes=sizes)))

    deck = wave(2) if panels == 1 else split(1, [360, 250], wave(2), wave(3))
    config["layout"]["center"] = split(0, [430, 810], browser, deck)
    return config


def screen_point(hwnd, x, y):
    bounds = W.RECT()
    user32.GetClientRect(hwnd, C.byref(bounds))
    point = W.POINT(int(x * bounds.right), int(y * bounds.bottom))
    user32.ClientToScreen(hwnd, C.byref(point))
    return point


def wheel(hwnd, x, y, delta, horizontal=False):
    point = screen_point(hwnd, x, y)
    position = ((point.y & 0xffff) << 16) | (point.x & 0xffff)
    user32.SendMessageW(hwnd, 0x20e if horizontal else 0x20a, (delta & 0xffff) << 16, position)


def performance_counter():
    ticks = C.c_longlong()
    if not kernel.QueryPerformanceCounter(C.byref(ticks)):
        raise C.WinError()
    return ticks.value


def counter_frequency():
    frequency = C.c_longlong()
    if not kernel.QueryPerformanceFrequency(C.byref(frequency)):
        raise C.WinError()
    return frequency.value


def percentiles(values):
    values = sorted(values)
    return [values[round((len(values) - 1) * q)] for q in (0.5, 0.95, 0.99, 1)] if values else None


def drive(rate, seconds, action):
    start = time.perf_counter()
    for index in range(round(rate * seconds)):
        time.sleep(max(0, start + index / rate - time.perf_counter()))
        action(index)


def composed_statistics(output, phases, refresh_ms):
    report = output / "composed.json"
    if not report.is_file():
        return None
    summary = json.loads(report.read_text(encoding="utf-8"))
    with report.with_suffix(".samples.tsv").open(newline="") as file:
        frequency = int(file.readline().split("\t")[1])
        rows = [tuple(map(int, (row[key] for key in ("qpc", "accumulated", "marker"))))
                for row in csv.DictReader(file, delimiter="\t")]
    stats = {"foreground_lost_samples": summary["foregroundLostSamples"],
             "refresh_ms": refresh_ms, "phases": {}}
    for start, end in zip(phases, phases[1:]):
        observed = [row for row in rows if start["qpc"] <= row[0] < end["qpc"]]
        if not observed:
            continue
        previous = None
        changes = []
        repeated = 0
        skipped_markers = 0
        for qpc, _, marker in observed:
            if marker < 0:
                continue
            if marker == previous:
                repeated += 1
            else:
                if previous is not None:
                    skipped_markers += (marker - previous) % 6 - 1
                changes.append(qpc)
                previous = marker
        intervals = [(b - a) * 1000 / frequency for a, b in zip(changes, changes[1:])]
        elapsed = (end["qpc"] - start["qpc"]) / frequency
        stats["phases"][start["name"]] = {
            "distinct_composed_updates": len(changes),
            "distinct_composed_hz": len(changes) / elapsed,
            "interval_ms_p50_p95_p99_max": percentiles(intervals),
            "gaps_over_two_refreshes": sum(dt > 2 * refresh_ms for dt in intervals),
            "refresh_slots_without_a_new_observed_marker": sum(max(0, round(dt / refresh_ms) - 1) for dt in intervals),
            "repeated_markers_on_observed_desktop_updates": repeated,
            "coalesced_desktop_updates": sum(max(0, count - 1) for _, count, _ in observed),
            "unreadable_samples": sum(marker < 0 for _, _, marker in observed),
            "skipped_marker_steps_modulo_six": skipped_markers,
        }
    return stats


def presentation_stages(path, phases):
    if not path.is_file():
        return None
    with path.open(newline="") as file:
        rows = sorted((dict(row, qpc=int(row["qpc"]), ticks=int(row["ticks"]),
                            value=int(row["value"]), detail=int(row["detail"]))
                       for row in csv.DictReader(line for line in file if line.endswith("\n"))), key=lambda row: row["qpc"])
    frequency = counter_frequency()
    stats = {"dropped_stage_records": sum(row["value"] for row in rows if row["stage"] == "dropped"),
             "device_recreations": sum(row["stage"] == "device_recreated" for row in rows),
             "phases": {}}
    for start, end in zip(phases, phases[1:]):
        groups = {}
        for row in rows:
            if start["qpc"] <= row["qpc"] < end["qpc"]:
                groups.setdefault(row["stage"], []).append(row)
        ms = 1000 / frequency
        elapsed = (end["qpc"] - start["qpc"]) / frequency
        result = stats["phases"][start["name"]] = {
            "seconds": elapsed,
            "stages": {stage: {"count": len(items), "duration_ms_p50_p95_p99_max": percentiles(
                row["ticks"] * ms for row in items)} for stage, items in groups.items()},
        }
        presents = [row for row in groups.get("present", []) if row["detail"] == 0]
        attempts = groups.get("present", [])
        intervals = [(b["qpc"] - a["qpc"]) * ms for a, b in zip(attempts, attempts[1:])]
        result["present_attempts_per_second"] = len(attempts) / elapsed
        result["successful_present_calls_per_second"] = len(presents) / elapsed
        result["occluded_present_calls"] = sum(row["detail"] == 0x087a0001 for row in attempts)
        result["present_attempt_interval_ms_p50_p95_p99_max"] = percentiles(intervals)
        result["present_attempt_gaps_over_two_240hz_refreshes"] = sum(ms > 1000 / 120 for ms in intervals)
        result["gpui_entities_invalidated"] = sum(row["value"] for row in groups.get("gpui_draw", []))
        result["retained_requests"] = sum(row["detail"] for row in groups.get("window_frame", []))
        result["device_upload_bytes"] = sum(row["value"] for row in groups.get("shared_device_update", []))
        result["native_vsync_signals"] = (
            sum(row["value"] for row in groups.get("vsync_wait", []))
            + sum(row["value"] in (0, 1) for row in groups.get("compositor_wait", []))
        )
        playhead = {}
        for row in groups.get("wave_playhead", []):
            frame, span = struct.unpack("<dd", struct.pack("<QQ", row["value"], row["detail"]))
            playhead.setdefault(round(span, 5), []).append(frame)
        result["wave_playhead"] = {span: {
            "samples": len(frames), "distinct_source_positions": len(set(frames)),
            "backward_steps": sum(a > b for a, b in zip(frames, frames[1:])),
            "advance_frames_p50_p95_p99_max": percentiles(b - a for a, b in zip(frames, frames[1:]) if a != b),
        } for span, frames in playhead.items()}
    return stats


def display_statistics(path, phases):
    if not path.is_file():
        return None
    with path.open(newline="") as file:
        rows = [dict((key, int(value)) for key, value in row.items()) for row in csv.DictReader(line for line in file if line.endswith("\n"))]
    frequency = counter_frequency()
    valid = [row for row in rows if row["foreground"] and not row["statistics_status"]
             and row["displayed"] and row["present_refresh"] and row["sync_qpc"]]
    refresh_periods = []
    for a, b in zip(valid, valid[1:]):
        refreshes = b["sync_refresh"] - a["sync_refresh"]
        ticks = b["sync_qpc"] - a["sync_qpc"]
        if refreshes > 0 and ticks > 0:
            refresh_periods.append(1000 * ticks / (frequency * refreshes))
    stats = {"calls": len(rows), "valid_foreground_observations": len(valid)}
    if not refresh_periods:
        return stats
    refresh_ms = statistics.median(refresh_periods)
    stats["refresh_ms"] = refresh_ms
    stats["phases"] = {}
    for start, end in zip(phases, phases[1:]):
        observed = [r for r in valid if start["qpc"] <= r["qpc"] < end["qpc"]]
        advances = []
        present_ids = 0
        previous = None
        for row in observed:
            if previous and row["displayed"] > previous["displayed"]:
                refreshes = row["present_refresh"] - previous["present_refresh"]
                if refreshes > 0:
                    advances.append(refreshes)
                    present_ids += row["displayed"] - previous["displayed"]
            if not previous or row["displayed"] != previous["displayed"]:
                previous = row
        if not advances:
            continue
        intervals = sorted(advance * refresh_ms for advance in advances)
        elapsed = sum(intervals)
        stats["phases"][start["name"]] = {
            "distinct_observed_intervals": len(intervals),
            "displayed_present_id_advance": present_ids,
            "distinct_observed_hz": 1000 * len(intervals) / elapsed,
            "interval_ms_p50_p95_p99_max": percentiles(intervals),
            "intervals_over_one_refresh": sum(advance > 1 for advance in advances),
            "intervals_over_two_refreshes": sum(advance > 2 for advance in advances),
            "unexplained_refreshes_between_samples": max(0, sum(advances) - present_ids),
        }
    return stats


def gpu_memory(pid):
    class Value(C.Structure):
        _fields_ = [("status", W.DWORD), ("bytes", C.c_longlong)]

    class Item(C.Structure):
        _fields_ = [("name", W.LPWSTR), ("value", Value)]

    pdh = C.windll.pdh
    query = W.HANDLE()
    memory = {}
    if pdh.PdhOpenQueryW(None, 0, C.byref(query)):
        return memory
    try:
        for usage in ("Dedicated", "Shared"):
            counter = W.HANDLE()
            path = rf"\GPU Process Memory(pid_{pid}_*)\{usage} Usage"
            if pdh.PdhAddEnglishCounterW(query, path, 0, C.byref(counter)) or pdh.PdhCollectQueryData(query):
                continue
            size, count = W.DWORD(), W.DWORD()
            parameters = (counter, 0x400, C.byref(size), C.byref(count))
            pdh.PdhGetFormattedCounterArrayW(*parameters, None)
            buffer = C.create_string_buffer(size.value)
            if size.value and pdh.PdhGetFormattedCounterArrayW(*parameters, buffer) == 0:
                entries = C.cast(buffer, C.POINTER(Item))
                numbers = [entries[i].value.bytes for i in range(count.value) if entries[i].value.status <= 1]
                if numbers:
                    memory[usage.lower() + "_bytes"] = sum(numbers)
    finally:
        pdh.PdhCloseQuery(query)
    return memory


def measure(args):
    args.output.mkdir(parents=True, exist_ok=False)
    if args.warm_cache:
        shutil.copytree(args.warm_cache, args.output / "Cache")
    trace = args.output / "trace.csv"
    (args.output / "workspace-v1.json").write_text(json.dumps(settings(args.source, args.panels)), encoding="utf-8")
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PLAYBACK_TRACE=str(trace))
    env["DISQRO_PRESENTATION_STAGES"] = str(args.output / "stages.csv")
    env["DISQRO_DXGI_TRACE"] = str(args.output / "dxgi.csv")
    if args.display_probe:
        env["DISQRO_QUALIFY_PRESENT_MARKER"] = "1"
    if args.exercise_host_recovery:
        env["DISQRO_QUALIFY_TARGET_FAILURE"] = "1"
        env["DISQRO_QUALIFY_DEVICE_RECREATION_MS"] = "5000"
    process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)], env=env)
    hwnd = thread = None
    memory = []
    present = None
    probe = None
    phases = []

    def phase(name):
        phases.append({"name": name, "qpc": performance_counter()})

    if args.presentmon:
        with (args.output / "presentmon.log").open("w", encoding="utf-8") as log:
            present = subprocess.Popen(
                [str(args.presentmon), "--process_id", str(process.pid), "--v2_metrics",
                 "--output_file", str(args.output / "presented.csv"), "--qpc_time_ms",
                 "--write_display_metadata", "--no_console_stats", "--terminate_on_proc_exit",
                 "--session_name", f"DisqroStroke{process.pid}"],
                stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW,
            )

    def rows():
        if not trace.is_file():
            return []
        with trace.open(newline="") as data:
            return list(csv.DictReader(data))

    try:
        for _ in range(600):
            hwnd = hwnd or find_window(process.pid)
            if hwnd and any(int(row["callbacks"]) and int(row["wave_paints"]) for row in rows()):
                break
            if process.poll() is not None:
                raise RuntimeError("Player exited before painting a waveform")
            time.sleep(0.05)
        else:
            raise RuntimeError("No committed playback and waveform paint")
        if args.foreground:
            user32.GetForegroundWindow.restype = W.HWND
            if user32.GetForegroundWindow() != hwnd:
                user32.SetForegroundWindow(hwnd)
            for _ in range(1200):
                if user32.GetForegroundWindow() == hwnd:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Disqro did not acquire the foreground window")
        dpi = user32.GetDpiForWindow(hwnd)
        tid = user32.GetWindowThreadProcessId(hwnd, None)
        kernel.OpenThread.restype = W.HANDLE
        thread = W.HANDLE(kernel.OpenThread(0x0800, False, tid))
        if not thread:
            raise C.WinError()

        gpu = GpuTime(process.pid)
        start = sample(process, thread)
        memory.append(start[3])
        screenshot(hwnd, args.output / "overview.png")
        if args.display_probe:
            with (args.output / "desktop-probe.log").open("w", encoding="utf-8") as log:
                probe = subprocess.Popen([str(args.display_probe), str(hwnd), str(4 * args.steady_seconds + 14),
                                          str(args.output / "composed.json")],
                                         stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
            time.sleep(0.25)
        phase("Playback during startup analysis")
        time.sleep(args.steady_seconds)
        phase("Steady playback")
        time.sleep(args.steady_seconds)
        phase("Continuous zoom")
        drive(240, 3, lambda step: wheel(hwnd, args.anchor_fraction, 0.29, 8 if step % 96 < 48 else -8))
        phase("Library browsing")
        for notch in range(72):
            wheel(hwnd, 0.23, 0.35, 120 if notch % 4 else -120)
            if notch % 5 == 0:
                click_control(hwnd, screen_point(hwnd, 0.13, 0.2 + (notch % 6) * 0.05))
            time.sleep(0.022)
        phase("Step zoom to samples")
        for _ in range(8):
            wheel(hwnd, args.anchor_fraction, 0.29, 120)
            wheel(hwnd, 0.23, 0.35, -120)
            memory.append(sample(process, thread)[3])
            time.sleep(0.09)
        time.sleep(0.35)
        screenshot(hwnd, args.output / "detail.png")
        additional = args.detail_notches
        if additional is None:
            additional = 12
        for _ in range(additional):
            wheel(hwnd, args.anchor_fraction, 0.29, 480)
            memory.append(sample(process, thread)[3])
            time.sleep(0.055)
        phase("Extreme zoom playback")
        time.sleep(args.steady_seconds)
        screenshot(hwnd, args.output / "samples.png")
        gpu_residency = gpu_memory(process.pid)
        phase("Continuous pan")
        drive(240, 3, lambda step: wheel(hwnd, args.anchor_fraction, 0.29,
                                        12 if step % 96 < 48 else -12, horizontal=True))
        phase("Resize")
        frame = W.RECT()
        user32.GetWindowRect(hwnd, C.byref(frame))
        for width, height in [(720, 460), (1240, 780), (950, 650), (720, 460)] * 3:
            height = round(height * dpi / 96)
            # Keep the Desktop Duplication probe inside the lower-left marker.
            if not user32.SetWindowPos(hwnd, 0, frame.left, frame.bottom - height, round(width * dpi / 96), height, 0x14):
                raise C.WinError()
            time.sleep(0.045)
        time.sleep(1.1)
        screenshot(hwnd, args.output / "compact.png")
        click_control(hwnd, control_centers(hwnd, ("Pause",))[0])
        time.sleep(0.5)
        phase("Paused idle")
        time.sleep(args.steady_seconds)
        phase("End")
        end = sample(process, thread)
        memory.append(end[3])
        observations = rows()
        stats = dict(
            dpi=dpi,
            panels=args.panels,
            duration_seconds=end[0] - start[0],
            process_cpu_seconds=end[1] - start[1],
            ui_cpu_seconds=end[2] - start[2],
            max_private_mib=max(memory),
            busiest_gpu_engine_percent=gpu.finish(),
            gpu_memory_at_detail=gpu_residency,
            callbacks=sum(int(row["callbacks"]) for row in observations),
            max_callback_permille=max(int(row["callback_max_permille"]) for row in observations),
            max_wave_paint_us=max(int(row["wave_paint_max_us"]) for row in observations),
            wave_gpu_draws=sum(int(row.get("wave_gpu_draws", 0)) for row in observations),
            wave_source_upload_bytes=sum(int(row.get("wave_source_upload_bytes", 0)) for row in observations),
            max_input_to_draw_us=max(int(row.get("wave_input_to_draw_max_us", 0)) for row in observations),
            first_data_us=max(int(row["first_data_us"]) for row in observations),
            first_wave_to_paint_us=max(int(row["first_wave_to_paint_us"]) for row in observations),
            max_retained_geometry_bytes=max(int(row["retained_geometry_bytes"]) for row in observations),
            page_misses=sum(int(row["page_misses"]) for row in observations),
            page_hits=sum(int(row["page_hits"]) for row in observations),
        )
        errors = ["decode_errors", "device_errors", "discarded_device_errors", "wave_gpu_errors"]
        for error in errors:
            stats[error] = sum(int(row.get(error, 0)) for row in observations)
        (args.output / "report.json").write_text(json.dumps(stats, indent=2), encoding="utf-8")
        (args.output / "phases.json").write_text(json.dumps(phases, indent=2), encoding="utf-8")
        if not stats["callbacks"] or any(stats[x] for x in errors[:-1]) or stats["wave_gpu_errors"] != int(args.exercise_host_recovery):
            raise RuntimeError("Playback trace failed during the waveform journey")
    finally:
        if probe:
            try:
                probe.wait(timeout=12)
            except subprocess.TimeoutExpired:
                probe.terminate()
                probe.wait()
        if thread:
            kernel.CloseHandle(thread)
        if hwnd:
            user32.PostMessageW(hwnd, 0x10, 0, 0)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait()
        if (args.output / "report.json").is_file():
            stats["presentation_stages"] = presentation_stages(args.output / "stages.csv", phases)
            stats["dxgi"] = display_statistics(args.output / "dxgi.csv", phases)
            stats["composed"] = composed_statistics(args.output, phases, (stats["dxgi"] or {}).get("refresh_ms", 1000 / 240))
            (args.output / "report.json").write_text(json.dumps(stats, indent=2), encoding="utf-8")
        if present:
            try:
                present.wait(timeout=8)
            except subprocess.TimeoutExpired:
                present.terminate()
                present.wait()


def qualify_deck_lifecycle(args):
    output = args.output
    output.mkdir(parents=True, exist_ok=False)
    if args.warm_cache:
        shutil.copytree(args.warm_cache, output / "Cache")
    config = settings(args.source, 2)
    center = config["layout"]["center"]
    browser, decks = center["children"]
    first, second = [group["children"][0] for group in decks["children"]]
    first["info"]["panel"]["name"] = "First deck"
    second["info"]["panel"]["name"] = "Second deck"
    queue = dict(panel_name="disqro.queue", children=[], info=dict(panel=dict(id=4, state=None)))
    tabs = dict(panel_name="TabPanel", children=[queue, first, second],
                info=dict(tabs=dict(active_index=0)))
    center["children"] = [browser, tabs]
    (output / "workspace-v1.json").write_text(json.dumps(config), encoding="utf-8")
    trace, stages = output / "trace.csv", output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(output), DISQRO_PLAYBACK_TRACE=str(trace),
               DISQRO_PRESENTATION_STAGES=str(stages), DISQRO_QUALIFY_TARGET_FAILURE="1")

    def rows(path):
        if not path.is_file():
            return []
        with path.open(newline="", encoding="utf-8") as file:
            return [row for row in csv.DictReader(file) if None not in row.values()]

    def matching(stages_after, *names):
        return [row for row in rows(stages) if row["stage"] in names
                and int(row["qpc"]) >= stages_after]

    def wait_for(probe, description):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            result = probe()
            if result:
                return result
            time.sleep(0.04)
        raise RuntimeError(f"Timed out waiting for {description}")

    def click(label):
        click_control(hwnd, control_centers(hwnd, [label])[0])

    def close_tab(label):
        p = control_centers(hwnd, [label])[0]
        user32.ScreenToClient(hwnd, C.byref(p))
        position = (p.y << 16) | (p.x & 0xffff)
        for message, key in [(0x200, 0), (0x204, 2), (0x205, 0)]:
            user32.SendMessageW(hwnd, message, key, position)
        click("Close Tab")

    def inactive_stages():
        # Allow a previously queued frame, then observe the inactive interval.
        time.sleep(0.4)
        since = performance_counter()
        time.sleep(0.65)
        return matching(since, "wave_request", "wave_request_to_dispatch", "wave_compact_cursor",
                        "wave_playhead", "wave_deck_paint")

    hwnd = None
    with (output / "player.log").open("x", encoding="utf-8") as log:
        process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)],
                                   env=env, stdout=log, stderr=log)
        try:
            hwnd = wait_for(lambda: find_window(process.pid), "the private player window")
            wait_for(lambda: any(int(row["callbacks"]) for row in rows(trace)), "playback")
            click("Pause")
            play = control_centers(hwnd, ["Play"])[0]
            shown = performance_counter()
            click("First deck")

            def recovered():
                draws = matching(shown, "wave_deck_paint")
                if draws and draws[0]["value"] == "0":
                    return next(((int(draws[0]["qpc"]), int(row["qpc"])) for row in draws
                                 if row["value"] == "1"), None)
                return None

            failure, recovery = wait_for(recovered, "paused deck recovery without input")
            recovery_ms = (recovery - failure) * 1000 / counter_frequency()
            assert recovery_ms >= 450, recovery_ms
            assert not matching(shown, "wave_request", "wave_playhead"), "Paused deck requested animation"
            screenshot(hwnd, output / "recovered-paused.png")

            resumed = performance_counter()
            click_control(hwnd, play)
            wait_for(lambda: matching(resumed, "wave_playhead"), "resumed deck motion")
            user32.ShowWindow(hwnd, 6)  # SW_MINIMIZE
            wait_for(lambda: user32.IsIconic(hwnd), "minimization")
            minimized_frames = len(inactive_stages())
            assert minimized_frames == 0, "Minimized deck still painted"
            restored = performance_counter()
            user32.ShowWindow(hwnd, 9)  # SW_RESTORE
            wait_for(lambda: matching(restored, "wave_playhead"), "restored deck motion")

            previous_dpi = user32.GetDpiForWindow(hwnd)
            for dpi in (96, 192, 144):
                rect = W.RECT()
                user32.GetWindowRect(hwnd, C.byref(rect))
                rect.right = rect.left + round((rect.right - rect.left) * dpi / previous_dpi)
                rect.bottom = rect.top + round((rect.bottom - rect.top) * dpi / previous_dpi)
                moved = performance_counter()
                user32.SendMessageW(hwnd, 0x02e0, dpi | (dpi << 16), C.byref(rect))  # WM_DPICHANGED
                wait_for(lambda: any(row["value"] == "1" and int(row["detail"]) == dpi
                                     for row in matching(moved, "wave_deck_paint")), f"DPI {dpi} paint")
                previous_dpi = dpi

            switched = performance_counter()
            click("Second deck")
            wait_for(lambda: any(row["value"] == "1" for row in matching(switched, "wave_deck_paint")),
                     "second deck paint")
            close_tab("First deck")
            close_tab("Second deck")
            inactive = inactive_stages()
            # The shared frame pump keeps moving the visible compact cursor.
            # Its dispatch count is the number of detailed deck clients.
            closed_frames = sum(row["stage"] in ("wave_playhead", "wave_deck_paint") for row in inactive)
            deck_dispatches = sum(int(row["value"]) for row in inactive if row["stage"] == "wave_request_to_dispatch")
            compact_draws = sum(row["stage"] == "wave_compact_cursor" for row in inactive)
            assert closed_frames == 0 and deck_dispatches == 0, "Closed deck still received frames"
            assert compact_draws > 0, "Visible compact cursor stopped with the last deck"
            previous = int(rows(trace)[-1]["ms"])
            wait_for(lambda: any(int(row["ms"]) > previous and int(row["callbacks"]) > 0
                                 for row in rows(trace)), "playback callbacks after deck closure")
            observations = rows(trace)
            errors = {name: sum(int(row[name]) for row in observations)
                      for name in ("decode_errors", "device_errors", "wave_gpu_errors")}
            assert errors == dict(decode_errors=0, device_errors=0, wave_gpu_errors=1), errors
            report = dict(paused_recovery_ms=round(recovery_ms, 2), minimized_frames=minimized_frames,
                          frames_after_close=closed_frames, deck_dispatches_after_close=deck_dispatches,
                          compact_draws_after_close=compact_draws, closed_tabs=2, synthetic_dpi=[96, 192, 144],
                          playback_after_close=True, **errors)
            (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        finally:
            if hwnd:
                user32.PostMessageW(hwnd, 0x10, 0, 0)
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("source", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("output", type=lambda path: Path(path).resolve())
    parser.add_argument("--warm-cache", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("--anchor-fraction", type=float, default=0.602)
    parser.add_argument("--detail-notches", type=int)
    parser.add_argument("--panels", type=int, choices=(1, 2), default=2)
    parser.add_argument("--steady-seconds", type=float, default=3.0)
    parser.add_argument("--exercise-host-recovery", action="store_true")
    parser.add_argument("--exercise-deck-lifecycle", action="store_true")
    parser.add_argument("--foreground", action="store_true")
    parser.add_argument("--presentmon", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("--display-probe", type=lambda path: Path(path).resolve(strict=True),
                        help="Dekzer present-probe executable for Desktop Duplication with a frame marker")
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 1 <= args.steady_seconds <= 10:
        parser.error("Use phase durations between 1 and 10 seconds")
    if args.display_probe and not args.foreground:
        parser.error("The composed-frame probe requires an explicitly authorized foreground run")
    if args.exercise_deck_lifecycle and (args.foreground or args.exercise_host_recovery):
        parser.error("Run deck lifecycle qualification on its own private desktop")
    if not 0.36 <= args.anchor_fraction < 1. or (args.detail_notches is not None and not 1 <= args.detail_notches <= 24):
        parser.error("The zoom anchor must be inside the waveform pane and the detail step count must be 1–24")
    if not args.desktop_worker and not args.foreground:
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 75)
        print((args.output / "report.json").read_text(encoding="utf-8"))
    else:
        try:
            if args.exercise_deck_lifecycle:
                qualify_deck_lifecycle(args)
            else:
                measure(args)
            if args.foreground:
                print((args.output / "report.json").read_text(encoding="utf-8"))
        except Exception:
            if args.output.is_dir():
                with (args.output / "failure.txt").open("x", encoding="utf-8") as log:
                    traceback.print_exc(file=log)
            raise

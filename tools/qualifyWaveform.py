"""Exercise the muted player privately or record foreground display events."""

import argparse
import csv
import ctypes as C
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time
import traceback

from privateDesktop import run
from qualifySpectrum import GpuTime, W, find_window, kernel, profile, sample, screenshot, user32


def settings(source):
    config = profile("baseline", source)
    browser = config["layout"]["center"]

    def wave(identity):
        pane = dict(panel_name="disqro.waveform", children=[], info=dict(panel=dict(id=identity, state=None)))
        return dict(panel_name="TabPanel", children=[pane], info=dict(tabs=dict(active_index=0)))

    def split(axis, sizes, *children):
        return dict(panel_name="StackPanel", children=children, info=dict(stack=dict(axis=axis, sizes=sizes)))

    config["layout"]["center"] = split(
        0, [430, 810], browser,
        split(1, [360, 250], wave(2), wave(3)),
    )
    return config


def wheel(hwnd, x, y, delta, shift=False):
    bounds = W.RECT()
    user32.GetClientRect(hwnd, C.byref(bounds))
    point = W.POINT(int(x * bounds.right), int(y * bounds.bottom))
    user32.ClientToScreen(hwnd, C.byref(point))
    position = ((point.y & 0xffff) << 16) | (point.x & 0xffff)
    user32.SendMessageW(hwnd, 0x20a, ((delta & 0xffff) << 16) | (0x4 if shift else 0), position)


def performance_counter():
    ticks = C.c_longlong()
    if not kernel.QueryPerformanceCounter(C.byref(ticks)):
        raise C.WinError()
    return ticks.value


def display_statistics(path, phases):
    if not path.is_file():
        return None
    with path.open(newline="") as file:
        rows = [dict((key, int(value)) for key, value in row.items()) for row in csv.DictReader(file)]
    frequency = C.c_longlong()
    if not kernel.QueryPerformanceFrequency(C.byref(frequency)):
        raise C.WinError()
    valid = [row for row in rows if row["foreground"] and not row["statistics_status"]
             and row["displayed"] and row["present_refresh"] and row["sync_qpc"]]
    refresh_periods = []
    for a, b in zip(valid, valid[1:]):
        refreshes = b["sync_refresh"] - a["sync_refresh"]
        ticks = b["sync_qpc"] - a["sync_qpc"]
        if refreshes > 0 and ticks > 0:
            refresh_periods.append(1000 * ticks / (frequency.value * refreshes))
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
            "interval_ms_p50_p95_p99_max": [intervals[round((len(intervals) - 1) * q)] for q in (0.5, 0.95, 0.99, 1)],
            "intervals_over_one_refresh": sum(advance > 1 for advance in advances),
            "intervals_over_two_refreshes": sum(advance > 2 for advance in advances),
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
    (args.output / "workspace-v1.json").write_text(json.dumps(settings(args.source)), encoding="utf-8")
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PLAYBACK_TRACE=str(trace))
    if args.foreground:
        env["DISQRO_DXGI_TRACE"] = str(args.output / "dxgi.csv")
    process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)], env=env)
    hwnd = thread = None
    memory = []
    present = None
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
            for _ in range(400):
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
        phase("Playback and full-CD analysis")
        time.sleep(1.5 if args.foreground else 0.35)
        phase("Continuous zoom and library scrolling")
        if args.foreground:
            for notch in range(96):
                wheel(hwnd, args.anchor_fraction, 0.29, 36 if notch % 24 < 12 else -36)
                if notch % 4 == 0:
                    wheel(hwnd, 0.23, 0.35, 120 if notch % 8 else -120)
                    wheel(hwnd, args.anchor_fraction, 0.65, 36 if notch % 24 < 12 else -36)
                time.sleep(0.018)
        phase("Step zoom and library scrolling")
        for _ in range(8):
            wheel(hwnd, args.anchor_fraction, 0.29, 120)
            wheel(hwnd, 0.23, 0.35, -120)
            memory.append(sample(process, thread)[3])
            time.sleep(0.09)
        time.sleep(0.35)
        screenshot(hwnd, args.output / "detail.png")
        phase("Sample zoom, pan, and rapid reversals")
        additional = args.detail_notches
        if additional is None:
            additional = 12
        for _ in range(additional):
            wheel(hwnd, args.anchor_fraction, 0.29, 480)
            memory.append(sample(process, thread)[3])
            time.sleep(0.055)
        time.sleep(0.35)
        wheel(hwnd, args.anchor_fraction, 0.29, 120, shift=True)
        time.sleep(0.15)
        screenshot(hwnd, args.output / "samples.png")
        gpu_residency = gpu_memory(process.pid)
        for delta in [-120, -120, 120, -120, 120, 120] * 4:
            wheel(hwnd, args.anchor_fraction, 0.29, delta)
            wheel(hwnd, 0.23, 0.35, 120)
            time.sleep(0.045)
        phase("Resize")
        if not user32.SetWindowPos(hwnd, 0, 0, 0, round(720 * dpi / 96), round(460 * dpi / 96), 0x16):
            raise C.WinError()
        time.sleep(1.1)
        phase("End")
        screenshot(hwnd, args.output / "compact.png")
        end = sample(process, thread)
        memory.append(end[3])
        observations = rows()
        stats = dict(
            dpi=dpi,
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
        if not stats["callbacks"] or any(stats[x] for x in errors):
            raise RuntimeError("Playback trace failed during the waveform journey")
    finally:
        if thread:
            kernel.CloseHandle(thread)
        if hwnd:
            user32.PostMessageW(hwnd, 0x10, 0, 0)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait()
        if args.foreground and (args.output / "report.json").is_file():
            stats["dxgi"] = display_statistics(args.output / "dxgi.csv", phases)
            (args.output / "report.json").write_text(json.dumps(stats, indent=2), encoding="utf-8")
        if present:
            try:
                present.wait(timeout=8)
            except subprocess.TimeoutExpired:
                present.terminate()
                present.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("source", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("output", type=lambda path: Path(path).resolve())
    parser.add_argument("--warm-cache", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("--anchor-fraction", type=float, default=0.602)
    parser.add_argument("--detail-notches", type=int)
    parser.add_argument("--foreground", action="store_true")
    parser.add_argument("--presentmon", type=lambda path: Path(path).resolve(strict=True))
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.presentmon and not args.foreground:
        parser.error("PresentMon requires a foreground qualification run")
    if not 0.36 <= args.anchor_fraction < 1. or (args.detail_notches is not None and not 1 <= args.detail_notches <= 24):
        parser.error("The zoom anchor must be inside the waveform pane and the detail step count must be 1–24")
    if not args.desktop_worker and not args.foreground:
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 75)
        print((args.output / "report.json").read_text(encoding="utf-8"))
    else:
        try:
            measure(args)
            if args.foreground:
                print((args.output / "report.json").read_text(encoding="utf-8"))
        except Exception:
            if args.output.is_dir():
                with (args.output / "failure.txt").open("x", encoding="utf-8") as log:
                    traceback.print_exc(file=log)
            raise

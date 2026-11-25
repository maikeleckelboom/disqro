"""Measure a release player on Windows using a fresh profile and a silent post-tap output graph."""
import argparse
import csv
import ctypes as C
from ctypes import wintypes as W
import json
import hashlib
import os
from pathlib import Path
from statistics import median
import subprocess
import time
import sys

from PIL import Image

user32 = C.windll.user32
kernel = C.windll.kernel32
gdi = C.windll.gdi32
user32.SetProcessDpiAwarenessContext(C.c_void_p(-4))


class Memory(C.Structure):
    _fields_ = [("cb", W.DWORD), ("faults", W.DWORD)] + [
        (name, C.c_size_t) for name in (
            "peak_working", "working", "peak_paged", "paged", "peak_nonpaged",
            "nonpaged", "pagefile", "peak_pagefile", "private",
        )
    ]


def thread_seconds(handle):
    if not handle:
        return 0.
    ticks = [C.c_uint64() for _ in range(4)]
    if not kernel.GetThreadTimes(handle, *map(C.byref, ticks)):
        raise C.WinError()
    return (ticks[2].value + ticks[3].value) * 1e-7


def worker_thread(pid):
    class ThreadEntry(C.Structure):
        _fields_ = [(k, W.DWORD) for k in ("size", "usage", "tid", "pid", "base", "delta", "flags")]
    kernel.CreateToolhelp32Snapshot.restype = W.HANDLE
    snapshot = W.HANDLE(kernel.CreateToolhelp32Snapshot(4, 0))
    entry = ThreadEntry()
    entry.size = C.sizeof(entry)
    try:
        more = kernel.Thread32First(snapshot, C.byref(entry))
        while more:
            if entry.pid == pid:
                handle = W.HANDLE(kernel.OpenThread(0x0800, False, entry.tid))
                name = W.LPWSTR()
                if handle and kernel.GetThreadDescription(handle, C.byref(name)) >= 0:
                    match = name.value == "disqro-spectrum"
                    kernel.LocalFree(C.cast(name, W.HANDLE))
                    if match:
                        return handle
                if handle:
                    kernel.CloseHandle(handle)
            more = kernel.Thread32Next(snapshot, C.byref(entry))
    finally:
        kernel.CloseHandle(snapshot)


class GpuTime:
    """PDH per-process engine utilization over the phase, busiest engine."""
    def __init__(self, pid):
        self.pdh = C.windll.pdh
        self.query, self.counter = W.HANDLE(), W.HANDLE()
        self.ok = (self.pdh.PdhOpenQueryW(None, 0, C.byref(self.query)) == 0
                   and self.pdh.PdhAddEnglishCounterW(self.query,
                       rf"\GPU Engine(pid_{pid}_*)\Utilization Percentage", 0, C.byref(self.counter)) == 0
                   and self.pdh.PdhCollectQueryData(self.query) == 0)

    def finish(self):
        class Value(C.Structure):
            _fields_ = [("status", W.DWORD), ("number", C.c_double)]
        class Item(C.Structure):
            _fields_ = [("name", W.LPWSTR), ("value", Value)]
        try:
            if not self.ok or self.pdh.PdhCollectQueryData(self.query):
                return ""
            size, count = W.DWORD(), W.DWORD()
            args = (self.counter, 0x200, C.byref(size), C.byref(count))
            self.pdh.PdhGetFormattedCounterArrayW(*args, None)
            buffer = C.create_string_buffer(size.value)
            if not size.value or self.pdh.PdhGetFormattedCounterArrayW(*args, buffer):
                return ""
            items = C.cast(buffer, C.POINTER(Item))
            valid = [items[i].value.number for i in range(count.value) if items[i].value.status <= 1]
            return round(max(valid), 3) if valid else ""
        finally:
            if self.query:
                self.pdh.PdhCloseQuery(self.query)


def sample(process, thread):
    handle = W.HANDLE(int(process._handle))
    def cpu(query, handle):
        ticks = [C.c_uint64() for _ in range(4)]
        if not query(handle, *map(C.byref, ticks)):
            raise C.WinError()
        return (ticks[2].value + ticks[3].value) * 1e-7
    memory = Memory()
    memory.cb = C.sizeof(memory)
    if not C.windll.psapi.GetProcessMemoryInfo(handle, C.byref(memory), memory.cb):
        raise C.WinError()
    return (time.perf_counter(), cpu(kernel.GetProcessTimes, handle),
            cpu(kernel.GetThreadTimes, thread), memory.private / 1048576)


def find_window(pid):
    matches = []
    @C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
    def visit(hwnd, _):
        owner = W.DWORD()
        user32.GetWindowThreadProcessId(hwnd, C.byref(owner))
        name = C.create_unicode_buffer(96)
        user32.GetClassNameW(hwnd, name, len(name))
        if owner.value == pid and name.value in {"Zed::Window", "GpuiFast::Window"} and user32.IsWindowVisible(hwnd):
            matches.append(hwnd)
        return True
    user32.EnumWindows(visit, 0)
    return next(iter(matches), None)


def screenshot(hwnd, destination):
    rect = W.RECT()
    user32.GetWindowRect(hwnd, C.byref(rect))
    width, height = rect.right - rect.left, rect.bottom - rect.top
    dc = user32.GetDC(hwnd)
    mem = gdi.CreateCompatibleDC(dc)
    bitmap = gdi.CreateCompatibleBitmap(dc, width, height)
    previous = gdi.SelectObject(mem, bitmap)
    try:
        if not user32.PrintWindow(hwnd, mem, 2):
            raise C.WinError()
        gdi.SelectObject(mem, previous)
        from struct import pack
        info = C.create_string_buffer(pack("<IiiHHIIiiII", 40, width, -height, 1, 32, 0, 0, 0, 0, 0, 0))
        pixels = C.create_string_buffer(width * height * 4)
        if gdi.GetDIBits(mem, bitmap, 0, height, pixels, info, 0) != height:
            raise C.WinError()
        image = Image.frombytes("RGB", (width, height), pixels.raw, "raw", "BGRX")
        visible = W.RECT()
        if C.windll.dwmapi.DwmGetWindowAttribute(hwnd, 9, C.byref(visible), C.sizeof(visible)) == 0:
            image = image.crop((visible.left - rect.left, visible.top - rect.top,
                                visible.right - rect.left, visible.bottom - rect.top))
        image.save(destination, lossless=True)
    finally:
        gdi.SelectObject(mem, previous)
        gdi.DeleteObject(bitmap)
        gdi.DeleteDC(mem)
        user32.ReleaseDC(hwnd, dc)


def profile(scenario, source):
    def pane(key, identity, **state):
        return dict(panel_name="disqro." + key, children=[], info=dict(panel=dict(id=identity, state=state)))
    def tabs(*children):
        return dict(panel_name="TabPanel", children=children, info=dict(tabs=dict(active_index=0)))
    def split(axis, sizes, *children):
        return dict(panel_name="StackPanel", children=children, info=dict(stack=dict(axis=axis, sizes=sizes)))
    folder = str(source.parent)
    browser = tabs(pane("browser", 1, location={"Folder": folder}, folder=folder, root=folder))
    spectrum = tabs(pane("spectrum", 2), pane("queue", 3))
    history = tabs(pane("spectrogram", 5))
    field = tabs(pane("stereo-field", 6))
    if scenario in ("heat", "field", "waterfall", "xy"):
        other = {"heat": history, "field": field,
                 "waterfall": tabs(pane("spectrogram", 5, view="Waterfall", span="Short")),
                 "xy": tabs(pane("stereo-field", 6, axes="XY", ink="Trace"))}[scenario]
        center = split(0, [610, 615], browser, other)
    elif scenario in ("pair", "all", "duplicate", "sustained", "fields", "activity"):
        if scenario == "duplicate":
            other = tabs(pane("spectrogram", 7, view="Waterfall", span="Short", palette="Marine"))
        else:
            other = field
        center = split(0, [560, 665], browser, split(1, [300, 300], history, other))
        if scenario == "fields":
            center = split(0, [560, 665], browser, split(1, [300, 300],
                tabs(pane("stereo-field", 6, speed="Fast")),
                tabs(pane("stereo-field", 8, speed="Slow", statistics=False))))
        if scenario in ("all", "sustained", "activity"):
            center = split(0, [560, 665], split(1, [360, 240], browser, spectrum),
                           split(1, [365, 235], history, field))
    elif scenario == "three-views":
        center = split(1, [350, 390], tabs(pane("spectrogram", 5, span="Short")),
                       split(0, [850, 530], tabs(pane("spectrogram", 7, view="Waterfall", span="Short", palette="Marine")), field))
    elif scenario == "hidden":
        center = tabs(browser["children"][0], history["children"][0], field["children"][0])
    elif scenario in ("baseline", "off"):
        center = browser
    elif scenario in ("two", "two-only"):
        second = tabs(pane("spectrum", 4, mode="Side", style="Bands", speed="Slow", resolution="Fine"))
        center = split(0, [605, 620], browser, split(1, [300, 300], spectrum, second))
    else:
        center = split(0, [610, 615], browser, spectrum)
    return dict(volume=1.0, full_cd=True, show_levels=scenario in ("meter", "two", "soak"),
                window=dict(close_to_tray=False), layout=dict(version=3, center=center))


def control_centers(hwnd, labels, *, min_y=None):
    # Query once. Every input message below targets this private HWND.
    script = r"""
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [Windows.Automation.AutomationElement]::FromHandle([IntPtr]HWND)
foreach ($label in (ConvertFrom-Json $env:DISQRO_CONTROL_LABELS)) {
    $match = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $label)
    if ($env:DISQRO_CONTROL_MIN_Y) {
        $element = $null
        $matches = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $match)
        for ($i=0; $i -lt $matches.Count; $i++) {
            $candidate = $matches.Item($i)
            if ($candidate.Current.BoundingRectangle.Y -ge [int]$env:DISQRO_CONTROL_MIN_Y) {
                $element = $candidate
                break
            }
        }
    } else {
        $element = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $match)
    }
    if ($null -eq $element) { throw "Missing native control $label" }
    $r = $element.Current.BoundingRectangle
    Write-Output ([int]($r.X+$r.Width/2))
    Write-Output ([int]($r.Y+$r.Height/2))
}
"""
    output = subprocess.check_output(["powershell", "-NoProfile", "-Command", script.replace("HWND", str(hwnd))],
                                     env=dict(os.environ, DISQRO_CONTROL_LABELS=json.dumps(labels),
                                              DISQRO_CONTROL_MIN_Y='' if min_y is None else str(min_y)),
                                     creationflags=subprocess.CREATE_NO_WINDOW, timeout=15)
    coords = list(map(int, output.split()))
    return [W.POINT(x, y) for x, y in zip(coords[::2], coords[1::2], strict=True)]


def click_control(hwnd, target):
    p = W.POINT(target.x, target.y)
    user32.ScreenToClient(hwnd, C.byref(p))
    position = (p.y << 16) | (p.x & 0xffff)
    for message, key in [(0x200, 0), (0x201, 1), (0x202, 0)]:
        user32.SendMessageW(hwnd, message, key, position)


def activity_step(hwnd, scale, buttons, step):
    p = W.POINT(round(205*scale), round(215*scale))
    user32.ClientToScreen(hwnd, C.byref(p))
    position = (p.y << 16) | (p.x & 0xffff)
    user32.SendMessageW(hwnd, 0x20a, (120 if step % 8 < 4 else 0xff88) << 16, position)
    if step % 4 == 0:
        click_control(hwnd, buttons[(step // 4) % 2])


def measure(args):
    # create_new semantics keep previous traces and user profiles intact.
    args.output.mkdir(parents=True, exist_ok=False)
    trace = args.output / "trace.csv"
    (args.output / "workspace-v1.json").write_text(json.dumps(profile(args.scenario, args.source)))
    def rows():
        if not trace.exists():
            return []
        with trace.open(newline="") as data:
            return list(csv.DictReader(data))
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(args.output), DISQRO_PLAYBACK_TRACE=str(trace))
    process = subprocess.Popen([str(args.exe), "--qualify-silent", str(args.source)], env=env)
    hwnd = thread = worker = None
    try:
        with args.exe.open("rb") as binary:
            build_digest = hashlib.file_digest(binary, "sha256").hexdigest()
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            hwnd = find_window(process.pid)
            if hwnd and any(int(row["callbacks"]) for row in rows()):
                break
            if process.poll() is not None:
                raise RuntimeError("Player exited before playback")
            time.sleep(0.1)
        else:
            raise RuntimeError("No playback callbacks")
        tid = user32.GetWindowThreadProcessId(hwnd, None)
        kernel.OpenThread.restype = W.HANDLE
        thread = W.HANDLE(kernel.OpenThread(0x0800, False, tid))
        if not thread:
            raise C.WinError()
        worker = worker_thread(process.pid)
        if not worker and args.scenario not in ("off", "baseline", "hidden"):
            raise RuntimeError("Could not sample the live analysis thread")
        scale = user32.GetDpiForWindow(hwnd) / 96
        def resize(width, height):
            if not user32.SetWindowPos(hwnd, 0, 0, 0, round(width*scale), round(height*scale), 0x16):
                raise C.WinError()
        dimensions = (1380, 900) if args.scenario == "three-views" else (1240, 780)
        if args.physical_size:
            width, height = args.physical_size
            if not user32.SetWindowPos(hwnd, 0, 0, 0, width, height, 0x416):
                raise C.WinError()
        else:
            resize(*dimensions)
        time.sleep(3)
        if args.scenario == "three-views":
            # Opening a source selects its browser tab. Reveal the top heatmap again.
            click_control(hwnd, control_centers(hwnd, ["Spectrogram"])[0])
        screenshot(hwnd, args.output / "native.webp")
        buttons = control_centers(hwnd, ["Track", "Full CD"]) if args.scenario == "activity" else None
        phases = [("activity" if buttons else "steady", 25)]
        if args.exercise_resize or args.scenario in ("off", "two", "all"):
            phases += [("resize", 15), ("minimized", 20)]
        if args.scenario in ("soak", "sustained"):
            phases = [("sustained", 600)]
        for phase, seconds in phases:
            if phase == "minimized":
                user32.ShowWindow(hwnd, 6)
                time.sleep(1.5)
            gpu = GpuTime(process.pid)
            start_worker = thread_seconds(worker)
            first = len(rows())
            start = sample(process, thread)
            memory = [start[3]]
            retained = []
            resizes = activity_steps = 0
            while time.perf_counter() - start[0] < seconds:
                if process.poll() is not None:
                    raise RuntimeError("Player exited during " + phase)
                if phase == "resize":
                    width = (720, 900, 1240, 1500)[resizes % 4]
                    resize(width, round(width * 0.63))
                    resizes += 1
                if buttons:
                    activity_step(hwnd, scale, buttons, activity_steps)
                    activity_steps += 1
                memory.append(sample(process, thread)[3])
                if phase == "sustained" and time.perf_counter()-start[0] >= 90:
                    retained.append(memory[-1])
                time.sleep(0.06 if phase == "resize" else 0.5)
            end = sample(process, thread)
            memory.append(end[3])
            if retained:
                retained.append(end[3])
            elapsed = end[0] - start[0]
            worker_cpu = 100*(thread_seconds(worker)-start_worker)/elapsed
            observed = rows()[first:]
            gpu_busy = gpu.finish()
            if not observed:
                raise RuntimeError("No playback trace during " + phase)
            if (args.output / "observed.f32").exists():
                raise RuntimeError("Use a live qualification build for timing, never a held reference player")
            result = dict(build=args.scenario, binary_sha256=build_digest, phase=phase, seconds=round(elapsed, 2),
                          worker_cpu=round(worker_cpu, 3), gpu_busiest_engine=gpu_busy, process_cpu=round((end[1]-start[1])*100/elapsed, 2),
                          ui_cpu=round((end[2]-start[2])*100/elapsed, 2),
                          private_start_mib=round(start[3], 2), private_max_mib=round(max(memory), 2),
                          private_end_mib=round(end[3], 2), resizes=resizes)
            result["private_after_90s_mib"] = round(retained[0], 2) if retained else ""
            result["private_after_90s_max_mib"] = round(max(retained), 2) if retained else ""
            for key in ("switches", "callbacks", "commands", "opens", "voices", "device_errors", "decode_errors",
                        "discarded_device_errors", "unchanged_ticks", "capture_frames", "capture_drops",
                        "fft_pairs", "spectrum_paints", "spectrum_updates", "wave_paints", "stft_columns", "visual_updates", "visual_paints", "texture_bytes", "browser_prepaints"):
                result[key] = sum(int(row.get(key, 0)) for row in observed)
            for key in ("callback_max_permille", "fft_max_us", "spectrum_paint_max_us",
                        "publish_to_paint_max_us", "position_delta_error_max_us", "history_max_us", "field_block_max_us", "visual_paint_max_us", "player_render_max_us", "worker_tick_max_us", "visual_to_paint_max_us"):
                result[key] = max(int(row.get(key, 0)) for row in observed)
            result["median_callback_max_permille"] = median(int(row["callback_max_permille"]) for row in observed)
            with (args.output / "results.csv").open("a", newline="") as data:
                writer = csv.DictWriter(data, fieldnames=result)
                if not data.tell():
                    writer.writeheader()
                writer.writerow(result)
            print(result, flush=True)
            if (any(int(row["callbacks"]) == 0 for row in observed)
                    or any(result[key] for key in ("decode_errors", "device_errors", "discarded_device_errors", "capture_drops"))):
                raise RuntimeError("Playback trace failed in " + phase)
            if phase == "sustained" and (
                    any(result[k] for k in ("commands", "opens", "voices"))
                    or any(not int(row["spectrum_updates"]) for row in observed)
                    or (args.scenario == "sustained" and any(not int(row["stft_columns"]) for row in observed))):
                raise RuntimeError("Transport or analyzer continuity interrupted during soak")
            if args.scenario == "three-views" and result["visual_updates"] < 3 * 30 * seconds * 0.8:
                raise RuntimeError("Three instrument tabs did not remain visible")
            if buttons and result["switches"] < 8:
                raise RuntimeError("Waveform controls did not switch perspectives")
            if phase in ("steady", "sustained", "activity"):
                screenshot(hwnd, args.output / "native.webp")
            if args.scenario not in ("baseline", "off", "hidden"):
                if phase == "minimized" and any(result[key] for key in ("capture_frames", "fft_pairs", "spectrum_paints", "visual_paints")):
                    raise RuntimeError("Capture remained active while minimized")
                if phase != "minimized" and (not (result["fft_pairs"] or result["visual_updates"]) or any(int(row["capture_frames"]) == 0 for row in observed)):
                    raise RuntimeError("No measured spectrum")
            elif result["capture_frames"] or result["fft_pairs"]:
                raise RuntimeError("Spectrum work without a visible panel")
    finally:
        if worker:
            kernel.CloseHandle(worker)
        if thread:
            kernel.CloseHandle(thread)
        if hwnd:
            user32.PostMessageW(hwnd, 0x10, 0, 0)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scenario", choices=["baseline", "off", "one", "two", "two-only", "meter", "soak", "heat", "field", "waterfall", "xy", "pair", "all", "duplicate", "hidden", "sustained", "fields", "activity", "three-views"])
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("source", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--physical-size", type=int, nargs=2, metavar=("WIDTH", "HEIGHT"),
                        help="Set the private native window dimensions in device pixels")
    parser.add_argument("--exercise-resize", action="store_true", help="Also measure drag-resize and minimize")
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_worker:
        try:
            measure(args)
        except Exception:
            import traceback
            if args.output.is_dir() and not (args.output / "failure.txt").exists():
                with (args.output / "failure.txt").open("x") as log:
                    traceback.print_exc(file=log)
            raise
    else:
        from privateDesktop import run
        if args.output.exists():
            parser.error("Use a new directory for each qualification")
        run([sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 720)
        print((args.output / "results.csv").read_text(), end="")

"""Measure a release player on Windows using a fresh profile and the default endpoint."""
import argparse
import csv
import ctypes as C
from ctypes import wintypes as W
import json
import os
from pathlib import Path
from statistics import median
import subprocess
import time

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
        if owner.value == pid and name.value == "Zed::Window" and user32.IsWindowVisible(hwnd):
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
    if scenario in ("baseline", "off"):
        center = browser
    elif scenario in ("two", "two-only"):
        second = tabs(pane("spectrum", 4, mode="Side", style="Bands", speed="Slow", resolution="Fine"))
        center = split(0, [605, 620], browser, split(1, [300, 300], spectrum, second))
    else:
        center = split(0, [610, 615], browser, spectrum)
    return dict(volume=0.22, full_cd=True, show_levels=scenario in ("meter", "two", "soak"),
                window=dict(close_to_tray=False), layout=dict(version=3, center=center))


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
    process = subprocess.Popen([str(args.exe), str(args.source)], env=env)
    hwnd = thread = None
    try:
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
        scale = user32.GetDpiForWindow(hwnd) / 96
        def resize(width, height):
            if not user32.SetWindowPos(hwnd, 0, 0, 0, round(width*scale), round(height*scale), 0x16):
                raise C.WinError()
        resize(1240, 780)
        time.sleep(3)
        screenshot(hwnd, args.output / "native.webp")
        phases = [("steady", 25)]
        if args.scenario in ("baseline", "off", "two"):
            phases += [("resize", 15), ("minimized", 20)]
        if args.scenario == "soak":
            phases = [("sustained", 600)]
        for phase, seconds in phases:
            if phase == "minimized":
                user32.ShowWindow(hwnd, 6)
                time.sleep(1.5)
            first = len(rows())
            start = sample(process, thread)
            memory = [start[3]]
            resizes = 0
            while time.perf_counter() - start[0] < seconds:
                if process.poll() is not None:
                    raise RuntimeError("Player exited during " + phase)
                if phase == "resize":
                    width = (720, 900, 1240, 1500)[resizes % 4]
                    resize(width, round(width * 0.63))
                    resizes += 1
                memory.append(sample(process, thread)[3])
                time.sleep(0.06 if phase == "resize" else 0.5)
            end = sample(process, thread)
            elapsed = end[0] - start[0]
            observed = rows()[first:]
            if not observed:
                raise RuntimeError("No playback trace during " + phase)
            result = dict(build=args.scenario, phase=phase, seconds=round(elapsed, 2),
                          process_cpu=round((end[1]-start[1])*100/elapsed, 2),
                          ui_cpu=round((end[2]-start[2])*100/elapsed, 2),
                          private_start_mib=round(start[3], 2), private_max_mib=round(max(memory), 2),
                          private_end_mib=round(end[3], 2), resizes=resizes)
            for key in ("callbacks", "commands", "opens", "voices", "device_errors", "decode_errors",
                        "discarded_device_errors", "unchanged_ticks", "capture_frames", "capture_drops",
                        "fft_pairs", "spectrum_paints", "spectrum_updates", "wave_paints"):
                result[key] = sum(int(row.get(key, 0)) for row in observed)
            for key in ("callback_max_permille", "fft_max_us", "spectrum_paint_max_us",
                        "publish_to_paint_max_us", "position_delta_error_max_us"):
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
            if args.scenario not in ("baseline", "off"):
                if phase == "minimized" and any(result[key] for key in ("capture_frames", "fft_pairs", "spectrum_paints")):
                    raise RuntimeError("Capture remained active while minimized")
                if phase != "minimized" and (not result["fft_pairs"] or any(int(row["capture_frames"]) == 0 for row in observed)):
                    raise RuntimeError("No measured spectrum")
            elif result["capture_frames"] or result["fft_pairs"]:
                raise RuntimeError("Spectrum work without a visible panel")
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scenario", choices=["baseline", "off", "one", "two", "two-only", "meter", "soak"])
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("source", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    measure(parser.parse_args())

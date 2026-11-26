"""Run paired, muted foreground captures with one PresentMon elevation."""

import argparse
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import time
import tkinter as tk
from tkinter import ttk
from uuid import uuid4

from analyzeWaveformForeground import analyze_take, summarize
from qualifySpectrum import GpuTime, W, find_window, kernel, sample, user32
from qualifyWaveform import counter_frequency, performance_counter, settings
from qualifyWaveformMotion import Record, exact_zoom, real, zoom_controls


SPANS = ("6.000", "2.000", "1.799", "1.000", "0.500")
LAYOUTS = ("split", "wide", "maximized")
MODES = ("baseline", "paint", "posted", "posted-uncached", "early", "sync-one",
         "waitable-one", "waitable-two", "waitable-double")
SCREEN = ("paint", "posted-uncached", "early", "sync-one",
          "waitable-one", "waitable-two", "waitable-double")
TRACE_ENV = (
    "DISQRO_PLAYBACK_TRACE", "DISQRO_PRESENTATION_STAGES", "DISQRO_DXGI_TRACE",
    "DISQRO_GPU_TIMESTAMPS", "DISQRO_QUALIFY_PRESENT_MARKER", "DISQRO_QUALIFY_BUFFER_FRAMES",
    "DISQRO_QUALIFY_TARGET_FAILURE", "DISQRO_QUALIFY_DEVICE_RECREATION_MS",
    "DISQRO_RESIDENCY_REACTIVE", "GPUI_DISABLE_DIRECT_COMPOSITION", "GPUI_WINDOWS_PRESENTATION",
)
user32.GetForegroundWindow.restype = W.HWND
user32.GetWindowThreadProcessId.argtypes = (W.HWND, C.POINTER(W.DWORD))
user32.GetClientRect.argtypes = (W.HWND, C.POINTER(W.RECT))
user32.GetDpiForWindow.argtypes = (W.HWND,)
user32.PostMessageW.argtypes = (W.HWND, W.UINT, W.WPARAM, W.LPARAM)
user32.SetForegroundWindow.argtypes = (W.HWND,)
user32.ShowWindow.argtypes = (W.HWND, C.c_int)
kernel.OpenThread.restype = W.HANDLE
kernel.CloseHandle.argtypes = (W.HANDLE,)


def dump(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def wait_for(condition, seconds, error, collector=None):
    stop = time.monotonic() + seconds
    while time.monotonic() < stop:
        if collector is not None and collector.poll() is not None:
            raise RuntimeError("PresentMon exited. See presentmon.log.")
        if value := condition():
            return value
        time.sleep(0.05)
    raise RuntimeError(error)


def capture_path(output, pid):
    matches = list(output.glob(f"presents-*-{pid}.csv"))
    return matches[0] if len(matches) == 1 else None


def launch_player(exe, directory, source, mode, warm_cache, layout="split"):
    directory.mkdir()
    profile = settings(source, 1)
    if layout != "split":
        # Keep the browser available for opening the source, on the tab
        # behind the waveform. The waveform takes the full content width.
        center = profile["layout"]["center"]
        profile["layout"]["center"] = dict(
            panel_name="TabPanel",
            children=[child["children"][0] for child in center["children"]],
            info=dict(tabs=dict(active_index=1)),
        )
    (directory / "workspace-v1.json").write_text(
        json.dumps(profile), encoding="utf-8"
    )
    if warm_cache:
        shutil.copytree(warm_cache, directory / "Cache")
    env = {key: value for key, value in os.environ.items() if key not in TRACE_ENV}
    env.update(
        DISQRO_PROFILE_DIR=str(directory),
        DISQRO_PRESENTATION_STAGES=str(directory / "stages.csv"),
        DISQRO_DXGI_TRACE=str(directory / "dxgi.csv"),
    )
    if mode != "baseline":
        env["GPUI_WINDOWS_PRESENTATION"] = mode
    with (directory / "player.log").open("wb") as log:
        return subprocess.Popen([str(exe), "--qualify-silent", str(source)], cwd=exe.parent,
                                env=env, stdout=log, stderr=subprocess.STDOUT)


def close_player(process, hwnd):
    if process.poll() is not None:
        return
    if hwnd:
        user32.PostMessageW(hwnd, 0x0010, 0, 0)  # WM_CLOSE on this runner's player.
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(timeout=5)
        raise RuntimeError("The silent player did not close and flush its trace.")


def foreground(hwnd):
    return user32.GetForegroundWindow() == hwnd


def focus_from_launcher(hwnd):
    active = user32.GetForegroundWindow()
    if active == hwnd:
        return True
    owner = W.DWORD()
    user32.GetWindowThreadProcessId(active, C.byref(owner))
    if owner.value == os.getpid():
        user32.SetForegroundWindow(hwnd)
    return foreground(hwnd)


def playhead(path, hwnd):
    # Only the most recent records are needed to arm a take. Reading the
    # whole trace here competes with the UI while we approach the excerpt.
    try:
        with path.open("rb") as trace:
            length = trace.seek(0, os.SEEK_END)
            start = trace.seek(max(0, length - 64 * 1024))
            tail = trace.read()
    except FileNotFoundError:
        return None
    lines = tail.splitlines()
    if start:
        lines = lines[1:]
    if not tail.endswith(b"\n"):
        lines = lines[:-1]
    for line in reversed(lines):
        if b",wave_playhead," not in line:
            continue
        fields = line.split(b",")
        if len(fields) == 8 and int(fields[6]) == hwnd:
            return Record(int(fields[0]), int(fields[1]), "wave_playhead", int(fields[3]),
                          int(fields[4]), int(fields[5]), hwnd, int(fields[7]))
    return None


def capture_take(args, directory, mode, round_number, span, collector, frequency,
                 layout="split", source_role="reference"):
    exe = args.baseline if mode == "baseline" else args.candidate
    source = args.source if source_role == "reference" else args.comparison_source
    process = launch_player(exe, directory, source, mode, args.warm_cache, layout)
    hwnd = thread = gpu = None
    try:
        hwnd = wait_for(lambda: find_window(process.pid), 20, "No qualification window opened.", collector)
        wait_for(lambda: focus_from_launcher(hwnd), 8,
                 "Windows did not transfer focus from the capture launcher to its test window.", collector)
        if layout == "maximized":
            user32.ShowWindow(hwnd, 3)  # SW_MAXIMIZE on this runner's player.
        stages = directory / "stages.csv"
        wait_for(lambda: playhead(stages, hwnd), 12, "The source did not start playing.", collector)
        thread = W.HANDLE(kernel.OpenThread(0x0800, False, user32.GetWindowThreadProcessId(hwnd, None)))
        if not thread:
            raise C.WinError()
        controls, _ = zoom_controls(hwnd, 1, stages)
        if not foreground(hwnd):
            raise RuntimeError("Foreground was lost before setting the zoom.")
        exact_zoom(hwnd, f"{span} s", controls)
        frames = float(span) * args.source_rate
        excerpt_frame = args.excerpt * args.source_rate

        def start_of_excerpt():
            if not foreground(hwnd):
                raise RuntimeError("Foreground was lost before the timed sample.")
            row = playhead(stages, hwnd)
            if row is None or abs(real(row.detail) - frames) >= 0.05:
                return None
            return row if real(row.value) >= excerpt_frame else None

        gpu = GpuTime(process.pid)
        first = wait_for(start_of_excerpt, args.excerpt + 4,
                         f"Could not verify {span} seconds at {args.source_rate} Hz.", collector)
        first_position = real(first.value)
        if first_position > excerpt_frame + args.source_rate * 0.25:
            raise RuntimeError("The verified zoom settled after the selected excerpt started.")
        bounds = W.RECT()
        if not user32.GetClientRect(hwnd, C.byref(bounds)):
            raise C.WinError()
        start = first.qpc
        initial = sample(process, thread)
        until = start + args.seconds * frequency
        while performance_counter() < until:
            if not foreground(hwnd):
                raise RuntimeError("Foreground was lost during the timed sample.")
            if process.poll() is not None or (collector is not None and collector.poll() is not None):
                raise RuntimeError("The player or PresentMon exited during the timed sample.")
            time.sleep(0.05)
        final = sample(process, thread)
        final_bounds = W.RECT()
        if not user32.GetClientRect(hwnd, C.byref(final_bounds)) or (
            final_bounds.right, final_bounds.bottom
        ) != (bounds.right, bounds.bottom):
            raise RuntimeError("The player changed window dimensions during the take.")
        gpu_usage = gpu.finish()
        gpu = None
        elapsed = final[0] - initial[0]
        dump(directory / "capture.json", dict(
            mode=mode, round=round_number, span=span, span_frames=frames, layout=layout,
            source_role=source_role,
            pid=process.pid, hwnd=hwnd,
            source_rate=args.source_rate, refresh=args.refresh, qpc_frequency=frequency,
            start_qpc=start, stop_qpc=until, first_position_frame=first_position,
            client_px=(bounds.right, bounds.bottom), dpi=user32.GetDpiForWindow(hwnd),
            process_cpu_core_percent=100 * (final[1] - initial[1]) / elapsed,
            ui_cpu_core_percent=100 * (final[2] - initial[2]) / elapsed,
            sampled_private_mib=max(initial[3], final[3]), busiest_gpu_engine_percent=gpu_usage,
        ))
    finally:
        if gpu:
            gpu.finish()
        if thread:
            kernel.CloseHandle(thread)
        close_player(process, hwnd or find_window(process.pid))


def analyze_existing(output):
    reports, errors = [], []
    session = json.loads((output / "session.json").read_text(encoding="utf-8"))
    etw = Path(session.get("etw_directory", output))
    for metadata in sorted(output.glob("*/capture.json")):
        try:
            pid = json.loads(metadata.read_text(encoding="utf-8"))["pid"]
            data = capture_path(etw, pid)
            if not data:
                raise ValueError(f"No unique PresentMon file for PID {pid}.")
            reports.append(analyze_take(metadata.parent, data))
        except (OSError, ValueError) as error:
            errors.append(f"{metadata.parent.name}: {error}")
    dump(output / "report.json", dict(takes=reports, errors=errors))
    summarize(output, reports)
    return reports, errors


def capture_plan(args):
    modes = tuple(args.modes)
    sources = ("reference", "comparison") if args.comparison_source else ("reference",)
    takes = [(repeat + 1, mode, span, layout, source) for repeat in range(args.repeats)
             for mode in (modes if repeat % 2 == 0 else modes[::-1])
             for span in (args.spans if repeat % 2 == 0 else args.spans[::-1])
             for layout in (args.layouts if repeat % 2 == 0 else args.layouts[::-1])
             for source in (sources if repeat % 2 == 0 else sources[::-1])]
    for repeat in range(args.repeats):
        order = modes if repeat % 2 == 0 else modes[::-1]
        takes.extend((repeat + 1, mode, "0.500", layout, "reference")
                     for mode in order for layout in args.close_layouts if layout not in args.layouts)
    if args.screen:
        for repeat in range(1, 3):
            order = SCREEN if repeat == 1 else SCREEN[::-1]
            takes.extend((repeat, mode, "0.500", args.layouts[0], "reference")
                         for mode in order if mode not in modes)
    return takes


def run_capture(args):
    args.output.mkdir(parents=True)
    status = args.output / "status.json"
    frequency = counter_frequency()
    takes = capture_plan(args)
    dump(args.output / "session.json", dict(
        source=str(args.source), source_sha256=digest(args.source),
        baseline_sha256=digest(args.baseline), candidate_sha256=digest(args.candidate),
        source_rate=args.source_rate, refresh=args.refresh, excerpt_seconds=args.excerpt,
        take_seconds=args.seconds, takes=takes, warm_cache=bool(args.warm_cache),
        etw_directory=str(args.attach_etw or args.output),
        comparison_source=(str(args.comparison_source) if args.comparison_source else None),
        comparison_source_sha256=(digest(args.comparison_source) if args.comparison_source else None),
    ))
    session = "DisqroJudder-" + uuid4().hex
    names = {args.baseline.name, args.candidate.name}
    if args.capture_compositor:
        names.add("dwm.exe")
    arguments = [
        str(args.presentmon), "--restart_as_admin", "--session_name", session,
        *[value for name in sorted(names) for value in ("--process_name", name)],
        "--timed", str(args.limit), "--terminate_after_timed", "--multi_csv",
        "--v1_metrics", "--qpc_time", "--no_console_stats", "--no_track_input",
        "--output_file", str(args.output / "presents.csv"),
    ]
    collector = None
    if not args.attach_etw:
        dump(status, dict(state="approval", total=len(takes)))
        print("Approve the PresentMon UAC prompt. Leave the silent player in the foreground until capture ends.",
              flush=True)
        with (args.output / "presentmon.log").open("wb") as log:
            collector = subprocess.Popen(arguments, cwd=args.output, stdout=log, stderr=subprocess.STDOUT,
                                         creationflags=subprocess.CREATE_NO_WINDOW)
    error = None
    try:
        # The first owned window warms the launch path and confirms that the
        # elevated collector can see presents. Measured takes start at a fresh excerpt.
        if collector:
            primer = launch_player(args.baseline, args.output / "primer", args.source, "baseline", None)
            try:
                wait_for(lambda: capture_path(args.output, primer.pid), 75,
                         "PresentMon did not capture the primer window.", collector)
            finally:
                close_player(primer, find_window(primer.pid))
        for index, (repeat, mode, span, layout, source) in enumerate(takes, 1):
            dump(status, dict(state="capturing", take=index, total=len(takes), mode=mode,
                              span=span, layout=layout, source_role=source))
            print(f"{index}/{len(takes)}  {mode}  {span} s  {layout}  {source}", flush=True)
            capture_take(args, args.output / f"{index:02d}-{mode}-{span}-{layout}-{source}",
                         mode, repeat, span, collector, frequency, layout, source)
    except (KeyboardInterrupt, RuntimeError, OSError) as failure:
        error = str(failure) or "Capture canceled"
        print(error, flush=True)
    finally:
        dump(status, dict(state="collecting", error=error))
        if collector and collector.poll() is None:
            with (args.output / "presentmon-stop.log").open("wb") as log:
                subprocess.run([str(args.presentmon), "--session_name", session, "--terminate_existing_session"],
                               stdout=log, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
            # A normal process cannot stop an elevated trace on some hosts.
            # The bounded collector releases its files at the timed end.
            collector.wait(timeout=args.limit + 15)
        elif args.attach_etw:
            def readable():
                for metadata in args.output.glob("*/capture.json"):
                    pid = json.loads(metadata.read_text(encoding="utf-8"))["pid"]
                    path = capture_path(args.attach_etw, pid)
                    if path:
                        try:
                            with path.open("rb"):
                                pass
                        except PermissionError:
                            return False
                return True

            wait_for(readable, args.limit + 15, "The active PresentMon session did not release its files.")
    reports, errors = analyze_existing(args.output)
    verified = 0
    geometries = {}
    for item in reports:
        metadata = item["capture"]
        geometries.setdefault(metadata["layout"], set()).add(
            (*metadata["client_px"], metadata["dpi"], tuple(item["waveform_widths_px"]))
        )
        chain = item["chains"].get(item["chain"])
        seconds = (metadata["stop_qpc"] - metadata["start_qpc"]) / metadata["qpc_frequency"]
        if (chain and chain["display_interval_ms"]
                and chain["observed_submission_seconds"] >= seconds - 0.1
                and chain["joined_displayed"] >= 0.95 * chain["displayed"]):
            verified += 1
    stable_geometry = bool(geometries) and all(len(per_layout) == 1 for per_layout in geometries.values())
    complete = verified == len(takes) and stable_geometry and error is None and not errors
    dump(status, dict(state="complete" if complete else "incomplete",
                      expected=len(takes), verified=verified,
                      geometries={layout: sorted(items) for layout, items in geometries.items()},
                      error=error, analysis_errors=errors))
    print(args.output / "summary.md", flush=True)
    return 0 if complete else 2


def launch_from_window(args):
    root = tk.Tk()
    root.title("Disqro foreground capture")
    root.geometry("430x155")
    root.attributes("-topmost", True)
    view = ttk.Frame(root, padding=20)
    view.pack(expand=True, fill="both")
    ttk.Label(view, text="Disqro 240 Hz foreground comparison", font=("Segoe UI", 12, "bold")).pack(anchor="w")
    ttk.Label(view, text="Start once, then leave the muted test windows in the foreground.",
              wraplength=385).pack(anchor="w", pady=10)
    result = []
    worker = None

    def collect():
        try:
            result.append(run_capture(args))
        except (OSError, RuntimeError) as error:
            print(error, flush=True)
            result.append(2)

    def begin():
        nonlocal worker
        root.attributes("-topmost", False)
        button.configure(state="disabled", text="Capture running")
        worker = threading.Thread(target=collect)
        worker.start()

    def check():
        if worker and not worker.is_alive():
            root.destroy()
        else:
            root.after(250, check)

    button = ttk.Button(view, text="Start capture", command=begin)
    button.pack(anchor="w")
    root.protocol("WM_DELETE_WINDOW", lambda: root.destroy() if not worker else None)
    root.after(250, check)
    root.mainloop()
    return result[0] if result else 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--presentmon", type=Path)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--comparison-source", type=Path, help="Second recording at the same source rate")
    parser.add_argument("--source-rate", type=int, default=48000)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--spans", nargs="+", choices=SPANS, default=SPANS)
    parser.add_argument("--layouts", nargs="+", choices=LAYOUTS, default=("split",))
    parser.add_argument("--close-layouts", nargs="+", choices=LAYOUTS, default=(),
                        help="Repeat the mode comparison in these additional layouts at 0.500 seconds")
    parser.add_argument("--modes", nargs="+", choices=MODES, default=("baseline", "posted"))
    parser.add_argument("--repeats", type=int, choices=(1, 2, 3), default=2)
    parser.add_argument("--screen", action="store_true",
                        help="Repeat the remaining presentation policies at 0.500 seconds in the same session")
    parser.add_argument("--excerpt", type=float, default=4.)
    parser.add_argument("--seconds", type=int, default=8)
    parser.add_argument("--refresh", type=float, default=240.)
    parser.add_argument("--limit", type=int, default=420)
    parser.add_argument("--warm-cache", type=Path)
    parser.add_argument("--analyze-existing", action="store_true")
    parser.add_argument("--attach-etw", type=Path, help="Reuse the active collector from an earlier capture")
    parser.add_argument("--capture-compositor", action="store_true", help="Include DWM timing in the raw ETW export")
    args = parser.parse_args()
    args.output = args.output.resolve()
    if args.analyze_existing:
        reports, errors = analyze_existing(args.output)
        print(args.output / "summary.md")
        return 0 if reports and not errors else 2
    for key in ("baseline", "candidate", "presentmon", "source"):
        value = getattr(args, key)
        if value is None or not value.is_file():
            parser.error(f"--{key} must name an existing file")
        setattr(args, key, value.resolve())
    if args.warm_cache and not args.warm_cache.is_dir():
        parser.error("--warm-cache must name a Cache directory")
    if args.comparison_source:
        args.comparison_source = args.comparison_source.resolve()
        if not args.comparison_source.is_file():
            parser.error("--comparison-source must name an existing recording")
    if args.output.exists() or C.windll.shell32.IsUserAnAdmin():
        parser.error("Choose a fresh output directory and run at normal privileges.")
    if not 8000 <= args.source_rate <= 384000 or not 24 <= args.refresh <= 1000:
        parser.error("Invalid source or display rate.")
    if not 6 <= args.seconds <= 45 or not 2 <= args.excerpt <= 30 or not 60 <= args.limit <= 900:
        parser.error("Use a 6–45 second sample, a 2–30 second excerpt start, and a 60–900 second limit.")
    if any(len(set(values)) != len(values)
           for values in (args.modes, args.spans, args.layouts, args.close_layouts)):
        parser.error("Modes, spans, and layouts must be unique within a round.")
    if len(capture_plan(args)) * (args.excerpt + args.seconds) + 20 > args.limit:
        parser.error("The collector limit is too short for the requested takes.")
    if args.attach_etw:
        args.attach_etw = args.attach_etw.resolve()
        if not (args.attach_etw / "session.json").is_file():
            parser.error("--attach-etw must name the previous capture directory")
    return launch_from_window(args)


if __name__ == "__main__":
    raise SystemExit(main())

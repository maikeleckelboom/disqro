"""Capture the detailed waveform at a chapter boundary with no reachable
successor, on a muted private desktop.

A CUE program is played and paused in its first chapter; the saved queue is
then cut to that chapter alone and, optionally, an unrelated recording, and
the player restored. In the Track perspective the waveform must end where
the chapter ends. With --full-cd the authored program continues instead.

    python tools/captureTimelineBoundary.py <disqro.exe> <output> [--unrelated] [--full-cd]
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback

from privateDesktop import run
from qualifySpectrum import find_window, screenshot, user32
from qualifyWaveform import settings
from qualifyWaveformScrub import rows, synthetic_cue, wait_for


def launch(exe, output, source=None):
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(output),
               DISQRO_PRESENTATION_STAGES=str(output / "stages.csv"))
    command = [str(exe), "--qualify-silent"] + ([str(source)] if source else [])
    log = (output / "player.log").open("a", encoding="utf-8")
    return subprocess.Popen(command, env=env, stdout=log, stderr=log)


def close(app):
    hwnd = find_window(app.pid)
    if hwnd:
        user32.PostMessageW(hwnd, 0x10, 0, 0)
    try:
        app.wait(timeout=10)
    except subprocess.TimeoutExpired:
        app.terminate()
        app.wait()


def measure(args):
    output = args.output
    output.mkdir(parents=True, exist_ok=False)
    source, boundary = synthetic_cue(output)
    config = settings(source, 1)
    config.update(volume=0.0, full_cd=args.full_cd)
    workspace = output / "workspace-v1.json"
    workspace.write_text(json.dumps(config), encoding="utf-8")
    stages = output / "stages.csv"
    app = launch(args.exe, output, source)
    try:
        hwnd = wait_for(lambda: find_window(app.pid), "the private player window")
        wait_for(lambda: [r for r in rows(stages) if r["stage"] == "wave_playhead"], "a running detail deck")
        time.sleep(2.5)
        user32.PostMessageW(hwnd, 0x0100, 0x20, 0)  # Space pauses.
        user32.PostMessageW(hwnd, 0x0101, 0x20, 0xC0000000)
        time.sleep(0.6)
    finally:
        close(app)
    saved = json.loads(workspace.read_text(encoding="utf-8"))
    queue = saved["queue"]["queue"]
    current = queue["current"]
    entries = [queue["entries"][current]]
    if args.unrelated:
        other = dict(entries[0])
        track = dict(other["track"])
        track.update(source=str(output / "fixture" / "another.wav"), disc=None, title="Another Recording")
        other.update(id=queue["next_id"] + 1, track=track)
        queue["next_id"] = other["id"]
        entries.append(other)
    queue.update(entries=entries, current=0)
    workspace.write_text(json.dumps(saved), encoding="utf-8")
    (output / "queue.json").write_text(json.dumps(entries, indent=2), encoding="utf-8")
    app = launch(args.exe, output)
    try:
        hwnd = wait_for(lambda: find_window(app.pid), "the restored player window")
        wait_for(lambda: [r for r in rows(stages) if r["stage"] == "wave_scene_playhead"], "the restored deck")
        time.sleep(1.5)
        name = "full-cd" if args.full_cd else ("unrelated" if args.unrelated else "only-chapter")
        screenshot(hwnd, output / f"boundary-{name}.png")
    finally:
        close(app)
    (output / "result.json").write_text(json.dumps(dict(boundary_frame=boundary, queue=len(entries)), indent=2),
                                        encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda value: Path(value).resolve(strict=True))
    parser.add_argument("output", type=lambda value: Path(value).resolve())
    parser.add_argument("--unrelated", action="store_true")
    parser.add_argument("--full-cd", action="store_true")
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_worker:
        try:
            measure(args)
        except Exception:
            if args.output.is_dir():
                with (args.output / "failure.txt").open("x", encoding="utf-8") as log:
                    traceback.print_exc(file=log)
            raise
    else:
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 90)

"""Measure paused wheel input to the first matching deck draw and scene paint."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

from private_desktop import run
from qualify_compact_playhead import tone
from qualify_spectrum import find_window, user32
from qualify_waveform import counter_frequency, performance_counter, percentiles, settings, wheel
from qualify_waveform_motion import DECK, real, records
from qualify_waveform_transport import NativePlayer, wait_for


def measure(exe, output):
    output.mkdir(parents=True, exist_ok=False)
    source = output / "Wheel control.wav"
    tone(source, 60)
    profile = settings(source, 1)
    profile.update(volume=0.0, full_cd=False)
    (output / "workspace-v1.json").write_text(json.dumps(profile), encoding="utf-8")
    stages = output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(output), DISQRO_PRESENTATION_STAGES=str(stages))
    sent = []
    with (output / "player.log").open("x") as log:
        process = subprocess.Popen([exe, "--qualify-silent", source], env=env, stdout=log, stderr=log)
        hwnd = None
        try:
            hwnd = wait_for(lambda: find_window(process.pid) or None, "player window")
            player = NativePlayer(hwnd, stages)
            player.click("Pause")
            player.control("Play")
            time.sleep(1)
            # Leave the paused view idle between inputs. Each notch changes
            # the span, alternating between the same two registered views.
            for delta in (120, -120) * 12:
                sent.append(performance_counter())
                wheel(hwnd, *DECK, delta)
                time.sleep(0.33)
        finally:
            if hwnd:
                user32.PostMessageW(hwnd, 0x10, 0, 0)
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    draws = [r for r in records(stages) if r.stage in ("wave_scene_playhead", "wave_playhead")]
    previous = next(real(r.detail) for r in reversed(draws) if r.qpc < sent[0])
    initial = previous
    frequency = counter_frequency()
    samples = []
    for i, (start, end) in enumerate(zip(sent, sent[1:] + [performance_counter()])):
        matching = [r for r in draws if start <= r.qpc < end and not math.isclose(real(r.detail), previous)]
        if not matching:
            raise RuntimeError(f"Wheel event {i + 1} produced no changed viewport")
        first = matching[0]
        scene = next(r for r in matching if r.stage == "wave_scene_playhead")
        span = real(first.detail)
        assert math.isclose(real(scene.detail), span)
        assert (span < previous) == (i % 2 == 0)
        if i % 2:
            assert math.isclose(span, initial)
        samples.append(dict(span=span, center=real(first.value), draw_ms=(first.qpc-start)*1000/frequency,
                            scene_ms=(scene.qpc-start)*1000/frequency))
        previous = span
    assert max(s["center"] for s in samples) - min(s["center"] for s in samples) <= 1
    result = {key: percentiles([s[key] for s in samples]) for key in ("draw_ms", "scene_ms")}
    (output / "wheel.json").write_text(json.dumps(dict(percentiles=result, samples=samples), indent=2))
    print(result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            measure(args.exe, args.output)
        except Exception:
            import traceback
            (args.output / "failure.txt").write_text(traceback.format_exc())
            raise
    else:
        run([sys.executable, "-B", __file__, *sys.argv[1:], "--worker"], 90)
        print(json.loads((args.output / "wheel.json").read_text())["percentiles"])

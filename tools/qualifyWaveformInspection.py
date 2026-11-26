"""Capture the silent native inspection journey on a private Win32 desktop.

Input is dispatched by the qualification build inside GPUI. The worker only
captures its own fixture window, without system-wide keyboard or mouse input.
"""
import argparse
import csv
import struct
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time
import wave
from array import array

from privateDesktop import run
from qualifySpectrum import find_window, screenshot
from qualifyWaveform import settings


def measure(exe, folder):
    folder.mkdir(parents=True, exist_ok=False)
    recordings = folder / "Signal studies"
    recordings.mkdir()
    source = recordings / "Stereo inspection.wav"
    rate = 48000
    second = array("h")
    for i in range(rate):
        for frequency, phase in [(1847, 0), (2333, 0.4)]:
            second.append(round(21500 * math.sin(math.tau * i * frequency / rate + phase)))
    with wave.open(str(source), "wb") as out:
        out.setparams((2, 2, rate, 0, "NONE", ""))
        for _ in range(60):
            out.writeframesraw(second.tobytes())
    cue = recordings / "Stereo inspection.cue"
    cue.write_text('FILE "Stereo inspection.wav" WAVE\n'
                   'TRACK 01 AUDIO\nTITLE "Stereo reference"\nINDEX 01 00:00:03\n'
                   'TRACK 02 AUDIO\nTITLE "Continuation"\nINDEX 01 00:30:00\n', encoding="ascii")
    profile = settings(cue, 1)
    def restore(node):
        if node["panel_name"] == "disqro.waveform":
            node["info"]["panel"]["state"] = {"showSamplePoints": True}
        for child in node["children"]:
            restore(child)
    restore(profile["layout"]["center"])
    (folder / "workspace-v1.json").write_text(json.dumps(profile), encoding="utf-8")
    stages = folder / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(folder), DISQRO_INSPECTION_JOURNEY=str(folder),
               DISQRO_PRESENTATION_STAGES=str(stages))
    captures = ("zoom-editor", "sample-points", "sample-inspector", "playback")
    log = folder / "journey.log"
    with log.open("w", encoding="utf-8") as stream:
        player = subprocess.Popen([exe, "--qualify-silent", cue], env=env, stdout=stream, stderr=stream)
        try:
            deadline = time.monotonic() + 75
            while time.monotonic() < deadline and player.poll() is None:
                records = log.read_text(encoding="utf-8", errors="replace").splitlines()
                if "inspection-complete" in records:
                    return
                if any(record.startswith("inspection-failed") for record in records):
                    raise RuntimeError(records[-1])
                for name in captures:
                    path = folder / (name + ".png")
                    if "capture " + name in records and not path.exists():
                        if name == "sample-inspector":
                            if not stages.is_file():
                                continue
                            with stages.open(newline="") as events:
                                hits = [row for row in csv.DictReader(events) if row["stage"] == "wave_sample_hit" and row.get("detail")]
                            if not hits:
                                continue
                            hit = hits[-1]
                            frame, bits = int(hit["value"]), int(hit["detail"])
                            measured = struct.unpack(">ff", bits.to_bytes(8, "big"))
                            start = 2 * (frame % rate)
                            assert measured == tuple(v / 32768 for v in second[start:start + 2])

                        time.sleep(0.2)
                        screenshot(find_window(player.pid), path)
                time.sleep(0.05)
            raise RuntimeError("Inspection journey did not complete. See " + str(log))
        finally:
            if player.poll() is None:
                player.terminate()
            player.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        measure(args.exe, args.output)
    else:
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.output), "--worker"], 90)

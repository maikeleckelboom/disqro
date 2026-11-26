"""Capture the silent native inspection journey on a private Win32 desktop.

Input is dispatched by the qualification build inside GPUI. The worker only
captures its own fixture window, without system-wide keyboard or mouse input.
"""
import argparse
import csv
import ctypes
import struct
import json
import math
import os
import shutil
from pathlib import Path
import subprocess
import sys
import time
import wave
from array import array

from PIL import Image, ImageChops

from privateDesktop import run
from qualifySpectrum import find_window, screenshot
from qualifyWaveform import settings


def measure(exe, folder, band, control_wav):
    folder.mkdir(parents=True, exist_ok=False)
    recordings = folder / "Signal studies"
    recordings.mkdir()
    source = recordings / "Stereo inspection.wav"
    if control_wav:
        shutil.copyfile(control_wav, source)
    else:
        rate = 48000
        second = array("h")
        frequencies = {"mid": (1847, 2333), "bass": (83, 127), "high": (7393, 8681)}[band]
        for i in range(rate):
            for frequency, phase in zip(frequencies, (0, 0.4)):
                second.append(round(21500 * math.sin(math.tau * i * frequency / rate + phase)))
        with wave.open(str(source), "wb") as out:
            out.setparams((2, 2, rate, 0, "NONE", ""))
            for _ in range(60):
                out.writeframesraw(second.tobytes())
    cue = recordings / "Stereo inspection.cue"
    # At 3.2 seconds the frozen control contains a 6.3 kHz mono tone.
    # Starting the CUE chapter here exercises coincident channel outlines.
    index, next_index = ("00:03:08", "00:18:00") if control_wav else ("00:00:03", "00:30:00")
    cue.write_text('FILE "Stereo inspection.wav" WAVE\n'
                   f'TRACK 01 AUDIO\nTITLE "Stereo reference"\nINDEX 01 {index}\n'
                   f'TRACK 02 AUDIO\nTITLE "Continuation"\nINDEX 01 {next_index}\n', encoding="ascii")
    profile = settings(cue, 1)
    def restore(node):
        if node["panel_name"] == "disqro.waveform":
            node["info"]["panel"]["state"] = {"showSamplePoints": True, "showSampleRuler": True}
        for child in node["children"]:
            restore(child)
    restore(profile["layout"]["center"])
    (folder / "workspace-v1.json").write_text(json.dumps(profile), encoding="utf-8")
    stages = folder / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(folder), DISQRO_INSPECTION_JOURNEY=str(folder),
               DISQRO_PRESENTATION_STAGES=str(stages))
    if control_wav:
        env["DISQRO_INSPECTION_MIN_FRAME"] = str(round(3.2 * 44100))
    captures = ("zoom-editor", "sample-points", "sample-ruler", "sample-inspector", "cue-times", "playback", "sample-narrow")
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
                            if control_wav:
                                assert 3 * 44100 <= frame < 4 * 44100, "Expected the control's treble section"
                            measured = struct.unpack(">ff", bits.to_bytes(8, "big"))
                            with wave.open(str(source), "rb") as reference:
                                reference.setpos(frame)
                                stereo = struct.unpack("<hh", reference.readframes(1))
                            assert measured == tuple(v / 32768 for v in stereo)

                        time.sleep(0.2)
                        handle = find_window(player.pid)
                        screenshot(handle, path)
                        if name == "playback":
                            with Image.open(path) as playing:
                                # Below the traces and above the transport, within the
                                # fixture's waveform ruler. Exclude the center playhead.
                                w, h = playing.size
                                ruler = playing.convert("L").crop((round(w * 0.7), round(h * 0.83),
                                                                  round(w * 0.97), round(h * 0.85)))
                                assert sum(ruler.histogram()[52:]) > 100, "Sample ruler lost its ticks during playback"
                        if name in ("sample-inspector", "cue-times"):
                            baseline = "sample-points" if name == "sample-inspector" else "sample-inspector"
                            with Image.open(folder / (baseline + ".png")) as before, Image.open(path) as after:
                                diff = ImageChops.difference(before, after).convert("L")
                                # The fixture's header is above the waveform. Holding Shift
                                # changes measurements with the same sample still selected.
                                header = diff.crop((0, 0, diff.width, round(diff.height * 0.09)))
                                assert sum(header.histogram()[12:]) > 200, f"{name} header did not change"
                                scale = ctypes.windll.user32.GetDpiForWindow(handle) / 96
                                right_half = header.crop((header.width // 2, 0, header.width, header.height))
                                ink = right_half.point(lambda value: 255 if value >= 12 else 0).getbbox()
                                assert ink is not None, f"{name} missing from the right side of the header"
                                assert right_half.width - ink[2] <= 30 * scale, f"{name} lost its right anchor"
                                assert ink[3] - ink[1] <= 14 * scale, f"{name} spans multiple lines"
                                if name == "sample-inspector":
                                    assert ink[2] - ink[0] <= 270 * scale, "Frame and amplitudes exceed their compact footprint"
                                if name == "cue-times":
                                    body = diff.crop((diff.width // 2, round(diff.height * 0.11),
                                                      diff.width, round(diff.height * 0.82)))
                                    assert body.getbbox() is None, "CUE times changed waveform pixels"
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
    parser.add_argument("--control-wav", type=lambda p: Path(p).resolve(strict=True), help="Use Waveform Native's 24-second synthetic control WAV")
    parser.add_argument("--band", choices=("mid", "bass", "high"), default="mid")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        measure(args.exe, args.output, args.band, args.control_wav)
    else:
        fixture = ["--control-wav", str(args.control_wav)] if args.control_wav else []
        run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(args.output), "--band", args.band, *fixture, "--worker"], 90)

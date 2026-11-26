"""Exercise replay and compact waveform seeking in the muted native player."""

import argparse
from array import array
from bisect import bisect_right
import csv
import ctypes as C
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
import time
import traceback
import wave

from privateDesktop import run
from qualifySpectrum import W, click_control, control_centers, find_window, screenshot, user32
from qualifyWaveform import performance_counter, settings


CASES = ("replay", "single", "multiple", "only")
TITLES = ("Chapter Alpha", "Chapter Bravo", "Chapter Charlie")
DURATIONS = (18, 12, 6)
STARTS = (0, 18, 30)
TOTAL = sum(DURATIONS)


def fixture(folder, case):
    folder.mkdir()

    def recording(name, rate, sections):
        path = folder / name
        with wave.open(str(path), "wb") as out:
            out.setparams((2, 2, rate, 0, "NONE", "not compressed"))
            for seconds, frequency in sections:
                second = array("h")
                for sample in range(rate):
                    t = sample / rate
                    envelope = 0.4 + 0.5 * math.sin(math.pi * t * 3) ** 2
                    second.extend(round(14000 * envelope * math.sin(math.tau * t * hz))
                                  for hz in (frequency, frequency + 120))
                encoded = second.tobytes()
                for _ in range(seconds):
                    out.writeframesraw(encoded)
        return path

    if case == "replay":
        return recording("Replay.wav", 48_000, [(3, 249)]), (48_000,)
    multi = case == "multiple"
    rates = (48_000, 44_100, 32_000) if multi else (48_000,) * 3
    sections = list(zip(DURATIONS, (223, 277, 349), strict=True))
    lines = []
    if not multi:
        recording("CD1.wav", rates[0], sections)
        lines.append('FILE "CD1.wav" WAVE')
    for i, title in enumerate(TITLES):
        if multi:
            name = f"chapter-{i + 1}.wav"
            recording(name, rates[i], sections[i:i + 1])
            lines.append(f'FILE "{name}" WAVE')
        lines.extend((f"TRACK {i + 1:02d} AUDIO", f'TITLE "{title}"',
                      f"INDEX 01 00:{0 if multi else STARTS[i]:02d}:00"))
    cue = folder / "CD1.cue"
    cue.write_text("\n".join(lines) + "\n", encoding="ascii")
    return cue, rates


def wait_for(probe, description):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result = probe()
        if result is not None and result is not False:
            return result
        time.sleep(0.04)
    raise RuntimeError(f"Timed out waiting for {description}")


def real(bits):
    return struct.unpack("<d", struct.pack("<Q", bits))[0]


class NativePlayer:
    def __init__(self, hwnd, stages):
        self.hwnd = hwnd
        self.stages = stages
        self.client = W.RECT()
        user32.GetClientRect(hwnd, C.byref(self.client))
        self.dpi = user32.GetDpiForWindow(hwnd) / 96

    def records(self, since=0):
        if not self.stages.is_file():
            return []
        with self.stages.open(newline="", encoding="utf-8") as trace:
            return [(int(row["qpc"]), int(row["qpc"]) - int(row["ticks"]), row["stage"],
                     int(row["value"]), int(row["detail"])) for row in csv.DictReader(trace)
                    if all(row.values()) and int(row["qpc"]) >= since]

    def stage(self, label, since=0):
        return [row for row in self.records(since) if row[2] == label]

    def control(self, label, compact=False):
        def lookup():
            try:
                minimum = None
                if compact:
                    p = W.POINT(0, round(self.client.bottom * 0.8))
                    user32.ClientToScreen(self.hwnd, C.byref(p))
                    minimum = p.y
                return control_centers(self.hwnd, [label], min_y=minimum)[0]
            except (IndexError, subprocess.CalledProcessError):
                return None

        return wait_for(lookup, label)

    def click(self, label):
        click_control(self.hwnd, self.control(label))

    def click_x(self, x):
        p = W.POINT(x, self.client.bottom - round(14 * self.dpi))
        user32.ClientToScreen(self.hwnd, C.byref(p))
        click_control(self.hwnd, p)

    def bounds(self):
        row = wait_for(lambda: self.stage("wave_compact_bounds") or None, "the painted strip bounds")[-1]
        return real(row[3]), real(row[4])

    def scene(self, disc, playing, entry, length, since, includes, duration):
        lane = "wave_compact_disc" if disc else "wave_compact_track"
        mode = "wave_compact_playing" if playing else "wave_compact_paused"

        def probe():
            rows = self.records(since)
            queue = {start: (selected, count) for _, start, stage, selected, count in rows if stage == mode}
            return next(((qpc, real(progress) * duration) for qpc, start, stage, progress, shown in rows
                         if stage == lane and shown == entry and queue.get(start) == (entry, length)
                         and includes(real(progress) * duration)), None)

        return wait_for(probe, f"{lane} playing={playing} entry={entry} to render the requested cursor")

    def deck(self, since, rate, frame, playing):
        def matches(position):
            error = position - frame
            return -2 <= error <= rate * 0.32 if playing else abs(error) <= 2

        def probe():
            rows = self.records(since)
            painted = any(matches(real(row[3])) for row in rows if row[2] == "wave_playhead") if playing else any(
                row[2] == "wave_deck_paint" and row[3] == 1 for row in rows)
            if painted:
                return next(((real(row[3]), real(row[4])) for row in rows if row[2] == "wave_deck_position"
                             and matches(real(row[3])) and matches(real(row[4]))
                             and abs(real(row[3]) - real(row[4])) <= 1.01), None)
            return None

        return wait_for(probe, f"renderer and deck at source frame {frame}")


def replay(player):
    player.scene(False, True, 1, 1, 0, lambda seconds: seconds < 0.5, 3)
    player.scene(False, False, 1, 1, 0, lambda seconds: seconds == 3, 3)
    completed, _ = player.deck(0, 48_000, 144_000, False)
    play = player.control("Play")
    restart = performance_counter()
    click_control(player.hwnd, play)
    _, position = player.scene(False, True, 1, 1, restart, lambda seconds: seconds < 0.3, 3)
    deck, renderer = player.deck(restart, 48_000, 0, True)
    return dict(completed_frame=completed, restarted_compact_seconds=position,
                restarted_deck_frame=deck, restarted_renderer_frame=renderer)


def cd_seeks(player, case, rates, output):
    selected = player.control(TITLES[1])
    if case == "only":
        p = W.POINT(selected.x, selected.y)
        user32.ScreenToClient(player.hwnd, C.byref(p))
        position = (p.y << 16) | (p.x & 0xffff)
        user32.SendMessageW(player.hwnd, 0x204, 2, position)  # WM_RBUTTONDOWN
        user32.SendMessageW(player.hwnd, 0x205, 0, position)  # WM_RBUTTONUP
        player.click("Play Only This Track")
    else:
        click_control(player.hwnd, selected)
        click_control(player.hwnd, selected)
    size = 1 if case == "only" else 3
    entry = 1 if case == "only" else 2
    player.scene(False, True, entry, size, 0, lambda seconds: seconds < 2, DURATIONS[1])
    player.deck(0, rates[1], 0 if case == "multiple" else STARTS[1] * rates[1], True)
    full, track = control_centers(player.hwnd, ["Full CD", "Track"])
    left, width = player.bounds()
    outcomes = []

    def source_frame(chapter, time):
        seconds = time - STARTS[chapter] if case == "multiple" else time
        return math.floor(seconds * rates[chapter] + 1e-6)

    def seek_at(x, playing, switch=False, expected=None, entries=None):
        since = performance_counter()
        if switch:
            click_control(player.hwnd, full)
        player.click_x(x)
        seconds = round((x - left) / width * TOTAL, 9)
        chapter = bisect_right(STARTS, seconds) - 1
        expected_id = expected if expected is not None else chapter + 1
        _, compact = player.scene(True, playing, expected_id, entries or size, since,
                                  lambda value: abs(value - seconds) <= (0.22 if playing else 0.00006), TOTAL)
        deck, renderer = player.deck(since, rates[chapter], source_frame(chapter, seconds), playing)
        player.control(TITLES[chapter], compact=True)
        outcomes.append(dict(playing=playing, after_switch=switch, x=x, chapter=chapter + 1,
                             target_seconds=seconds, compact_seconds=compact,
                             deck_frame=deck, renderer_frame=renderer))
        return chapter

    def x_at(seconds):
        return round(left + width * seconds / TOTAL)

    if case == "only":
        player.click("Pause")
        player.control("Play")
        since = performance_counter()
        click_control(player.hwnd, full)
        player.scene(True, False, entry, size, since, lambda seconds: 18 <= seconds < 30, TOTAL)
        # Full CD is freely navigable. Each chapter outside the contiguous
        # queue sequence becomes one new occurrence after the active entry.
        for occurrence, seconds in enumerate((8, 33, 23), start=2):
            seek_at(x_at(seconds), False, expected=occurrence, entries=occurrence)
            screenshot(player.hwnd, output / f"outside-{seconds}.png")
        if player.stage("wave_compact_blocked"):
            raise RuntimeError("a Full CD seek was blocked")
        return dict(seeks=outcomes, blocked=0, inserted=3, queue_entries=4)

    for playing in (True, False):
        if not playing:
            player.click("Pause")
            player.control("Play")
        current = 1 if playing else 2
        for seconds in (23, 8, 33):
            since = performance_counter()
            click_control(player.hwnd, track)
            player.scene(False, playing, current + 1, size, since, lambda _: True, DURATIONS[current])
            # Send the strip click right after the perspective button release.
            current = seek_at(x_at(seconds), playing, switch=True)

    for boundary in STARTS[1:]:
        edge = left + width * boundary / TOTAL
        before = math.ceil(edge) - 1
        seek_at(before, False)
        seek_at(before + 1, False)
        if edge.is_integer():
            seek_at(before + 2, False)

    # Resume before an authored boundary. The playhead and queue advance into
    # the next chapter without an input event or a new seek.
    seek_at(x_at(17.6), False)
    resume = performance_counter()
    player.click("Play")
    player.scene(True, True, 1, size, resume, lambda seconds: 17.5 <= seconds < 18, TOTAL)
    player.scene(True, True, 2, size, resume, lambda seconds: 18 <= seconds < 18.4, TOTAL)
    player.deck(resume, rates[1], source_frame(1, 18), True)
    screenshot(player.hwnd, output / "continued.png")
    return dict(seeks=outcomes, blocked=0, queue_entries=size, natural_chapter_transition=True)


def measure(exe, output, case):
    output.mkdir(parents=True, exist_ok=False)
    source, rates = fixture(output / "fixture", case)
    config = settings(source, 1)
    config["full_cd"] = False
    config["collection"] = dict(albums=False, subfolders=[])
    profile = output / "workspace-v1.json"
    profile.write_text(json.dumps(config), encoding="utf-8")
    stages, trace = output / "stages.csv", output / "trace.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(output), DISQRO_PLAYBACK_TRACE=str(trace),
               DISQRO_PRESENTATION_STAGES=str(stages))
    env.pop("DISQRO_QUALIFY_TARGET_FAILURE", None)
    env.pop("DISQRO_QUALIFY_DEVICE_RECREATION_MS", None)
    command = [str(exe), "--qualify-silent"] + ([str(source)] if case == "replay" else [])
    hwnd = None
    with (output / "player.log").open("x", encoding="utf-8") as log:
        app = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            hwnd = wait_for(lambda: find_window(app.pid) or None, "the private player window")
            player = NativePlayer(hwnd, stages)
            report = replay(player) if case == "replay" else cd_seeks(player, case, rates, output)
        except Exception:
            if hwnd:
                screenshot(hwnd, output / "failure.png")
            raise
        finally:
            if hwnd:
                user32.PostMessageW(hwnd, 0x10, 0, 0)  # WM_CLOSE
            try:
                app.wait(timeout=8)
            except subprocess.TimeoutExpired:
                app.terminate()
                app.wait()
    with trace.open(newline="", encoding="utf-8") as data:
        rows = [row for row in csv.DictReader(data) if all(row.values())]
    assert rows and all(int(row[field]) == 0 for row in rows
                        for field in ("device_errors", "decode_errors", "wave_gpu_errors"))
    saved = json.loads(profile.read_text(encoding="utf-8"))["queue"]["queue"]
    entries = [entry["id"] for entry in saved["entries"]]
    assert entries == ([1, 2, 3] if case in ("single", "multiple") else [1]), entries
    report["queue_ids"] = entries
    (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda value: Path(value).resolve(strict=True))
    parser.add_argument("output", type=lambda value: Path(value).resolve())
    parser.add_argument("--case", choices=CASES)
    parser.add_argument("--desktop-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop_worker:
        try:
            measure(args.exe, args.output, args.case)
        except Exception:
            if args.output.is_dir():
                (args.output / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        for case in (args.case,) if args.case else CASES:
            output = args.output / case
            run([sys.executable, "-B", str(Path(__file__).resolve()), str(args.exe), str(output),
                 "--case", case, "--desktop-worker"], 100)
            print(f"{case} passed · {output / 'report.json'}", flush=True)

"""Exercise waveform grabs with a synthetic CUE on a muted private desktop.

A press on the detailed waveform holds the recording at once, wherever it
lands, and moving the pointer turns it relative to that grip. A press that
does not move never seeks, and releasing it continues the paused voice."""
import argparse
from array import array
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

from private_desktop import run
from qualify_spectrum import W, click_control, control_centers, find_window, screenshot, user32
from qualify_waveform import counter_frequency, drive, percentiles, performance_counter, settings, wheel


def synthetic_cue(directory):
    folder = directory / "fixture"
    folder.mkdir()
    rate = 48_000
    samples = array("h")
    for frame in range(rate):
        t = frame / rate
        envelope = 0.3 + 0.6 * math.sin(3 * math.pi * t) ** 2
        samples.extend(round(15000 * envelope * math.sin(2 * math.pi * hz * t)) for hz in (229, 379))
    with wave.open(str(folder / "disc.wav"), "wb") as recording:
        recording.setparams((2, 2, rate, 0, "NONE", "not compressed"))
        one_second = samples.tobytes()
        for _ in range(300):
            recording.writeframesraw(one_second)
    source = folder / "disc.cue"
    source.write_text('FILE "disc.wav" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n'
                      'TRACK 02 AUDIO\nINDEX 01 00:05:00\n', encoding="ascii")
    return source, 5 * rate


def rows(path):
    if not path.is_file():
        return []
    with path.open(newline="", encoding="utf-8") as data:
        return [row for row in csv.DictReader(data) if None not in row.values()]


def wait_for(probe, description):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        result = probe()
        if result:
            return result
        time.sleep(0.025)
    raise RuntimeError(f"Timed out waiting for {description}")


def source_frame(row):
    return struct.unpack("<d", struct.pack("<Q", int(row["value"])))[0]


def measure(args):
    output = args.output
    output.mkdir(parents=True, exist_ok=False)
    source, boundary = synthetic_cue(output)
    (output / "workspace-v1.json").write_text(json.dumps(settings(source, 2)), encoding="utf-8")
    trace, stages = output / "trace.csv", output / "stages.csv"
    env = dict(os.environ, DISQRO_PROFILE_DIR=str(output), DISQRO_PLAYBACK_TRACE=str(trace),
               DISQRO_PRESENTATION_STAGES=str(stages))
    env.pop("DISQRO_QUALIFY_TARGET_FAILURE", None)
    env.pop("DISQRO_QUALIFY_DEVICE_RECREATION_MS", None)

    def stage(name, since=0):
        return [row for row in rows(stages) if row["stage"] == name and int(row["qpc"]) >= since]

    def mouse(message, x, y=0.27, down=False):
        client = W.RECT()
        user32.GetClientRect(hwnd, C.byref(client))
        position = ((round(y * client.bottom) & 0xffff) << 16) | (round(x * client.right) & 0xffff)
        user32.SendMessageW(hwnd, message, int(down), position)

    def glide(first, last, seconds, y=0.27):
        steps = round(120 * seconds)
        drive(120, seconds, lambda n: mouse(0x200, first + (last - first) * (n + 1) / steps, y, down=True))

    def click(label):
        click_control(hwnd, control_centers(hwnd, [label])[0])

    def finish(since):
        end = wait_for(lambda: stage("wave_scrub_end", since), "scrub to settle")[0]
        begin = stage("wave_scrub_begin", since)
        assert len(begin) == 1 and begin[0]["detail"] == end["detail"], (begin, end)
        return source_frame(begin[0]), int(end["value"])

    def wait_for_span(since, frames):
        bits = struct.unpack("<Q", struct.pack("<d", frames))[0]
        wait_for(lambda: any(int(row["detail"]) == bits for row in stage("wave_playhead", since)),
                 f"the drawn {frames:,}-frame transform")

    hwnd = None
    with (output / "player.log").open("x", encoding="utf-8") as log:
        app = subprocess.Popen([str(args.exe), "--qualify-silent", str(source)], env=env, stdout=log, stderr=log)
        try:
            hwnd = wait_for(lambda: find_window(app.pid), "the private player window")
            wait_for(lambda: stage("wave_playhead"), "a running detail deck")
            click("Pause")
            control_centers(hwnd, ["Play"])

            # A stationary press holds at the playhead and never seeks to the
            # position under the pointer.
            before_click = source_frame(stage("wave_playhead")[-1])
            stationary = performance_counter()
            mouse(0x201, 0.72, down=True)
            time.sleep(0.25)
            mouse(0x202, 0.72)
            still_origin, still_end = finish(stationary)
            assert abs(still_origin - before_click) <= 1, (before_click, still_origin)
            assert abs(still_origin - still_end) <= 0.5, (still_origin, still_end)
            assert abs(source_frame(stage("wave_playhead")[-1]) - before_click) <= 1, "A stationary press moved the recording"

            paused = performance_counter()
            mouse(0x201, 0.79, down=True)
            glide(0.79, 0.68, 0.35)
            glide(0.68, 0.80, 0.4)
            glide(0.80, 0.21, 1.0)  # Finish outside the waveform pane.
            time.sleep(0.10)
            mouse(0x202, 0.21)
            paused_origin, paused_end = finish(paused)
            assert abs(paused_origin - before_click) <= 1, (before_click, paused_origin)
            assert paused_origin < boundary <= paused_end, (paused_origin, boundary, paused_end)
            control_centers(hwnd, ["Play"])

            click("Play")
            control_centers(hwnd, ["Pause"])
            escaped = performance_counter()
            mouse(0x201, 0.70, down=True)
            glide(0.70, 0.65, 0.3)
            glide(0.65, 0.72, 0.3)
            # GPUI translates queued keyboard messages before dispatch.
            user32.PostMessageW(hwnd, 0x100, 0x1b, 0x00010001)  # Escape
            user32.PostMessageW(hwnd, 0x101, 0x1b, 0xc0010001)
            canceled_origin, canceled_end = finish(escaped)
            mouse(0x202, 0.72)
            assert abs(canceled_origin - canceled_end) <= 0.5, (canceled_origin, canceled_end)
            assert stage("wave_scrub_resume", escaped), "Escape reopened the transport"
            control_centers(hwnd, ["Pause"])

            playing = performance_counter()
            mouse(0x201, 0.64, down=True)
            glide(0.64, 0.82, 0.55)
            mouse(0x202, 0.82)
            playing_origin, playing_end = finish(playing)
            assert playing_end < playing_origin, (playing_origin, playing_end)
            control_centers(hwnd, ["Pause"])

            # Holding still stops the recording; releasing continues the same
            # voice from where it stopped.
            held = performance_counter()
            mouse(0x201, 0.60, down=True)
            time.sleep(0.4)
            mouse(0x202, 0.60)
            held_origin, held_end = finish(held)
            assert abs(held_origin - held_end) <= 0.5, (held_origin, held_end)
            assert stage("wave_scrub_resume", held), "A stationary hold reopened the transport"
            time.sleep(0.3)
            assert source_frame(stage("wave_playhead")[-1]) > held_end, "Playback did not continue after the hold"
            control_centers(hwnd, ["Pause"])

            click("Pause")
            control_centers(hwnd, ["Play"])
            zoomed = performance_counter()
            for _ in range(18):
                wheel(hwnd, 0.62, 0.27, 480)
            wait_for_span(zoomed, 32)
            deep = performance_counter()
            mouse(0x201, 0.66, down=True)
            glide(0.66, 0.75, 0.35)
            mouse(0x202, 0.75)
            deep_origin, deep_end = finish(deep)
            assert 0 < abs(deep_origin - deep_end) < 32, (deep_origin, deep_end)
            screenshot(hwnd, output / "sample-zoom.png")

            hidden = performance_counter()
            mouse(0x201, 0.70, 0.61, down=True)
            glide(0.70, 0.61, 0.30, y=0.61)
            wait_for(lambda: stage("wave_scrub_begin", hidden), "second deck to own the grab")
            user32.ShowWindow(hwnd, 6)  # SW_MINIMIZE
            hidden_origin, hidden_end = finish(hidden)
            assert abs(hidden_origin - hidden_end) <= 0.5, (hidden_origin, hidden_end)
            user32.ShowWindow(hwnd, 9)  # SW_RESTORE
            mouse(0x202, 0.61, 0.61)
            control_centers(hwnd, ["Play"])

            # The wide deck still follows the grab while the sample deck holds an inspection view.
            zoomed = performance_counter()
            for _ in range(18):
                wheel(hwnd, 0.65, 0.61, -480)
            wait_for_span(zoomed, 120 * 48_000)
            wheel(hwnd, 0.65, 0.27, -120, horizontal=True)
            wide = performance_counter()
            mouse(0x201, 0.73, 0.61, down=True)
            glide(0.73, 0.68, 0.4, y=0.61)
            mouse(0x202, 0.68, 0.61)
            wide_origin, wide_end = finish(wide)
            assert 48_000 < wide_end - wide_origin < 120 * 48_000, (wide_origin, wide_end)
            inspecting_decks = {row["detail"] for row in stage("wave_scrub_draw", wide)}
            assert len(inspecting_decks) == 1, "The inspecting deck followed another deck's grab"

            # Match actual deck updates to the pointer targets sampled by each view.
            inputs = stage("wave_scrub_input", paused)
            draws = stage("wave_scrub_draw", paused)
            matched = []
            frequency = counter_frequency()
            for draw in draws:
                displayed = source_frame(draw)
                qpc = int(draw["qpc"])
                seen = next((event for event in reversed(inputs) if int(event["qpc"]) <= qpc
                             and abs(source_frame(event) - displayed) < 0.00001), None)
                if seen:
                    matched.append((qpc - int(seen["qpc"])) * 1000 / frequency)
            decks = {row["detail"] for row in draws}
            assert len(inputs) > 100 and len(matched) > 100 and len(decks) == 2, (len(inputs), len(matched), decks)
            starts = {row["detail"]: int(row["qpc"]) for row in stage("wave_scrub_begin", paused)}
            ends = {row["detail"]: int(row["qpc"]) for row in stage("wave_scrub_end", paused)}
            scene_draws = sum(any(start <= int(row["qpc"]) <= ends[gesture]
                                  for gesture, start in starts.items())
                              for row in stage("gpui_draw", paused))
            assert scene_draws < len(inputs) * 0.6, (scene_draws, len(inputs))
            time.sleep(1.1)  # Flush the final interval of audio counters.
            observed = rows(trace)
            total = lambda field: sum(int(row[field]) for row in observed)
            peak = lambda field: max(int(row[field]) for row in observed)
            analysis_pages = sum(int(row["page_misses"]) for row in observed
                                 if int(row["scrub_rendered_movements"]) > 0)
            assert total("scrub_rendered_movements") > 100, "Too few PCM movements rendered"
            assert analysis_pages > 0, "No waveform analysis completed in an audio render interval"
            assert all(total(field) == 0 for field in ("decode_errors", "device_errors", "wave_gpu_errors"))
            # The first play, Play after the paused release, and the moved
            # release while playing. Holds that never moved reopened nothing.
            assert total("opens") == total("voices") == 3, "Unexpected normal voice reopen during a gesture"
            assert total("scrub_decoder_opens") == 1, "Scrubbing reopened the physical source"
            report = dict(stationary_paused=[still_origin, still_end],
                          stationary_playing=[held_origin, held_end],
                          paused_cue_crossing=[paused_origin, paused_end],
                          escape_restored=[canceled_origin, canceled_end],
                          playing_reverse_release=[playing_origin, playing_end],
                          sample_zoom=[deep_origin, deep_end], visual_inputs=len(inputs),
                          minimize_restored=[hidden_origin, hidden_end],
                          wide_zoom=[wide_origin, wide_end],
                          analysis_pages_during_audition=analysis_pages,
                          matched_deck_draws=len(matched), simultaneous_decks=len(decks),
                          scene_draws_during_grabs=scene_draws,
                          pointer_to_deck_ms_p50_p95_p99_max=percentiles(matched),
                          rendered_movements=total("scrub_rendered_movements"),
                          pointer_to_audio_callback_max_ms=peak("scrub_pointer_to_audio_max_us") / 1000,
                          normal_voices=total("voices"), normal_file_opens=total("opens"),
                          scrub_source_opens=total("scrub_decoder_opens"))
            (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        finally:
            if hwnd:
                user32.PostMessageW(hwnd, 0x10, 0, 0)
            try:
                app.wait(timeout=8)
            except subprocess.TimeoutExpired:
                app.terminate()
                app.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda value: Path(value).resolve(strict=True))
    parser.add_argument("output", type=lambda value: Path(value).resolve())
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
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--desktop-worker"], 75)
        print((args.output / "report.json").read_text(encoding="utf-8"))

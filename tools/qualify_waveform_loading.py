"""Run the frozen paired wall-clock gate with prebuilt Disqro test executables."""

import argparse
import csv
import ctypes as C
from ctypes import wintypes as W
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import random
from statistics import median
import subprocess
import time

PAIRS = 8
LIMIT = 1.05
FIXTURES = {
    "track.wav": ("b89f571ea827465e", 442368),
    "600.flac": ("667dd5e28ef73e78", 442368),
    "disc.cue": ("977f1aeb40e0f543", 6635520),
    "10800.flac": ("da187aad7a21bda0", 442368),
}
METRICS = ("first_geometry_ms", "full_overview_ms", "exact_visible_ms", "complete_ms")
TEST = ["analysis::baseline_tests::timeline_waveform_baseline", "--exact", "--ignored", "--nocapture", "--test-threads=1"]


def hash_file(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def cpu_seconds(process):
    query = C.windll.kernel32.GetProcessTimes
    query.argtypes = [W.HANDLE, *([C.POINTER(W.FILETIME)] * 4)]
    times = [W.FILETIME() for _ in range(4)]
    if not query(W.HANDLE(int(process._handle)), *(C.byref(value) for value in times)):
        raise C.WinError()
    return sum((value.dwHighDateTime << 32) | value.dwLowDateTime for value in times[2:]) / 1e7


def measure(exe, fixture, detail, log, stop):
    env = os.environ.copy()
    env["DISQRO_WAVE_BASELINE_SOURCE"] = str(fixture)
    env.pop("DISQRO_WAVE_BASELINE_DETAIL", None)
    if detail:
        env["DISQRO_WAVE_BASELINE_DETAIL"] = "1"
    if stop.exists():
        raise InterruptedError("stop marker set before starting")
    interrupted = None
    with log.open("xb") as stream:
        began = time.perf_counter()
        process = subprocess.Popen([str(exe), *TEST], env=env, stdout=stream, stderr=subprocess.STDOUT)
        try:
            while process.poll() is None:
                if stop.exists():
                    raise InterruptedError("quiet window interrupted")
                try:
                    process.wait(timeout=0.5)
                except subprocess.TimeoutExpired:
                    continue
        except (InterruptedError, KeyboardInterrupt) as error:
            interrupted = str(error) or "owner interrupted"
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait()
        wall = time.perf_counter() - began
        cpu = cpu_seconds(process)
    timing = {"processWallSeconds": wall, "processCpuSeconds": cpu}
    if interrupted:
        return {**timing, "interruption": interrupted}
    if process.returncode:
        raise RuntimeError(f"test exited {process.returncode}: {log}")
    lines = log.read_text(encoding="utf-8").splitlines()
    header = next(line[line.index("cache,segments,"):] for line in lines if "cache,segments," in line)
    rows = {row["cache"]: row for row in csv.DictReader(
        [header, *(line for line in lines if line.startswith(("cold,", "warm,")))]
    )}
    if set(rows) != {"cold", "warm"}:
        raise RuntimeError(f"missing cold/warm test result: {log}")
    digest, bytes_held = FIXTURES[fixture.name]
    for row in rows.values():
        if row["geometry_digest"] != digest or int(row["retained_bins_bytes"]) != bytes_held:
            raise RuntimeError(f"incorrect geometry digest or residency: {log}")
    return {**timing, "rows": rows}


def decide(pairs, cache, metric):
    left, right = ([float(pair[side]["rows"][cache][metric]) for pair in pairs]
                   for side in ("baseline", "candidate"))
    if min(left + right) <= 0:
        return {"outcome": "INCONCLUSIVE", "reason": "reported wall time is below the instrument's resolution"}
    observed = median(right) / median(left)
    rng = random.Random(0)
    ratios = []
    for _ in range(5_000):
        indices = rng.choices(range(PAIRS), k=PAIRS)
        ratios.append(median(right[i] for i in indices) / median(left[i] for i in indices))
    ratios.sort()
    low, high = ratios[125], ratios[4_874]
    if high <= LIMIT:
        outcome = "PASS"
    elif low > LIMIT:
        outcome = "REGRESSION"
    else:
        outcome = "INCONCLUSIVE"
    pair_ratios = [c / b for b, c in zip(left, right)]
    return {"baselineMedianMs": median(left), "candidateMedianMs": median(right),
            "medianRatio": observed, "pairedRatioRange": [min(pair_ratios), max(pair_ratios)],
            "pairedBootstrap95": [low, high], "outcome": outcome}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for side in ("baseline", "candidate"):
        parser.add_argument(f"--{side}", type=Path, required=True)
        parser.add_argument(f"--{side}-lock", type=Path, required=True)
        parser.add_argument(f"--{side}-revision", required=True)
        parser.add_argument(f"--{side}-tree", required=True)
    parser.add_argument("--engine-revision", required=True)
    parser.add_argument("--engine-tree", required=True)
    parser.add_argument("--engine-lock", type=Path, required=True)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--quiet-window", required=True, help="Owner-confirmed local window, including timezone.")
    parser.add_argument("--configuration", required=True, help="AC/battery, power profile, OS and other relevant settings.")
    args = parser.parse_args()
    output = args.output.resolve()
    fixture_root = args.fixtures.resolve(strict=True)
    fixture_hashes = {name: hash_file(fixture_root / name) for name in (*FIXTURES, "4500.flac")}
    exes = {side: getattr(args, side).resolve(strict=True) for side in ("baseline", "candidate")}
    artifacts = {side: {"exe": str(exe), "exeSha256": hash_file(exe),
                        "revision": getattr(args, f"{side}_revision"), "tree": getattr(args, f"{side}_tree"),
                        "lockSha256": hash_file(getattr(args, f"{side}_lock"))}
                 for side, exe in exes.items()}
    if len(set(exes.values())) != 2:
        parser.error("Use separate prebuilt baseline and candidate outputs.")
    output.mkdir(parents=True)
    stop = output / "STOP"
    order = [(fixture, detail, pair + 1,
              ["baseline", "candidate"] if (index + detail + pair) % 2 == 0 else ["candidate", "baseline"])
             for index, fixture in enumerate(FIXTURES) for detail in (False, True) for pair in range(PAIRS)]
    write_json(output / "plan.json", {
        "startedUtc": datetime.now(timezone.utc).isoformat(),
        "quietWindow": args.quiet_window, "configuration": args.configuration,
        "os": platform.platform(), "powerScheme": subprocess.check_output(["powercfg", "/getactivescheme"], text=True).strip(),
        "runnerSha256": hash_file(Path(__file__)), "executableArguments": TEST, "artifacts": artifacts,
        "engine": {"revision": args.engine_revision, "tree": args.engine_tree, "lockSha256": hash_file(args.engine_lock)},
        "fixtureSha256": fixture_hashes, "warmup": "one ungated track.wav without detail per build",
        "cache": "Fresh temporary evidence database per process. Warm reopens that process's populated SQLite database.",
        "statistic": "candidate median ms / baseline median ms; paired bootstrap of the same statistic",
        "metrics": METRICS, "ratioLimit": LIMIT, "order": order,
        "exclusion": "External interruption stops the run; retain all logs and the partial pair. No exclusions for slowness.",
    })
    attempts = []
    decisions = {}
    try:
        for side, exe in exes.items():
            attempt = {"warmup": side}
            attempts.append(attempt)
            attempt["result"] = measure(exe, fixture_root / "track.wav", False, output / f"warmup-{side}.log", stop)
            if "interruption" in attempt["result"]:
                raise InterruptedError(attempt["result"]["interruption"])
        for fixture, detail, pair, sides in order:
            workload = f"{fixture}-detail-{int(detail)}"
            attempt = {"workload": workload, "pair": pair, "order": sides}
            attempts.append(attempt)
            for side in sides:
                log = output / f"{workload}-{pair:02d}-{side}.log"
                attempt[side] = measure(exes[side], fixture_root / fixture, detail, log, stop)
                write_json(output / "attempts.json", attempts)
                if "interruption" in attempt[side]:
                    raise InterruptedError(attempt[side]["interruption"])
            if pair == PAIRS:
                pairs = [a for a in attempts if a.get("workload") == workload]
                decisions[workload] = {
                    cache: {metric: decide(pairs, cache, metric)
                            for metric in METRICS if detail or metric != "exact_visible_ms"}
                    for cache in ("cold", "warm")
                }
                write_json(output / "decisions.json", decisions)
    except (InterruptedError, KeyboardInterrupt) as error:
        write_json(output / "interruption.json", {"reason": str(error) or "owner interrupted"})
        return 2
    finally:
        write_json(output / "attempts.json", attempts)
    outcomes = {metric["outcome"] for workload in decisions.values()
                for cache in workload.values() for metric in cache.values()}
    outcome = "REGRESSION" if "REGRESSION" in outcomes else (
        "INCONCLUSIVE" if "INCONCLUSIVE" in outcomes else "PASS"
    )
    write_json(output / "result.json", {"outcome": outcome, "workloads": len(decisions)})
    return {"PASS": 0, "REGRESSION": 1, "INCONCLUSIVE": 2}[outcome]


if __name__ == "__main__":
    raise SystemExit(main())

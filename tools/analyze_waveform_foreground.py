"""Join Disqro frame timing to a PresentMon v1 foreground capture."""

from bisect import bisect_left, bisect_right
from collections import Counter, defaultdict
import csv
import json
import math

from qualify_waveform_motion import quantiles, real, records, signed


def read_rows(path):
    with path.open(encoding="utf-8-sig", newline="") as source:
        return list(csv.DictReader(line for line in source if line.endswith("\n")))


def number(row, key):
    try:
        value = float(row[key])
    except (KeyError, TypeError, ValueError):
        return None
    return value if math.isfinite(value) else None


def chain_calls(stages, dxgi, hwnd):
    # Only a successful base Present with the same window, frame, and chain
    # may be joined. QPC proximity alone also matches overlay swap chains.
    base = defaultdict(list)
    for row in dxgi:
        if int(row["window"]) == hwnd and row["foreground"] == "1" and row["present_status"] == "0":
            base[int(row["frame"])].append(int(row["chain"]))
    calls = defaultdict(list)
    frames = defaultdict(dict)
    for row in stages:
        if row.window != hwnd or not row.frame:
            continue
        if row.stage == "present" and row.detail == 0 and len(base[row.frame]) == 1:
            calls[base[row.frame][0]].append((row.qpc - row.ticks, row.frame))
        frames[row.frame][(row.stage, row.panel)] = row
    return {chain: sorted(presents) for chain, presents in calls.items()}, frames


def join_presents(presents, calls, tolerance):
    ticks = [tick for tick, _ in calls]
    matched = {}
    for index, (tick, _, _, _) in enumerate(presents):
        lo = bisect_left(ticks, tick - tolerance)
        hi = bisect_right(ticks, tick + tolerance)
        if hi == lo + 1:
            matched[index] = calls[lo][1]
    claims = Counter(matched.values())
    return {index: frame for index, frame in matched.items() if claims[frame] == 1}


def motion_metrics(displayed, joined, frames, frequency, refresh, span):
    origins = {}
    panels = set()
    for index, *_ in displayed:
        if index not in joined:
            continue
        latest = {}
        for (stage, panel), row in frames[joined[index]].items():
            if stage in ("wave_motion", "wave_scene_motion") and (
                panel not in latest or row.qpc >= latest[panel].qpc
            ):
                latest[panel] = row
        origins[index] = latest
        panels.update(latest)
    result = {}
    for panel in sorted(panels):
        errors, one_refresh, steps = [], [], []
        for (first, a), (second, b) in zip(displayed, displayed[1:]):
            left = origins.get(first, {}).get(panel)
            right = origins.get(second, {}).get(panel)
            if left is None or right is None or left.detail != right.detail or b <= a:
                continue
            # Keep the actual ETW adjacency. A missing match never turns two
            # short intervals into a longer velocity sample.
            seconds = (b - a) / frequency
            expected = left.detail * seconds / span
            if expected <= 0:
                continue
            step = real(right.value) - real(left.value)
            error = abs(100 * (step - expected) / expected)
            errors.append(error)
            if abs(seconds * refresh - 1) <= 0.12:
                steps.append(step)
                one_refresh.append(error)
        result[panel] = dict(
            matched_origins=sum(panel in points for points in origins.values()),
            adjacent_intervals=len(errors),
            one_refresh_intervals=len(one_refresh),
            absolute_velocity_error_percent=quantiles(errors),
            one_refresh_absolute_velocity_error_percent=quantiles(one_refresh),
            one_refresh_step_px=quantiles(steps),
        )
    return result


def swap_chain_metrics(presents, calls, frames, frequency, refresh, span):
    joined = join_presents(presents, calls, round(frequency * 0.00025))
    screen_slots = defaultdict(list)
    for index, (_, screen, _, _) in enumerate(presents):
        if screen is not None:
            screen_slots[screen].append(index)
    displayed = [(indices[0] if len(indices) == 1 else None, screen)
                 for screen, indices in sorted(screen_slots.items())]
    intervals = [(b - a) * 1000 / frequency for (_, a), (_, b) in zip(displayed, displayed[1:])]
    held = Counter(max(1, round(ms * refresh / 1000)) for ms in intervals if ms > 0)
    discarded_indices = [index for index, item in enumerate(presents) if item[2]]
    estimates = {}
    for label, dropped in (("displayed", False), ("discarded", True)):
        wake, ready_to_target, ready_to_estimate = [], [], []
        for index, frame in joined.items():
            submit, screen, discarded, event = presents[index]
            if discarded != dropped or (not discarded and screen is None):
                continue
            details = frames[frame]
            signal = details.get(("wave_frame_signal", 0))
            if signal:
                wake.append(signal.value / 1e6)
            render = number(event, "msUntilRenderComplete")
            if render is None:
                continue
            end = submit + round(render * frequency / 1000)
            target = details.get(("wave_frame_target", 0))
            if target:
                target_qpc = target.qpc - target.ticks + round(signed(target.value) * frequency / 1e9)
                ready_to_target.append((end - target_qpc) * 1000 / frequency)
            estimate = details.get(("compositor_estimate", 0))
            if estimate and 0 < estimate.value - estimate.qpc < frequency // 20:
                ready_to_estimate.append((end - estimate.value) * 1000 / frequency)
        estimates[label] = dict(
            ui_dispatch_ms=quantiles(wake),
            gpu_complete_minus_app_target_ms=quantiles(ready_to_target),
            gpu_complete_minus_dcomp_estimate_ms=quantiles(ready_to_estimate),
        )
    return dict(
        submissions=len(presents),
        observed_submission_seconds=(presents[-1][0] - presents[0][0]) / frequency,
        discarded=len(discarded_indices),
        submissions_between_discards=dict(sorted(Counter(
            second - first for first, second in zip(discarded_indices, discarded_indices[1:])
        ).items())),
        displayed=len(displayed),
        ambiguous_display_slots=sum(len(indices) > 1 for indices in screen_slots.values()),
        unknown=sum(item[1] is None and not item[2] for item in presents),
        display_interval_ms=quantiles(intervals),
        displayed_interval_refreshes=dict(sorted(held.items())),
        missed_refreshes=sum((count - 1) * occurrences for count, occurrences in held.items()),
        joined_submissions=len(joined),
        joined_displayed=sum(index in joined for index, _ in displayed),
        unjoined_displayed=sum(index not in joined for index, _ in displayed),
        present_modes=dict(Counter(item[3]["PresentMode"] for item in presents)),
        sync_intervals=dict(Counter(item[3]["SyncInterval"] for item in presents)),
        present_call_ms=quantiles([value for item in presents
                                   if (value := number(item[3], "msInPresentAPI")) is not None]),
        gpu_active_ms=quantiles([value for item in presents
                                  if (value := number(item[3], "msGPUActive")) is not None]),
        motion=motion_metrics(displayed, joined, frames, frequency, refresh, span),
        outcome_timing=estimates,
    )


def analyze_take(directory, etw_path):
    metadata = json.loads((directory / "capture.json").read_text(encoding="utf-8"))
    start, stop = metadata["start_qpc"], metadata["stop_qpc"]
    frequency = metadata["qpc_frequency"]
    stages = records(directory / "stages.csv")
    calls, frames = chain_calls(stages, read_rows(directory / "dxgi.csv"), metadata["hwnd"])
    chains = defaultdict(list)
    for row in read_rows(etw_path):
        tick = int(row["QPCTime"])
        if int(row["ProcessID"]) != metadata["pid"] or not start <= tick < stop:
            continue
        chain = int(row["SwapChainAddress"], 16)
        delay = number(row, "msUntilDisplayed")
        dropped = row["Dropped"].strip().lower() in ("1", "true")
        screen = tick + round(delay * frequency / 1000) if delay and delay > 0 and not dropped else None
        chains[chain].append((tick, screen, dropped, row))
    span = metadata["span_frames"] / metadata["source_rate"]
    metrics = {
        f"{chain:#x}": swap_chain_metrics(sorted(rows, key=lambda row: row[0]), calls[chain],
                                         frames, frequency, metadata["refresh"], span)
        for chain, rows in chains.items() if chain in calls
    }
    phases = defaultdict(list)
    for row in stages:
        if start <= row.qpc < stop:
            phases[row.stage].append(row)
    playheads = phases["wave_playhead"]
    observed_frames = {real(row.detail) for row in playheads if row.window == metadata["hwnd"]}
    if len(observed_frames) != 1 or abs(observed_frames.pop() - metadata["span_frames"]) >= 0.05:
        raise ValueError("The captured frame span did not match the verified seconds.")
    scene_upload = phases["scene_upload"]
    widths = sorted({row.detail for stage in ("wave_motion", "wave_scene_motion")
                     for row in phases[stage] if row.window == metadata["hwnd"]})
    commits = defaultdict(list)
    for row in phases["audio_commit"]:
        commits[row.value].append(row)
    observed_rates = []
    for voice in commits.values():
        first, last = min(voice, key=lambda row: row.qpc), max(voice, key=lambda row: row.qpc)
        if last.qpc > first.qpc and last.detail > first.detail:
            observed_rates.append((last.detail - first.detail) * frequency / (last.qpc - first.qpc))
    return dict(
        capture=metadata,
        chain=next(iter(metrics)) if len(metrics) == 1 else None,
        chains=metrics,
        excluded_swap_chains={f"{chain:#x}": len(rows) for chain, rows in chains.items() if chain not in calls},
        swap_chain_configuration={
            f"{row.value:#x}": dict(buffers=row.detail >> 32, maximum_frame_latency=row.detail & 0xFFFFFFFF)
            for row in stages if row.stage == "swapchain_configuration" and row.window == metadata["hwnd"]
        },
        scene_upload_bytes=quantiles([row.value for row in scene_upload]),
        scene_primitives=quantiles([row.detail for row in scene_upload]),
        waveform_widths_px=widths,
        scene_rebuilds=len(phases["gpui_draw"]),
        posted_dispatches=sum(row.value for row in phases["frame_dispatch"]),
        backpressured_dispatches=sum(not row.detail for row in phases["frame_dispatch"]),
        early_dispatch_count=sum(row.detail > 0 for row in phases["early_frame_wait"]),
        early_dispatch_lead_ms=quantiles([row.detail / 1e6 for row in phases["early_frame_wait"] if row.detail]),
        measured_pacing_period_ms=quantiles([row.detail / 1e6 for row in phases["wave_frame_target"]]),
        measured_source_frames_per_second=observed_rates,
        stage_ms={name: quantiles([row.ticks * 1000 / frequency for row in phases[name]])
                  for name in ("wave_frame", "wave_retained_submit", "scene_upload", "composition",
                               "window_frame", "present", "wave_projection")},
        audio_clock_phase_ms=quantiles([abs(signed(row.value)) / 1e6 for row in phases["wave_clock"]]),
        dropped_trace_records=sum(row.value for row in phases["dropped"]),
    )


def summarize(output, reports):
    lines = [
        "# Foreground waveform presentation", "",
        "| Mode / round | Layout / source | Span | Display interval p50 / p95 / p99 / max (ms) | 1 / 2 / 3+ refresh intervals | Discarded | Joined display | One-refresh velocity error p99 |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for report in reports:
        capture = report["capture"]
        layout = capture.get("layout", "split")
        source = capture.get("source_role", "reference")
        prefix = f"| {capture['mode']} / {capture['round']} | {layout} / {source} | {capture['span']} s |"
        chain = report["chains"].get(report["chain"])
        if not chain or not chain["display_interval_ms"]:
            lines.append(prefix + " Unresolved | | | | |")
            continue
        seconds = (capture["stop_qpc"] - capture["start_qpc"]) / capture["qpc_frequency"]
        if chain["observed_submission_seconds"] < seconds - 0.1:
            lines.append(prefix + f" Partial ETW ({chain['observed_submission_seconds']:.3f} s) | | | | |")
            continue
        timing = " / ".join(f"{chain['display_interval_ms'][key]:.3f}" for key in ("p50", "p95", "p99", "max"))
        slots = chain["displayed_interval_refreshes"]
        counts = f"{slots.get(1, 0)} / {slots.get(2, 0)} / {sum(n for k, n in slots.items() if k > 2)}"
        matched = chain["joined_displayed"]
        panel = next(iter(chain["motion"].values())) if len(chain["motion"]) == 1 else {}
        velocity = panel.get("one_refresh_absolute_velocity_error_percent")
        value = f"{velocity['p99']:.2f}%" if velocity else "Unavailable"
        lines.append(prefix + f" {timing} | {counts} | {chain['discarded'] / chain['submissions']:.2%} | "
                     f"{matched}/{chain['displayed']} | {value} |")
    (output / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

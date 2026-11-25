"""Replay Dekzer's pinned source renderers in hidden Electron. Keep output local."""
import argparse
from hashlib import file_digest
from io import BytesIO
import json
from math import ceil, floor, log2
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

from privateDesktop import run as private_desktop


ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "tools" / "waveformParity"
REFERENCE = "8107c46fd9c7cbb697927a59def9a52f9079dc4f"
REJECTED = "a2e1590882fa9ff5b03d0ba9c5ee566590a68119"
REVISIONS = {
    "stroke": REFERENCE,
    "source-tiers": "e3a7d420bf1a9ca8584d8e854fd316229d335db1",
    "direct": "06d27de71b7a6f651b606ed8e0c7963af35e6f19",
    "ribbon": "312d6c42dd48b40a5bd9126cab9757fbc8f995c2",
    "spectral": "3ada7bcc58e1b9ab8726bdd3afb15b9f773f561a",
}
RATES = (8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000,
         64000, 88200, 96000, 176400, 192000)
FRAMES_PER_TILE = 262144


def command(*args, cwd=ROOT, **kwargs):
    return subprocess.run([str(a) for a in args], cwd=cwd, check=True, **kwargs)


def git(dekzer, *args):
    return command("git", "-C", dekzer, *args, stdout=subprocess.PIPE).stdout


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def stage_file(path, data):
    if not path.exists() or path.read_bytes() != data:
        path.write_bytes(data)


def digest(path):
    with path.open("rb") as data:
        return file_digest(data, "sha256").hexdigest()


def extract(dekzer, revision, destination, *paths):
    with zipfile.ZipFile(BytesIO(git(dekzer, "archive", "--format=zip", revision, "--", *paths))) as archive:
        archive.extractall(destination)


def prepare(args):
    work = args.work
    work.mkdir(parents=True, exist_ok=True)
    native = work / "native-source"
    if not (native / "Cargo.lock").exists():
        extract(args.dekzer, REFERENCE, native, "Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                "crates", "apps/desktop/native")
    example = native / "crates" / "playback" / "examples" / "disqro-oracle.rs"
    example.parent.mkdir(exist_ok=True)
    stage_file(example, (TOOLS / "referenceAnalysis.rs").read_bytes())
    target = work / "build"
    command("cargo", "build", "--release", "--locked", "-p", "dekzer-playback",
            "--example", "disqro-oracle", "--target-dir", target, cwd=native)
    dependencies = args.dekzer / "apps" / "desktop" / "node_modules"
    esbuild = dependencies / "esbuild" / "bin" / "esbuild"
    electron = dependencies / "electron" / "dist" / "electron.exe"
    host = work / "electronHost.cjs"
    command("node", esbuild, TOOLS / "electronHost.cts", "--bundle", "--platform=node",
            "--external:electron", f"--outfile={host}")
    manifest = {"electronSha256": digest(electron), "revisions": {}}
    for name, revision in REVISIONS.items():
        stage = work / name
        stage.mkdir(exist_ok=True)
        if not (stage / "source").exists():
            extract(args.dekzer, revision, stage, "apps/desktop/src")
            (stage / "apps" / "desktop" / "src").rename(stage / "source")
        # Older direct code has a different signature and is retained as source
        # for inspection. The standalone replay covers the current and ribbon paths.
        if name in ("stroke", "ribbon", "spectral"):
            entry = stage / "entry.ts"
            adapter = "renderer.ts" if name == "stroke" else "legacyRenderer.ts"
            shutil.copyfile(TOOLS / adapter, entry)
            command("node", esbuild, entry, "--bundle", "--platform=browser",
                    f"--outfile={stage / 'renderer.js'}")
        tree = git(args.dekzer, "rev-parse", f"{revision}^{{tree}}").decode().strip()
        manifest["revisions"][name] = {"commit": revision, "tree": tree}
        if (stage / "renderer.js").exists():
            manifest["revisions"][name]["rendererSha256"] = digest(stage / "renderer.js")
    write(work / "reference.json", manifest)
    prepare_rejected(work)


def prepare_rejected(work):
    project = work / "rejected-source"
    source = project / "src"
    source.mkdir(parents=True, exist_ok=True)
    stage_file(project / "Cargo.toml",
        """[package]
name = "disqro-rejected-waveform"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
image = { version = "0.25", default-features = false, features = ["png"] }
""".encode("utf-8"))
    stage_file(project / "rust-toolchain.toml", (ROOT / "rust-toolchain.toml").read_bytes())
    stage_file(source / "main.rs", (TOOLS / "rejected.rs").read_bytes())
    for file in ("overview.rs", "waveformBands.rs"):
        stage_file(source / file, git(ROOT, "show", f"{REJECTED}:src/{file}"))
    original = git(ROOT, "show", f"{REJECTED}:src/waveform.rs").decode("utf-8")
    measures = original[original.index("pub const HZ:"):original.index("    fn bytes(")]
    stage_file(source / "waveform.rs",
               ("use crate::Frame;\n" + measures.replace("const RECORD: usize = 56;\n", "") + "}\n").encode("utf-8"))
    original = git(ROOT, "show", f"{REJECTED}:src/analysis.rs").decode("utf-8")
    projection = original[original.index("struct Column {"):original.index("fn analyse(")]
    stage_file(source / "projection.rs", projection.encode("utf-8"))
    # The isolated adapter uses only libraries already built by Disqro.
    command("cargo", "build", "--offline", "--release", cwd=project)


def historical_analyzer(dekzer, work, name):
    native = work / name / "native"
    if not (native / "Cargo.lock").exists():
        extract(dekzer, REVISIONS[name], native, "Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                "crates", "apps/desktop/native")
    folder = native / "crates" / "playback" / "examples" / f"disqro-{name}"
    folder.mkdir(parents=True, exist_ok=True)
    runner = (TOOLS / "historicalAnalysis.rs").read_text(encoding="utf-8")
    if name == "ribbon":
        begin, end = runner.index("mod octave {"), runner.index("#[derive(Deserialize)]")
        runner = runner[:begin] + "mod octave;\n\n" + runner[end:]
        shutil.copyfile(TOOLS / "historicalOctave.rs", folder / "octave.rs")
    (folder / "main.rs").write_text(runner, encoding="utf-8")
    if name == "spectral":
        # This revision's playback crate had no serde_json dev dependency.
        # Use the already locked version from its workspace for export only.
        manifest = native / "crates" / "playback" / "Cargo.toml"
        cargo = manifest.read_text(encoding="utf-8")
        version = re.search(r'(?m)^name = "serde_json"\nversion = "([^"]+)"',
                            (native / "Cargo.lock").read_text(encoding="utf-8"))[1]
        if 'serde_json =' not in cargo:
            annotation = f'serde_json = "={version}"\n'
            if "[dev-dependencies]" in cargo:
                cargo = cargo.replace("[dev-dependencies]\n", "[dev-dependencies]\n" + annotation)
            else:
                cargo += "\n[dev-dependencies]\n" + annotation
            manifest.write_text(cargo, encoding="utf-8")
        # Cargo adds the extra example dependency edge to its staged lockfile.
        offline = "--offline"
    else:
        offline = "--locked"
    command("cargo", "build", "--release", offline, "-p", "dekzer-playback",
            "--example", f"disqro-{name}", "--target-dir", work / "build", cwd=native)
    return work / "build" / "release" / "examples" / f"disqro-{name}.exe"


def plan(start, end, width, *, leaf=None, octave=None, overview=False):
    return dict(startFrame=start, endFrame=end, framesPerColumn=width, leafFrames=leaf,
                overview=overview, octave=octave)


def aligned_pages(start, end, total, width, size, *, leaf=None, octave=None):
    return [plan(a, min(total, a + size), width, leaf=leaf, octave=octave)
            for a in range(start // size * size, end, size)]


def detail_plans(first, width, pps, rate, count, mode):
    start = max(0, floor(first * rate / pps) - ceil(8 * rate / pps) - 1024)
    end = min(count, ceil((first + width) * rate / pps) + ceil(8 * rate / pps) + 1024)
    if mode == "stroke":
        model_rate = rate in (44100, 48000)
        pages = aligned_pages(start, end, count, 256, FRAMES_PER_TILE, leaf=32,
                              octave="stroke-hann512" if model_rate else "blackman512" if rate in RATES else None)
        if model_rate:
            closeup = max(0, min(1, log2(8 * pps / width)))
            columns = min(49152, ceil(max(128, min(16384, width * 10)) * (1 + 3 * closeup)))
            seconds = width / pps
            density = 2 ** ceil(log2(max(1, seconds * rate / (columns - 1))))
            if density < 32:
                pages += aligned_pages(start, end, count, density, 1024 * density, leaf=density)
        return pages
    density = 2 ** ceil(log2(max(1, rate * (width / pps) / (max(128, width * 2) - 1))))
    return aligned_pages(start, end, count, density, 1024 * density)


def cases(args, metadata, revision):
    rate, total = metadata["rate"], metadata["frameCount"]
    frame_count = min(total - args.origin, args.frames or total)
    if not 0 <= args.origin < total or frame_count <= 0:
        raise ValueError("Invalid decoded source segment")
    modes = ("stroke",) if revision == "stroke" else ("spectral", "layered")
    dimensions = [(1920, 166, 1.5), (1200, 132, 1), (560, 36, 2)]
    result = []
    for mode in modes:
        for width, height, dpr in dimensions:
            settings = dict(gain=args.gain, intensity=args.intensity, light=False, mode=mode)
            if not args.details_only:
                case = dict(role="overview", id=f"{mode}-overview-{width}", width=width, height=height,
                            dpr=dpr, first=0, pixelsPerSecond=width * rate / frame_count,
                            origin=args.origin, frameCount=frame_count, rate=rate,
                            visibleFrames=[args.origin, args.origin + frame_count], style=settings)
                spectrum = "blackman512" if rate in RATES and "stroke" == revision else None
                result.append((case, [plan(0, frame_count, ceil(frame_count / 4096),
                                           octave=spectrum, overview=True)]))
            spans = args.seconds if width == dimensions[0][0] or args.all_sizes else (2, 8)
            for span in spans:
                if rate * span > frame_count:
                    continue
                pps = width / span
                first = max(0, min(floor(frame_count * pps / rate) - width,
                                   round(args.center * pps) - width // 2))
                a, b = first * rate / pps, (first + width) * rate / pps
                case = dict(role="detail", id=f"{mode}-{span:g}s-{width}", width=width, height=height,
                            dpr=dpr, first=first, pixelsPerSecond=pps, origin=args.origin, rate=rate,
                            frameCount=frame_count, visibleFrames=[args.origin + a, args.origin + b],
                            style=settings)
                if revision == "stroke":
                    pages = detail_plans(first, width, pps, rate, frame_count, mode)
                else:
                    # Historic Spectral uses the original source span and fixed-rate
                    # octave halo. The primary page covers exactly the visible span.
                    start, end = floor(a), ceil(b)
                    density = ceil((end - start) / 4096)
                    pages = [plan(start, end, density)]
                    if revision == "ribbon" and mode == "spectral":
                        closeup = max(0, min(1, log2(8 / span)))
                        columns = min(49152, ceil(max(128, min(16384, width * 8)) * (1 + 3 * closeup)))
                        density = min(1024, 2 ** ceil(log2(max(1, (end - start) / (columns - 1)))))
                        halo_start = max(0, start - ceil(8 * rate / pps) - 1024)
                        halo_end = min(frame_count, end + ceil(8 * rate / pps) + 1024)
                        pages = aligned_pages(halo_start, halo_end, frame_count, density,
                                              density * 1024, octave="blackman512" if rate in RATES else None)
                result.append((case, pages))
    return result


def replay(args):
    if args.revision not in ("stroke", "ribbon", "spectral"):
        raise ValueError("Standalone replay is available for stroke, ribbon and spectral")
    out = args.output
    out.mkdir(parents=True, exist_ok=False)
    decoder = args.work / "build" / "release" / "examples" / "disqro-oracle.exe"
    reference = decoder if args.revision == "stroke" else historical_analyzer(
        args.dekzer, args.work, args.revision)
    pcm, meta = (args.pcm, args.metadata) if args.pcm else (out / "source.f32", out / "decode.json")
    if not args.pcm:
        command(decoder, "decode", args.audio, pcm, meta)
    metadata = read(meta)
    if args.revision != "stroke":
        metadata["decodeAnalysis"] = metadata["analysis"]
        metadata["analysis"] = command(reference, "identity", stdout=subprocess.PIPE).stdout.decode().strip()
    metadata["pcmSha256"] = digest(pcm)
    metadata["reference"] = read(args.work / "reference.json")["revisions"][args.revision]
    write(out / "source.json", metadata)
    requests = cases(args, metadata, args.revision)
    unique = {}
    for case, pages in requests:
        # The two modes and several panel sizes can reuse the same whole-source analysis.
        plan_key = json.dumps(pages, sort_keys=True)
        if plan_key not in unique:
            label = f"facts-{len(unique)}"
            request = out / f"{label}-request.json"
            request_facts = out / f"{label}.json"
            write(request, dict(rate=case["rate"], origin=case["origin"], frameCount=case["frameCount"],
                                pages=pages))
            command(reference, "facts", pcm, request, request_facts)
            unique[plan_key] = request_facts.name
        case["pages"] = unique[plan_key]
    write(out / "render.json", {"cases": [case for case, _ in requests]})
    if not args.facts_only:
        render(args.dekzer, args.work, out, args.revision)


def render(dekzer, work, out, revision="stroke"):
    renderer = work / revision / "renderer.js"
    shutil.copyfile(renderer, out / "renderer.js")
    (out / "oracle.html").write_text(
        '<!doctype html><meta charset="utf-8"><script defer src="renderer.js"></script>\n',
        encoding="utf-8")
    electron = dekzer / "apps" / "desktop" / "node_modules" / "electron" / "dist" / "electron.exe"
    try:
        private_desktop([str(electron), str(work / "electronHost.cjs"), str(out)], 240)
    except RuntimeError as error:
        log = out / "electron-error.txt"
        if log.exists():
            raise RuntimeError(log.read_text(encoding="utf-8")) from error
        raise


def disqro(exe, output, *args):
    log = output / "disqro-error.txt"
    try:
        command(exe, "--waveform-parity", log, *args)
    except subprocess.CalledProcessError as error:
        if log.exists():
            raise RuntimeError(log.read_text(encoding="utf-8")) from error
        raise


def pixel_score(reference, actual):
    import numpy as np

    a, b = (np.asarray(p, dtype=np.int16) for p in (reference, actual))
    diff = np.abs(a - b)
    local = diff[a.any(axis=-1) | b.any(axis=-1)]
    return {
        "pixelMaxCodes": int(diff.max()),
        "pixelMeanCodes": float(diff.mean()),
        "activePixelMeanCodes": float(local.mean()) if local.size else 0.0,
        "activePixelP99Codes": float(np.percentile(local, 99)) if local.size else 0.0,
    }


def outside_limits(score, direct, limits):
    if direct:
        return (score["pixelMaxCodes"] > limits["strokePixelMaximumCodes"]
                or score["pixelMeanCodes"] > limits["strokePixelMeanCodes"])
    return (score["pixelMaxCodes"] > limits["canvasPixelMaximumCodes"]
            or score["activePixelMeanCodes"] > limits["canvasActiveMeanCodes"]
            or score["activePixelP99Codes"] > limits["canvasActiveP99Codes"])


def score_gpu_case(reference, oracle, actual, case, limits):
    import numpy as np
    from PIL import Image

    name = case["id"]
    reference_image, cpu_image, gpu_image = (
        Image.open(folder / f"{name}.png").convert("RGB") for folder in (reference, oracle, actual)
    )
    detail = case["role"] == "detail"
    direct = "starts" in read(reference / f"{name}.json")
    source_score = pixel_score(reference_image, gpu_image)
    cpu_score = pixel_score(cpu_image, gpu_image)
    # Canvas snapping and coverage remain independent from the shader. Keep
    # the frozen Electron image limits and also budget only one code vs Rust.
    failed = outside_limits(source_score, direct, limits) or cpu_score["pixelMaxCodes"] > 1
    geometry = None
    if detail:
        cpu = read(oracle / f"{name}.json")["Stroke"]
        gpu = read(actual / f"{name}.json")["Stroke"]
        a, b = (np.asarray(data["columns"], dtype=np.float32) for data in (cpu, gpu))
        geometry = {
            "boundariesExact": cpu["firstCell"] == gpu["firstCell"] and cpu["starts"] == gpu["starts"],
            "extremaAndWitnessesExact": cpu["cores"] == gpu["cores"],
            "statesExact": bool(np.array_equal(a[:, 7], b[:, 7])),
            "geometryMaxPx": float(np.max(np.abs(a[:, :4] - b[:, :4]))),
            "rgbMaxCodes": float(np.max(np.abs(a[:, 4:7] - b[:, 4:7]))),
        }
        failed |= (not all(geometry[key] for key in ("boundariesExact", "extremaAndWitnessesExact", "statesExact"))
                   or geometry["geometryMaxPx"] > limits["geometryDevicePixels"]
                   or geometry["rgbMaxCodes"] > limits["strokeRgbCodes"])
    return {"electron": source_score, "rust": cpu_score, "geometry": geometry, "passed": not failed}


def checkpoint_gpu(args):
    args.output.mkdir(parents=True, exist_ok=False)
    limits = read(TOOLS / "tolerances.json")
    batch = {}
    for stored in sorted(args.checkpoint.iterdir()):
        if not (stored / "parity.json").is_file():
            continue
        reference = args.checkpoint.parent / stored.name
        approved = read(stored / "parity.json")
        if approved["failures"] or approved["reference"]["commit"] != REFERENCE:
            raise ValueError(f"{stored.name}: checkpoint did not pass the frozen reference gate")
        cases = [case for case in read(reference / "render.json")["cases"]
                 if case["id"].startswith("stroke-") and case["id"] in approved["cases"]]
        target = args.output / stored.name
        target.mkdir()
        cases_path = target / "cases.json"
        write(cases_path, {"cases": cases})
        facts = stored / "facts"
        disqro(args.exe, target, "render-gpu", cases_path, facts, target)
        batch[stored.name] = {
            "adapter": read(target / "adapter.json"),
            "pcmSha256": approved["pcmSha256"],
            "cases": {case["id"]: score_gpu_case(reference, stored / "candidate", target, case, limits)
                      for case in cases},
        }
    if not batch:
        raise ValueError("No accepted Stroke comparisons were present")
    failures = [f"{recording}/{case}" for recording, result in batch.items()
                for case, score in result["cases"].items() if not score["passed"]]
    write(args.output / "parity.json", {"exeSha256": digest(args.exe), "recordings": batch, "failures": failures})
    count = sum(len(recording["cases"]) for recording in batch.values())
    print(f"{count - len(failures)}/{count} frozen Stroke views passed on D3D11")
    if failures:
        raise SystemExit("Failed " + ", ".join(failures))


def compare(args):
    import numpy as np
    from PIL import Image, ImageDraw

    reference, output = args.reference, args.output
    output.mkdir(parents=True, exist_ok=False)
    original = read(reference / "source.json")
    if original["reference"]["commit"] != REFERENCE:
        raise ValueError("The numerical gate is frozen for the selected Stroke revision")
    pcm = args.pcm or reference / "source.f32"
    if digest(pcm) != original["pcmSha256"]:
        raise ValueError("PCM no longer matches the reference decode")
    limits = read(TOOLS / "tolerances.json")
    native_facts = output / "facts"
    native_facts.mkdir()
    isolated = output / "same-facts"
    isolated.mkdir()
    candidate = output / "candidate"
    candidate.mkdir()
    cases = [case for case in read(reference / "render.json")["cases"] if case["id"].startswith("stroke-")]
    manifest = output / "cases.json"
    write(manifest, {"cases": cases})
    baseline_dir = args.baseline or output / "rejected"
    if not args.baseline:
        baseline_dir.mkdir()
        command(args.work / "rejected-source" / "target" / "release" / "disqro-rejected-waveform.exe",
                manifest, pcm, baseline_dir)
    report = {"facts": {}, "cases": {}, "tolerances": limits, "pcmSha256": original["pcmSha256"],
              "reference": original["reference"], "rejected": REJECTED, "candidateExeSha256": digest(args.exe)}
    problems = []
    if args.audio:
        native_pcm, native_meta = output / "native.f32", output / "native-decode.json"
        disqro(args.exe, output, "decode", args.audio, native_pcm, native_meta)
        native = read(native_meta)
        native["sha256"] = digest(native_pcm)
        native["matches"] = (
            native["sha256"] == original["pcmSha256"] and
            all(native[key] == original[key] for key in ("rate", "frameCount"))
        )
        report["decodedPcm"] = native
        if not native["matches"]:
            problems.append("decoded PCM")
    source_samples = np.memmap(pcm, dtype="<f4", mode="r").reshape(-1, 2)
    for name in sorted({case["pages"] for case in cases}):
        request = reference / name.replace(".json", "-request.json")
        destination = native_facts / name
        disqro(args.exe, output, "facts", pcm, request, destination)
        ref_pages, actual_pages = read(reference / name), read(destination)
        scores = {"geometryFactsMax": 0.0, "amplitudesExact": True, "witnessesExact": True,
                  "octaveMax": 0.0, "octaveWithinTolerance": True}
        if len(ref_pages) != len(actual_pages):
            raise ValueError(f"{name}: the two analyzers returned unequal page counts")
        for ref, actual in zip(ref_pages, actual_pages):
            for key in ("startFrame", "framesPerColumn", "frameCount"):
                if ref[key] != actual[key]:
                    raise ValueError(f"{name}: unequal analysis page boundary {key}")
            if ref["facts"]:
                a = np.asarray(ref["facts"], dtype=np.float32).reshape(-1, 13)[:, :7]
                b = np.asarray(actual["facts"], dtype=np.float32).reshape(-1, 7)
                scores["amplitudesExact"] &= bool(np.array_equal(a[:, :2], b[:, :2]))
                scores["geometryFactsMax"] = max(scores["geometryFactsMax"], float(np.max(np.abs(a - b))))
                scores["witnessesExact"] &= ref["extrema"] == actual["extrema"]
            if "sourceDetail" in ref:
                a, b = ref["sourceDetail"], actual["sourceDetail"]
                for key in ("startFrame", "firstCellFrame", "leafFrames", "frameCount"):
                    if a[key] != b[key]:
                        raise ValueError(f"{name}: unequal analysis leaf boundary {key}")
                scores["amplitudesExact"] &= bool(np.array_equal(
                    np.asarray(a["bounds"], dtype=np.float32), np.asarray(b["bounds"], dtype=np.float32)))
                scores["witnessesExact"] &= a["offsets"] == b["offsets"]
            a = ref.get("spectrum") or ref.get("octave", {}).get("powers")
            b = actual.get("spectrum") or actual.get("octave", {}).get("powers")
            if a is not None:
                a, b = np.asarray(a), np.asarray(b)
                scores["octaveMax"] = max(scores["octaveMax"], float(np.max(np.abs(a - b))))
                scores["octaveWithinTolerance"] &= bool(np.allclose(
                    a, b, rtol=limits["octavePowerRelative"], atol=limits["octavePowerAbsolute"]))
        report["facts"][name] = scores
        if (not all(scores[k] for k in ("amplitudesExact", "witnessesExact", "octaveWithinTolerance"))
                or scores["geometryFactsMax"] > limits["amplitudeAbsolute"]):
            problems.append(f"analysis {name}")

    disqro(args.exe, output, "render", manifest, reference, isolated)
    disqro(args.exe, output, "render", manifest, native_facts, candidate)
    gpu = output / "gpu"
    gpu.mkdir()
    disqro(args.exe, gpu, "render-gpu", manifest, native_facts, gpu)
    report["gpu"] = read(gpu / "adapter.json")
    for case in cases:
        name = case["id"]
        ref = read(reference / f"{name}.json")
        actual = read(isolated / f"{name}.json")
        final = read(candidate / f"{name}.json")
        score = {}
        if "Stroke" in actual and "starts" in ref:
            a = np.asarray(ref["columns"], dtype=np.float32).reshape(-1, 8)
            b = np.asarray(actual["Stroke"]["columns"], dtype=np.float32)
            c = np.asarray(final["Stroke"]["columns"], dtype=np.float32)
            cores = actual["Stroke"]["cores"]
            score["boundariesExact"] = (ref["firstCell"] == actual["Stroke"]["firstCell"]
                                         and ref["starts"] == actual["Stroke"]["starts"])
            expected = np.asarray(list(zip(ref["min"], ref["max"])), dtype=np.float32)
            measured = np.asarray([p["bounds"] or (None, None) for p in cores], dtype=np.float32)
            score["coresExact"] = (ref["held"] == [int(p["held"]) for p in cores]
                                    and bool(np.array_equal(expected, measured, equal_nan=True)))
            score["witnessesBackedByPcm"] = True
            for i, core in enumerate(cores):
                if core["witnesses"] is None:
                    continue
                start = case["origin"] + max(actual["Stroke"]["starts"][i], 0)
                end = case["origin"] + min(actual["Stroke"]["starts"][i + 1], case["frameCount"])
                for frame, value in zip(core["witnesses"], core["bounds"]):
                    if not start <= frame < end or np.float32(value) not in source_samples[frame]:
                        score["witnessesBackedByPcm"] = False
                        break
            score["statusesExact"] = bool(np.array_equal(a[:, 7], b[:, 7]))
            score["geometryMaxPx"] = float(np.max(np.abs(a[:, :4] - b[:, :4])))
            score["rgbMaxCodes"] = float(np.max(np.abs(a[:, 4:7] - b[:, 4:7])))
            score["analysisRgbMaxCodes"] = float(np.max(np.abs(b[:, 4:7] - c[:, 4:7])))
            if (not all(score[k] for k in ("boundariesExact", "coresExact", "statusesExact",
                                            "witnessesBackedByPcm"))
                    or score["geometryMaxPx"] > limits["geometryDevicePixels"]
                    or score["rgbMaxCodes"] > limits["strokeRgbCodes"]
                    or score["analysisRgbMaxCodes"] > limits["strokeRgbCodes"]):
                problems.append(f"geometry or colour {name}")
        ref_png, actual_png = (Image.open(p / f"{name}.png").convert("RGB") for p in (reference, candidate))
        a, b = (np.asarray(p, dtype=np.int16) for p in (ref_png, actual_png))
        diff = np.abs(a - b)
        active = (a.any(axis=-1) | b.any(axis=-1))
        local = diff[active]
        score["pixelMaxCodes"] = int(diff.max())
        y, x, channel = np.unravel_index(diff.argmax(), diff.shape)
        score["pixelMaxAt"] = [int(x), int(y), int(channel)]
        score["pixelMeanCodes"] = float(diff.mean())
        score["activePixelMeanCodes"] = float(local.mean()) if local.size else 0.0
        score["activePixelP99Codes"] = float(np.percentile(local, 99)) if local.size else 0.0
        direct = "starts" in ref
        if direct:
            failed = (score["pixelMaxCodes"] > limits["strokePixelMaximumCodes"]
                      or score["pixelMeanCodes"] > limits["strokePixelMeanCodes"])
        else:
            failed = (score["pixelMaxCodes"] > limits["canvasPixelMaximumCodes"]
                      or score["activePixelMeanCodes"] > limits["canvasActiveMeanCodes"]
                      or score["activePixelP99Codes"] > limits["canvasActiveP99Codes"])
        if failed:
            problems.append(f"pixels {name}")
        score["gpu"] = score_gpu_case(reference, candidate, gpu, case, limits)
        if not score["gpu"]["passed"]:
            problems.append(f"GPU {name}")
        report["cases"][name] = score
        baseline = baseline_dir / f"{name}.png" if baseline_dir else None
        images = [("Dekzer Electron", ref_png)]
        if baseline:
            images.append(("Rejected Disqro", Image.open(baseline).convert("RGB")))
        images += [("Disqro Rust", actual_png),
                   ("Absolute difference (8x)", Image.fromarray((diff * 8).clip(0, 255).astype(np.uint8)))]
        width, height = ref_png.size
        sheet = Image.new("RGB", (width, (height + 20) * len(images)), (23, 23, 23))
        draw = ImageDraw.Draw(sheet)
        for i, (label, pic) in enumerate(images):
            if pic.size != ref_png.size:
                raise ValueError(f"Unmatched dimensions for {name}")
            draw.text((5, i * (height + 20) + 3), label, fill=(238, 238, 238))
            sheet.paste(pic, (0, i * (height + 20) + 20))
        sheet.save(output / f"{name}.png")
    report["failures"] = problems
    write(output / "parity.json", report)
    if problems:
        raise SystemExit(f"{len(problems)} parity checks failed. See {output / 'parity.json'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    paths = parser.add_argument_group("Workspace paths")
    paths.add_argument("--dekzer", type=Path, required=True)
    paths.add_argument("--work", type=Path, required=True)
    actions = parser.add_subparsers(dest="action", required=True)
    actions.add_parser("prepare").set_defaults(do=prepare)
    play = actions.add_parser("replay")
    source = play.add_mutually_exclusive_group(required=True)
    source.add_argument("--audio", type=Path)
    source.add_argument("--pcm", type=Path)
    play.add_argument("--metadata", type=Path, help="Decode JSON accompanying --pcm")
    play.add_argument("--output", type=Path, required=True)
    play.add_argument("--revision", choices=REVISIONS, default="stroke")
    play.add_argument("--origin", type=int, default=0)
    play.add_argument("--frames", type=int)
    play.add_argument("--center", type=float, default=96)
    play.add_argument("--seconds", type=float, nargs="+", default=(64, 32, 16, 8, 4, 2))
    play.add_argument("--gain", type=float, default=1)
    play.add_argument("--intensity", type=float, default=1)
    play.add_argument("--all-sizes", action="store_true")
    play.add_argument("--details-only", action="store_true")
    play.add_argument("--facts-only", action="store_true")
    play.set_defaults(do=replay)
    redraw = actions.add_parser("render")
    redraw.add_argument("--output", type=Path, required=True)
    redraw.add_argument("--revision", choices=REVISIONS, default="stroke")
    redraw.set_defaults(do=lambda a: render(a.dekzer, a.work, a.output, a.revision))
    diff = actions.add_parser("compare")
    diff.add_argument("--reference", type=Path, required=True)
    diff.add_argument("--output", type=Path, required=True)
    diff.add_argument("--exe", type=Path, required=True)
    diff.add_argument("--audio", type=Path)
    diff.add_argument("--pcm", type=Path)
    diff.add_argument("--baseline", type=Path)
    diff.set_defaults(do=compare)
    frozen = actions.add_parser("checkpoint-gpu")
    frozen.add_argument("--checkpoint", type=Path, required=True)
    frozen.add_argument("--output", type=Path, required=True)
    frozen.add_argument("--exe", type=Path, required=True)
    frozen.set_defaults(do=checkpoint_gpu)
    args = parser.parse_args()
    for key, value in vars(args).items():
        if isinstance(value, Path):
            setattr(args, key, value.resolve())
    if args.action == "replay" and args.pcm and not args.metadata:
        parser.error("--pcm requires --metadata")
    args.do(args)


if __name__ == "__main__":
    main()

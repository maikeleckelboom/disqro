"""Replay the accepted Windows GPU views and verify both native output bytestrings."""
import argparse
from hashlib import file_digest
import json
import os
from pathlib import Path
import subprocess
import sys

from private_desktop import run

MANIFEST = Path(__file__).with_name("waveform_parity") / "native_cases.json"


def digest(path):
    with path.open("rb") as data:
        return file_digest(data, "sha256").hexdigest()


def verify(output, recordings):
    failures = []
    for recording, cases in recordings.items():
        for case in cases:
            for extension, expected in [(".png", case["imageSha256"]), (".json", case["geometrySha256"])]:
                name = Path(recording) / (case["id"] + extension)
                file = output / name
                if not file.is_file() or digest(file) != expected:
                    failures.append(name.as_posix())
    return failures


def replay(exe, fixtures, output):
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    recordings = manifest["recordings"]
    count = sum(map(len, recordings.values()))
    if count != 103 or any(len({c["id"] for c in cases}) != len(cases) for cases in recordings.values()):
        raise ValueError("Expected 103 distinct native views across the frozen fixture groups")
    profile = output / "profile"
    profile.mkdir()
    for recording, cases in recordings.items():
        target = output / recording
        target.mkdir()
        views = [{key: value for key, value in case.items() if key not in ("imageSha256", "geometrySha256")}
                 for case in cases]
        inputs = fixtures / recording / "facts"
        for view in views:
            if not (inputs / view["pages"]).is_file():
                raise FileNotFoundError(inputs / view["pages"])
        case_file = target / "cases.json"
        case_file.write_text(json.dumps({"cases": views}), encoding="utf-8")
        error_file = target / "renderer-error.txt"
        subprocess.run([exe, "--render-waveform-cases", error_file, case_file, inputs, target],
                       env=dict(os.environ, DISQRO_PROFILE_DIR=str(profile)), check=True, timeout=90)
    failures = verify(output, recordings)
    report = {"baseline": manifest["baseline"], "exeSha256": digest(exe), "cases": count,
              "filesChecked": count * 2, "differences": failures}
    (output / "result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    if failures:
        raise RuntimeError(f"{len(failures)} native outputs changed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("--fixtures", required=True, type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("--output", required=True, type=lambda p: Path(p).resolve())
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            replay(args.exe, args.fixtures, args.output)
        except Exception:
            import traceback
            (args.output / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--worker"], 600)
        print((args.output / "result.json").read_text(encoding="utf-8"))

"""Check tracked first-party Rust module paths and Python tool names."""
from pathlib import PurePosixPath
import re
import subprocess


def main():
    tracked = subprocess.check_output(["git", "ls-files", "-z"]).decode("utf-8")
    snake_case = re.compile(r"[a-z][a-z0-9]*(?:_[a-z0-9]+)*")
    invalid = []
    for filename in tracked.split("\0"):
        path = PurePosixPath(filename)
        if path.is_relative_to("vendor"):
            continue
        if path.suffix not in {".rs", ".py"}:
            continue
        parts = (*path.parts[:-1], path.stem)
        if any(not snake_case.fullmatch(part) for part in parts):
            invalid.append(filename)
    if invalid:
        raise SystemExit("Use snake_case for these source paths\n" + "\n".join(invalid))


if __name__ == "__main__":
    main()

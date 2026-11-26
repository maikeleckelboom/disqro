"""Exercise large indexed destinations and a paged folder during silent CUE playback."""
import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time
import traceback

from capture_interface import layout
from private_desktop import run
from qualify_music_browser import close, launch, wait_for
from qualify_spectrum import click_control

CARDINALITY = 40960


def detached_catalog(profile):
    # Model an indexed drive that is absent during startup. The real audio
    # played in this journey comes from the separate, online CUE fixture.
    music = profile / "Detached collection"
    with sqlite3.connect(profile / "library.sqlite3") as db:
        root = db.execute("INSERT INTO roots(path,path_key,name,added_at) VALUES (?,?,?,1)",
                          (str(music), str(music).lower(), music.name)).lastrowid
        db.execute("""
            WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i < ?)
            INSERT INTO files(id,path,path_key,name,kind,root_id,size,modified)
            SELECT i+100000, ?||printf('/scale-%06d.wav',i), ?||printf('/scale-%06d.wav',i),
                   printf('scale-%06d.wav',i),0,?,400,1 FROM n
            """, (CARDINALITY, str(music), str(music).lower(), root))
        db.execute("""
            INSERT INTO artists(id,key,name)
            SELECT id,printf('aa artist %06d',id-100000),printf('AA Artist %06d',id-100000)
            FROM files WHERE root_id=?""", (root,))
        db.execute("""
            INSERT INTO albums(id,key,root_id,title,folder,sort_key,is_release)
            SELECT id,path,root_id,printf('AA Release %06d',id-100000),?,path,1
            FROM files WHERE root_id=?""", (str(music), root))
        db.execute("""
            INSERT INTO tracks(anchor,part,source,root_id,album_id,title,artist,album,format,
                               title_key,artist_key,album_key,search,added_at)
            SELECT f.id,0,f.id,f.root_id,a.id,f.name,p.name,a.title,'WAV',f.name,p.key,
                   lower(a.title),f.path,1
            FROM files f JOIN artists p ON p.id=f.id JOIN albums a ON a.id=f.id
            WHERE f.root_id=?""", (root,))


def journey(exe, test_exe, output):
    subprocess.run([test_exe, "--ignored", "--exact", "browser::tree::tests::seed_music_browser_qualification"],
                   env=dict(os.environ, DISQRO_BROWSER_FIXTURE=str(output)), check=True)
    detached_catalog(output)
    folders = output / "Tree fixture"
    folders.mkdir()
    for index in range(4400):
        (folders / f"Folder {index:05}").mkdir()
    root = int((output / "reference-root.txt").read_text())
    settings = layout(root, 260, 980, mode="tracks", location={"Library": root})
    settings["places"] = {"items": [{"path": str(folders), "origin": "Custom", "label": "Tree fixture"}]}
    elapsed = {}
    process = launch(exe, output, settings)
    try:
        hwnd, chapter = wait_for(process, "Chapter 01")
        click_control(hwnd, chapter)
        click_control(hwnd, chapter)
        wait_for(process, "Pause")
        for action, content in [("Artists", "AA Artist 000001"), ("Releases", "AA Release 000001"),
                                ("Open Tree fixture", "Tree"), ("Tree", "Folder 00001")]:
            start = time.monotonic()
            click_control(hwnd, wait_for(process, action)[1])
            wait_for(process, content, timeout=20)
            wait_for(process, "Pause")
            elapsed[action] = round(time.monotonic() - start, 3)
    finally:
        close(process)
    queue = json.loads((output / "workspace-v1.json").read_text(encoding="utf-8"))["queue"]
    current = queue["queue"]["current"]
    assert "Chapter" in queue["queue"]["entries"][current]["track"]["title"]
    assert current > 0 or queue["position"] > 0
    report = {"cardinality": CARDINALITY, "treeFolders": 4400, "secondsIncludingUIAutomation": elapsed,
              "queuePosition": queue["position"], "current": current}
    (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("test_exe", type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument("output", type=lambda p: Path(p).resolve())
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            journey(args.exe, args.test_exe, args.output)
        except Exception:
            (args.output / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            raise
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        run([sys.executable, "-B", str(Path(__file__).resolve()), *sys.argv[1:], "--worker"], 160)
        print((args.output / "report.json").read_text(encoding="utf-8"))

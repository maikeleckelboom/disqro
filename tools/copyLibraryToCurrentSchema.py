"""Copy a schema 5 profile into a new schema 6 profile. The source is read-only."""

import argparse
from contextlib import closing
from pathlib import Path
import shutil
import sqlite3
import tempfile


SCHEMA = Path(__file__).resolve().parents[1] / "src" / "store"
TABLES = (
    "roots", "files", "albums", "tracks", "likes", "ratings", "playlists",
    "playlist_entries", "listening_occurrences", "listening_minutes",
    "listening_release_evidence",
)
PROJECTIONS = ("listening_totals", "listening_groups")


def copy_table(db, table):
    source = {row[1] for row in db.execute(f"PRAGMA source.table_info({table})")}
    columns = ", ".join(
        row[1] for row in db.execute(f"PRAGMA main.table_info({table})")
        if row[1] in source
    )
    db.execute(f"INSERT INTO main.{table} ({columns}) SELECT {columns} FROM source.{table}")
    counts = db.execute(
        f"SELECT (SELECT count(*) FROM main.{table}), (SELECT count(*) FROM source.{table})"
    ).fetchone()
    if counts[0] != counts[1]:
        raise ValueError(f"Row counts differ for {table}")


def copy_library(source, destination):
    with closing(sqlite3.connect(destination, uri=True)) as db:
        db.execute("PRAGMA foreign_keys = ON")
        db.execute("ATTACH DATABASE ? AS source", (source.as_uri() + "?mode=ro",))
        if db.execute("PRAGMA source.user_version").fetchone()[0] != 5:
            raise ValueError("The source library must use schema 5")
        ddl = ("library.sql", "catalog-search.sql", "listening-history.sql")
        db.executescript("BEGIN;\n" + "\n".join(
            (SCHEMA / file).read_text(encoding="utf-8") for file in ddl
        ))
        for table in TABLES:
            copy_table(db, table)
        for table in PROJECTIONS:
            db.execute(f"DELETE FROM main.{table}")
            copy_table(db, table)
        # Preserve allocation even when the last identity was deleted.
        db.execute("DELETE FROM main.sqlite_sequence")
        db.execute("INSERT INTO main.sqlite_sequence SELECT * FROM source.sqlite_sequence")
        # Re-read tags and CUE metadata on the first scan of the new copy.
        db.execute("UPDATE files SET tagged = NULL")
        performers = db.execute(
            "SELECT DISTINCT artist, artist_key FROM tracks WHERE artist <> ''"
        ).fetchall()
        db.executemany(
            "UPDATE tracks SET artist_key = ? WHERE artist = ? AND artist_key = ?",
            ((name.strip().lower(), name, key) for name, key in performers
             if name.strip().lower() != key),
        )
        db.executemany(
            "INSERT INTO artists (key, name) VALUES (?, ?) ON CONFLICT(key) DO NOTHING",
            ((name.strip().lower(), name.strip()) for name, _ in sorted(performers) if name.strip()),
        )
        db.execute("PRAGMA user_version = 6")
        if db.execute("PRAGMA foreign_key_check").fetchone():
            raise ValueError("A foreign key check failed in the converted library")
        if db.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
            raise ValueError("An integrity check failed in the converted library")
        db.commit()


def copy_profile(source, destination):
    destination = destination.parent.resolve(strict=True) / destination.name
    if destination.exists() or destination.is_symlink() or destination.is_relative_to(source):
        raise ValueError("Choose a new destination outside the source profile")
    library = source / "library.sqlite3"
    if not library.is_file():
        raise ValueError("The source profile has no library.sqlite3")
    with tempfile.TemporaryDirectory(prefix="disqro-library-copy-", dir=destination.parent) as temp:
        staging = Path(temp) / "profile"
        staging.mkdir()
        copy_library(library, staging / library.name)
        workspace = source / "workspace-v1.json"
        if workspace.is_file():
            shutil.copyfile(workspace, staging / workspace.name)
        staging.rename(destination)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    try:
        copy_profile(args.source.resolve(strict=True), args.destination.absolute())
    except (OSError, ValueError, sqlite3.Error) as error:
        parser.exit(1, f"{error}\n")

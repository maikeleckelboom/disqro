-- Release a path when its recording is replaced. The historical file keeps
-- its display path and all references. CUE slices have immutable coordinates.
CREATE TEMP TABLE library_sequences AS
    SELECT name, seq FROM sqlite_sequence WHERE name IN ('files', 'tracks');
CREATE TABLE new_files(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    path_key TEXT UNIQUE,
    name TEXT NOT NULL,
    kind INTEGER NOT NULL,
    root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    volume INTEGER,
    file_index INTEGER,
    digest INTEGER,
    tagged INTEGER,
    present INTEGER NOT NULL DEFAULT 1,
    seen INTEGER NOT NULL DEFAULT 0,
    sample INTEGER
);

INSERT INTO new_files SELECT *, NULL FROM files;
DROP TABLE files;
ALTER TABLE new_files RENAME TO files;
CREATE INDEX files_root ON files(root_id, present);
CREATE INDEX files_missing ON files(kind, size) WHERE present = 0;

CREATE TABLE new_tracks(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    anchor INTEGER NOT NULL REFERENCES files(id),
    part INTEGER NOT NULL,
    source INTEGER NOT NULL REFERENCES files(id),
    first_tick INTEGER,
    last_tick INTEGER,
    root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
    album_id INTEGER REFERENCES albums(id) ON DELETE SET NULL,
    ordinal INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT NOT NULL,
    number INTEGER,
    disc_number INTEGER,
    year INTEGER,
    duration REAL,
    format TEXT NOT NULL,
    title_key TEXT NOT NULL,
    artist_key TEXT NOT NULL,
    album_key TEXT NOT NULL,
    search TEXT NOT NULL,
    present INTEGER NOT NULL DEFAULT 1,
    seen INTEGER NOT NULL DEFAULT 0,
    added_at INTEGER NOT NULL
);

INSERT INTO new_tracks SELECT * FROM tracks;
DROP TABLE tracks;
ALTER TABLE new_tracks RENAME TO tracks;
CREATE UNIQUE INDEX tracks_slice ON tracks(anchor, part, source, ifnull(first_tick,-1), ifnull(last_tick,-1));
CREATE INDEX tracks_root ON tracks(root_id, present);
CREATE INDEX tracks_album ON tracks(album_id, ordinal);
CREATE INDEX tracks_source ON tracks(source);
CREATE INDEX tracks_title ON tracks(title_key);
CREATE INDEX tracks_artist ON tracks(artist_key);
UPDATE sqlite_sequence SET seq = max(seq, coalesce(
    (SELECT seq FROM library_sequences WHERE name = sqlite_sequence.name), 0))
WHERE name IN ('files', 'tracks');
DROP TABLE library_sequences;

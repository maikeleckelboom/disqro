-- Registered music collections. Removing one never touches files on disk.
CREATE TABLE roots(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    path_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    added_at INTEGER NOT NULL,
    scanned_at INTEGER
);

-- Physical files: audio sources (kind 0) and CUE sheets (kind 1). A row
-- keeps its last known location while the file is missing. `seen` is the
-- root generation that last found it; `tagged` is the modification time
-- whose tags the tracks hold. Identity evidence (volume, file_index, digest)
-- supports verified relocation.
CREATE TABLE files(
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
CREATE INDEX files_root ON files(root_id, present);
CREATE INDEX files_missing ON files(kind, size) WHERE present = 0;


-- An authored, normalized credit has one stable catalog identity. Keep the
-- identity even when a rescan temporarily removes every matching recording.
CREATE TABLE artists(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE CHECK (key <> ''),
    name TEXT NOT NULL
);

-- Discs and albums: an authored CUE program, or tagged files sharing a
-- folder, album and disc number. Rebuilt by scans.
CREATE TABLE albums(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE,
    root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    album_artist_id INTEGER REFERENCES artists(id),
    is_release INTEGER NOT NULL DEFAULT 0,
    year INTEGER,
    folder TEXT NOT NULL,
    cover TEXT,
    sort_key TEXT NOT NULL
);
CREATE INDEX albums_root ON albums(root_id);
CREATE INDEX albums_tree_folder ON albums(root_id, substr(sort_key, 1, instr(sort_key, char(1)) - 1));

CREATE INDEX albums_artist ON albums(album_artist_id) WHERE album_artist_id IS NOT NULL;
CREATE INDEX albums_releases ON albums(root_id) WHERE is_release = 1;

-- The authored grouping of indexed discs. Computed once per root scan by
-- the same release rules as physical browsing.
CREATE TABLE release_discs(
    album_id INTEGER PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
    first_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    number INTEGER NOT NULL,
    subtitle TEXT NOT NULL
);
CREATE INDEX release_discs_order ON release_discs(first_id, number);

-- Logical tracks. Identity is (anchor, part): the audio file and part 0 for
-- a whole file; the CUE sheet and the chapter number, or its negation for
-- the recorded lead-in before that chapter. Metadata columns are refreshed
-- by every scan; the row itself outlives its file.
CREATE TABLE tracks(
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
    album_artist_id INTEGER REFERENCES artists(id),
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
CREATE UNIQUE INDEX tracks_slice ON tracks(anchor, part, source, ifnull(first_tick,-1), ifnull(last_tick,-1));
CREATE INDEX tracks_root ON tracks(root_id, present);
CREATE INDEX tracks_album ON tracks(album_id, ordinal);
CREATE INDEX tracks_source ON tracks(source);
CREATE INDEX tracks_title ON tracks(title_key);
CREATE INDEX tracks_artist ON tracks(artist_key);
CREATE INDEX tracks_performer ON tracks(artist_key, album_id) WHERE present = 1 AND root_id IS NOT NULL AND artist_key <> '';

-- User-owned data. RESTRICT keeps every referenced track alive through
-- rescans, collection removal and cleanup.
CREATE TABLE likes(
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE RESTRICT,
    liked_at INTEGER NOT NULL
);
CREATE TABLE ratings(
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE RESTRICT,
    stars INTEGER NOT NULL CHECK (stars BETWEEN 1 AND 5),
    rated_at INTEGER NOT NULL
);
CREATE TABLE playlists(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    modified_at INTEGER NOT NULL
);
CREATE TABLE playlist_entries(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL,
    added_at INTEGER NOT NULL
);
CREATE INDEX playlist_order ON playlist_entries(playlist_id, position);
CREATE INDEX playlist_tracks ON playlist_entries(track_id);

-- Personal activity survives catalog detachment and source relocation. The
-- snapshot describes the recording as it was at the time of playback.
CREATE TABLE listening_occurrences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token TEXT NOT NULL UNIQUE,
    track_id INTEGER REFERENCES tracks(id) ON DELETE RESTRICT,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    artist_key TEXT,
    album TEXT NOT NULL,
    release_key TEXT,
    search TEXT NOT NULL DEFAULT '',
    snapshot TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    activity_at INTEGER NOT NULL,
    ended_at INTEGER,
    reason TEXT,
    credited_us INTEGER NOT NULL CHECK (credited_us > 0),
    qualified_at INTEGER,
    checkpoint INTEGER NOT NULL
);
CREATE INDEX listening_history_order ON listening_occurrences(started_at, id);
CREATE INDEX listening_history_track ON listening_occurrences(track_id, qualified_at);
CREATE INDEX listening_history_qualified ON listening_occurrences(qualified_at)
    WHERE qualified_at IS NOT NULL;
CREATE INDEX listening_history_open ON listening_occurrences(id) WHERE ended_at IS NULL;

-- UTC minute bins are sparse and keep period queries independent of an
-- occurrence's start time. The timestamp denotes the start of the bin.
CREATE TABLE listening_minutes (
    occurrence_id INTEGER NOT NULL REFERENCES listening_occurrences(id) ON DELETE CASCADE,
    minute INTEGER NOT NULL,
    credited_us INTEGER NOT NULL CHECK (credited_us > 0),
    PRIMARY KEY (occurrence_id, minute)
) WITHOUT ROWID;
CREATE INDEX listening_minutes_period ON listening_minutes(minute, credited_us);

-- Transactional projection for music tables and smart-library predicates.
CREATE TABLE listening_totals (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    credited_us INTEGER NOT NULL,
    plays INTEGER NOT NULL,
    last_played INTEGER
);
CREATE INDEX recently_played ON listening_totals(last_played DESC, track_id)
    WHERE last_played IS NOT NULL;

-- All-time rankings use one row per artist/release spelling. Dated rankings
-- continue to use each occurrence's metadata and activity bins.
CREATE TABLE listening_groups (
    kind INTEGER NOT NULL,
    key TEXT NOT NULL,
    label TEXT NOT NULL,
    plays INTEGER NOT NULL,
    credited_us INTEGER NOT NULL,
    PRIMARY KEY(kind, key, label)
) WITHOUT ROWID;

CREATE TRIGGER listening_group_insert AFTER INSERT ON listening_occurrences
BEGIN
    INSERT INTO listening_groups
    SELECT kind,key,label,new.qualified_at IS NOT NULL,new.credited_us FROM (
        SELECT 0 AS kind, new.artist_key AS key, new.artist AS label
        UNION ALL SELECT 1, new.release_key, new.album
    ) WHERE key IS NOT NULL
    ON CONFLICT DO UPDATE SET plays = plays + excluded.plays,
        credited_us = credited_us + excluded.credited_us;
END;

CREATE TRIGGER listening_group_update AFTER UPDATE OF credited_us, qualified_at ON listening_occurrences
BEGIN
    UPDATE listening_groups SET credited_us = credited_us + new.credited_us - old.credited_us,
        plays = plays + (new.qualified_at IS NOT NULL) - (old.qualified_at IS NOT NULL)
    WHERE (kind=0 AND key=new.artist_key AND label=new.artist)
       OR (kind=1 AND key=new.release_key AND label=new.album);
END;

CREATE TRIGGER listening_group_delete AFTER DELETE ON listening_occurrences
BEGIN
    UPDATE listening_groups SET credited_us = credited_us - old.credited_us,
        plays = plays - (old.qualified_at IS NOT NULL)
    WHERE (kind=0 AND key=old.artist_key AND label=old.artist)
       OR (kind=1 AND key=old.release_key AND label=old.album);
    DELETE FROM listening_groups
    WHERE credited_us = 0 AND ((kind=0 AND key=old.artist_key AND label=old.artist)
                           OR (kind=1 AND key=old.release_key AND label=old.album));
END;

CREATE TRIGGER listening_total_insert AFTER INSERT ON listening_occurrences
WHEN new.track_id IS NOT NULL
BEGIN
    INSERT INTO listening_totals VALUES
        (new.track_id, new.credited_us, new.qualified_at IS NOT NULL, new.qualified_at)
    ON CONFLICT(track_id) DO UPDATE SET
        credited_us = credited_us + excluded.credited_us,
        plays = plays + excluded.plays,
        last_played = coalesce(max(last_played, excluded.last_played), last_played, excluded.last_played);
END;

CREATE TRIGGER listening_total_update AFTER UPDATE OF credited_us, qualified_at ON listening_occurrences
WHEN new.track_id IS NOT NULL
BEGIN
    UPDATE listening_totals SET
        credited_us = credited_us + new.credited_us - old.credited_us,
        plays = plays + (new.qualified_at IS NOT NULL) - (old.qualified_at IS NOT NULL),
        last_played = coalesce(max(last_played, new.qualified_at), last_played, new.qualified_at)
    WHERE track_id = new.track_id;
END;

CREATE TRIGGER listening_total_delete AFTER DELETE ON listening_occurrences
WHEN old.track_id IS NOT NULL
BEGIN
    UPDATE listening_totals SET
        credited_us = credited_us - old.credited_us,
        plays = plays - (old.qualified_at IS NOT NULL),
        last_played = CASE WHEN last_played = old.qualified_at THEN (
            SELECT max(qualified_at) FROM listening_occurrences WHERE track_id = old.track_id
        ) ELSE last_played END
    WHERE track_id = old.track_id;
    DELETE FROM listening_totals WHERE track_id = old.track_id AND credited_us = 0;
END;

-- Established by the same authored-disc rules as collection navigation.
-- The release ID is the original first disc's ID, even if its album row is
-- replaced by a later tag edit or folder move.
CREATE TABLE listening_release_evidence (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    release_id INTEGER NOT NULL,
    title TEXT NOT NULL
);

CREATE INDEX files_detached_move ON files(kind, size)
WHERE root_id IS NULL AND present = 1;

CREATE INDEX files_live_native_identity ON files(volume, file_index)
WHERE present = 1 AND volume IS NOT NULL AND file_index IS NOT NULL;

CREATE TRIGGER listening_release_rekey AFTER UPDATE OF release_key ON listening_occurrences
WHEN old.release_key IS NOT new.release_key
BEGIN
    UPDATE listening_groups SET credited_us = credited_us - old.credited_us,
        plays = plays - (old.qualified_at IS NOT NULL)
    WHERE kind=1 AND key=old.release_key AND label=old.album;
    DELETE FROM listening_groups
    WHERE kind=1 AND key=old.release_key AND label=old.album AND credited_us=0;
    INSERT INTO listening_groups
    SELECT 1,new.release_key,new.album,new.qualified_at IS NOT NULL,new.credited_us
    WHERE new.release_key IS NOT NULL
    ON CONFLICT DO UPDATE SET plays = plays + excluded.plays,
        credited_us = credited_us + excluded.credited_us;
END;

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

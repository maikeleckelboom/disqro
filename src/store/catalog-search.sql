-- External-content indexes store tokens only. The catalog owns every identity and label.

CREATE VIRTUAL TABLE tracks_fts USING fts5(title, artist, album, content='tracks', content_rowid='id');
INSERT INTO tracks_fts(tracks_fts) VALUES('rebuild');
CREATE TRIGGER tracks_fts_insert AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album) VALUES(new.id, new.title, new.artist, new.album);
END;
CREATE TRIGGER tracks_fts_delete AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album) VALUES('delete', old.id, old.title, old.artist, old.album);
END;
CREATE TRIGGER tracks_fts_update AFTER UPDATE OF title, artist, album ON tracks
WHEN old.title IS NOT new.title OR old.artist IS NOT new.artist OR old.album IS NOT new.album BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album) VALUES('delete', old.id, old.title, old.artist, old.album);
    INSERT INTO tracks_fts(rowid, title, artist, album) VALUES(new.id, new.title, new.artist, new.album);
END;

CREATE VIRTUAL TABLE albums_fts USING fts5(title, folder, content='albums', content_rowid='id');
INSERT INTO albums_fts(albums_fts) VALUES('rebuild');
CREATE TRIGGER albums_fts_insert AFTER INSERT ON albums BEGIN
    INSERT INTO albums_fts(rowid, title, folder) VALUES(new.id, new.title, new.folder);
END;
CREATE TRIGGER albums_fts_delete AFTER DELETE ON albums BEGIN
    INSERT INTO albums_fts(albums_fts, rowid, title, folder) VALUES('delete', old.id, old.title, old.folder);
END;
CREATE TRIGGER albums_fts_update AFTER UPDATE OF title, folder ON albums
WHEN old.title IS NOT new.title OR old.folder IS NOT new.folder BEGIN
    INSERT INTO albums_fts(albums_fts, rowid, title, folder) VALUES('delete', old.id, old.title, old.folder);
    INSERT INTO albums_fts(rowid, title, folder) VALUES(new.id, new.title, new.folder);
END;

CREATE VIRTUAL TABLE playlists_fts USING fts5(name, content='playlists', content_rowid='id');
INSERT INTO playlists_fts(playlists_fts) VALUES('rebuild');
CREATE TRIGGER playlists_fts_insert AFTER INSERT ON playlists BEGIN
    INSERT INTO playlists_fts(rowid, name) VALUES(new.id, new.name);
END;
CREATE TRIGGER playlists_fts_delete AFTER DELETE ON playlists BEGIN
    INSERT INTO playlists_fts(playlists_fts, rowid, name) VALUES('delete', old.id, old.name);
END;
CREATE TRIGGER playlists_fts_update AFTER UPDATE OF name ON playlists
WHEN old.name IS NOT new.name BEGIN
    INSERT INTO playlists_fts(playlists_fts, rowid, name) VALUES('delete', old.id, old.name);
    INSERT INTO playlists_fts(rowid, name) VALUES(new.id, new.name);
END;

CREATE VIRTUAL TABLE roots_fts USING fts5(name, content='roots', content_rowid='id');
INSERT INTO roots_fts(roots_fts) VALUES('rebuild');
CREATE TRIGGER roots_fts_insert AFTER INSERT ON roots BEGIN
    INSERT INTO roots_fts(rowid, name) VALUES(new.id, new.name);
END;
CREATE TRIGGER roots_fts_delete AFTER DELETE ON roots BEGIN
    INSERT INTO roots_fts(roots_fts, rowid, name) VALUES('delete', old.id, old.name);
END;
CREATE TRIGGER roots_fts_update AFTER UPDATE OF name ON roots
WHEN old.name IS NOT new.name BEGIN
    INSERT INTO roots_fts(roots_fts, rowid, name) VALUES('delete', old.id, old.name);
    INSERT INTO roots_fts(rowid, name) VALUES(new.id, new.name);
END;

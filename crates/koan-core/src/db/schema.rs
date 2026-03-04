use rusqlite::Connection;

/// Create all tables. Idempotent — safe to call on every startup.
pub fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS artists (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL,
            sort_name   TEXT,
            mbid        TEXT,
            remote_id   TEXT,
            UNIQUE(name)
        );

        CREATE TABLE IF NOT EXISTS albums (
            id           INTEGER PRIMARY KEY,
            title        TEXT NOT NULL,
            artist_id    INTEGER REFERENCES artists(id),
            date         TEXT,
            total_discs  INTEGER,
            total_tracks INTEGER,
            codec        TEXT,
            label        TEXT,
            remote_id    TEXT,
            UNIQUE(title, artist_id)
        );

        CREATE TABLE IF NOT EXISTS tracks (
            id            INTEGER PRIMARY KEY,
            album_id      INTEGER REFERENCES albums(id),
            artist_id     INTEGER REFERENCES artists(id),
            disc          INTEGER,
            track_number  INTEGER,
            title         TEXT NOT NULL,
            duration_ms   INTEGER,
            path          TEXT,
            codec         TEXT,
            sample_rate   INTEGER,
            bit_depth     INTEGER,
            channels      INTEGER,
            bitrate       INTEGER,
            size_bytes    INTEGER,
            mtime         INTEGER,
            genre         TEXT,
            source        TEXT NOT NULL DEFAULT 'local',
            remote_id     TEXT,
            remote_url    TEXT,
            cached_path   TEXT,
            UNIQUE(path)
        );

        CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album_id);
        CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist_id);
        CREATE INDEX IF NOT EXISTS idx_tracks_source ON tracks(source);
        CREATE INDEX IF NOT EXISTS idx_tracks_remote_id ON tracks(remote_id);
        CREATE INDEX IF NOT EXISTS idx_albums_artist ON albums(artist_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
            title,
            artist_name,
            album_title,
            genre
        );

        CREATE TABLE IF NOT EXISTS library_folders (
            id        INTEGER PRIMARY KEY,
            path      TEXT NOT NULL UNIQUE,
            last_scan INTEGER
        );

        CREATE TABLE IF NOT EXISTS scan_cache (
            path      TEXT PRIMARY KEY,
            mtime     INTEGER NOT NULL,
            size      INTEGER NOT NULL,
            track_id  INTEGER REFERENCES tracks(id)
        );

        CREATE TABLE IF NOT EXISTS remote_servers (
            id        INTEGER PRIMARY KEY,
            url       TEXT NOT NULL UNIQUE,
            username  TEXT NOT NULL,
            last_sync INTEGER
        );

        CREATE TABLE IF NOT EXISTS organize_log (
            id         INTEGER PRIMARY KEY,
            batch_id   TEXT NOT NULL,
            track_id   INTEGER,
            from_path  TEXT NOT NULL,
            to_path    TEXT NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS favourites (
            track_path  TEXT PRIMARY KEY,
            created_at  TEXT DEFAULT (datetime('now'))
        );
        ",
    )?;
    Ok(())
}

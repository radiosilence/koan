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
            source        TEXT NOT NULL DEFAULT 'local' CHECK (source IN ('local', 'remote', 'cached')),
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
        CREATE INDEX IF NOT EXISTS idx_tracks_album_order ON tracks(album_id, disc, track_number);

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

        CREATE TABLE IF NOT EXISTS lyrics_cache (
            id          INTEGER PRIMARY KEY,
            track_id    INTEGER REFERENCES tracks(id),
            source      TEXT NOT NULL,
            synced      INTEGER DEFAULT 0,
            content     TEXT NOT NULL,
            fetched_at  INTEGER NOT NULL,
            UNIQUE(track_id)
        );

        CREATE TABLE IF NOT EXISTS favourites (
            track_path  TEXT PRIMARY KEY,
            created_at  TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS playback_state (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            queue_json  TEXT NOT NULL DEFAULT '[]',
            cursor_id   TEXT,
            position_ms INTEGER NOT NULL DEFAULT 0,
            updated_at  TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS similar_artists (
            artist_id       INTEGER NOT NULL REFERENCES artists(id),
            similar_id      INTEGER NOT NULL REFERENCES artists(id),
            score           REAL NOT NULL DEFAULT 0.0,
            source          TEXT NOT NULL DEFAULT 'subsonic',
            relationship    TEXT NOT NULL DEFAULT 'similar',
            updated_at      TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (artist_id, similar_id, source)
        );

        CREATE TABLE IF NOT EXISTS play_history (
            id          INTEGER PRIMARY KEY,
            track_id    INTEGER REFERENCES tracks(id),
            played_at   INTEGER NOT NULL,
            duration_ms INTEGER,
            source      TEXT DEFAULT 'local'
        );

        CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id);
        CREATE INDEX IF NOT EXISTS idx_play_history_time ON play_history(played_at);

        CREATE TABLE IF NOT EXISTS queue_snapshots (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            queue_json  TEXT NOT NULL DEFAULT '[]',
            cursor_path TEXT,
            position_ms INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS track_vectors (
            track_id    INTEGER PRIMARY KEY REFERENCES tracks(id),
            embedding   BLOB NOT NULL,
            updated_at  TEXT DEFAULT (datetime('now'))
        );

        -- Auth tables
        CREATE TABLE IF NOT EXISTS users (
            id            INTEGER PRIMARY KEY,
            username      TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user', 'readonly')),
            created_at    TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS refresh_tokens (
            id          TEXT PRIMARY KEY,
            user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            expires_at  INTEGER NOT NULL,
            revoked     INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);
        CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires ON refresh_tokens(expires_at);
        ",
    )?;
    // --- Migrations: add columns that didn't exist in earlier versions ---
    // SQLite has no ADD COLUMN IF NOT EXISTS, so we catch the "duplicate column" error.
    let migrations = [
        "ALTER TABLE tracks ADD COLUMN cache_size_bytes INTEGER",
        "ALTER TABLE tracks ADD COLUMN cache_download_date INTEGER",
        "ALTER TABLE similar_artists ADD COLUMN relationship TEXT NOT NULL DEFAULT 'similar'",
    ];
    for sql in &migrations {
        match conn.execute(sql, []) {
            Ok(_) => {}
            Err(rusqlite::Error::ExecuteReturnedResults) => {}
            Err(e) if e.to_string().contains("duplicate column") => {}
            Err(e) => return Err(e),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_similar_artists_relationship_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE TABLE similar_artists (
                 artist_id  INTEGER NOT NULL REFERENCES artists(id),
                 similar_id INTEGER NOT NULL REFERENCES artists(id),
                 score      REAL NOT NULL DEFAULT 0.0,
                 source     TEXT NOT NULL DEFAULT 'subsonic',
                 updated_at TEXT DEFAULT (datetime('now')),
                 PRIMARY KEY (artist_id, similar_id, source)
             );",
        )
        .unwrap();

        create_tables(&conn).unwrap();

        let has_relationship: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('similar_artists') WHERE name = 'relationship'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(has_relationship, "relationship column was not added");

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'A'), (2, 'B')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO similar_artists (artist_id, similar_id, score, source)
             VALUES (1, 2, 0.9, 'subsonic')",
            [],
        )
        .unwrap();
        let rel: String = conn
            .query_row(
                "SELECT relationship FROM similar_artists WHERE artist_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rel, "similar");
    }
}

use rusqlite::Connection;

/// Create all tables. Idempotent — safe to call on every startup.
pub fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    // Before any DDL: the ORDER BY clauses that use it are everywhere, and a
    // connection without it fails them rather than sorting differently.
    super::connection::register_library_collation(conn)?;

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
            added_at     TEXT,
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
            size_bytes INTEGER,
            mtime      INTEGER,
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
        // Undo checks these against the file before moving it back, so a file that
        // was replaced since the organize is left alone.
        "ALTER TABLE organize_log ADD COLUMN size_bytes INTEGER",
        "ALTER TABLE organize_log ADD COLUMN mtime INTEGER",
        // When the album entered the library, so clients can offer a
        // recently-added ordering. Remote sync supplies the server's own
        // `created`; a local scan the earliest mtime among the album's files.
        "ALTER TABLE albums ADD COLUMN added_at TEXT",
        // Locally-scanned albums were briefly stamped with the time the scan
        // ran, which pinned every one of them to the top of recently-added and
        // buried what the server actually considered new. Clearing the
        // scan-time values lets the next scan refill them from the files
        // themselves; the server's own ISO 8601 dates are left alone.
        // Whether playback was running when the session was saved, so
        // reopening can pick up where it left off rather than always in a
        // paused state.
        "ALTER TABLE playback_state ADD COLUMN was_playing INTEGER NOT NULL DEFAULT 0",
        // Radio is a mode you leave on, not a per-session choice: having it
        // switch itself off every launch is a setting that will not stay set.
        "ALTER TABLE playback_state ADD COLUMN radio_enabled INTEGER NOT NULL DEFAULT 0",
        "UPDATE albums SET added_at = NULL
           WHERE added_at IS NOT NULL AND added_at NOT LIKE '%T%Z'",
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
    use crate::db::connection::Database;

    #[test]
    fn clears_scan_time_added_at_but_keeps_the_servers() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Klaxons');
             INSERT INTO albums (id, title, artist_id, added_at)
               VALUES (1, 'Local', 1, '2026-08-23 12:14:57'),
                      (2, 'Remote', 1, '2026-08-06T22:53:14.851697506Z'),
                      (3, 'Neither', 1, NULL);",
        )
        .unwrap();

        // Migrations live inside `create_tables` and are idempotent.
        create_tables(&conn).unwrap();

        let added = |id: i64| -> Option<String> {
            conn.query_row("SELECT added_at FROM albums WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(added(1), None, "scan-time stamp cleared");
        assert_eq!(
            added(2).as_deref(),
            Some("2026-08-06T22:53:14.851697506Z"),
            "the server's own date is left alone"
        );
        assert_eq!(added(3), None);
    }

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

    /// `create_tables` runs its ALTER TABLE migrations unconditionally and
    /// detects the already-migrated case from SQLite's "duplicate column" error
    /// text. On an existing database that is the *normal* path, taken on every
    /// open, so a change in SQLite's wording would stop koan starting.
    #[test]
    fn sqlite_still_reports_duplicate_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER, b INTEGER);")
            .unwrap();
        let err = conn
            .execute("ALTER TABLE t ADD COLUMN b INTEGER", [])
            .unwrap_err();
        assert!(
            err.to_string().contains("duplicate column"),
            "SQLite error wording moved, create_tables no longer detects \
             already-applied migrations: {err}"
        );
    }

    #[test]
    fn reopening_a_migrated_database_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        Database::open(&path).unwrap();
        Database::open(&path).unwrap();
        Database::open(&path).unwrap();
    }

    #[test]
    fn pre_migration_database_gains_the_new_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        {
            // Build the current schema, then strip the migrated columns back off
            // to reproduce a database written by an older koan.
            let db = Database::open(&path).unwrap();
            db.conn
                .execute_batch(
                    "ALTER TABLE tracks DROP COLUMN cache_size_bytes;
                     ALTER TABLE tracks DROP COLUMN cache_download_date;
                     ALTER TABLE similar_artists DROP COLUMN relationship;",
                )
                .unwrap();
        }

        let db = Database::open(&path).unwrap();
        for (table, column) in [
            ("tracks", "cache_size_bytes"),
            ("tracks", "cache_download_date"),
            ("similar_artists", "relationship"),
        ] {
            let found: i64 = db
                .conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table}.{column} was not migrated");
        }
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = Database::open(&path).unwrap();

        db.conn
            .execute(
                "INSERT INTO users (id, username, password_hash) VALUES (1, 'u', 'h')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO refresh_tokens (id, user_id, expires_at) VALUES ('t', 1, 9999)",
                [],
            )
            .unwrap();
        assert!(
            db.conn
                .execute(
                    "INSERT INTO refresh_tokens (id, user_id, expires_at) VALUES ('t2', 999, 9999)",
                    [],
                )
                .is_err(),
            "foreign key constraint did not fire"
        );

        db.conn
            .execute("DELETE FROM users WHERE id = 1", [])
            .unwrap();
        let remaining: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM refresh_tokens", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "ON DELETE CASCADE did not fire");
    }
}

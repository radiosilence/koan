use rusqlite::Connection;

/// Create all tables. Idempotent — safe to call on every startup.
/// Bumped whenever the schema changes. Stored in `PRAGMA user_version` so an
/// older build refuses a database it does not understand rather than writing to it.
pub const SCHEMA_VERSION: i64 = 2;

pub fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    // Before any DDL: the ORDER BY clauses that use it are everywhere, and a
    // connection without it fails them rather than sorting differently.
    super::connection::register_library_collation(conn)?;
    super::connection::register_shuffle_function(conn)?;
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some(format!(
                "database schema version {found} is newer than this build understands \
                 ({SCHEMA_VERSION}) — upgrade koan rather than downgrading the library"
            )),
        ));
    }

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
        -- Favourites are keyed by path, and a track can be reached by three of
        -- them. Without these, matching a favourite to its track means reading
        -- every row in the library: the query planner said SCAN, and finding
        -- a hundred favourites among fifty thousand tracks took fifty
        -- milliseconds, on every listing that wanted to know what was starred.
        -- Partial, because both columns are null for anything purely local.
        CREATE INDEX IF NOT EXISTS idx_tracks_cached_path ON tracks(cached_path)
            WHERE cached_path IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_tracks_remote_url ON tracks(remote_url)
            WHERE remote_url IS NOT NULL;
        -- A remote sync enriches every album and every artist it paged through,
        -- matched on the server's id. Without these that is one full table read
        -- per record, so a library twice the size costs four times as much to
        -- sync. Partial, because a locally-scanned record has no remote id.
        CREATE INDEX IF NOT EXISTS idx_albums_remote_id ON albums(remote_id)
            WHERE remote_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_artists_remote_id ON artists(remote_id)
            WHERE remote_id IS NOT NULL;
        -- Radio resolves the artists a recommender names back to local rows,
        -- by MusicBrainz id and then by name. `UNIQUE(name)` is a binary index
        -- and the name lookup is case-insensitive, so it could not use it.
        CREATE INDEX IF NOT EXISTS idx_artists_mbid ON artists(mbid)
            WHERE mbid IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_artists_name_nocase
            ON artists(name COLLATE NOCASE);

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

        -- Forgetting a track deletes its scan cache entry by track id, and the
        -- primary key is the path.
        CREATE INDEX IF NOT EXISTS idx_scan_cache_track ON scan_cache(track_id);

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

        -- Undo reads back one batch at a time.
        CREATE INDEX IF NOT EXISTS idx_organize_log_batch ON organize_log(batch_id);

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

        -- Albums and artists are favourited by name, not by row id, for the
        -- same reason tracks are favourited by path: a rebuilt index assigns
        -- new ids, and losing every favourite to a reindex is not acceptable.
        CREATE TABLE IF NOT EXISTS favourite_albums (
            artist_name TEXT NOT NULL,
            album_title TEXT NOT NULL,
            created_at  TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (artist_name, album_title)
        );

        CREATE TABLE IF NOT EXISTS favourite_artists (
            artist_name TEXT PRIMARY KEY,
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
            track_id    INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
            played_at   INTEGER NOT NULL,
            duration_ms INTEGER,
            source      TEXT DEFAULT 'local'
        );

        CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id);
        CREATE INDEX IF NOT EXISTS idx_play_history_time ON play_history(played_at);

        -- Playlists carry what Subsonic carries and nothing else, so a
        -- playlist made here and one made on the server are the same object.
        -- `sort_order` and `grouped` are the exceptions: where a playlist sits
        -- in your sidebar and how you like to look at it are facts about this
        -- machine, and no server has anywhere to put them.
        CREATE TABLE IF NOT EXISTS playlists (
            id         INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            comment    TEXT,
            public     INTEGER NOT NULL DEFAULT 0,
            owner      TEXT,
            remote_id  TEXT UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            changed_at TEXT NOT NULL DEFAULT (datetime('now')),
            sort_order INTEGER NOT NULL DEFAULT 0,
            grouped    INTEGER
        );

        -- One row per entry, with an id of its own.
        --
        -- A playlist may hold the same track twice, so a track id names neither
        -- a row nor a place. The entry id does: it survives a reorder, it is
        -- what a queue item remembers it came from, and it is how the two
        -- copies of a song are told apart when one of them is playing.
        CREATE TABLE IF NOT EXISTS playlist_tracks (
            id          INTEGER PRIMARY KEY,
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            position    INTEGER NOT NULL,
            track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            UNIQUE (playlist_id, position)
        );

        CREATE INDEX IF NOT EXISTS idx_playlist_tracks_track ON playlist_tracks(track_id);

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
        -- Expiry was indexed and never used: the only query that reads it is the
        -- cleanup sweep, whose `revoked = 1 OR expires_at <= ?` spans two columns
        -- and reads the table either way. An index nothing reads is a cost paid
        -- on every sign-in.
        DROP INDEX IF EXISTS idx_refresh_tokens_expires;
        ",
    )?;
    apply_migrations(conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;

    Ok(())
}

/// Columns added after the initial schema. Applied when absent, so a database
/// created by any earlier version converges on the current shape.
///
/// `organize_log.size_bytes`/`mtime` are checked against the file before undo
/// moves it back, so a file replaced since the organize is left alone.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    ("tracks", "cache_size_bytes", "INTEGER"),
    ("tracks", "cache_download_date", "INTEGER"),
    (
        "similar_artists",
        "relationship",
        "TEXT NOT NULL DEFAULT 'similar'",
    ),
    ("organize_log", "size_bytes", "INTEGER"),
    ("organize_log", "mtime", "INTEGER"),
    // When the album entered the library, so clients can offer a
    // recently-added ordering. Remote sync supplies the server's own `created`;
    // a local scan the earliest mtime among the album's files.
    ("albums", "added_at", "TEXT"),
    // Whether playback was running when the session was saved, so reopening can
    // pick up where it left off rather than always paused.
    (
        "playback_state",
        "was_playing",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    // Radio is a mode you leave on, not a per-session choice: switching itself
    // off every launch makes it a setting that will not stay set.
    (
        "playback_state",
        "radio_enabled",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    // MusicBrainz ids are the join key for anything that wants to look a
    // release or a recording up elsewhere. The server hands them over on every
    // album and every song and koan was discarding all of them.
    ("albums", "mbid", "TEXT"),
    ("tracks", "mbid", "TEXT"),
    // The server's own sort key, which is what it orders by. Artists already
    // had this column and nothing ever filled it.
    ("albums", "sort_name", "TEXT"),
];

fn apply_migrations(conn: &Connection) -> rusqlite::Result<()> {
    for (table, column, ty) in ADDED_COLUMNS {
        if !column_exists(conn, table, column)? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
        }
    }

    // Locally-scanned albums were briefly stamped with the time the scan ran,
    // which pinned every one of them to the top of recently-added and buried
    // whatever the server actually considered new. Clearing the scan-time
    // values lets the next scan refill them from the files themselves; the
    // server's own ISO 8601 dates are left alone.
    conn.execute(
        "UPDATE albums SET added_at = NULL
           WHERE added_at IS NOT NULL AND added_at NOT LIKE '%T%Z'",
        [],
    )?;

    cascade_play_history(conn)?;
    snapshots_to_playlists(conn)?;
    crate::db::queries::tracks::merge_split_cross_source_tracks(conn)?;

    Ok(())
}

/// Turn saved queues into playlists, then drop the table they lived in.
///
/// Snapshots were playlists with a resume position — a whole second feature to
/// maintain for one number, and one the server had no idea about. The track
/// lists are real work someone did, so they come across; only the position is
/// lost. The Subsonic API already served snapshots as playlists, so its clients
/// see the same names either side of this.
fn snapshots_to_playlists(conn: &Connection) -> rusqlite::Result<()> {
    if !table_exists(conn, "queue_snapshots")? {
        return Ok(());
    }

    let mut saved: Vec<(String, String, String)> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT name, queue_json, created_at FROM queue_snapshots")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        })?;
        for row in rows {
            saved.push(row?);
        }
    }

    for (name, json, created_at) in saved {
        let paths: Vec<String> = serde_json::from_str::<Vec<serde_json::Value>>(&json)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                item.get("path")
                    .and_then(|p| p.as_str())
                    .map(|p| p.to_string())
            })
            .collect();

        conn.execute(
            "INSERT INTO playlists (name, created_at, changed_at)
             VALUES (?1, COALESCE(NULLIF(?2, ''), datetime('now')), datetime('now'))",
            rusqlite::params![name, created_at],
        )?;
        let playlist_id = conn.last_insert_rowid();

        let mut position = 0i64;
        for path in paths {
            let track_id: Option<i64> = conn
                .query_row("SELECT id FROM tracks WHERE path = ?1", [&path], |r| {
                    r.get(0)
                })
                .ok();
            // A snapshot held enough metadata to play a file that was never
            // indexed. A playlist points at library rows, so anything with no
            // row behind it cannot come across.
            if let Some(track_id) = track_id {
                conn.execute(
                    "INSERT INTO playlist_tracks (playlist_id, position, track_id)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![playlist_id, position, track_id],
                )?;
                position += 1;
            }
        }
    }

    conn.execute("DROP TABLE queue_snapshots", [])?;
    Ok(())
}

/// Whether `table` exists.
fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let found: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get(0),
    )?;
    Ok(found > 0)
}

/// Give `play_history.track_id` its `ON DELETE CASCADE`.
///
/// The column shipped as a bare `REFERENCES`, which under `foreign_keys = ON`
/// makes a track with history undeletable unless the caller remembers to clear
/// the history first. One caller does; the constraint should not depend on the
/// next one remembering. SQLite cannot alter a constraint in place, so the
/// table is rebuilt.
fn cascade_play_history(conn: &Connection) -> rusqlite::Result<()> {
    if fk_cascades(conn, "play_history")? {
        return Ok(());
    }

    // Pragma changes are no-ops inside a transaction, so this must bracket it.
    conn.pragma_update(None, "foreign_keys", "off")?;
    let rebuild = conn.execute_batch(
        "BEGIN;
         CREATE TABLE play_history_new (
             id          INTEGER PRIMARY KEY,
             track_id    INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
             played_at   INTEGER NOT NULL,
             duration_ms INTEGER,
             source      TEXT DEFAULT 'local'
         );
         -- Entries whose track has already gone would violate the new
         -- constraint the moment it is enforced. They are unreachable anyway.
         INSERT INTO play_history_new (id, track_id, played_at, duration_ms, source)
             SELECT id, track_id, played_at, duration_ms, source FROM play_history
             WHERE track_id IS NULL OR track_id IN (SELECT id FROM tracks);
         DROP TABLE play_history;
         ALTER TABLE play_history_new RENAME TO play_history;
         CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id);
         CREATE INDEX IF NOT EXISTS idx_play_history_time ON play_history(played_at);
         COMMIT;",
    );
    conn.pragma_update(None, "foreign_keys", "on")?;
    rebuild
}

/// Whether every foreign key on `table` deletes its rows with the parent.
fn fk_cascades(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
    let mut rows = stmt.query([])?;
    let mut any = false;
    while let Some(row) = rows.next()? {
        any = true;
        // Column 6 is `on_delete`.
        if !row.get::<_, String>(6)?.eq_ignore_ascii_case("CASCADE") {
            return Ok(false);
        }
    }
    Ok(any)
}

/// Whether `table` already has `column`.
///
/// PRAGMA cannot take a bound parameter for the table name, so the name is
/// interpolated — every caller passes a literal from `ADDED_COLUMNS`, never
/// user input.
fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;

    /// Queries that answer a question about a handful of rows, and must not
    /// read the library to do it.
    ///
    /// The planner will happily fall back to a full scan when a query is
    /// written in a shape no index can serve — an `OR` spanning two columns, a
    /// `LIKE` pattern, a collation the index does not use — and nothing about
    /// the result says it happened. It costs, it does not fail, and it gets
    /// worse with the size of somebody's library. So the plans are asserted.
    #[test]
    fn hot_queries_do_not_scan() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();

        let plan = |sql: &str| -> Vec<String> {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let nulls = vec![rusqlite::types::Null; stmt.parameter_count()];
            stmt.query_map(rusqlite::params_from_iter(nulls), |r| r.get::<_, String>(3))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };

        let cases: &[(&str, &str)] = &[
            (
                "a track by any of its three paths",
                "SELECT id FROM tracks WHERE path = ?1 OR cached_path = ?1 OR remote_url = ?1",
            ),
            (
                "an artist's tracks, own or album credit",
                "SELECT t.id FROM tracks t LEFT JOIN albums al ON t.album_id = al.id
                  WHERE t.artist_id = ?1
                     OR t.album_id IN (SELECT id FROM albums WHERE artist_id = ?1)",
            ),
            (
                "every favourited track",
                "SELECT id FROM tracks WHERE path IN (SELECT track_path FROM favourites)
                 UNION
                 SELECT id FROM tracks WHERE cached_path IN (SELECT track_path FROM favourites)
                 UNION
                 SELECT id FROM tracks WHERE remote_url IN (SELECT track_path FROM favourites)",
            ),
            (
                "tracks under a folder",
                "SELECT id FROM tracks WHERE path >= ?1 AND path < ?2",
            ),
            (
                "an album by the server's id",
                "SELECT id FROM albums WHERE remote_id = ?1",
            ),
            (
                "an artist by the server's id",
                "SELECT id FROM artists WHERE remote_id = ?1",
            ),
            (
                "an artist by MusicBrainz id",
                "SELECT id FROM artists WHERE mbid = ?1",
            ),
            (
                "an artist by name, however it is capitalised",
                "SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE",
            ),
            (
                "a scan cache entry by track",
                "SELECT path FROM scan_cache WHERE track_id = ?1",
            ),
            (
                "one organize batch",
                "SELECT id FROM organize_log WHERE batch_id = ?1",
            ),
        ];

        for (what, sql) in cases {
            let steps = plan(sql);
            assert!(
                !steps.iter().any(|s| s.starts_with("SCAN")),
                "{what}: reads the whole table\n  {}",
                steps.join("\n  ")
            );
        }
    }

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
    fn saved_queues_become_playlists() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        // The table as it stood before playlists existed.
        conn.execute_batch(
            "CREATE TABLE queue_snapshots (
                 id          INTEGER PRIMARY KEY,
                 name        TEXT NOT NULL UNIQUE,
                 queue_json  TEXT NOT NULL DEFAULT '[]',
                 cursor_path TEXT,
                 position_ms INTEGER NOT NULL DEFAULT 0,
                 created_at  TEXT DEFAULT (datetime('now'))
             );
             INSERT INTO artists (id, name) VALUES (1, 'Klaxons');
             INSERT INTO albums (id, title, artist_id) VALUES (1, 'Myths', 1);
             INSERT INTO tracks (id, album_id, artist_id, title, path)
               VALUES (1, 1, 1, 'Atlantis', '/music/atlantis.flac'),
                      (2, 1, 1, 'Golden Skans', '/music/golden.flac');
             INSERT INTO queue_snapshots (name, queue_json, created_at) VALUES
               ('techno',
                '[{\"path\":\"/music/golden.flac\"},{\"path\":\"/music/nowhere.flac\"},{\"path\":\"/music/atlantis.flac\"}]',
                '2026-01-01 10:00:00');",
        )
        .unwrap();

        create_tables(&conn).unwrap();

        assert!(!table_exists(&conn, "queue_snapshots").unwrap());
        let (id, name, created): (i64, String, String) = conn
            .query_row("SELECT id, name, created_at FROM playlists", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(name, "techno");
        assert_eq!(created, "2026-01-01 10:00:00", "when it was saved is kept");

        let mut stmt = conn
            .prepare(
                "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
            )
            .unwrap();
        let members: Vec<i64> = stmt
            .query_map([id], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            members,
            vec![2, 1],
            "order is kept, and a file with no library row cannot come across"
        );
    }

    #[test]
    fn migrates_similar_artists_relationship_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE artists (
                 id        INTEGER PRIMARY KEY,
                 name      TEXT NOT NULL UNIQUE,
                 sort_name TEXT,
                 mbid      TEXT,
                 remote_id TEXT
             );
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
    #[test]
    fn play_history_from_before_the_cascade_is_rebuilt_keeping_its_rows() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();

        // Seeded with enforcement off so the deliberately-orphaned entry lands.
        conn.pragma_update(None, "foreign_keys", "off").unwrap();
        // Put back the original constraint-free table and refill it.
        conn.execute_batch(
            "DROP TABLE play_history;
             CREATE TABLE play_history (
                 id          INTEGER PRIMARY KEY,
                 track_id    INTEGER REFERENCES tracks(id),
                 played_at   INTEGER NOT NULL,
                 duration_ms INTEGER,
                 source      TEXT DEFAULT 'local'
             );
             INSERT INTO artists (id, name) VALUES (1, 'A');
             INSERT INTO tracks (id, artist_id, title, source) VALUES (7, 1, 'T', 'local');
             INSERT INTO play_history (id, track_id, played_at, duration_ms, source)
                 VALUES (1, 7, 100, 5000, 'local'),
                        (2, 999, 200, NULL, 'local');",
        )
        .unwrap();
        assert!(!fk_cascades(&conn, "play_history").unwrap());

        apply_migrations(&conn).unwrap();

        assert!(fk_cascades(&conn, "play_history").unwrap());
        let kept: Vec<(i64, i64, Option<i64>)> = conn
            .prepare("SELECT id, played_at, duration_ms FROM play_history ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            kept,
            vec![(1, 100, Some(5000))],
            "the live entry survives; the one pointing at a track that is gone does not"
        );

        // And the constraint now does the work the callers were doing by hand.
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 7", []).unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM play_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn cascading_play_history_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        cascade_play_history(&conn).unwrap();
        cascade_play_history(&conn).unwrap();
        assert!(fk_cascades(&conn, "play_history").unwrap());
    }

    #[test]
    fn fresh_database_is_stamped_with_the_current_version() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn create_tables_is_idempotent_across_repeated_opens() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..3 {
            create_tables(&conn).unwrap();
        }
        assert!(column_exists(&conn, "tracks", "cache_size_bytes").unwrap());
        assert!(column_exists(&conn, "organize_log", "mtime").unwrap());
    }

    #[test]
    fn a_database_missing_added_columns_is_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        // Rebuild `organize_log` without the columns added after the initial
        // schema, so the file looks like one written by an earlier version.
        conn.execute_batch(
            "DROP TABLE organize_log;
             CREATE TABLE organize_log (
                 id         INTEGER PRIMARY KEY,
                 batch_id   TEXT NOT NULL,
                 track_id   INTEGER,
                 from_path  TEXT NOT NULL,
                 to_path    TEXT NOT NULL,
                 created_at TEXT DEFAULT (datetime('now'))
             );
             PRAGMA user_version = 0;",
        )
        .unwrap();
        assert!(!column_exists(&conn, "organize_log", "size_bytes").unwrap());

        create_tables(&conn).unwrap();

        assert!(column_exists(&conn, "organize_log", "size_bytes").unwrap());
        assert!(column_exists(&conn, "organize_log", "mtime").unwrap());
    }

    #[test]
    fn migration_does_not_depend_on_sqlite_error_text() {
        // The previous implementation swallowed a duplicate-column ALTER by
        // string-matching SQLite's message, so a wording change in a bundled
        // SQLite upgrade would have failed every open. Adding a column that is
        // already present must now be a no-op decided by schema inspection.
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        apply_migrations(&conn).unwrap();
        apply_migrations(&conn).unwrap();
    }

    #[test]
    fn a_newer_database_is_refused_rather_than_written_to() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();

        let err = create_tables(&conn).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("newer than this build"), "unexpected: {msg}");
    }
}

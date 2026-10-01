use rusqlite::Connection;

/// Bumped whenever the schema changes. Stored in `PRAGMA user_version` so an
/// older build refuses a database it does not understand rather than writing to it.
pub const SCHEMA_VERSION: i64 = 9;

/// Create all tables. Idempotent — safe to call on every startup.
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

    // Every statement past here takes the write lock, whether or not it
    // changes a row, and an open waits behind any scan or sync holding it. A
    // database already at this version has nothing for them to do. The column
    // checks are reads, and catch a column added without a version bump.
    if found == SCHEMA_VERSION {
        add_missing_columns(conn)?;
        return crate::db::queries::auth::adopt_local_rows(conn);
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
        -- The genre list and the genre filter both match case-insensitively,
        -- and both want the albums a genre spans.
        CREATE INDEX IF NOT EXISTS idx_tracks_genre
            ON tracks(genre COLLATE NOCASE, album_id);

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
        -- Merging two track rows repoints the log from one to the other.
        CREATE INDEX IF NOT EXISTS idx_organize_log_track ON organize_log(track_id);

        CREATE TABLE IF NOT EXISTS lyrics_cache (
            id          INTEGER PRIMARY KEY,
            track_id    INTEGER REFERENCES tracks(id),
            source      TEXT NOT NULL,
            synced      INTEGER DEFAULT 0,
            content     TEXT NOT NULL,
            fetched_at  INTEGER NOT NULL,
            UNIQUE(track_id)
        );

        -- Rows deleted from albums, artists and tracks, for front ends that
        -- cache by id: SQLite hands a freed id to the next row, and a cover
        -- cached under it would be shown for the wrong record.
        CREATE TABLE IF NOT EXISTS art_evictions (
            seq   INTEGER PRIMARY KEY AUTOINCREMENT,
            kind  TEXT NOT NULL,
            id    INTEGER NOT NULL
        );
        CREATE TRIGGER IF NOT EXISTS evict_album_art AFTER DELETE ON albums
            BEGIN INSERT INTO art_evictions (kind, id) VALUES ('album', old.id); END;
        CREATE TRIGGER IF NOT EXISTS evict_artist_art AFTER DELETE ON artists
            BEGIN INSERT INTO art_evictions (kind, id) VALUES ('artist', old.id); END;
        CREATE TRIGGER IF NOT EXISTS evict_track_art AFTER DELETE ON tracks
            BEGIN INSERT INTO art_evictions (kind, id) VALUES ('track', old.id); END;

        -- A server's record of the koan apps that have linked to it, and what
        -- waits for each while it is away: see koan-server's clients.rs.
        CREATE TABLE IF NOT EXISTS link_devices (
            device     TEXT NOT NULL,
            username   TEXT NOT NULL,
            name       TEXT NOT NULL,
            platform   TEXT NOT NULL,
            last_seen  INTEGER NOT NULL,
            PRIMARY KEY (device, username)
        );

        -- Where Apple's push service reaches each linked iOS app, kept after
        -- its socket closes: reaching a suspended app is what it is for.
        CREATE TABLE IF NOT EXISTS link_push (
            device      TEXT NOT NULL,
            username    TEXT NOT NULL,
            token       TEXT NOT NULL,
            sandbox     INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL,
            PRIMARY KEY (device, username)
        );

        CREATE TABLE IF NOT EXISTS link_orders (
            id          TEXT PRIMARY KEY,
            body        TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS link_outbox (
            id          INTEGER PRIMARY KEY,
            device      TEXT NOT NULL,
            username    TEXT NOT NULL,
            command     TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );

        -- One row per artist looked up, misses included: an empty row is the
        -- answer that nothing was found, which stops a page asking again.
        CREATE TABLE IF NOT EXISTS artist_info (
            artist_id     INTEGER PRIMARY KEY REFERENCES artists(id) ON DELETE CASCADE,
            bio           TEXT,
            bio_url       TEXT,
            image_url     TEXT,
            image_credit  TEXT,
            fetched_at    INTEGER NOT NULL
        );

        -- Favourites, playlists, play history and shares belong to a user:
        -- an account's id, or 0 for the implicit user of an install with no
        -- admin account (see `queries::auth::LOCAL_USER`). 0 names no row in
        -- `users`, so there is no foreign key; the `users_personal_data`
        -- trigger does the cascading one would.
        CREATE TABLE IF NOT EXISTS favourites (
            user_id     INTEGER NOT NULL DEFAULT 0,
            track_path  TEXT NOT NULL,
            created_at  TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (user_id, track_path)
        );

        -- Albums and artists are favourited by name, not by row id, for the
        -- same reason tracks are favourited by path: a rebuilt index assigns
        -- new ids, and losing every favourite to a reindex is not acceptable.
        CREATE TABLE IF NOT EXISTS favourite_albums (
            user_id     INTEGER NOT NULL DEFAULT 0,
            artist_name TEXT NOT NULL,
            album_title TEXT NOT NULL,
            created_at  TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (user_id, artist_name, album_title)
        );

        CREATE TABLE IF NOT EXISTS favourite_artists (
            user_id     INTEGER NOT NULL DEFAULT 0,
            artist_name TEXT NOT NULL,
            created_at  TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (user_id, artist_name)
        );

        CREATE TABLE IF NOT EXISTS playback_state (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            queue_json  TEXT NOT NULL DEFAULT '[]',
            cursor_id   TEXT,
            position_ms INTEGER NOT NULL DEFAULT 0,
            updated_at  TEXT DEFAULT (datetime('now'))
        );

        -- Where the saved queue is up to, written every second while music
        -- plays. A row of its own because SQLite rewrites a record whenever
        -- its size changes, and in `playback_state` that record carries the
        -- whole queue.
        CREATE TABLE IF NOT EXISTS playback_position (
            id            INTEGER PRIMARY KEY CHECK (id = 1),
            cursor_id     TEXT,
            position_ms   INTEGER NOT NULL DEFAULT 0,
            was_playing   INTEGER NOT NULL DEFAULT 0,
            radio_enabled INTEGER NOT NULL DEFAULT 0,
            updated_at    TEXT DEFAULT (datetime('now'))
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
        -- The primary key serves `artist_id`; deleting an artist also looks
        -- for it as someone else's similar artist.
        CREATE INDEX IF NOT EXISTS idx_similar_artists_similar ON similar_artists(similar_id);

        CREATE TABLE IF NOT EXISTS play_history (
            id          INTEGER PRIMARY KEY,
            track_id    INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
            played_at   INTEGER NOT NULL,
            duration_ms INTEGER,
            source      TEXT DEFAULT 'local'
        );

        -- With `played_at`, a track's last play is read off the index.
        CREATE INDEX IF NOT EXISTS idx_play_history_track_played
            ON play_history(track_id, played_at);
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
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
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

        -- Subsonic API keys. Only `sha256(key)` is kept: a key is 32 random
        -- bytes, so a fast hash is enough, and a database read yields nothing
        -- that signs in.
        CREATE TABLE IF NOT EXISTS api_keys (
            id           INTEGER PRIMARY KEY,
            user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name         TEXT NOT NULL,
            key_hash     TEXT NOT NULL UNIQUE,
            created_at   INTEGER NOT NULL,
            last_used_at INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);

        -- Share links this koan serves itself. The tracks are an explicit list,
        -- not a query: what a link names is all an anonymous visitor can play,
        -- so it must not grow when the library does.
        CREATE TABLE IF NOT EXISTS shares (
            id           TEXT PRIMARY KEY,
            description  TEXT,
            created_at   INTEGER NOT NULL,
            expires_at   INTEGER,
            visits       INTEGER NOT NULL DEFAULT 0,
            last_visited INTEGER
        );

        CREATE TABLE IF NOT EXISTS share_tracks (
            share_id TEXT NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
            position INTEGER NOT NULL,
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            PRIMARY KEY (share_id, position)
        );
        -- Deleting or merging a track looks for the shares that name it.
        CREATE INDEX IF NOT EXISTS idx_share_tracks_track ON share_tracks(track_id);
        -- Expiry was indexed and never used: the only query that reads it is the
        -- cleanup sweep, whose `revoked = 1 OR expires_at <= ?` spans two columns
        -- and reads the table either way. An index nothing reads is a cost paid
        -- on every sign-in.
        DROP INDEX IF EXISTS idx_refresh_tokens_expires;
        ",
    )?;
    apply_migrations(conn, found)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;

    Ok(())
}

/// Columns added after the initial schema. Applied when absent, so a database
/// created by any earlier version converges on the current shape.
///
/// `organize_log.size_bytes`/`mtime` are checked against the file before undo
/// moves it back, so a file replaced since the organize is left alone.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    // The password sealed under the server's Subsonic key, since token auth
    // needs the plaintext and `password_hash` cannot give it back.
    ("users", "sealed_password", "BLOB"),
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
    // album and every song.
    ("albums", "mbid", "TEXT"),
    ("tracks", "mbid", "TEXT"),
    // The server's own sort key, which is what it orders by.
    ("albums", "sort_name", "TEXT"),
    // What a share is a slice of, so its page shows an album or an artist as
    // one. The track list stays authoritative; shares made before are loose
    // tracks.
    ("shares", "kind", "TEXT NOT NULL DEFAULT 'tracks'"),
    ("shares", "subject_id", "INTEGER"),
    ("shares", "start_track_id", "INTEGER"),
    // Whose it is. See the note above `favourites`.
    ("play_history", "user_id", "INTEGER NOT NULL DEFAULT 0"),
    ("playlists", "user_id", "INTEGER NOT NULL DEFAULT 0"),
    ("shares", "user_id", "INTEGER NOT NULL DEFAULT 0"),
    // The id every surface publishes; see `queries::uids`. Row ids are
    // numbered per table and per database, so album 5 and song 5 are the same
    // id to an endpoint that takes either, and neither means anything on
    // another device.
    ("artists", "uid", "TEXT"),
    ("albums", "uid", "TEXT"),
    ("tracks", "uid", "TEXT"),
    ("playlists", "uid", "TEXT"),
    // Two-way playlist sync. `revision` counts local edits, `synced_revision`
    // is the one the last push or pull covered, `remote_changed` the server's
    // `changed` as of then, and `remote_account` whose server `remote_id`
    // names a playlist on. Each side is judged against its own record, which
    // is what lets a sync tell "the server moved" from "we moved" without
    // comparing two machines' clocks.
    ("playlists", "revision", "INTEGER NOT NULL DEFAULT 0"),
    ("playlists", "synced_revision", "INTEGER"),
    ("playlists", "remote_changed", "TEXT"),
    ("playlists", "remote_account", "TEXT"),
];

/// A UUIDv7 in SQL, for the triggers that give every new row its `uid`: a
/// trigger covers every insert, however it is written, where a Rust-side
/// default would have to be remembered at each one.
const SQL_UUID7: &str = "(SELECT substr(t, 1, 8) || '-' || substr(t, 9, 4) || '-7' || substr(r, 1, 3)
        || '-' || substr('89ab', 1 + (random() & 3), 1) || substr(r, 4, 3) || '-' || substr(r, 7, 12)
    FROM (SELECT printf('%012x', CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)) AS t,
                 lower(hex(randomblob(9))) AS r))";

/// Add whichever of `ADDED_COLUMNS` the database lacks. Reads alone when none
/// are missing.
fn add_missing_columns(conn: &Connection) -> rusqlite::Result<()> {
    let had_remote_account = column_exists(conn, "playlists", "remote_account")?;
    for (table, column, ty) in ADDED_COLUMNS {
        if !column_exists(conn, table, column)? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
        }
    }

    // Playlists synced before `remote_account` existed belong to the server
    // last synced with. Recorded now, before a sync against a different one
    // can take their ids for its own.
    if !had_remote_account {
        conn.execute(
            "UPDATE playlists SET remote_account =
               (SELECT username || '@' || rtrim(url, '/') FROM remote_servers
                ORDER BY last_sync DESC LIMIT 1)
             WHERE remote_id IS NOT NULL",
            [],
        )?;
    }
    Ok(())
}

fn apply_migrations(conn: &Connection, found: i64) -> rusqlite::Result<()> {
    add_missing_columns(conn)?;

    // Cross-source dedup looks tracks up by recording id.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tracks_mbid ON tracks(mbid) WHERE mbid IS NOT NULL",
        [],
    )?;

    // Scans before version 3 never read MusicBrainz ids from tags. Forgetting
    // the files that came through without one has the next scan read them
    // again, which is what pairs them with the server's copy by id. Files that
    // have none are read once more and then left alone.
    if found < 3 {
        conn.execute(
            "DELETE FROM scan_cache WHERE track_id IN
               (SELECT id FROM tracks WHERE path IS NOT NULL AND mbid IS NULL)",
            [],
        )?;
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

    // Syncs before 0.36.2 stored a server's "no id" (`musicBrainzId: ""`) as
    // an empty string, which everything downstream took for an id: artist
    // info looked "" up instead of resolving one, and never showed anything.
    // Sort names, labels and genres came through the same way. Found-nothing
    // artist info is no longer cached, and what was is dropped. Idempotent,
    // and cheap once there are none.
    conn.execute_batch(
        "DELETE FROM artist_info WHERE bio IS NULL AND image_url IS NULL;
         UPDATE artists SET mbid = NULL WHERE mbid = '';
         UPDATE artists SET sort_name = NULL WHERE sort_name = '';
         UPDATE albums SET sort_name = NULL WHERE sort_name = '';
         UPDATE albums SET label = NULL WHERE label = '';
         UPDATE tracks SET genre = NULL WHERE genre = '';
         UPDATE albums SET mbid = NULL WHERE mbid = '';
         UPDATE tracks SET mbid = NULL WHERE mbid = '';",
    )?;
    // Stale-track removal once left the album of a moved or deleted record
    // behind with nothing in it, and a server lists it to everyone who syncs.
    conn.execute(
        "DELETE FROM albums WHERE NOT EXISTS
           (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
        [],
    )?;

    autoincrement_user_ids(conn)?;
    merge_case_duplicate_artists(conn)?;
    // A client more than this far behind purges its whole cache instead.
    conn.execute(
        "DELETE FROM art_evictions WHERE seq < (SELECT MAX(seq) FROM art_evictions) - 50000",
        [],
    )?;
    cascade_play_history(conn)?;
    snapshots_to_playlists(conn)?;
    per_user_favourites(conn)?;
    // After the rebuilds above, which drop a table's indexes with it.
    // `idx_play_history_track` is a prefix of `idx_play_history_track_played`.
    // The favourites key leads with the user, and merging two tracks repoints
    // every user's favourite by path.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_play_history_user ON play_history(user_id, played_at);
         CREATE INDEX IF NOT EXISTS idx_play_history_track_played ON play_history(track_id, played_at);
         DROP INDEX IF EXISTS idx_play_history_track;
         CREATE INDEX IF NOT EXISTS idx_favourites_path ON favourites(track_path);
         CREATE INDEX IF NOT EXISTS idx_playlists_user ON playlists(user_id);
         CREATE TRIGGER IF NOT EXISTS users_personal_data AFTER DELETE ON users BEGIN
             DELETE FROM favourites WHERE user_id = OLD.id;
             DELETE FROM favourite_albums WHERE user_id = OLD.id;
             DELETE FROM favourite_artists WHERE user_id = OLD.id;
             DELETE FROM play_history WHERE user_id = OLD.id;
             DELETE FROM playlists WHERE user_id = OLD.id;
             DELETE FROM shares WHERE user_id = OLD.id;
         END;",
    )?;
    crate::db::queries::auth::adopt_local_rows(conn)?;

    // The position moved out of the queue's row; the last one saved comes too.
    if found < 9 {
        conn.execute(
            "INSERT OR IGNORE INTO playback_position
                 (id, cursor_id, position_ms, was_playing, radio_enabled)
             SELECT 1, cursor_id, position_ms, was_playing, radio_enabled
               FROM playback_state WHERE id = 1",
            [],
        )?;
    }

    for table in ["artists", "albums", "tracks", "playlists"] {
        conn.execute_batch(&format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_{table}_uid ON {table}(uid);
             CREATE TRIGGER IF NOT EXISTS {table}_uid AFTER INSERT ON {table}
               WHEN NEW.uid IS NULL
             BEGIN
               UPDATE {table} SET uid = {SQL_UUID7} WHERE id = NEW.id;
             END;"
        ))?;
        backfill_uids(conn, table)?;
    }

    // Once: `upsert_track` stores no new zeros, and the sweep reads every track.
    if found < 3 {
        crate::db::queries::tracks::clear_zero_discs(conn)?;
    }
    crate::db::queries::tracks::merge_split_cross_source_tracks(conn)?;
    crate::db::queries::tracks::merge_spelling_twins(conn)?;

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

/// Give every row of `table` that has none a `uid`, in row order so the ids
/// keep the order the rows were added in. Rows written since the trigger
/// existed already have one, so after the first run this finds nothing.
fn backfill_uids(conn: &Connection, table: &str) -> rusqlite::Result<()> {
    let ids: Vec<i64> = conn
        .prepare(&format!(
            "SELECT id FROM {table} WHERE uid IS NULL ORDER BY id"
        ))?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if ids.is_empty() {
        return Ok(());
    }
    conn.execute_batch("SAVEPOINT backfill_uids")?;
    let written = (|| {
        let mut update = conn.prepare(&format!("UPDATE {table} SET uid = ?1 WHERE id = ?2"))?;
        for id in ids {
            update.execute(rusqlite::params![uuid::Uuid::now_v7().to_string(), id])?;
        }
        Ok(())
    })();
    match written {
        Ok(()) => conn.execute_batch("RELEASE backfill_uids"),
        Err(e) => {
            conn.execute_batch("ROLLBACK TO backfill_uids; RELEASE backfill_uids")?;
            Err(e)
        }
    }
}

/// Rebuild the favourite tables keyed by user, keeping every row.
///
/// They were keyed by what was favourited alone, and SQLite cannot change a
/// primary key in place. Existing rows come across as the implicit local user;
/// `adopt_local_rows` then hands them to the first admin, if there is one.
fn per_user_favourites(conn: &Connection) -> rusqlite::Result<()> {
    if column_exists(conn, "favourites", "user_id")? {
        return Ok(());
    }
    // A trigger naming a table mid-rebuild fails the rename that completes
    // it. `apply_migrations` recreates it afterwards.
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS users_personal_data;
         BEGIN;
         CREATE TABLE favourites_new (
             user_id     INTEGER NOT NULL DEFAULT 0,
             track_path  TEXT NOT NULL,
             created_at  TEXT DEFAULT (datetime('now')),
             PRIMARY KEY (user_id, track_path)
         );
         INSERT INTO favourites_new (track_path, created_at)
             SELECT track_path, created_at FROM favourites;
         DROP TABLE favourites;
         ALTER TABLE favourites_new RENAME TO favourites;

         CREATE TABLE favourite_albums_new (
             user_id     INTEGER NOT NULL DEFAULT 0,
             artist_name TEXT NOT NULL,
             album_title TEXT NOT NULL,
             created_at  TEXT DEFAULT (datetime('now')),
             PRIMARY KEY (user_id, artist_name, album_title)
         );
         INSERT INTO favourite_albums_new (artist_name, album_title, created_at)
             SELECT artist_name, album_title, created_at FROM favourite_albums;
         DROP TABLE favourite_albums;
         ALTER TABLE favourite_albums_new RENAME TO favourite_albums;

         CREATE TABLE favourite_artists_new (
             user_id     INTEGER NOT NULL DEFAULT 0,
             artist_name TEXT NOT NULL,
             created_at  TEXT DEFAULT (datetime('now')),
             PRIMARY KEY (user_id, artist_name)
         );
         INSERT INTO favourite_artists_new (artist_name, created_at)
             SELECT artist_name, created_at FROM favourite_artists;
         DROP TABLE favourite_artists;
         ALTER TABLE favourite_artists_new RENAME TO favourite_artists;
         COMMIT;",
    )
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

/// Fold artists whose names differ only in letter case into one: the row that
/// owns the most albums, then the most tracks. Tags spell one act differently
/// from record to record, and the split left an artist's page without the
/// albums its tracks belong to. Idempotent; a single query when there are none.
fn merge_case_duplicate_artists(conn: &Connection) -> rusqlite::Result<()> {
    let groups: Vec<String> = conn
        .prepare("SELECT lower(name) FROM artists GROUP BY lower(name) HAVING COUNT(*) > 1")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for key in groups {
        let ids: Vec<i64> = conn
            .prepare(
                "SELECT a.id FROM artists a WHERE lower(a.name) = ?1
                 ORDER BY (SELECT COUNT(*) FROM albums WHERE artist_id = a.id) DESC,
                          (SELECT COUNT(*) FROM tracks WHERE artist_id = a.id) DESC,
                          a.id",
            )?
            .query_map([&key], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let Some((&keep, rest)) = ids.split_first() else {
            continue;
        };
        for &gone in rest {
            // An album both spellings hold under one title is one album: its
            // tracks join the kept artist's copy. Moving the row would break
            // `UNIQUE(title, artist_id)`.
            conn.execute(
                "UPDATE tracks SET album_id = (SELECT k.id FROM albums k, albums g
                                                WHERE g.id = tracks.album_id
                                                  AND k.artist_id = ?1 AND k.title = g.title)
                  WHERE album_id IN (SELECT g.id FROM albums g
                                      WHERE g.artist_id = ?2
                                        AND EXISTS (SELECT 1 FROM albums k
                                                     WHERE k.artist_id = ?1 AND k.title = g.title))",
                [keep, gone],
            )?;
            conn.execute(
                "DELETE FROM albums WHERE artist_id = ?2
                   AND EXISTS (SELECT 1 FROM albums k WHERE k.artist_id = ?1 AND k.title = albums.title)",
                [keep, gone],
            )?;
            conn.execute(
                "UPDATE albums SET artist_id = ?1 WHERE artist_id = ?2",
                [keep, gone],
            )?;
            conn.execute(
                "UPDATE tracks SET artist_id = ?1 WHERE artist_id = ?2",
                [keep, gone],
            )?;
            conn.execute(
                "UPDATE artists SET
                     remote_id = COALESCE(remote_id, (SELECT remote_id FROM artists WHERE id = ?2)),
                     mbid = COALESCE(mbid, (SELECT mbid FROM artists WHERE id = ?2)),
                     sort_name = COALESCE(sort_name, (SELECT sort_name FROM artists WHERE id = ?2))
                 WHERE id = ?1",
                [keep, gone],
            )?;
            conn.execute("DELETE FROM artist_info WHERE artist_id = ?1", [gone])?;
            conn.execute(
                "UPDATE OR IGNORE similar_artists SET artist_id = ?1 WHERE artist_id = ?2",
                [keep, gone],
            )?;
            conn.execute(
                "UPDATE OR IGNORE similar_artists SET similar_id = ?1 WHERE similar_id = ?2",
                [keep, gone],
            )?;
            conn.execute(
                "DELETE FROM similar_artists WHERE artist_id = ?1 OR similar_id = ?1 OR artist_id = similar_id",
                [gone],
            )?;
            conn.execute("DELETE FROM artists WHERE id = ?1", [gone])?;
        }
    }
    Ok(())
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
    // Recreated by `apply_migrations`; see `per_user_favourites`.
    let rebuild = conn.execute_batch(
        "DROP TRIGGER IF EXISTS users_personal_data;
         BEGIN;
         CREATE TABLE play_history_new (
             id          INTEGER PRIMARY KEY,
             track_id    INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
             played_at   INTEGER NOT NULL,
             duration_ms INTEGER,
             source      TEXT DEFAULT 'local',
             user_id     INTEGER NOT NULL DEFAULT 0
         );
         -- Entries whose track has already gone would violate the new
         -- constraint the moment it is enforced. They are unreachable anyway.
         INSERT INTO play_history_new (id, track_id, played_at, duration_ms, source, user_id)
             SELECT id, track_id, played_at, duration_ms, source, user_id FROM play_history
             WHERE track_id IS NULL OR track_id IN (SELECT id FROM tracks);
         DROP TABLE play_history;
         ALTER TABLE play_history_new RENAME TO play_history;
         CREATE INDEX IF NOT EXISTS idx_play_history_track_played
             ON play_history(track_id, played_at);
         CREATE INDEX IF NOT EXISTS idx_play_history_time ON play_history(played_at);
         COMMIT;",
    );
    conn.pragma_update(None, "foreign_keys", "on")?;
    rebuild
}

/// Give `users.id` `AUTOINCREMENT`, keeping every row and id.
///
/// Without it SQLite hands the highest id out again once that account is
/// deleted, and whatever still names the old account by id — a live access
/// token, a link — would then name the new one. SQLite cannot add the keyword
/// in place, so the table is rebuilt. Ids deleted before this ran may still be
/// reused once; none after.
fn autoincrement_user_ids(conn: &Connection) -> rusqlite::Result<()> {
    let sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'users'",
        [],
        |r| r.get(0),
    )?;
    if sql.to_ascii_uppercase().contains("AUTOINCREMENT") {
        return Ok(());
    }

    // Off, or dropping `users` would cascade into every table that names it.
    // Pragma changes are no-ops inside a transaction, so this must bracket it.
    conn.pragma_update(None, "foreign_keys", "off")?;
    // Recreated by `apply_migrations`; see `per_user_favourites`.
    let rebuild = conn.execute_batch(
        "DROP TRIGGER IF EXISTS users_personal_data;
         BEGIN;
         CREATE TABLE users_new (
             id              INTEGER PRIMARY KEY AUTOINCREMENT,
             username        TEXT NOT NULL UNIQUE,
             password_hash   TEXT NOT NULL,
             role            TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user', 'readonly')),
             created_at      TEXT DEFAULT (datetime('now')),
             sealed_password BLOB
         );
         INSERT INTO users_new (id, username, password_hash, role, created_at, sealed_password)
             SELECT id, username, password_hash, role, created_at, sealed_password FROM users;
         DROP TABLE users;
         ALTER TABLE users_new RENAME TO users;
         COMMIT;",
    );
    if rebuild.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
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
    #[test]
    fn case_duplicate_artists_are_merged_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            super::create_tables(&conn).unwrap();
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'The Squire of Gothos'), (2, 'The Squire Of Gothos');
                 INSERT INTO albums (id, title, artist_id) VALUES (1, 'We Do Scorpion Things', 1);
                 INSERT INTO tracks (title, album_id, artist_id, path) VALUES ('Dark Ting', 1, 2, '/a.flac');
                 PRAGMA user_version = 8;",
            )
            .unwrap();
        }
        let conn = rusqlite::Connection::open(&path).unwrap();
        super::create_tables(&conn).unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM artists"), 1);
        assert_eq!(
            count("SELECT artist_id FROM tracks"),
            1,
            "onto the album owner's spelling"
        );
    }

    #[test]
    fn an_album_both_spellings_hold_becomes_one_album() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            super::create_tables(&conn).unwrap();
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (101, 'Tove Lo'), (102, 'TOVE LO');
                 INSERT INTO albums (id, title, artist_id) VALUES (101, 'Habits', 101), (102, 'Habits', 102), (103, 'Other', 102);
                 INSERT INTO tracks (title, album_id, artist_id, path) VALUES
                   ('a', 101, 101, '/a.flac'), ('b', 102, 102, '/b.flac'), ('c', 103, 102, '/c.flac');
                 PRAGMA user_version = 8;",
            )
            .unwrap();
        }
        let conn = rusqlite::Connection::open(&path).unwrap();
        super::create_tables(&conn).expect("opens despite the clash");
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT COUNT(*) FROM artists WHERE lower(name) = 'tove lo'"),
            1
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM albums WHERE title IN ('Habits', 'Other')"),
            2
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM tracks WHERE album_id = 102"),
            2,
            "Habits once, holding both copies' tracks"
        );
    }

    #[test]
    fn deleted_ids_are_logged_for_caches() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (7, 'A');
             INSERT INTO albums (id, title, artist_id) VALUES (9, 'B', 7);
             DELETE FROM albums WHERE id = 9;",
        )
        .unwrap();
        let logged: (String, i64) = conn
            .query_row("SELECT kind, id FROM art_evictions", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(logged, ("album".to_string(), 9));
    }

    #[test]
    fn empty_musicbrainz_ids_are_cleared_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            super::create_tables(&conn).unwrap();
            conn.execute("INSERT INTO artists (name, mbid) VALUES ('Crass', '')", [])
                .unwrap();
            conn.execute(
                "INSERT INTO artist_info (artist_id, fetched_at)
                   SELECT id, 1 FROM artists WHERE name = 'Crass'",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 8).unwrap();
        }
        let conn = rusqlite::Connection::open(&path).unwrap();
        super::create_tables(&conn).unwrap();
        let mbid: Option<String> = conn
            .query_row("SELECT mbid FROM artists WHERE name = 'Crass'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(mbid, None);
        let misses: i64 = conn
            .query_row("SELECT COUNT(*) FROM artist_info", [], |r| r.get(0))
            .unwrap();
        assert_eq!(misses, 0, "cached misses are dropped");
    }

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
                "every track a user favourited",
                "SELECT id FROM tracks WHERE path IN (SELECT track_path FROM favourites WHERE user_id = ?1)
                 UNION
                 SELECT id FROM tracks WHERE cached_path IN (SELECT track_path FROM favourites WHERE user_id = ?1)
                 UNION
                 SELECT id FROM tracks WHERE remote_url IN (SELECT track_path FROM favourites WHERE user_id = ?1)",
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
            (
                "the albums a genre spans",
                "SELECT DISTINCT album_id FROM tracks WHERE genre = ?1 COLLATE NOCASE",
            ),
            (
                "an artist's similar-artist links, either way round",
                "DELETE FROM similar_artists WHERE artist_id = ?1 OR similar_id = ?1",
            ),
            (
                "organize history repointed to a merged track",
                "UPDATE organize_log SET track_id = ?1 WHERE track_id = ?2",
            ),
            (
                "shares repointed to a merged track",
                "UPDATE share_tracks SET track_id = ?1 WHERE track_id = ?2",
            ),
            (
                "favourites repointed to a merged track's path",
                "UPDATE OR IGNORE favourites SET track_path = ?1 WHERE track_path = ?2",
            ),
            (
                "a track's last play",
                "SELECT MAX(played_at) FROM play_history WHERE track_id = ?1",
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
                      (3, 'Neither', 1, NULL);
             INSERT INTO tracks (title, album_id, artist_id, path)
               VALUES ('a', 1, 1, '/a'), ('b', 2, 1, '/b'), ('c', 3, 1, '/c');
             PRAGMA user_version = 8;",
        )
        .unwrap();

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
                '2026-01-01 10:00:00');
             PRAGMA user_version = 8;",
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

    /// Opening a database already at this version must not need the write
    /// lock: every background task opens one, and each would otherwise wait
    /// behind whatever scan or sync holds it.
    #[test]
    fn opening_a_current_database_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        Database::open(&path).unwrap();

        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();

        let conn = Connection::open(&path).unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(50))
            .unwrap();
        create_tables(&conn).expect("opened while another connection holds the write lock");

        // The whole open, pragmas and all, against its usual 30 s timeout.
        let started = std::time::Instant::now();
        Database::open(&path).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn the_sweeps_wait_for_an_older_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let conn = Connection::open(&path).unwrap();
        create_tables(&conn).unwrap();
        conn.execute("INSERT INTO artists (name, mbid) VALUES ('Crass', '')", [])
            .unwrap();
        let mbid = |conn: &Connection| -> Option<String> {
            conn.query_row("SELECT mbid FROM artists", [], |r| r.get(0))
                .unwrap()
        };

        create_tables(&conn).unwrap();
        assert_eq!(mbid(&conn).as_deref(), Some(""), "current: left alone");

        conn.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .unwrap();
        create_tables(&conn).unwrap();
        assert_eq!(mbid(&conn), None, "older: swept");
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn the_saved_position_moves_to_its_own_table() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn.execute_batch(
            "DROP TABLE playback_position;
             INSERT INTO playback_state
                 (id, queue_json, cursor_id, position_ms, was_playing, radio_enabled)
             VALUES (1, '[]', '/music/a.flac', 61000, 1, 1);
             PRAGMA user_version = 8;",
        )
        .unwrap();

        create_tables(&conn).unwrap();

        let moved: (String, i64, bool, bool) = conn
            .query_row(
                "SELECT cursor_id, position_ms, was_playing, radio_enabled
                   FROM playback_position WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(moved, ("/music/a.flac".into(), 61000, true, true));
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
    fn synced_playlists_are_tied_to_the_server_last_synced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        {
            let db = Database::open(&path).unwrap();
            db.conn
                .execute_batch(
                    "INSERT INTO remote_servers (url, username, last_sync)
                       VALUES ('https://old/', 'ann', 1), ('https://new', 'bob', 2);
                     INSERT INTO playlists (name, remote_id) VALUES ('Synced', 'p1');
                     INSERT INTO playlists (name) VALUES ('Local');
                     ALTER TABLE playlists DROP COLUMN remote_account;",
                )
                .unwrap();
        }

        let db = Database::open(&path).unwrap();
        let accounts: Vec<Option<String>> = db
            .conn
            .prepare("SELECT remote_account FROM playlists ORDER BY name DESC")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(accounts, [Some("bob@https://new".into()), None]);
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
                     ALTER TABLE similar_artists DROP COLUMN relationship;
                     ALTER TABLE shares DROP COLUMN kind;
                     ALTER TABLE shares DROP COLUMN subject_id;
                     ALTER TABLE shares DROP COLUMN start_track_id;",
                )
                .unwrap();
        }

        let db = Database::open(&path).unwrap();
        for (table, column) in [
            ("tracks", "cache_size_bytes"),
            ("tracks", "cache_download_date"),
            ("similar_artists", "relationship"),
            ("shares", "kind"),
            ("shares", "subject_id"),
            ("shares", "start_track_id"),
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
    fn rows_from_before_uids_are_given_one_and_new_rows_get_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        {
            let db = Database::open(&path).unwrap();
            db.conn
                .execute_batch(
                    "DROP TRIGGER artists_uid; DROP TRIGGER albums_uid; DROP TRIGGER tracks_uid;
                     DROP INDEX idx_artists_uid; DROP INDEX idx_albums_uid; DROP INDEX idx_tracks_uid;
                     ALTER TABLE artists DROP COLUMN uid;
                     ALTER TABLE albums DROP COLUMN uid;
                     ALTER TABLE tracks DROP COLUMN uid;
                     INSERT INTO artists (id, name) VALUES (5, 'Burial');
                     INSERT INTO albums (id, title, artist_id) VALUES (5, 'Untrue', 5);
                     INSERT INTO tracks (id, title, album_id, artist_id, path) VALUES
                         (5, 'Archangel', 5, 5, '/a.flac'), (6, 'Near Dark', 5, 5, '/b.flac');
                     PRAGMA user_version = 6;",
                )
                .unwrap();
        }

        let db = Database::open(&path).unwrap();
        db.conn
            .execute(
                "INSERT INTO tracks (title, album_id, artist_id, path) VALUES ('Ghost Hardware', 5, 5, '/c.flac')",
                [],
            )
            .unwrap();
        let uids: Vec<String> = db
            .conn
            .prepare(
                "SELECT uid FROM artists UNION ALL SELECT uid FROM albums
                 UNION ALL SELECT uid FROM tracks",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(uids.len(), 5);
        for uid in &uids {
            let parsed = uuid::Uuid::parse_str(uid).unwrap();
            assert_eq!(parsed.get_version_num(), 7, "{uid}");
            assert_eq!(parsed.to_string(), *uid, "stored hyphenated and lower case");
        }
        let distinct: std::collections::HashSet<_> = uids.iter().collect();
        assert_eq!(distinct.len(), uids.len());

        // Backfilled in row order.
        let tracks: Vec<String> = db
            .conn
            .prepare("SELECT uid FROM tracks WHERE id IN (5, 6) ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut sorted = tracks.clone();
        sorted.sort();
        assert_eq!(tracks, sorted);
    }

    /// A users table from before `AUTOINCREMENT`: rebuilt with its rows, ids
    /// and the tables that refer to it intact, and a deleted id not reissued.
    #[test]
    fn user_ids_are_never_reused_after_the_rebuild() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        create_tables(&conn).unwrap();
        conn.execute_batch(
            "DROP TRIGGER users_personal_data;
             DROP TABLE users;
             CREATE TABLE users (
                 id            INTEGER PRIMARY KEY,
                 username      TEXT NOT NULL UNIQUE,
                 password_hash TEXT NOT NULL,
                 role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user', 'readonly')),
                 created_at    TEXT DEFAULT (datetime('now')),
                 sealed_password BLOB
             );
             INSERT INTO users (id, username, password_hash, role, sealed_password) VALUES
                 (3, 'mate', 'h', 'user', x'01'), (5, 'owner', 'h', 'admin', NULL);
             INSERT INTO api_keys (user_id, name, key_hash, created_at) VALUES (3, 'k', 'kh', 0);
             INSERT INTO refresh_tokens (id, user_id, expires_at) VALUES ('t', 5, 9999);
             PRAGMA user_version = 8;",
        )
        .unwrap();

        create_tables(&conn).unwrap();

        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'users'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sql.contains("AUTOINCREMENT"), "{sql}");
        let sealed: Vec<u8> = conn
            .query_row("SELECT sealed_password FROM users WHERE id = 3", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(sealed, [1]);
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        // References still resolve, and still cascade.
        conn.execute("DELETE FROM users WHERE id = 3", []).unwrap();
        let keys: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(keys, 0);
        conn.execute("DELETE FROM users WHERE id = 5", []).unwrap();
        let tokens: i64 = conn
            .query_row("SELECT COUNT(*) FROM refresh_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, 0);

        // The highest id, deleted, is not handed out again.
        conn.execute(
            "INSERT INTO users (username, password_hash) VALUES ('new', 'h')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 6);
        assert!(conn.execute_batch("PRAGMA foreign_key_check").is_ok());
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

        apply_migrations(&conn, SCHEMA_VERSION).unwrap();

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
        apply_migrations(&conn, SCHEMA_VERSION).unwrap();
        apply_migrations(&conn, SCHEMA_VERSION).unwrap();
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

    /// A database from before favourites, playlists, history and shares were
    /// per user: every row goes to the first admin, and nothing is lost.
    #[test]
    fn shared_data_from_before_accounts_had_their_own_goes_to_the_first_admin() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        create_tables(&conn).unwrap();
        conn.execute_batch(
            "DROP TRIGGER users_personal_data;
             DROP INDEX idx_play_history_user;
             DROP INDEX idx_playlists_user;
             ALTER TABLE play_history DROP COLUMN user_id;
             ALTER TABLE playlists DROP COLUMN user_id;
             ALTER TABLE shares DROP COLUMN user_id;
             DROP TABLE favourites;
             DROP TABLE favourite_albums;
             DROP TABLE favourite_artists;
             CREATE TABLE favourites (
                 track_path TEXT PRIMARY KEY,
                 created_at TEXT DEFAULT (datetime('now'))
             );
             CREATE TABLE favourite_albums (
                 artist_name TEXT NOT NULL,
                 album_title TEXT NOT NULL,
                 created_at  TEXT DEFAULT (datetime('now')),
                 PRIMARY KEY (artist_name, album_title)
             );
             CREATE TABLE favourite_artists (
                 artist_name TEXT PRIMARY KEY,
                 created_at  TEXT DEFAULT (datetime('now'))
             );
             INSERT INTO users (id, username, password_hash, role) VALUES
                 (3, 'mate', 'h', 'user'), (5, 'owner', 'h', 'admin'), (7, 'late', 'h', 'admin');
             INSERT INTO artists (id, name) VALUES (1, 'A');
             INSERT INTO tracks (id, artist_id, title, source) VALUES (1, 1, 'T', 'local');
             INSERT INTO favourites (track_path) VALUES ('/a.flac'), ('/b.flac');
             INSERT INTO favourite_albums (artist_name, album_title) VALUES ('A', 'B');
             INSERT INTO favourite_artists (artist_name) VALUES ('A');
             INSERT INTO play_history (track_id, played_at) VALUES (1, 100);
             INSERT INTO playlists (name) VALUES ('Mix');
             INSERT INTO shares (id, created_at) VALUES ('s', 0);
             PRAGMA user_version = 6;",
        )
        .unwrap();

        create_tables(&conn).unwrap();

        for (table, rows) in [
            ("favourites", 2),
            ("favourite_albums", 1),
            ("favourite_artists", 1),
            ("play_history", 1),
            ("playlists", 1),
            ("shares", 1),
        ] {
            let owned: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_id = 5"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(owned, rows, "{table} did not go to the first admin");
        }
        // Keyed by user now: the same favourite for someone else is its own row.
        conn.execute(
            "INSERT INTO favourites (user_id, track_path) VALUES (3, '/a.flac')",
            [],
        )
        .unwrap();
    }
}

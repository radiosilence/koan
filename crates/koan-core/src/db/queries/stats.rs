use rusqlite::Connection;

use crate::db::connection::DbError;

use super::LibraryStats;

/// Library statistics broken down by source.
pub fn library_stats(conn: &Connection) -> Result<LibraryStats, DbError> {
    let total_tracks: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
    let local_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'local'",
        [],
        |r| r.get(0),
    )?;
    let remote_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'remote'",
        [],
        |r| r.get(0),
    )?;
    // A downloaded track keeps `source = 'remote'` — where it came from does
    // not change because a copy landed on disk. What changes is `cached_path`,
    // which is what `set_cached_path` writes and `clear_download_cache` nulls.
    // 'cached' is a source the CHECK constraint allows and nothing writes.
    let cached_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE cached_path IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let total_albums: i64 = conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
    let total_artists: i64 = conn.query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?;

    Ok(LibraryStats {
        total_tracks,
        local_tracks,
        remote_tracks,
        cached_tracks,
        total_albums,
        total_artists,
    })
}

/// A number that changes when tracks, albums or artists are added or removed.
///
/// For a process that has no say in the library's writes, which a scan, a sync
/// or another process may make, and so cannot keep a version of its own. It
/// does not see a row edited in place; a caller caching on it bounds how long
/// that may go unnoticed.
pub fn library_fingerprint(conn: &Connection) -> Result<u64, DbError> {
    use std::hash::{Hash, Hasher};
    let counts: [i64; 6] = conn
        .prepare_cached(
            "SELECT (SELECT COUNT(*) FROM tracks), (SELECT COALESCE(MAX(id), 0) FROM tracks),
                    (SELECT COUNT(*) FROM albums), (SELECT COALESCE(MAX(id), 0) FROM albums),
                    (SELECT COUNT(*) FROM artists), (SELECT COALESCE(MAX(id), 0) FROM artists)",
        )?
        .query_row([], |r| {
            Ok([
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ])
        })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    counts.hash(&mut hasher);
    Ok(hasher.finish())
}

static LIBRARY_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Say that this process wrote library rows: what a [`library_stamp`] sees at
/// once, edits in place included.
pub fn library_written() {
    LIBRARY_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// How many times this process has said it wrote library rows.
pub fn library_version() -> u64 {
    LIBRARY_WRITES.load(std::sync::atomic::Ordering::Relaxed)
}

/// How long a [`LibraryStamp`] holds while nothing it can see moves.
const STAMP_WINDOW_SECS: u64 = 300;

/// The library as a cache of its rows keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryStamp {
    /// Changes when this process says it wrote library rows
    /// ([`library_written`]) or when [`library_fingerprint`] moves, which is
    /// how a write by another process is seen.
    pub rows: u64,
    /// The five-minute window the stamp was taken in. An edit in place by
    /// another process moves no fingerprint; the window bounds how long it
    /// goes unseen.
    pub window: u64,
}

impl LibraryStamp {
    /// Both halves as one number.
    pub fn key(self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (self.rows, self.window).hash(&mut hasher);
        hasher.finish()
    }
}

/// The library as of now, for a cache of its rows.
///
/// Constant time while nothing has been committed: the fingerprint, which
/// counts rows, is read again on a connection only when its
/// `PRAGMA data_version` or its own `total_changes` has moved since that
/// connection last asked. A play recorded or a position saved moves those too,
/// so the fingerprint is then read again, and the stamp holds.
pub fn library_stamp(conn: &Connection) -> Result<LibraryStamp, DbError> {
    use std::hash::{Hash, Hasher};
    let seen = Commits::of(conn)?;
    let held = FINGERPRINTS
        .lock()
        .iter()
        .find(|(c, _)| c.connection == seen.connection)
        .copied();
    let fingerprint = match held {
        Some((c, fingerprint)) if c == seen => fingerprint,
        _ => {
            let fingerprint = library_fingerprint(conn)?;
            let mut held = FINGERPRINTS.lock();
            held.retain(|(c, _)| c.connection != seen.connection);
            // Bounded: a pool keeps a handful, and connections come and go.
            if held.len() >= 32 {
                held.remove(0);
            }
            held.push((seen, fingerprint));
            fingerprint
        }
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (library_version(), fingerprint).hash(&mut hasher);
    Ok(LibraryStamp {
        rows: hasher.finish(),
        window: crate::clock::unix_secs() as u64 / STAMP_WINDOW_SECS,
    })
}

/// Which connection, and the commits it has seen. Equal only when nothing
/// has been committed in between: `data_version` moves when another
/// connection commits, `total_changes` when this one writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Commits {
    connection: i64,
    data_version: i64,
    changes: u64,
}

impl Commits {
    fn of(conn: &Connection) -> Result<Self, DbError> {
        // A random id in a temporary table, which lives and dies with the
        // connection: an address could be reused by the next one opened.
        conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS koan_connection (id INTEGER NOT NULL);
             INSERT INTO koan_connection
                SELECT random() WHERE NOT EXISTS (SELECT 1 FROM koan_connection);",
        )?;
        let connection = conn
            .prepare_cached("SELECT id FROM koan_connection")?
            .query_row([], |r| r.get(0))?;
        let data_version = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        Ok(Self {
            connection,
            data_version,
            changes: conn.total_changes(),
        })
    }
}

/// Each connection's last [`Commits`] and the fingerprint read then.
static FINGERPRINTS: parking_lot::Mutex<Vec<(Commits, u64)>> = parking_lot::Mutex::new(Vec::new());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_library_stats() {
        let db = test_db();
        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 0);
        assert_eq!(stats.total_albums, 0);
        assert_eq!(stats.total_artists, 0);

        upsert_track(&db.conn, &sample_meta("T1", "A1", "Al1")).unwrap();
        upsert_track(&db.conn, &sample_meta("T2", "A2", "Al2")).unwrap();

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 2);
        assert_eq!(stats.local_tracks, 2);
        assert_eq!(stats.total_albums, 2);
        assert_eq!(stats.total_artists, 2);
    }

    /// Downloading does not change where a track came from, so counting
    /// `source = 'cached'` counted nothing — a library with a full cache
    /// reported "0 of 47,944 cached" and always would have.
    #[test]
    fn a_downloaded_track_counts_as_cached_and_stays_remote() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.remote_tracks, 1);
        assert_eq!(stats.cached_tracks, 0, "nothing downloaded yet");

        let file = std::env::temp_dir().join("koan-stats-cached.mp3");
        std::fs::write(&file, b"not really audio").unwrap();
        super::super::set_cached_path(&db.conn, id, &file.to_string_lossy()).unwrap();

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.cached_tracks, 1);
        assert_eq!(
            stats.remote_tracks, 1,
            "a cached copy does not change where the track came from"
        );

        super::super::clear_cached_paths(&db.conn).unwrap();
        assert_eq!(library_stats(&db.conn).unwrap().cached_tracks, 0);
        let _ = std::fs::remove_file(&file);
    }

    /// The stamp holds while nothing is written, moves when this process says
    /// it edited rows in place, and moves when another connection adds one.
    #[test]
    fn the_stamp_sees_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = Database::open(&path).unwrap();
        upsert_track(&db.conn, &sample_meta("One", "Ayla", "Moss")).unwrap();
        let first = library_stamp(&db.conn).unwrap();
        assert_eq!(library_stamp(&db.conn).unwrap().rows, first.rows);

        db.conn
            .execute("UPDATE albums SET title = 'Lichen'", [])
            .unwrap();
        library_written();
        let edited = library_stamp(&db.conn).unwrap();
        assert_ne!(edited.rows, first.rows);

        let other = Database::open(&path).unwrap();
        upsert_track(&other.conn, &sample_meta("Two", "Bryn", "Tide")).unwrap();
        assert_ne!(library_stamp(&db.conn).unwrap().rows, edited.rows);
    }
}

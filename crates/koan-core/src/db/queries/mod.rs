mod albums;
pub mod api_keys;
pub mod app_passwords;
mod artists;
pub mod auth;
pub mod batch;
mod favourites;
pub mod history;
pub mod lyrics;
pub mod playback_state;
pub mod playlists;
mod scan_cache;
mod search;
pub mod shares;
pub(crate) mod sources;
mod stats;
pub mod tracks;
pub mod uids;

use std::path::PathBuf;

// Re-exported so callers can `use queries::*`.
pub use albums::*;
pub use artists::*;
pub use auth::LOCAL_USER;
pub use batch::*;
pub use favourites::*;
pub use history::*;
pub use lyrics::*;
pub use playback_state::*;
pub use playlists::*;
pub use scan_cache::*;
pub use search::*;
pub use stats::*;
pub use tracks::*;
pub use uids::*;

/// A write transaction that holds the write lock from its first statement.
///
/// What `Connection::unchecked_transaction` gives is deferred: it takes the
/// lock at its first write, and if another connection wrote since its first
/// read, SQLite refuses that upgrade outright rather than waiting, since
/// waiting could deadlock. A scan chunk or a sync page that met another writer
/// therefore failed every statement in it. Immediate waits for the lock the way
/// any other statement does.
pub fn write_transaction(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<rusqlite::Transaction<'_>> {
    rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
}

/// Run `f` as one unit: its writes land together or not at all, readers never
/// see them half done, and the write lock is taken once rather than per
/// statement.
///
/// Inside a caller's transaction it is a savepoint, so it nests. Outside one it
/// begins `IMMEDIATE`: a deferred transaction that reads before it writes
/// fails outright, without waiting, when another connection wrote in between.
pub fn atomically<T, E: From<rusqlite::Error>>(
    conn: &rusqlite::Connection,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let (begin, commit, rollback) = if conn.is_autocommit() {
        ("BEGIN IMMEDIATE", "COMMIT", "ROLLBACK")
    } else {
        (
            "SAVEPOINT atomically",
            "RELEASE atomically",
            "ROLLBACK TO atomically; RELEASE atomically",
        )
    };
    conn.execute_batch(begin)?;
    let result = f();
    match &result {
        Ok(_) => conn.execute_batch(commit)?,
        Err(_) => conn.execute_batch(rollback)?,
    }
    result
}

/// A list as one JSON array, for `IN (SELECT value FROM json_each(?))`.
///
/// One placeholder per item stops at SQLite's limit of 32,766 parameters, and
/// a queue or a playlist may be longer than that. One parameter is not limited.
pub fn json_list<T: serde::Serialize>(items: &[T]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".into())
}

/// The half-open range of paths under a folder, for `path >= .0 AND path < .1`.
///
/// A prefix match on an indexed column, rather than `LIKE 'folder/%'` — which
/// SQLite answers by reading every row, because a pattern is opaque to an
/// index until it has been evaluated. It also takes the pattern out of the
/// path: `LIKE` reads `_` as "any character" and folds ASCII case, so
/// `/Volumes/My_Music` would match `/Volumes/My Music` and `/volumes/my_music`
/// alike.
///
/// The trailing separator is what keeps `/Volumes/Music` out of
/// `/Volumes/Music Backup`; the upper bound is the highest code point, so
/// every path under the folder sorts below it.
pub fn folder_prefix_range(folder: &std::path::Path) -> (String, String) {
    let prefix = format!(
        "{}{}",
        folder
            .to_string_lossy()
            .trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );
    let upper = format!("{prefix}\u{10FFFF}");
    (prefix, upper)
}

// --- Row types ---

#[derive(Debug, Clone)]
pub struct ArtistRow {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub remote_id: Option<String>,
    /// Albums credited to this artist, and tracks across them. Aggregated in
    /// the same query as the row itself — a count per artist would be one
    /// query per row in a list thousands long.
    pub album_count: i64,
    pub track_count: i64,
}

#[derive(Debug, Clone)]
pub struct AlbumRow {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: String,
    pub date: Option<String>,
    pub total_discs: Option<i32>,
    pub total_tracks: Option<i32>,
    pub codec: Option<String>,
    pub label: Option<String>,
    pub remote_id: Option<String>,
    /// When the album entered the library — the server's `created` for remote
    /// albums, otherwise the time it was first indexed.
    pub added_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackRow {
    pub id: i64,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub artist_name: String,
    pub album_artist_name: String,
    pub album_title: String,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub title: String,
    pub duration_ms: Option<i64>,
    pub path: Option<String>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub genre: Option<String>,
    pub source: String,
    pub remote_id: Option<String>,
    pub cached_path: Option<String>,
}

/// Where to get audio data for playback. Local always wins.
#[derive(Debug, Clone)]
pub enum PlaybackSource {
    Local(PathBuf),
    Cached(PathBuf),
    Remote(String),
}

#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
}

/// Metadata for inserting/updating a track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    pub album: String,
    pub date: Option<String>,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub size_bytes: Option<i64>,
    pub mtime: Option<i64>,
    pub path: Option<String>,
    pub source: String,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
    /// The server's ids for the album and its artist.
    ///
    /// Carried alongside the track's own, because the server keys stars,
    /// shares and cover art off them — a library synced without these has
    /// albums and artists it can name but cannot refer to.
    pub album_remote_id: Option<String>,
    pub artist_remote_id: Option<String>,
    /// MusicBrainz recording and release ids — `MUSICBRAINZ_TRACKID` and
    /// `MUSICBRAINZ_ALBUMID` in a file's tags, `musicBrainzId` on a server's
    /// song and album. Together they name one track whatever each source calls
    /// the album; the recording alone recurs on every compilation it is on.
    pub mbid: Option<String>,
    pub album_mbid: Option<String>,
    /// When the album this track belongs to entered the library. Remote sync
    /// supplies the server's `created`; anything else leaves it and the album
    /// is stamped with the time it was first seen.
    pub album_added_at: Option<String>,
}

/// Test helper: build a sample TrackMeta for use in tests across sub-modules.
#[cfg(test)]
pub fn sample_meta(title: &str, artist: &str, album: &str) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: artist.into(),
        album_artist: Some(artist.into()),
        album: album.into(),
        date: Some("2024".into()),
        disc: Some(1),
        track_number: Some(1),
        genre: Some("Electronic".into()),
        label: None,
        duration_ms: Some(240_000),
        codec: Some("FLAC".into()),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(2),
        bitrate: Some(1000),
        size_bytes: Some(30_000_000),
        mtime: Some(1700000000),
        path: Some(format!("/music/{}/{}.flac", album, title)),
        source: "local".into(),
        remote_id: None,
        album_remote_id: None,
        artist_remote_id: None,
        mbid: None,
        album_mbid: None,
        remote_url: None,
        album_added_at: None,
    }
}

#[cfg(test)]
mod write_transaction_tests {
    use std::time::Duration;

    fn open(path: &std::path::Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "wal").unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn
    }

    /// A deferred transaction that read before another connection committed
    /// cannot write at all, however long the busy timeout; `write_transaction`
    /// waits its turn instead. This is what a sync page meeting another writer
    /// ran into.
    #[test]
    fn a_write_transaction_waits_where_a_deferred_one_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        open(&path)
            .execute_batch("CREATE TABLE t (x INTEGER)")
            .unwrap();

        let deferred = open(&path);
        let tx = deferred.unchecked_transaction().unwrap();
        let _: i64 = tx
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        open(&path).execute("INSERT INTO t VALUES (1)", []).unwrap();
        let err = tx.execute("INSERT INTO t VALUES (2)", []).unwrap_err();
        assert!(
            err.to_string().contains("locked") || err.to_string().contains("busy"),
            "{err}"
        );
        drop(tx);

        let path_for_writer = path.clone();
        let writer = std::thread::spawn(move || {
            let conn = open(&path_for_writer);
            conn.execute_batch("BEGIN IMMEDIATE; INSERT INTO t VALUES (3)")
                .unwrap();
            std::thread::sleep(Duration::from_millis(200));
            conn.execute_batch("COMMIT").unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        let conn = open(&path);
        let tx = super::write_transaction(&conn).unwrap();
        let _: i64 = tx
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        tx.execute("INSERT INTO t VALUES (4)", []).unwrap();
        tx.commit().unwrap();
        writer.join().unwrap();

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }
}

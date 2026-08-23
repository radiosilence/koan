use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::TrackRow;

/// Where a play came from. `local` is koan playing the track itself; `subsonic`
/// is another client scrobbling to koan's own Subsonic endpoint.
pub const SOURCE_LOCAL: &str = "local";
pub const SOURCE_SUBSONIC: &str = "subsonic";

/// Record a play at an explicit time.
///
/// `listened_ms` is how long the track was actually listened to, not how long
/// the track is — a play recorded four minutes into a twenty-minute piece
/// stores four minutes.
pub fn record_play_at(
    conn: &Connection,
    track_id: i64,
    played_at: i64,
    listened_ms: Option<i64>,
    source: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO play_history (track_id, played_at, duration_ms, source)
         VALUES (?1, ?2, ?3, ?4)",
        params![track_id, played_at, listened_ms, source],
    )?;
    Ok(())
}

/// Record a play that happened just now.
pub fn record_play(
    conn: &Connection,
    track_id: i64,
    listened_ms: Option<i64>,
) -> Result<(), DbError> {
    record_play_at(conn, track_id, now_secs(), listened_ms, SOURCE_LOCAL)
}

/// Get the last play timestamp for a track, or None if never played.
pub fn last_played_at(conn: &Connection, track_id: i64) -> Result<Option<i64>, DbError> {
    let result = conn.query_row(
        "SELECT MAX(played_at) FROM play_history WHERE track_id = ?1",
        params![track_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(result)
}

/// Get track IDs from recent play history (most recent first), up to `limit`.
pub fn recent_track_ids(conn: &Connection, limit: usize) -> Result<Vec<i64>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT track_id FROM play_history
         ORDER BY played_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(rows)
}

/// Get play count for a track.
pub fn play_count(conn: &Connection, track_id: i64) -> Result<i64, DbError> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM play_history WHERE track_id = ?1",
        params![track_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// A play history entry with full track info.
#[derive(Debug, Clone)]
pub struct PlayHistoryEntry {
    pub track_id: i64,
    pub played_at: i64,
    pub duration_ms: Option<i64>,
}

/// Get recent play history entries (most recent first).
pub fn get_play_history(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<PlayHistoryEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT track_id, played_at, duration_ms FROM play_history
         ORDER BY played_at DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(PlayHistoryEntry {
                track_id: row.get(0)?,
                played_at: row.get(1)?,
                duration_ms: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A play history entry joined to the track it played.
///
/// History is a list of events, not of tracks: the same track played three
/// times is three rows, so this cannot be deduplicated into a track list.
#[derive(Debug, Clone)]
pub struct PlayHistoryRow {
    pub track: TrackRow,
    pub played_at: i64,
    pub listened_ms: Option<i64>,
    pub source: String,
}

/// Recent plays with their tracks, most recent first.
///
/// Joined rather than looked up per entry, and inner-joined so an entry whose
/// track has left the library simply does not appear.
///
/// Track columns come first so `row_to_track_row` reads them at the offsets it
/// always does; the history columns follow.
pub fn play_history_with_tracks(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<PlayHistoryRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path,
                h.played_at, h.duration_ms, COALESCE(h.source, 'local')
         FROM play_history h
         JOIN tracks t ON t.id = h.track_id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         ORDER BY h.played_at DESC, h.id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(PlayHistoryRow {
                track: super::row_to_track_row(row)?,
                played_at: row.get(20)?,
                listened_ms: row.get(21)?,
                source: row.get(22)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Delete every play history entry. Returns how many were removed.
pub fn clear_play_history(conn: &Connection) -> Result<usize, DbError> {
    Ok(conn.execute("DELETE FROM play_history", [])?)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

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

    fn seed_track(db: &Database, title: &str) -> i64 {
        let mut meta = sample_meta(title, "Artist1", "Album1");
        meta.path = Some(format!("/music/{title}.flac"));
        upsert_track(&db.conn, &meta).unwrap();
        db.conn
            .query_row(
                "SELECT id FROM tracks WHERE title = ?1",
                params![title],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn test_record_and_query_play_history() {
        let db = test_db();
        let track_id = seed_track(&db, "Track1");

        // No plays yet.
        assert_eq!(play_count(&db.conn, track_id).unwrap(), 0);
        assert!(last_played_at(&db.conn, track_id).unwrap().is_none());
        assert!(recent_track_ids(&db.conn, 10).unwrap().is_empty());

        // Record a play.
        record_play(&db.conn, track_id, Some(240_000)).unwrap();
        assert_eq!(play_count(&db.conn, track_id).unwrap(), 1);
        assert!(last_played_at(&db.conn, track_id).unwrap().is_some());

        let recent = recent_track_ids(&db.conn, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0], track_id);

        // Record another play.
        record_play(&db.conn, track_id, Some(240_000)).unwrap();
        assert_eq!(play_count(&db.conn, track_id).unwrap(), 2);
        // Still only 1 distinct track.
        assert_eq!(recent_track_ids(&db.conn, 10).unwrap().len(), 1);
    }

    #[test]
    fn history_is_a_list_of_events_not_of_tracks() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let b = seed_track(&db, "B");

        record_play_at(&db.conn, a, 100, Some(1000), SOURCE_LOCAL).unwrap();
        record_play_at(&db.conn, b, 200, None, SOURCE_SUBSONIC).unwrap();
        record_play_at(&db.conn, a, 300, Some(2000), SOURCE_LOCAL).unwrap();

        let rows = play_history_with_tracks(&db.conn, 10, 0).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.track.title.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "A"],
            "most recent first, and the same track appears once per play"
        );
        assert_eq!(rows[0].played_at, 300);
        assert_eq!(rows[0].listened_ms, Some(2000));
        assert_eq!(rows[1].source, SOURCE_SUBSONIC);
        assert_eq!(rows[1].listened_ms, None);
        assert_eq!(rows[0].track.artist_name, "Artist1");
    }

    #[test]
    fn history_paginates() {
        let db = test_db();
        let id = seed_track(&db, "A");
        for at in 0..5 {
            record_play_at(&db.conn, id, at, None, SOURCE_LOCAL).unwrap();
        }
        assert_eq!(play_history_with_tracks(&db.conn, 2, 0).unwrap().len(), 2);
        assert_eq!(play_history_with_tracks(&db.conn, 2, 4).unwrap().len(), 1);
        assert_eq!(play_history_with_tracks(&db.conn, 10, 5).unwrap().len(), 0);
    }

    #[test]
    fn plays_within_the_same_second_keep_their_order() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let b = seed_track(&db, "B");
        // played_at has one-second resolution, so a short track and its
        // successor can share a timestamp. Insertion order breaks the tie.
        record_play_at(&db.conn, a, 42, None, SOURCE_LOCAL).unwrap();
        record_play_at(&db.conn, b, 42, None, SOURCE_LOCAL).unwrap();

        let rows = play_history_with_tracks(&db.conn, 10, 0).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.track.title.as_str())
                .collect::<Vec<_>>(),
            ["B", "A"]
        );
    }

    #[test]
    fn deleting_a_track_takes_its_history_with_it() {
        let db = test_db();
        let id = seed_track(&db, "A");
        record_play(&db.conn, id, None).unwrap();

        db.conn
            .execute("DELETE FROM tracks WHERE id = ?1", params![id])
            .expect("a track with play history must still be deletable");

        assert_eq!(play_count(&db.conn, id).unwrap(), 0);
        assert!(
            play_history_with_tracks(&db.conn, 10, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn clearing_removes_everything() {
        let db = test_db();
        let id = seed_track(&db, "A");
        record_play(&db.conn, id, None).unwrap();
        record_play(&db.conn, id, None).unwrap();

        assert_eq!(clear_play_history(&db.conn).unwrap(), 2);
        assert_eq!(play_count(&db.conn, id).unwrap(), 0);
    }
}

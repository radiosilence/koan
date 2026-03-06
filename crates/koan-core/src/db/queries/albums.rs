use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::AlbumRow;

/// Get or create an album by title + artist. Returns the album ID.
#[allow(clippy::too_many_arguments)]
pub fn get_or_create_album(
    conn: &Connection,
    title: &str,
    artist_id: i64,
    date: Option<&str>,
    total_discs: Option<i32>,
    total_tracks: Option<i32>,
    codec: Option<&str>,
    label: Option<&str>,
    remote_id: Option<&str>,
) -> Result<i64, DbError> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM albums WHERE title = ?1 AND artist_id = ?2",
            params![title, artist_id],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        // Update mutable fields so rescans pick up format upgrades (e.g. MP3→FLAC),
        // corrected dates, or newly-added remote IDs.
        conn.execute(
            "UPDATE albums SET
                codec      = COALESCE(?1, codec),
                date       = COALESCE(?2, date),
                label      = COALESCE(?3, label),
                remote_id  = COALESCE(?4, remote_id)
             WHERE id = ?5",
            params![codec, date, label, remote_id, id],
        )?;
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO albums (title, artist_id, date, total_discs, total_tracks, codec, label, remote_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![title, artist_id, date, total_discs, total_tracks, codec, label, remote_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get albums for a specific artist, ordered chronologically.
pub fn albums_for_artist(conn: &Connection, artist_id: i64) -> Result<Vec<AlbumRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         WHERE al.artist_id = ?1
         ORDER BY al.date, al.title",
    )?;
    let rows = stmt
        .query_map(params![artist_id], |row| {
            Ok(AlbumRow {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                date: row.get(4)?,
                total_discs: row.get(5)?,
                total_tracks: row.get(6)?,
                codec: row.get(7)?,
                label: row.get(8)?,
                remote_id: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get the date string for an album by ID.
pub fn album_date(conn: &Connection, album_id: i64) -> Result<Option<String>, DbError> {
    Ok(conn
        .query_row(
            "SELECT date FROM albums WHERE id = ?1",
            params![album_id],
            |row| row.get(0),
        )
        .ok()
        .flatten())
}

/// Get all albums with their artist name, sorted.
pub fn all_albums(conn: &Connection) -> Result<Vec<AlbumRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         ORDER BY a.name, al.date, al.title",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok(AlbumRow {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                date: row.get(4)?,
                total_discs: row.get(5)?,
                total_tracks: row.get(6)?,
                codec: row.get(7)?,
                label: row.get(8)?,
                remote_id: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::get_or_create_artist;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_album_create_and_dedup() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "Boards of Canada", None).unwrap();
        let a1 = get_or_create_album(
            &db.conn,
            "Music Has the Right to Children",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
        )
        .unwrap();
        let a2 = get_or_create_album(
            &db.conn,
            "Music Has the Right to Children",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
        )
        .unwrap();
        assert_eq!(a1, a2);
    }

    #[test]
    fn test_album_codec_updated_on_format_upgrade() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "WAGDUG FUTURISTIC UNITY", None).unwrap();

        // First scan: album indexed as MP3.
        let id1 = get_or_create_album(
            &db.conn,
            "HAKAI",
            artist,
            Some("2008"),
            None,
            None,
            Some("MP3"),
            None,
            None,
        )
        .unwrap();

        let codec: Option<String> = db
            .conn
            .query_row(
                "SELECT codec FROM albums WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(codec.as_deref(), Some("MP3"));

        // Re-scan after upgrading MP3→FLAC: same album, new codec.
        let id2 = get_or_create_album(
            &db.conn,
            "HAKAI",
            artist,
            Some("2008"),
            None,
            None,
            Some("FLAC"),
            None,
            None,
        )
        .unwrap();

        assert_eq!(id1, id2, "should return the same album ID");

        let codec: Option<String> = db
            .conn
            .query_row(
                "SELECT codec FROM albums WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            codec.as_deref(),
            Some("FLAC"),
            "album codec should be updated after format upgrade"
        );
    }

    #[test]
    fn test_album_codec_not_nulled_by_missing_codec() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "Boards of Canada", None).unwrap();

        // First scan with codec.
        let id = get_or_create_album(
            &db.conn,
            "MHTRTC",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
        )
        .unwrap();

        // Re-encounter with no codec (e.g. remote sync without codec info).
        get_or_create_album(
            &db.conn,
            "MHTRTC",
            artist,
            Some("1998"),
            None,
            None,
            None, // no codec
            None, // no label
            None,
        )
        .unwrap();

        let (codec, label): (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT codec, label FROM albums WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            codec.as_deref(),
            Some("FLAC"),
            "codec should not be nulled by a None value"
        );
        assert_eq!(
            label.as_deref(),
            Some("Warp"),
            "label should not be nulled by a None value"
        );
    }

    #[test]
    fn test_all_albums_and_tracks() {
        use crate::db::queries::{sample_meta, tracks_for_album, upsert_track};

        let db = test_db();
        let mut m1 = sample_meta("Track1", "Artist1", "Album1");
        m1.track_number = Some(1);
        let mut m2 = sample_meta("Track2", "Artist1", "Album1");
        m2.track_number = Some(2);
        m2.path = Some("/music/Album1/Track2.flac".into());
        upsert_track(&db.conn, &m1).unwrap();
        upsert_track(&db.conn, &m2).unwrap();

        let albums = all_albums(&db.conn).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Album1");

        let tracks = tracks_for_album(&db.conn, albums[0].id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].track_number, Some(1));
        assert_eq!(tracks[1].track_number, Some(2));
    }
}

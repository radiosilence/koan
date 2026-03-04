use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::TrackRow;

/// Sanitize a user query for FTS5 MATCH: escapes double-quotes and wraps in
/// a quoted phrase so FTS5 special characters are treated as literals.
fn sanitize_fts_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Escape double quotes for FTS5 literal matching
    let escaped = trimmed.replace('"', "\"\"");
    format!("\"{}\"*", escaped)
}

/// Full-text search across track title, artist, album, genre.
pub fn search_tracks(conn: &Connection, query: &str) -> Result<Vec<TrackRow>, DbError> {
    // FTS5 query — sanitize input and append * for prefix matching.
    let fts_query = sanitize_fts_query(query);

    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks_fts f
         JOIN tracks t ON t.id = f.rowid
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE tracks_fts MATCH ?1
         ORDER BY a.name, al.date, al.title, t.disc, t.track_number
         LIMIT 100",
    )?;

    let rows = stmt
        .query_map(params![fts_query], |row| {
            let artist_name: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
            Ok(TrackRow {
                id: row.get(0)?,
                album_id: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: artist_name.clone(),
                album_artist_name: row.get::<_, Option<String>>(4)?.unwrap_or(artist_name),
                album_title: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                disc: row.get(6)?,
                track_number: row.get(7)?,
                title: row.get(8)?,
                duration_ms: row.get(9)?,
                path: row.get(10)?,
                codec: row.get(11)?,
                sample_rate: row.get(12)?,
                bit_depth: row.get(13)?,
                channels: row.get(14)?,
                bitrate: row.get(15)?,
                genre: row.get(16)?,
                source: row.get(17)?,
                remote_id: row.get(18)?,
                cached_path: row.get(19)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
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

    #[test]
    fn test_search_fts() {
        let db = test_db();
        upsert_track(&db.conn, &sample_meta("Vordhosbn", "Aphex Twin", "Drukqs")).unwrap();
        upsert_track(
            &db.conn,
            &sample_meta("Roygbiv", "Boards of Canada", "MHTRTC"),
        )
        .unwrap();
        upsert_track(
            &db.conn,
            &sample_meta("Tha", "Aphex Twin", "Selected Ambient Works"),
        )
        .unwrap();

        // Search by artist.
        let results = search_tracks(&db.conn, "Aphex").unwrap();
        assert_eq!(results.len(), 2);

        // Search by title.
        let results = search_tracks(&db.conn, "Roygbiv").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Roygbiv");

        // Search by album.
        let results = search_tracks(&db.conn, "Drukqs").unwrap();
        assert_eq!(results.len(), 1);
    }
}

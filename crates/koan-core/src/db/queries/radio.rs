use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::ArtistRow;

/// A similar artist entry with relationship metadata.
#[derive(Debug, Clone)]
pub struct SimilarArtistEntry {
    pub artist: ArtistRow,
    pub score: f64,
    pub source: String,
    pub relationship: String,
}

/// Cache similar artist relationships for a specific source.
/// Clears existing entries for the given (artist_id, source), then inserts the new set.
pub fn save_similar_artists(
    conn: &Connection,
    artist_id: i64,
    similar: &[(i64, f64)],
    source: &str,
) -> Result<(), DbError> {
    save_similar_artists_with_rel(conn, artist_id, similar, source, "similar")
}

/// Cache similar artist relationships with an explicit relationship type.
pub fn save_similar_artists_with_rel(
    conn: &Connection,
    artist_id: i64,
    similar: &[(i64, f64)],
    source: &str,
    relationship: &str,
) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM similar_artists WHERE artist_id = ?1 AND source = ?2",
        params![artist_id, source],
    )?;

    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO similar_artists (artist_id, similar_id, score, source, relationship)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    for &(similar_id, score) in similar {
        stmt.execute(params![artist_id, similar_id, score, source, relationship])?;
    }

    Ok(())
}

/// Load cached similar artists for a given artist (all sources merged, best score wins).
pub fn get_similar_artists(
    conn: &Connection,
    artist_id: i64,
) -> Result<Vec<(ArtistRow, f64)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name, a.sort_name, a.remote_id, MAX(sa.score) as best_score
         FROM similar_artists sa
         JOIN artists a ON a.id = sa.similar_id
         WHERE sa.artist_id = ?1
         GROUP BY a.id
         ORDER BY best_score DESC",
    )?;

    let rows = stmt
        .query_map(params![artist_id], |row| {
            Ok((
                ArtistRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort_name: row.get(2)?,
                    remote_id: row.get(3)?,
                    album_count: 0,
                    track_count: 0,
                },
                row.get::<_, f64>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Load similar artists with full metadata (source, relationship type).
pub fn get_similar_artists_detailed(
    conn: &Connection,
    artist_id: i64,
) -> Result<Vec<SimilarArtistEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name, a.sort_name, a.remote_id, sa.score, sa.source, sa.relationship
         FROM similar_artists sa
         JOIN artists a ON a.id = sa.similar_id
         WHERE sa.artist_id = ?1
         ORDER BY sa.score DESC",
    )?;

    let rows = stmt
        .query_map(params![artist_id], |row| {
            Ok(SimilarArtistEntry {
                artist: ArtistRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort_name: row.get(2)?,
                    remote_id: row.get(3)?,
                    album_count: 0,
                    track_count: 0,
                },
                score: row.get(4)?,
                source: row.get(5)?,
                relationship: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Check if we have cached similar artists for a given artist from a specific source
/// (and cache isn't stale). Consider cache stale after 7 days.
pub fn has_fresh_similar_artists(conn: &Connection, artist_id: i64) -> Result<bool, DbError> {
    has_fresh_similar_artists_for_source(conn, artist_id, None)
}

/// Check freshness for a specific source, or any source if `source` is None.
pub fn has_fresh_similar_artists_for_source(
    conn: &Connection,
    artist_id: i64,
    source: Option<&str>,
) -> Result<bool, DbError> {
    let count: i64 = if let Some(src) = source {
        conn.query_row(
            "SELECT COUNT(*) FROM similar_artists
             WHERE artist_id = ?1 AND source = ?2
               AND datetime(updated_at) > datetime('now', '-7 days')",
            params![artist_id, src],
            |row| row.get(0),
        )
        .unwrap_or(0)
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM similar_artists
             WHERE artist_id = ?1
               AND datetime(updated_at) > datetime('now', '-7 days')",
            params![artist_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
    };

    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::artists::get_or_create_artist;
    use crate::db::queries::{RandomFilter, random_tracks_where, sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_save_and_get_similar_artists() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let a2 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();
        let a3 = get_or_create_artist(&db.conn, "Autechre", None).unwrap();

        save_similar_artists(&db.conn, a1, &[(a2, 0.9), (a3, 0.7)], "subsonic").unwrap();

        let similar = get_similar_artists(&db.conn, a1).unwrap();
        assert_eq!(similar.len(), 2);
        assert_eq!(similar[0].0.name, "Squarepusher");
        assert!((similar[0].1 - 0.9).abs() < f64::EPSILON);
        assert_eq!(similar[1].0.name, "Autechre");
        assert!((similar[1].1 - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_save_replaces_per_source() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let a2 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();
        let a3 = get_or_create_artist(&db.conn, "Autechre", None).unwrap();

        save_similar_artists(&db.conn, a1, &[(a2, 0.9)], "subsonic").unwrap();
        assert_eq!(get_similar_artists(&db.conn, a1).unwrap().len(), 1);

        // Adding from different source keeps both.
        save_similar_artists(&db.conn, a1, &[(a3, 0.5)], "lastfm").unwrap();
        let similar = get_similar_artists(&db.conn, a1).unwrap();
        assert_eq!(similar.len(), 2);

        // Replacing same source only clears that source.
        save_similar_artists(&db.conn, a1, &[(a3, 0.8)], "subsonic").unwrap();
        let similar = get_similar_artists(&db.conn, a1).unwrap();
        // a3 from both sources (merged via MAX), a2 gone (subsonic replaced)
        assert_eq!(similar.len(), 1);
        assert_eq!(similar[0].0.name, "Autechre");
    }

    #[test]
    fn test_has_fresh_similar_artists() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let a2 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();

        // No cache yet.
        assert!(!has_fresh_similar_artists(&db.conn, a1).unwrap());

        // Save some — should be fresh.
        save_similar_artists(&db.conn, a1, &[(a2, 0.8)], "subsonic").unwrap();
        assert!(has_fresh_similar_artists(&db.conn, a1).unwrap());
    }

    #[test]
    fn test_has_fresh_similar_artists_stale() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let a2 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();

        save_similar_artists(&db.conn, a1, &[(a2, 0.8)], "subsonic").unwrap();

        // Manually backdate to make it stale.
        db.conn
            .execute(
                "UPDATE similar_artists SET updated_at = datetime('now', '-8 days')
                 WHERE artist_id = ?1",
                params![a1],
            )
            .unwrap();

        assert!(!has_fresh_similar_artists(&db.conn, a1).unwrap());
    }

    #[test]
    fn random_draws_leave_out_excluded_ids() {
        let db = test_db();
        let ids: Vec<i64> = (0..10)
            .map(|i| {
                let mut meta = sample_meta(&format!("Track{}", i), "Artist", "Album");
                meta.path = Some(format!("/music/Album/Track{}.flac", i));
                meta.track_number = Some(i);
                upsert_track(&db.conn, &meta).unwrap()
            })
            .collect();

        let filter = RandomFilter {
            exclude: &ids[..8],
            ..Default::default()
        };
        let mut drawn: Vec<i64> = random_tracks_where(&db.conn, 5, &filter)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        drawn.sort();
        assert_eq!(drawn, &ids[8..]);
    }

    #[test]
    fn test_get_similar_artists_empty() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Nobody", None).unwrap();
        let similar = get_similar_artists(&db.conn, a1).unwrap();
        assert!(similar.is_empty());
    }

    #[test]
    fn test_similar_artists_detailed() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let a2 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();
        let a3 = get_or_create_artist(&db.conn, "Autechre", None).unwrap();

        save_similar_artists(&db.conn, a1, &[(a2, 0.9)], "subsonic").unwrap();
        save_similar_artists_with_rel(&db.conn, a1, &[(a3, 0.7)], "musicbrainz", "collaborator")
            .unwrap();

        let detailed = get_similar_artists_detailed(&db.conn, a1).unwrap();
        assert_eq!(detailed.len(), 2);

        let subsonic_entry = detailed.iter().find(|e| e.source == "subsonic").unwrap();
        assert_eq!(subsonic_entry.artist.name, "Squarepusher");
        assert_eq!(subsonic_entry.relationship, "similar");

        let mb_entry = detailed.iter().find(|e| e.source == "musicbrainz").unwrap();
        assert_eq!(mb_entry.artist.name, "Autechre");
        assert_eq!(mb_entry.relationship, "collaborator");
    }

    #[test]
    fn test_fresh_similar_artists_per_source() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let a2 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();

        save_similar_artists(&db.conn, a1, &[(a2, 0.8)], "listenbrainz").unwrap();

        assert!(has_fresh_similar_artists_for_source(&db.conn, a1, Some("listenbrainz")).unwrap());
        assert!(!has_fresh_similar_artists_for_source(&db.conn, a1, Some("musicbrainz")).unwrap());
        // Any source.
        assert!(has_fresh_similar_artists(&db.conn, a1).unwrap());
    }
}

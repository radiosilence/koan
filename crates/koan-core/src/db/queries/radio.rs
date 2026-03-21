use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::{ArtistRow, TrackRow};

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

// --- Play history ---

/// Record a play event.
pub fn record_play(
    conn: &Connection,
    track_id: i64,
    duration_ms: Option<i64>,
) -> Result<(), DbError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    conn.execute(
        "INSERT INTO play_history (track_id, played_at, duration_ms)
         VALUES (?1, ?2, ?3)",
        params![track_id, now, duration_ms],
    )?;
    Ok(())
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

/// Get random tracks from the library, excluding specific track paths.
/// Returns up to `count` tracks, weighted towards tracks matching the given
/// genres and artist IDs (preferred tracks sorted first, then random fill).
pub fn random_tracks_excluding(
    conn: &Connection,
    exclude_paths: &[String],
    preferred_artist_ids: &[i64],
    preferred_genres: &[String],
    count: usize,
) -> Result<Vec<TrackRow>, DbError> {
    // Build the oversample pool query. We fetch count*10 random candidates,
    // then sort preferred ones first and take `count`.
    let pool_limit = count * 10;

    // Build comma-separated placeholders for exclude_paths.
    let exclude_clause = if exclude_paths.is_empty() {
        String::from("1=1")
    } else {
        let placeholders: Vec<String> = (0..exclude_paths.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        format!(
            "(t.path IS NULL OR t.path NOT IN ({}))",
            placeholders.join(",")
        )
    };

    // We'll use a two-step approach: first get a random pool, then score and sort.
    let base_offset = exclude_paths.len();

    // Build artist ID match expression.
    let artist_clause = if preferred_artist_ids.is_empty() {
        String::from("0")
    } else {
        let placeholders: Vec<String> = preferred_artist_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", base_offset + i + 1))
            .collect();
        let joined = placeholders.join(",");
        format!(
            "CASE WHEN t.artist_id IN ({joined}) OR al.artist_id IN ({joined}) THEN 1 ELSE 0 END",
        )
    };

    let genre_offset = base_offset + preferred_artist_ids.len();
    let genre_clause = if preferred_genres.is_empty() {
        String::from("0")
    } else {
        let placeholders: Vec<String> = preferred_genres
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", genre_offset + i + 1))
            .collect();
        format!(
            "CASE WHEN t.genre IN ({}) THEN 1 ELSE 0 END",
            placeholders.join(",")
        )
    };

    let limit_param_idx = genre_offset + preferred_genres.len() + 1;

    let sql = format!(
        "SELECT * FROM (
            SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                   t.disc, t.track_number, t.title, t.duration_ms, t.path,
                   t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                   t.genre, t.source, t.remote_id, t.cached_path,
                   ({artist_clause} + {genre_clause}) AS preference_score
            FROM tracks t
            LEFT JOIN artists a ON t.artist_id = a.id
            LEFT JOIN albums al ON t.album_id = al.id
            LEFT JOIN artists aa ON al.artist_id = aa.id
            WHERE {exclude_clause}
            ORDER BY RANDOM()
            LIMIT ?{limit_param_idx}
        ) sub
        ORDER BY preference_score DESC, RANDOM()"
    );

    let mut stmt = conn.prepare(&sql)?;

    // Bind all parameters.
    let mut param_idx = 1;
    for path in exclude_paths {
        stmt.raw_bind_parameter(param_idx, path)?;
        param_idx += 1;
    }
    for &aid in preferred_artist_ids {
        stmt.raw_bind_parameter(param_idx, aid)?;
        param_idx += 1;
    }
    for genre in preferred_genres {
        stmt.raw_bind_parameter(param_idx, genre)?;
        param_idx += 1;
    }
    stmt.raw_bind_parameter(param_idx, pool_limit as i64)?;

    let rows = stmt
        .raw_query()
        .mapped(|row| {
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
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Take only `count` from the scored results.
    Ok(rows.into_iter().take(count).collect())
}

/// Get all unique genres in the library.
pub fn all_genres(conn: &Connection) -> Result<Vec<String>, DbError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT genre FROM tracks WHERE genre IS NOT NULL ORDER BY genre")?;

    let rows = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::artists::get_or_create_artist;
    use crate::db::queries::{sample_meta, upsert_track};

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
    fn test_random_tracks_excluding() {
        let db = test_db();

        // Insert several tracks.
        for i in 0..10 {
            let mut meta = sample_meta(&format!("Track{}", i), "Artist", "Album");
            meta.path = Some(format!("/music/Album/Track{}.flac", i));
            meta.track_number = Some(i);
            upsert_track(&db.conn, &meta).unwrap();
        }

        // Get 5 random tracks, excluding none.
        let tracks = random_tracks_excluding(&db.conn, &[], &[], &[], 5).unwrap();
        assert_eq!(tracks.len(), 5);

        // Exclude some paths.
        let exclude: Vec<String> = (0..8)
            .map(|i| format!("/music/Album/Track{}.flac", i))
            .collect();
        let tracks = random_tracks_excluding(&db.conn, &exclude, &[], &[], 5).unwrap();
        // Only 2 tracks not excluded.
        assert_eq!(tracks.len(), 2);
    }

    #[test]
    fn test_random_tracks_prefers_artist_and_genre() {
        let db = test_db();

        // Insert tracks with different artists/genres.
        for i in 0..20 {
            let artist = if i < 5 { "Preferred" } else { "Other" };
            let genre = if i < 5 { "IDM" } else { "Pop" };
            let mut meta = sample_meta(&format!("T{}", i), artist, &format!("A{}", i));
            meta.path = Some(format!("/music/A{}/T{}.flac", i, i));
            meta.track_number = Some(i);
            meta.genre = Some(genre.into());
            upsert_track(&db.conn, &meta).unwrap();
        }

        // Get the artist_id for "Preferred".
        let preferred_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM artists WHERE name = 'Preferred'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let tracks =
            random_tracks_excluding(&db.conn, &[], &[preferred_id], &["IDM".into()], 5).unwrap();

        assert_eq!(tracks.len(), 5);
        // The first results should be preferred (IDM/Preferred artist).
        // Due to randomness we can't assert exact order, but the preferred
        // tracks should dominate the top of the list.
        let preferred_count = tracks
            .iter()
            .filter(|t| t.artist_name == "Preferred")
            .count();
        assert!(
            preferred_count >= 3,
            "expected at least 3 preferred tracks in top 5, got {}",
            preferred_count
        );
    }

    #[test]
    fn test_all_genres() {
        let db = test_db();

        let mut m1 = sample_meta("T1", "A1", "Al1");
        m1.genre = Some("Electronic".into());
        m1.path = Some("/music/Al1/T1.flac".into());
        upsert_track(&db.conn, &m1).unwrap();

        let mut m2 = sample_meta("T2", "A2", "Al2");
        m2.genre = Some("IDM".into());
        m2.path = Some("/music/Al2/T2.flac".into());
        upsert_track(&db.conn, &m2).unwrap();

        let mut m3 = sample_meta("T3", "A3", "Al3");
        m3.genre = Some("Electronic".into()); // duplicate
        m3.path = Some("/music/Al3/T3.flac".into());
        upsert_track(&db.conn, &m3).unwrap();

        let genres = all_genres(&db.conn).unwrap();
        assert_eq!(genres.len(), 2);
        assert!(genres.contains(&"Electronic".to_string()));
        assert!(genres.contains(&"IDM".to_string()));
    }

    #[test]
    fn test_get_similar_artists_empty() {
        let db = test_db();
        let a1 = get_or_create_artist(&db.conn, "Nobody", None).unwrap();
        let similar = get_similar_artists(&db.conn, a1).unwrap();
        assert!(similar.is_empty());
    }

    #[test]
    fn test_random_tracks_excluding_empty_library() {
        let db = test_db();
        let tracks = random_tracks_excluding(&db.conn, &[], &[], &[], 10).unwrap();
        assert!(tracks.is_empty());
    }

    #[test]
    fn test_record_and_query_play_history() {
        let db = test_db();
        let mut meta = sample_meta("Track1", "Artist1", "Album1");
        meta.path = Some("/music/Track1.flac".into());
        upsert_track(&db.conn, &meta).unwrap();

        let track_id: i64 = db
            .conn
            .query_row("SELECT id FROM tracks LIMIT 1", [], |row| row.get(0))
            .unwrap();

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

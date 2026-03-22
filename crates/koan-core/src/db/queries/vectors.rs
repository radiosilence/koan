//! Vector storage and brute-force KNN queries for acoustic similarity.

use rusqlite::{Connection, params};

use crate::db::connection::DbError;
use crate::index::features::{bytes_to_embedding, embedding_to_bytes, euclidean_distance};

/// Store (or replace) the acoustic embedding for a track.
pub fn store_vector(conn: &Connection, track_id: i64, embedding: &[f32]) -> Result<(), DbError> {
    let bytes = embedding_to_bytes(embedding);
    conn.execute(
        "INSERT OR REPLACE INTO track_vectors (track_id, embedding, updated_at)
         VALUES (?1, ?2, datetime('now'))",
        params![track_id, bytes],
    )?;
    Ok(())
}

/// Retrieve the acoustic embedding for a track.
pub fn get_vector(conn: &Connection, track_id: i64) -> Result<Option<Vec<f32>>, DbError> {
    let result: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM track_vectors WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )
        .ok();

    Ok(result.and_then(|bytes| bytes_to_embedding(&bytes)))
}

/// Check whether a track has an acoustic embedding.
pub fn has_vector(conn: &Connection, track_id: i64) -> Result<bool, DbError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM track_vectors WHERE track_id = ?1",
        params![track_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Find the k nearest tracks by euclidean distance (brute-force).
///
/// Returns (track_id, distance) pairs sorted by distance ascending.
/// At 23 dims x 100k rows this is ~9MB of data and sub-millisecond.
pub fn find_similar(
    conn: &Connection,
    track_id: i64,
    k: usize,
) -> Result<Vec<(i64, f32)>, DbError> {
    let target = match get_vector(conn, track_id)? {
        Some(v) => v,
        None => return Ok(vec![]),
    };

    find_similar_to_vector(conn, &target, k, Some(track_id))
}

/// Find the k nearest tracks to an arbitrary vector (e.g. a centroid).
///
/// Optionally excludes a specific track_id from results.
pub fn find_similar_to_vector(
    conn: &Connection,
    target: &[f32],
    k: usize,
    exclude_track_id: Option<i64>,
) -> Result<Vec<(i64, f32)>, DbError> {
    let mut stmt = conn.prepare("SELECT track_id, embedding FROM track_vectors")?;
    let rows = stmt.query_map([], |row| {
        let tid: i64 = row.get(0)?;
        let bytes: Vec<u8> = row.get(1)?;
        Ok((tid, bytes))
    })?;

    let mut distances: Vec<(i64, f32)> = Vec::new();
    for row in rows {
        let (tid, bytes) = row?;
        if exclude_track_id == Some(tid) {
            continue;
        }
        if let Some(emb) = bytes_to_embedding(&bytes)
            && emb.len() == target.len()
        {
            let dist = euclidean_distance(target, &emb);
            distances.push((tid, dist));
        }
    }

    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    distances.truncate(k);
    Ok(distances)
}

/// Get all track IDs that are missing acoustic embeddings.
pub fn tracks_missing_vectors(conn: &Connection) -> Result<Vec<(i64, String)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path FROM tracks t
         LEFT JOIN track_vectors v ON t.id = v.track_id
         WHERE v.track_id IS NULL AND t.path IS NOT NULL AND t.source = 'local'",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            Ok((id, path))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Count how many tracks have acoustic embeddings.
pub fn vector_count(conn: &Connection) -> Result<i64, DbError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM track_vectors", [], |row| row.get(0))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::index::features::EMBEDDING_DIMS;

    fn test_db() -> Database {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Database::open(tmp.path()).unwrap()
    }

    fn insert_track(conn: &Connection, title: &str) -> i64 {
        conn.execute(
            "INSERT INTO artists (name) VALUES (?1) ON CONFLICT DO NOTHING",
            params!["Test Artist"],
        )
        .unwrap();
        let artist_id: i64 = conn
            .query_row(
                "SELECT id FROM artists WHERE name = 'Test Artist'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO albums (title, artist_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            params!["Test Album", artist_id],
        )
        .unwrap();
        let album_id: i64 = conn
            .query_row(
                "SELECT id FROM albums WHERE title = 'Test Album'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO tracks (title, album_id, artist_id, path, source) VALUES (?1, ?2, ?3, ?4, 'local')",
            params![title, album_id, artist_id, format!("/music/{}.flac", title)],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn make_embedding(val: f32) -> Vec<f32> {
        vec![val; EMBEDDING_DIMS]
    }

    #[test]
    fn store_and_retrieve_vector() {
        let db = test_db();
        let tid = insert_track(&db.conn, "test-track");
        let emb: Vec<f32> = (0..EMBEDDING_DIMS).map(|i| i as f32 * 0.1).collect();
        store_vector(&db.conn, tid, &emb).unwrap();
        let recovered = get_vector(&db.conn, tid).unwrap().unwrap();
        assert_eq!(emb, recovered);
    }

    #[test]
    fn has_vector_returns_false_when_missing() {
        let db = test_db();
        let tid = insert_track(&db.conn, "no-vector");
        assert!(!has_vector(&db.conn, tid).unwrap());
    }

    #[test]
    fn has_vector_returns_true_after_store() {
        let db = test_db();
        let tid = insert_track(&db.conn, "has-vector");
        store_vector(&db.conn, tid, &make_embedding(1.0)).unwrap();
        assert!(has_vector(&db.conn, tid).unwrap());
    }

    #[test]
    fn find_similar_empty_db() {
        let db = test_db();
        let tid = insert_track(&db.conn, "lonely");
        store_vector(&db.conn, tid, &make_embedding(0.0)).unwrap();
        let results = find_similar(&db.conn, tid, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn find_similar_returns_nearest() {
        let db = test_db();
        let t1 = insert_track(&db.conn, "seed");
        let t2 = insert_track(&db.conn, "near");
        let t3 = insert_track(&db.conn, "far");

        store_vector(&db.conn, t1, &make_embedding(0.0)).unwrap();
        store_vector(&db.conn, t2, &make_embedding(0.1)).unwrap();
        store_vector(&db.conn, t3, &make_embedding(10.0)).unwrap();

        let results = find_similar(&db.conn, t1, 2).unwrap();
        assert_eq!(results.len(), 2);
        // Nearest should be t2
        assert_eq!(results[0].0, t2);
        assert_eq!(results[1].0, t3);
        // Near distance should be smaller than far distance
        assert!(results[0].1 < results[1].1);
    }

    #[test]
    fn find_similar_no_vector_returns_empty() {
        let db = test_db();
        let tid = insert_track(&db.conn, "no-emb");
        let results = find_similar(&db.conn, tid, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn tracks_missing_vectors_works() {
        let db = test_db();
        let t1 = insert_track(&db.conn, "analyzed");
        let _t2 = insert_track(&db.conn, "pending");
        store_vector(&db.conn, t1, &make_embedding(0.0)).unwrap();
        let missing = tracks_missing_vectors(&db.conn).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].1, "/music/pending.flac");
    }

    #[test]
    fn vector_count_works() {
        let db = test_db();
        assert_eq!(vector_count(&db.conn).unwrap(), 0);
        let tid = insert_track(&db.conn, "counted");
        store_vector(&db.conn, tid, &make_embedding(0.0)).unwrap();
        assert_eq!(vector_count(&db.conn).unwrap(), 1);
    }
}

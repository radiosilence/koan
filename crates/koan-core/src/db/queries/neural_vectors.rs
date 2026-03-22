//! Neural vector storage and KNN for DCLAP embeddings (512-dim).
//!
//! Same pattern as `vectors.rs` (bliss 23-dim) but targeting
//! `track_neural_vectors`. Reuses the shared `brute_force_knn` helper.

use rusqlite::{Connection, params};

use crate::db::connection::DbError;
use crate::index::features::{bytes_to_embedding, embedding_to_bytes};

use super::vectors::brute_force_knn;

/// Store (or replace) the neural embedding for a track.
pub fn store_neural_vector(
    conn: &Connection,
    track_id: i64,
    embedding: &[f32],
) -> Result<(), DbError> {
    let bytes = embedding_to_bytes(embedding);
    conn.execute(
        "INSERT OR REPLACE INTO track_neural_vectors (track_id, embedding, updated_at)
         VALUES (?1, ?2, datetime('now'))",
        params![track_id, bytes],
    )?;
    Ok(())
}

/// Retrieve the neural embedding for a track.
pub fn get_neural_vector(conn: &Connection, track_id: i64) -> Result<Option<Vec<f32>>, DbError> {
    let result: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM track_neural_vectors WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )
        .ok();

    Ok(result.and_then(|bytes| bytes_to_embedding(&bytes)))
}

/// Check whether a track has a neural embedding.
pub fn has_neural_vector(conn: &Connection, track_id: i64) -> Result<bool, DbError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM track_neural_vectors WHERE track_id = ?1",
        params![track_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Find the k nearest tracks by neural embedding (brute-force KNN).
pub fn find_similar_neural(
    conn: &Connection,
    track_id: i64,
    k: usize,
) -> Result<Vec<(i64, f32)>, DbError> {
    let target = match get_neural_vector(conn, track_id)? {
        Some(v) => v,
        None => return Ok(vec![]),
    };

    brute_force_knn(conn, "track_neural_vectors", &target, k, Some(track_id))
}

/// Find the k nearest tracks to a text embedding vector.
pub fn find_by_text_embedding(
    conn: &Connection,
    text_embedding: &[f32],
    k: usize,
) -> Result<Vec<(i64, f32)>, DbError> {
    brute_force_knn(conn, "track_neural_vectors", text_embedding, k, None)
}

/// Get all track IDs that are missing neural embeddings (local tracks only).
pub fn tracks_missing_neural_vectors(conn: &Connection) -> Result<Vec<(i64, String)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path FROM tracks t
         LEFT JOIN track_neural_vectors v ON t.id = v.track_id
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

/// Count how many tracks have neural embeddings.
pub fn neural_vector_count(conn: &Connection) -> Result<i64, DbError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM track_neural_vectors", [], |row| {
        row.get(0)
    })?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::index::neural::NEURAL_EMBEDDING_DIMS;

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

    fn make_neural_embedding(val: f32) -> Vec<f32> {
        vec![val; NEURAL_EMBEDDING_DIMS]
    }

    #[test]
    fn store_and_retrieve_neural_vector() {
        let db = test_db();
        let tid = insert_track(&db.conn, "neural-test");
        let emb: Vec<f32> = (0..NEURAL_EMBEDDING_DIMS)
            .map(|i| i as f32 * 0.01)
            .collect();
        store_neural_vector(&db.conn, tid, &emb).unwrap();
        let recovered = get_neural_vector(&db.conn, tid).unwrap().unwrap();
        assert_eq!(emb, recovered);
    }

    #[test]
    fn has_neural_vector_returns_false_when_missing() {
        let db = test_db();
        let tid = insert_track(&db.conn, "no-neural");
        assert!(!has_neural_vector(&db.conn, tid).unwrap());
    }

    #[test]
    fn has_neural_vector_returns_true_after_store() {
        let db = test_db();
        let tid = insert_track(&db.conn, "has-neural");
        store_neural_vector(&db.conn, tid, &make_neural_embedding(1.0)).unwrap();
        assert!(has_neural_vector(&db.conn, tid).unwrap());
    }

    #[test]
    fn find_similar_neural_returns_nearest() {
        let db = test_db();
        let t1 = insert_track(&db.conn, "seed-neural");
        let t2 = insert_track(&db.conn, "near-neural");
        let t3 = insert_track(&db.conn, "far-neural");

        store_neural_vector(&db.conn, t1, &make_neural_embedding(0.0)).unwrap();
        store_neural_vector(&db.conn, t2, &make_neural_embedding(0.1)).unwrap();
        store_neural_vector(&db.conn, t3, &make_neural_embedding(10.0)).unwrap();

        let results = find_similar_neural(&db.conn, t1, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, t2);
        assert!(results[0].1 < results[1].1);
    }

    #[test]
    fn find_by_text_embedding_works() {
        let db = test_db();
        let t1 = insert_track(&db.conn, "jazzy");
        let t2 = insert_track(&db.conn, "metal");

        store_neural_vector(&db.conn, t1, &make_neural_embedding(0.1)).unwrap();
        store_neural_vector(&db.conn, t2, &make_neural_embedding(10.0)).unwrap();

        let query = make_neural_embedding(0.0);
        let results = find_by_text_embedding(&db.conn, &query, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, t1);
    }

    #[test]
    fn tracks_missing_neural_vectors_works() {
        let db = test_db();
        let t1 = insert_track(&db.conn, "analyzed-neural");
        let _t2 = insert_track(&db.conn, "pending-neural");
        store_neural_vector(&db.conn, t1, &make_neural_embedding(0.0)).unwrap();
        let missing = tracks_missing_neural_vectors(&db.conn).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].1, "/music/pending-neural.flac");
    }

    #[test]
    fn neural_vector_count_works() {
        let db = test_db();
        assert_eq!(neural_vector_count(&db.conn).unwrap(), 0);
        let tid = insert_track(&db.conn, "counted-neural");
        store_neural_vector(&db.conn, tid, &make_neural_embedding(0.0)).unwrap();
        assert_eq!(neural_vector_count(&db.conn).unwrap(), 1);
    }
}

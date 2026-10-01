use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::TrackRow;

/// Sanitize a user query for FTS5 MATCH: escapes double-quotes and wraps in
/// a quoted phrase so FTS5 special characters are treated as literals.
pub(crate) fn sanitize_fts_query(query: &str) -> String {
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
    search_tracks_paged(conn, query, 100, 0)
}

/// Full-text search with configurable limit and offset for pagination.
pub fn search_tracks_paged(
    conn: &Connection,
    query: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<TrackRow>, DbError> {
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
         ORDER BY a.name COLLATE LIBRARY, al.date, al.title COLLATE LIBRARY, t.disc, t.track_number
         LIMIT ?2 OFFSET ?3",
    )?;

    let rows = stmt
        .query_map(params![fts_query, limit, offset], super::row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// What a fuzzy match runs over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorpusKind {
    Track,
    Album,
    Artist,
}

/// Each row's id, and the text a fuzzy match reads.
pub type Corpus = Arc<Vec<(i64, String)>>;

/// Every row of `kind` as its id and the text a fuzzy match reads: "artist —
/// album — title" for a track, "artist — album" for an album, the name for an
/// album artist.
///
/// Unordered, and only the matched columns: the matcher ranks what it finds,
/// so sorting on the LIBRARY collation — a Rust callback per comparison, the
/// most expensive part of reading the whole library — bought nothing.
pub fn fuzzy_corpus(conn: &Connection, kind: CorpusKind) -> Result<Vec<(i64, String)>, DbError> {
    let mut stmt = conn.prepare_cached(match kind {
        CorpusKind::Track => {
            "SELECT t.id, COALESCE(a.name, '') || ' — ' || COALESCE(al.title, '') || ' — ' || t.title
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id"
        }
        CorpusKind::Album => {
            "SELECT al.id, COALESCE(a.name, '') || ' — ' || al.title
             FROM albums al
             LEFT JOIN artists a ON al.artist_id = a.id"
        }
        CorpusKind::Artist => {
            "SELECT a.id, a.name FROM artists a
             WHERE EXISTS (SELECT 1 FROM albums al WHERE al.artist_id = a.id)"
        }
    })?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// A corpus per kind, read again only when the library has moved.
///
/// A search field asks on every keystroke, and the library is the same between
/// them.
#[derive(Default)]
pub struct CorpusCache {
    slots: parking_lot::Mutex<HashMap<CorpusKind, (u64, Corpus)>>,
}

impl CorpusCache {
    /// The corpus for `kind` as of `version`, a number the caller changes
    /// whenever library rows may have.
    pub fn get(
        &self,
        conn: &Connection,
        kind: CorpusKind,
        version: u64,
    ) -> Result<Corpus, DbError> {
        if let Some((at, corpus)) = self.slots.lock().get(&kind)
            && *at == version
        {
            return Ok(Arc::clone(corpus));
        }
        // Read without the lock held: a slow read for one kind is no reason to
        // hold up a cached answer for another.
        let corpus: Corpus = Arc::new(fuzzy_corpus(conn, kind)?);
        self.slots
            .lock()
            .insert(kind, (version, Arc::clone(&corpus)));
        Ok(corpus)
    }
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

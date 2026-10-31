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
                t.genre, t.source, t.remote_id, t.cached_path, al.artist_id
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
fn read_corpus(conn: &Connection, kind: CorpusKind) -> Result<Vec<(i64, String)>, DbError> {
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
             WHERE EXISTS (SELECT 1 FROM album_artist_links l
                            WHERE l.artist_id = a.id AND l.listed)"
        }
    })?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// The corpus for `kind`, read again only when the library has moved.
///
/// A search field asks on every keystroke, and the library is the same
/// between them. One cache for the process, keyed on the database file and its
/// [`library_stamp`](super::library_stamp): every front end, every pooled
/// connection and every kind of search read the same corpus. An in-memory
/// database has no name to key on and is read each time.
pub fn corpus(conn: &Connection, kind: CorpusKind) -> Result<Corpus, DbError> {
    let Some(stamp) = corpus_stamp(conn)? else {
        return Ok(Arc::new(read_corpus(conn, kind)?));
    };
    if let Some(slot) = CORPORA.lock().get(&kind)
        && slot.stamp == stamp
    {
        return Ok(Arc::clone(&slot.corpus));
    }
    // Read without the lock held: a slow read for one kind is no reason to
    // hold up a cached answer for another.
    let corpus: Corpus = Arc::new(read_corpus(conn, kind)?);
    CORPORA.lock().insert(
        kind,
        Slot {
            stamp,
            corpus: Arc::clone(&corpus),
            last: None,
        },
    );
    Ok(corpus)
}

/// Which database, and the library in it. `None` for an in-memory database.
fn corpus_stamp(conn: &Connection) -> Result<Option<u64>, DbError> {
    use std::hash::{Hash, Hasher};
    let Some(path) = conn.path().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (path, super::library_stamp(conn)?).hash(&mut hasher);
    Ok(Some(hasher.finish()))
}

/// A kind's corpus as of a stamp, and the last fuzzy fallback read from it: a
/// search page lists and counts each kind, and both fall back alike.
struct Slot {
    stamp: u64,
    corpus: Corpus,
    last: Option<(String, Vec<i64>)>,
}

static CORPORA: std::sync::LazyLock<parking_lot::Mutex<HashMap<CorpusKind, Slot>>> =
    std::sync::LazyLock::new(Default::default);

/// The entries of `corpus` that match `query`, best first, as indices into it
/// with their scores. The one ranking every fuzzy search uses.
///
/// Case-insensitive: a phone keyboard capitalises the first letter. Each word
/// is matched on its own, literally: nucleo's operators (`!`, `^`, `$`, `'`)
/// are characters someone typed, not syntax. Ties go to the shorter text, the
/// closer of two equal matches, then to the lower id.
pub fn fuzzy_rank(corpus: &[(i64, String)], query: &str) -> Vec<(u32, usize)> {
    use nucleo::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
    use nucleo::{Config, Matcher, Utf32Str};

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    // The matcher alone, on the caller's thread: `Nucleo` would build a
    // thread pool per search to do the same.
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = corpus
        .iter()
        .enumerate()
        .filter_map(|(i, (_, text))| {
            pattern
                .score(Utf32Str::new(text, &mut buf), &mut matcher)
                .map(|score| (score, i))
        })
        .collect();
    scored.sort_by_key(|&(score, i)| (std::cmp::Reverse(score), corpus[i].1.len(), corpus[i].0));
    scored
}

/// How closely `column` matches a search, as SQL: 0 when it is the query, 1
/// when it starts with it, 2 when a later word does, 3 otherwise. Takes the
/// three parameters [`match_rank_binds`] gives, in order.
pub(crate) fn match_rank(column: &str) -> String {
    format!(
        "CASE WHEN {column} LIKE ? ESCAPE '\\' THEN 0
              WHEN {column} LIKE ? ESCAPE '\\' THEN 1
              WHEN {column} LIKE ? ESCAPE '\\' THEN 2 ELSE 3 END"
    )
}

pub(crate) fn match_rank_binds(query: &str) -> [String; 3] {
    let q = super::artists::escape_like(query.trim());
    [q.clone(), format!("{q}%"), format!("% {q}%")]
}

/// How far below the best fuzzy score a match may fall and still be listed.
///
/// Nucleo matches any run of the query's letters in order, however scattered:
/// "Black Country, New Road" holds k, r, e and w. Scores are dominated by how
/// contiguous a match is and whether it starts words, so a fraction of the
/// best keeps the near misses of a typo and drops the scattered ones.
const FUZZY_FLOOR: f64 = 0.6;

/// At most how many fuzzy matches a search lists: plenty for a typo, and a
/// bound on what the listing joins and orders.
const FUZZY_LIMIT: usize = 200;

/// The rows of `kind` closest to `query` by [`fuzzy_rank`], best first, at
/// most [`FUZZY_LIMIT`], those scoring under [`FUZZY_FLOOR`] of the best left
/// out.
///
/// What a search falls back on when nothing holds the query as typed, so a
/// typo still finds what was meant.
pub fn fuzzy_ids(conn: &Connection, kind: CorpusKind, query: &str) -> Result<Vec<i64>, DbError> {
    let corpus = corpus(conn, kind)?;
    if let Some(slot) = CORPORA.lock().get(&kind)
        && Arc::ptr_eq(&slot.corpus, &corpus)
        && let Some((q, ids)) = &slot.last
        && q == query
    {
        return Ok(ids.clone());
    }
    let scored = fuzzy_rank(&corpus, query);
    let ids: Vec<i64> = match scored.first() {
        Some(&(best, _)) => {
            let floor = (best as f64 * FUZZY_FLOOR) as u32;
            scored
                .into_iter()
                .take_while(|&(score, _)| score >= floor)
                .take(FUZZY_LIMIT)
                .map(|(_, i)| corpus[i].0)
                .collect()
        }
        None => Vec::new(),
    };
    if let Some(slot) = CORPORA.lock().get_mut(&kind)
        && Arc::ptr_eq(&slot.corpus, &corpus)
    {
        slot.last = Some((query.to_owned(), ids.clone()));
    }
    Ok(ids)
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

    #[test]
    fn fuzzy_rank_ignores_case() {
        let corpus = vec![
            (1, "Aphex Twin — Selected Ambient Works".to_owned()),
            (2, "Squarepusher — Feed Me Weird Things".to_owned()),
        ];
        let ranked = fuzzy_rank(&corpus, "SqrPshr");
        assert_eq!(ranked.first().map(|&(_, i)| corpus[i].0), Some(2));
    }

    #[test]
    fn fuzzy_rank_reads_operators_as_characters() {
        let corpus = vec![(1, "!!! — Myth Takes".to_owned()), (2, "Mythos".to_owned())];
        let ranked = fuzzy_rank(&corpus, "!!!");
        assert_eq!(ranked.len(), 1);
        assert_eq!(corpus[ranked[0].1].0, 1);
    }

    /// A corpus read from a database file is kept, and read again once a row
    /// is added.
    #[test]
    fn a_file_corpus_follows_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        upsert_track(&db.conn, &sample_meta("One", "Ayla", "Moss")).unwrap();
        let first = corpus(&db.conn, CorpusKind::Album).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &corpus(&db.conn, CorpusKind::Album).unwrap()
        ));
        upsert_track(&db.conn, &sample_meta("Two", "Bryn", "Tide")).unwrap();
        let second = corpus(&db.conn, CorpusKind::Album).unwrap();
        assert_eq!(second.len(), 2);
    }
}

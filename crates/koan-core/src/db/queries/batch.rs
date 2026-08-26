//! Batched and SQL-filtered reads for API layers.
//!
//! Everything here exists so a caller serving many rows at once issues a bounded
//! number of statements: one query per batch of parents rather than one per
//! parent, and `WHERE`/`LIMIT` in SQLite rather than `retain()` in Rust.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, ToSql, params_from_iter};

use crate::db::connection::DbError;

use super::tracks::row_to_track_row;
use super::{AlbumRow, TrackRow};

const TRACK_COLUMNS: &str = "t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path";

const TRACK_JOINS: &str = "FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id";

/// `(?,?,?)` for `n` bound values.
fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(2 + n * 2);
    s.push('(');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s.push(')');
    s
}

/// Wrap a user substring for `LIKE ... ESCAPE '\'`, neutralising `%` and `_`.
fn like_contains(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len() + 2);
    escaped.push('%');
    for c in needle.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

// ---------------------------------------------------------------------------
// Batched parent → children loads
// ---------------------------------------------------------------------------

/// Albums for many artists in one query, keyed by artist ID.
pub fn albums_for_artists(
    conn: &Connection,
    artist_ids: &[i64],
) -> Result<HashMap<i64, Vec<AlbumRow>>, DbError> {
    if artist_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         WHERE al.artist_id IN {}
         ORDER BY al.date, al.title COLLATE LIBRARY",
        placeholders(artist_ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(artist_ids), |row| {
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
            added_at: row.get(10)?,
        })
    })?;

    let mut out: HashMap<i64, Vec<AlbumRow>> = HashMap::new();
    for album in rows {
        let album = album?;
        out.entry(album.artist_id).or_default().push(album);
    }
    Ok(out)
}

/// Tracks for many albums in one query, keyed by album ID.
pub fn tracks_for_albums(
    conn: &Connection,
    album_ids: &[i64],
) -> Result<HashMap<i64, Vec<TrackRow>>, DbError> {
    if album_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT {} {} WHERE t.album_id IN {} ORDER BY t.disc, t.track_number",
        TRACK_COLUMNS,
        TRACK_JOINS,
        placeholders(album_ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(album_ids), row_to_track_row)?;

    let mut out: HashMap<i64, Vec<TrackRow>> = HashMap::new();
    for track in rows {
        let track = track?;
        if let Some(aid) = track.album_id {
            out.entry(aid).or_default().push(track);
        }
    }
    Ok(out)
}

/// Tracks for many artists in one query, keyed by the requested artist ID.
///
/// A track matches on either its own artist or its album artist, so one row can
/// land under two keys — the map is built by re-checking each requested ID.
pub fn tracks_for_artists(
    conn: &Connection,
    artist_ids: &[i64],
) -> Result<HashMap<i64, Vec<TrackRow>>, DbError> {
    if artist_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ph = placeholders(artist_ids.len());
    let sql = format!(
        "SELECT {}, al.artist_id {} WHERE t.artist_id IN {} OR al.artist_id IN {}
         ORDER BY al.date, al.title, t.disc, t.track_number",
        TRACK_COLUMNS, TRACK_JOINS, ph, ph
    );
    let mut stmt = conn.prepare(&sql)?;
    let bind: Vec<i64> = artist_ids
        .iter()
        .chain(artist_ids.iter())
        .copied()
        .collect();
    let rows = stmt.query_map(params_from_iter(&bind), |row| {
        Ok((row_to_track_row(row)?, row.get::<_, Option<i64>>(20)?))
    })?;

    let wanted: HashSet<i64> = artist_ids.iter().copied().collect();
    let mut out: HashMap<i64, Vec<TrackRow>> = HashMap::new();
    for row in rows {
        let (track, album_artist_id) = row?;
        for key in [track.artist_id, album_artist_id].into_iter().flatten() {
            if wanted.contains(&key)
                && !out.entry(key).or_default().iter().any(|t| t.id == track.id)
            {
                out.entry(key).or_default().push(track.clone());
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Aggregates — counted in SQLite rather than by materialising rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct AlbumStats {
    pub track_count: i64,
    pub total_duration_ms: i64,
}

/// Track count and summed duration per album, in one query.
pub fn album_stats(
    conn: &Connection,
    album_ids: &[i64],
) -> Result<HashMap<i64, AlbumStats>, DbError> {
    if album_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT album_id, COUNT(*), COALESCE(SUM(duration_ms), 0)
         FROM tracks WHERE album_id IN {} GROUP BY album_id",
        placeholders(album_ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(album_ids), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            AlbumStats {
                track_count: row.get(1)?,
                total_duration_ms: row.get(2)?,
            },
        ))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ArtistStats {
    pub album_count: i64,
    pub track_count: i64,
}

/// Album and track counts per artist, in two grouped queries rather than one
/// full row fetch per artist per field.
pub fn artist_stats(
    conn: &Connection,
    artist_ids: &[i64],
) -> Result<HashMap<i64, ArtistStats>, DbError> {
    if artist_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ph = placeholders(artist_ids.len());
    let mut out: HashMap<i64, ArtistStats> = artist_ids
        .iter()
        .map(|&id| (id, ArtistStats::default()))
        .collect();

    let album_sql = format!(
        "SELECT artist_id, COUNT(*) FROM albums WHERE artist_id IN {} GROUP BY artist_id",
        ph
    );
    let mut stmt = conn.prepare(&album_sql)?;
    let rows = stmt.query_map(params_from_iter(artist_ids), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (id, count) = row?;
        out.entry(id).or_default().album_count = count;
    }

    // Mirrors `tracks_for_artist`: own artist or album artist, counted once.
    let track_sql = format!(
        "SELECT k.id, COUNT(DISTINCT t.id)
         FROM artists k
         JOIN tracks t ON t.artist_id = k.id OR t.album_id IN (
             SELECT al.id FROM albums al WHERE al.artist_id = k.id
         )
         WHERE k.id IN {}
         GROUP BY k.id",
        ph
    );
    let mut stmt = conn.prepare(&track_sql)?;
    let rows = stmt.query_map(params_from_iter(artist_ids), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (id, count) = row?;
        out.entry(id).or_default().track_count = count;
    }

    Ok(out)
}

/// Which of the given paths are favourited — one query instead of a full table
/// scan per track.
pub fn favourite_paths(conn: &Connection, paths: &[String]) -> Result<HashSet<String>, DbError> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let sql = format!(
        "SELECT track_path FROM favourites WHERE track_path IN {}",
        placeholders(paths.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(paths), |row| row.get::<_, String>(0))?;
    rows.collect::<Result<HashSet<_>, _>>().map_err(Into::into)
}

/// The album each of the given tracks belongs to, keyed by track ID.
///
/// Tracks with no album are absent from the map rather than present with a
/// `None`; a caller asking which album to draw has the same answer either way.
pub fn album_ids_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT id, album_id FROM tracks WHERE id IN {} AND album_id IS NOT NULL",
        placeholders(track_ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(track_ids), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

/// Where each track's bytes are: on the server, on this machine, or both.
///
/// One query for a whole queue. The same reading `Track` gives, so a row in the
/// queue and the same row in an album agree about what they are — asked here
/// per queue rather than per row, which is what a list of a thousand would
/// otherwise cost.
pub fn sources_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, (bool, bool)>, DbError> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT id, remote_id IS NOT NULL, COALESCE(cached_path, path) IS NOT NULL \
         FROM tracks WHERE id IN {}",
        placeholders(track_ids.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(track_ids), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            (row.get::<_, bool>(1)?, row.get::<_, bool>(2)?),
        ))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// SQL-side track filtering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOrder {
    ArtistAlbumDiscTrack,
    Title,
    Artist,
    Album,
    Duration,
}

#[derive(Debug, Clone, Default)]
pub struct TrackFilter {
    pub ids: Option<Vec<i64>>,
    /// FTS5 query. Applied as a subquery so it composes with the other filters.
    pub search: Option<String>,
    pub album_id: Option<i64>,
    pub artist_ids: Option<Vec<i64>>,
    pub title: Option<String>,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub genre: Option<String>,
    pub codec: Option<String>,
    pub source: Option<String>,
    pub year_start: Option<i32>,
    pub year_end: Option<i32>,
    pub min_sample_rate: Option<i32>,
    pub min_bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub min_duration_ms: Option<i64>,
    pub max_duration_ms: Option<i64>,
    pub favourites_only: bool,
}

/// Fetch a page of tracks matching `filter`.
///
/// Both the filtering and the windowing happen in SQLite: the caller never sees
/// more than `limit` rows regardless of library size.
pub fn filter_tracks(
    conn: &Connection,
    filter: &TrackFilter,
    order: TrackOrder,
    descending: bool,
    limit: u32,
    offset: u32,
) -> Result<Vec<TrackRow>, DbError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(ids) = &filter.ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        clauses.push(format!("t.id IN {}", placeholders(ids.len())));
        binds.extend(ids.iter().map(|&i| Box::new(i) as Box<dyn ToSql>));
    }

    if let Some(query) = &filter.search {
        clauses.push("t.id IN (SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?)".to_string());
        binds.push(Box::new(super::search::sanitize_fts_query(query)));
    }

    if let Some(album_id) = filter.album_id {
        clauses.push("t.album_id = ?".to_string());
        binds.push(Box::new(album_id));
    }

    if let Some(ids) = &filter.artist_ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ph = placeholders(ids.len());
        clauses.push(format!("(t.artist_id IN {ph} OR al.artist_id IN {ph})"));
        for _ in 0..2 {
            binds.extend(ids.iter().map(|&i| Box::new(i) as Box<dyn ToSql>));
        }
    }

    if let Some(title) = &filter.title {
        clauses.push("t.title LIKE ? ESCAPE '\\'".to_string());
        binds.push(Box::new(like_contains(title)));
    }

    if let Some(name) = &filter.artist_name {
        clauses.push("(a.name LIKE ? ESCAPE '\\' OR aa.name LIKE ? ESCAPE '\\')".to_string());
        binds.push(Box::new(like_contains(name)));
        binds.push(Box::new(like_contains(name)));
    }

    if let Some(title) = &filter.album_title {
        clauses.push("al.title LIKE ? ESCAPE '\\'".to_string());
        binds.push(Box::new(like_contains(title)));
    }

    if let Some(genre) = &filter.genre {
        clauses.push("t.genre LIKE ? ESCAPE '\\'".to_string());
        binds.push(Box::new(like_contains(genre)));
    }

    if let Some(codec) = &filter.codec {
        clauses.push("t.codec LIKE ? ESCAPE '\\'".to_string());
        binds.push(Box::new(like_contains(codec)));
    }

    if let Some(source) = &filter.source {
        clauses.push("t.source = ?".to_string());
        binds.push(Box::new(source.clone()));
    }

    if filter.year_start.is_some() || filter.year_end.is_some() {
        // A track with no parsable four-digit year is excluded, matching the
        // behaviour of the year extraction it replaces.
        clauses.push("substr(al.date, 1, 4) GLOB '[0-9][0-9][0-9][0-9]'".to_string());
        if let Some(start) = filter.year_start {
            clauses.push("CAST(substr(al.date, 1, 4) AS INTEGER) >= ?".to_string());
            binds.push(Box::new(start));
        }
        if let Some(end) = filter.year_end {
            clauses.push("CAST(substr(al.date, 1, 4) AS INTEGER) <= ?".to_string());
            binds.push(Box::new(end));
        }
    }

    for (column, value) in [
        ("t.sample_rate >= ?", filter.min_sample_rate),
        ("t.bit_depth >= ?", filter.min_bit_depth),
        ("t.channels = ?", filter.channels),
    ] {
        if let Some(v) = value {
            clauses.push(column.to_string());
            binds.push(Box::new(v));
        }
    }

    for (column, value) in [
        ("t.duration_ms >= ?", filter.min_duration_ms),
        ("t.duration_ms <= ?", filter.max_duration_ms),
    ] {
        if let Some(v) = value {
            clauses.push(column.to_string());
            binds.push(Box::new(v));
        }
    }

    if filter.favourites_only {
        clauses.push(
            "EXISTS (SELECT 1 FROM favourites f
                     WHERE f.track_path = t.path OR f.track_path = t.cached_path)"
                .to_string(),
        );
    }

    let dir = if descending { "DESC" } else { "ASC" };
    let order_by = match order {
        TrackOrder::ArtistAlbumDiscTrack => format!(
            "a.name {dir}, al.date {dir}, al.title {dir}, t.disc {dir}, t.track_number {dir}"
        ),
        TrackOrder::Title => format!("t.title {dir}"),
        TrackOrder::Artist => {
            format!("a.name {dir}, al.date {dir}, t.disc {dir}, t.track_number {dir}")
        }
        TrackOrder::Album => format!("al.title {dir}, t.disc {dir}, t.track_number {dir}"),
        TrackOrder::Duration => format!("t.duration_ms {dir}"),
    };

    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };

    let sql = format!(
        "SELECT {TRACK_COLUMNS} {TRACK_JOINS} {where_clause}
         ORDER BY {order_by}, t.id {dir} LIMIT ? OFFSET ?"
    );

    binds.push(Box::new(limit));
    binds.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(binds.iter()), row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{add_favourite, sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    fn seed(db: &Database) {
        for (title, artist, album) in [
            ("Vordhosbn", "Aphex Twin", "Drukqs"),
            ("Avril 14th", "Aphex Twin", "Drukqs"),
            ("Roygbiv", "Boards of Canada", "MHTRTC"),
        ] {
            upsert_track(&db.conn, &sample_meta(title, artist, album)).unwrap();
        }
    }

    #[test]
    fn filter_pushes_limit_into_sql() {
        let db = test_db();
        seed(&db);
        let page = filter_tracks(
            &db.conn,
            &TrackFilter::default(),
            TrackOrder::Title,
            false,
            2,
            0,
        )
        .unwrap();
        assert_eq!(page.len(), 2);
        let page2 = filter_tracks(
            &db.conn,
            &TrackFilter::default(),
            TrackOrder::Title,
            false,
            2,
            2,
        )
        .unwrap();
        assert_eq!(page2.len(), 1);
    }

    #[test]
    fn filter_matches_substrings_and_escapes_wildcards() {
        let db = test_db();
        seed(&db);
        let filter = TrackFilter {
            title: Some("vril".into()),
            ..Default::default()
        };
        let hits = filter_tracks(&db.conn, &filter, TrackOrder::Title, false, 50, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Avril 14th");

        // A bare `%` must not match everything.
        let filter = TrackFilter {
            title: Some("%".into()),
            ..Default::default()
        };
        assert!(
            filter_tracks(&db.conn, &filter, TrackOrder::Title, false, 50, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn filter_composes_search_with_other_predicates() {
        let db = test_db();
        seed(&db);
        let filter = TrackFilter {
            search: Some("Aphex".into()),
            title: Some("Vordhosbn".into()),
            ..Default::default()
        };
        let hits = filter_tracks(&db.conn, &filter, TrackOrder::Title, false, 50, 0).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn stats_are_aggregated_not_materialised() {
        let db = test_db();
        seed(&db);
        let album_id = db
            .conn
            .query_row("SELECT id FROM albums WHERE title = 'Drukqs'", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        let stats = album_stats(&db.conn, &[album_id]).unwrap();
        assert_eq!(stats[&album_id].track_count, 2);
        assert_eq!(stats[&album_id].total_duration_ms, 480_000);

        let artist_id = db
            .conn
            .query_row(
                "SELECT id FROM artists WHERE name = 'Aphex Twin'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        let stats = artist_stats(&db.conn, &[artist_id]).unwrap();
        assert_eq!(stats[&artist_id].album_count, 1);
        assert_eq!(stats[&artist_id].track_count, 2);
    }

    #[test]
    fn favourite_paths_only_returns_the_requested_paths() {
        let db = test_db();
        seed(&db);
        add_favourite(
            &db.conn,
            std::path::Path::new("/music/Drukqs/Vordhosbn.flac"),
        )
        .unwrap();
        let hits = favourite_paths(
            &db.conn,
            &[
                "/music/Drukqs/Vordhosbn.flac".to_string(),
                "/music/MHTRTC/Roygbiv.flac".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits.contains("/music/Drukqs/Vordhosbn.flac"));
    }

    #[test]
    fn batched_children_are_keyed_by_parent() {
        let db = test_db();
        seed(&db);
        let album_ids: Vec<i64> = db
            .conn
            .prepare("SELECT id FROM albums ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let map = tracks_for_albums(&db.conn, &album_ids).unwrap();
        assert_eq!(map.values().map(Vec::len).sum::<usize>(), 3);

        let artist_ids: Vec<i64> = db
            .conn
            .prepare("SELECT id FROM artists ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let map = albums_for_artists(&db.conn, &artist_ids).unwrap();
        assert_eq!(map.values().map(Vec::len).sum::<usize>(), 2);
        let map = tracks_for_artists(&db.conn, &artist_ids).unwrap();
        assert_eq!(map.values().map(Vec::len).sum::<usize>(), 3);
    }
}

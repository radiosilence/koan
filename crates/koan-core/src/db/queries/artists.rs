use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::ArtistRow;

/// Escape SQL LIKE wildcard characters in user input.
pub(super) fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Get or create an artist by name. Returns the artist ID.
pub fn get_or_create_artist(
    conn: &Connection,
    name: &str,
    remote_id: Option<&str>,
) -> Result<i64, DbError> {
    // Try to find existing.
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM artists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        // Update remote_id if we have one and the existing doesn't.
        if let Some(rid) = remote_id {
            conn.execute(
                "UPDATE artists SET remote_id = ?1 WHERE id = ?2 AND remote_id IS NULL",
                params![rid, id],
            )?;
        }
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO artists (name, remote_id) VALUES (?1, ?2)",
        params![name, remote_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// What to list. Artists are always album artists — a track-only credit (a
/// featured guest) appears inline in the queue, not as a shelf of its own.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArtistQuery<'a> {
    /// Case-insensitive substring over the name.
    pub search: Option<&'a str>,
    /// Favourited artists only.
    pub favourites_only: bool,
    /// `None` for the whole listing. A client that scrolls should page.
    pub limit: Option<u32>,
    pub offset: u32,
}

/// Artists with their album and track counts, narrowed and paged by the
/// database. Ordered by sort name, falling back to the name.
pub fn list_artists(conn: &Connection, q: &ArtistQuery) -> Result<Vec<ArtistRow>, DbError> {
    let mut sql = String::from(
        "SELECT a.id, a.name, a.sort_name, a.remote_id,
                COUNT(DISTINCT al.id), COUNT(t.id)
         FROM artists a
         INNER JOIN albums al ON al.artist_id = a.id
         LEFT JOIN tracks t ON t.album_id = al.id",
    );
    if q.favourites_only {
        sql.push_str(" JOIN favourite_artists f ON f.artist_name = a.name");
    }

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(query) = q.search {
        params.push(Box::new(format!("%{}%", escape_like(query))));
        sql.push_str(" WHERE a.name LIKE ? COLLATE NOCASE ESCAPE '\\'");
    }
    sql.push_str(" GROUP BY a.id ORDER BY COALESCE(a.sort_name, a.name) COLLATE LIBRARY");
    if let Some(limit) = q.limit {
        params.push(Box::new(limit as i64));
        params.push(Box::new(q.offset as i64));
        sql.push_str(" LIMIT ? OFFSET ?");
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), artist_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn artist_row(row: &rusqlite::Row) -> rusqlite::Result<ArtistRow> {
    Ok(ArtistRow {
        id: row.get(0)?,
        name: row.get(1)?,
        sort_name: row.get(2)?,
        remote_id: row.get(3)?,
        album_count: row.get(4)?,
        track_count: row.get(5)?,
    })
}

/// One artist, with its counts. `None` if it owns no albums.
pub fn get_artist(conn: &Connection, artist_id: i64) -> Result<Option<ArtistRow>, DbError> {
    Ok(conn
        .query_row(
            "SELECT a.id, a.name, a.sort_name, a.remote_id,
                    COUNT(DISTINCT al.id), COUNT(t.id)
             FROM artists a
             INNER JOIN albums al ON al.artist_id = a.id
             LEFT JOIN tracks t ON t.album_id = al.id
             WHERE a.id = ?1
             GROUP BY a.id",
            params![artist_id],
            artist_row,
        )
        .ok())
}

/// Find artists by name (case-insensitive substring match).
pub fn find_artists(conn: &Connection, query: &str) -> Result<Vec<ArtistRow>, DbError> {
    list_artists(
        conn,
        &ArtistQuery {
            search: Some(query),
            ..Default::default()
        },
    )
}

/// Every album artist, sorted by name.
pub fn all_artists(conn: &Connection) -> Result<Vec<ArtistRow>, DbError> {
    list_artists(conn, &ArtistQuery::default())
}

/// Record what the server knows about an artist beyond its name.
///
/// Fills blanks rather than overwriting: a local scan may have set a sort name
/// from tags, and the server's should not clobber it. Matched on `remote_id`,
/// which the artist already has from the track upserts.
pub fn enrich_remote_artist(
    conn: &Connection,
    remote_id: &str,
    mbid: Option<&str>,
    sort_name: Option<&str>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE artists SET
             mbid      = COALESCE(mbid, ?2),
             sort_name = COALESCE(sort_name, ?3)
         WHERE remote_id = ?1",
        params![remote_id, mbid, sort_name],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    /// Artists only appear once they own an album, so the fixture goes in
    /// through a track.
    fn stocked_db() -> Database {
        use crate::db::queries::{sample_meta, upsert_track};
        let db = test_db();
        for (i, artist) in ["Autechre", "Boards of Canada", "Coil", "Dopplereffekt"]
            .iter()
            .enumerate()
        {
            let mut m = sample_meta("t", artist, "Album");
            m.path = Some(format!("/music/{i}/t.flac"));
            upsert_track(&db.conn, &m).unwrap();
        }
        db
    }

    #[test]
    fn paging_walks_the_listing_without_repeating() {
        let db = stocked_db();
        let page = |offset| {
            list_artists(
                &db.conn,
                &ArtistQuery {
                    limit: Some(2),
                    offset,
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect::<Vec<_>>()
        };
        assert_eq!(page(0), ["Autechre", "Boards of Canada"]);
        assert_eq!(page(2), ["Coil", "Dopplereffekt"]);
        assert!(page(4).is_empty());
    }

    #[test]
    fn favourites_only_lists_what_was_hearted() {
        use crate::db::queries::toggle_favourite_artist;
        let db = stocked_db();
        toggle_favourite_artist(&db.conn, "Coil").unwrap();
        let rows = list_artists(
            &db.conn,
            &ArtistQuery {
                favourites_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Coil"]
        );
    }

    #[test]
    fn one_artist_carries_its_counts() {
        let db = stocked_db();
        let id = find_artists(&db.conn, "Coil").unwrap()[0].id;
        let artist = get_artist(&db.conn, id)
            .unwrap()
            .expect("Coil owns an album");
        assert_eq!(artist.name, "Coil");
        assert_eq!(artist.album_count, 1);
        assert_eq!(artist.track_count, 1);
        assert!(get_artist(&db.conn, 9999).unwrap().is_none());
    }

    #[test]
    fn test_artist_create_and_dedup() {
        let db = test_db();
        let id1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let id2 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        assert_eq!(id1, id2);

        let id3 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();
        assert_ne!(id1, id3);
    }
}

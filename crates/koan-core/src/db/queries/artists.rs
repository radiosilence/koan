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
///
/// Names that differ only in letter case are one artist: tags spell the same
/// act "The Squire Of Gothos" on one record and "of" on the next, and two rows
/// split its albums from its tracks. An exact match wins over a case-folded one.
pub fn get_or_create_artist(
    conn: &Connection,
    name: &str,
    remote_id: Option<&str>,
) -> Result<i64, DbError> {
    // Try to find existing.
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE
             ORDER BY name = ?1 DESC, id LIMIT 1",
            params![name],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        // The server's current id wins: one that renumbers its library would
        // otherwise leave the artist under an id it no longer answers to.
        if let Some(rid) = remote_id {
            conn.execute(
                "UPDATE artists SET remote_id = ?1 WHERE id = ?2 AND remote_id IS NOT ?1",
                params![rid, id],
            )?;
            super::adopt_uid(conn, super::UidKind::Artist, id, rid)?;
        }
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO artists (name, remote_id) VALUES (?1, ?2)",
        params![name, remote_id],
    )?;
    let id = conn.last_insert_rowid();
    if let Some(rid) = remote_id {
        super::adopt_uid(conn, super::UidKind::Artist, id, rid)?;
    }
    Ok(id)
}

/// How to order the artist listing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArtistOrder {
    /// By name, as it reads. Sort names from tags are too erratic to order by.
    #[default]
    Name,
    /// Most albums first.
    AlbumCount,
    /// The artist whose newest album arrived most recently first.
    RecentlyAdded,
    /// Insertion order, for an offset walk that must not skip or repeat.
    Id,
}

impl ArtistOrder {
    fn clause(self) -> &'static str {
        match self {
            Self::Name => "a.name COLLATE LIBRARY",
            Self::AlbumCount => "COUNT(DISTINCT al.id) DESC, a.name COLLATE LIBRARY",
            Self::RecentlyAdded => "COALESCE(MAX(al.added_at), '') DESC, a.name COLLATE LIBRARY",
            Self::Id => "a.id",
        }
    }
}

/// What to list. Artists are always album artists — a track-only credit (a
/// featured guest) appears inline in the queue, not as a shelf of its own.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArtistQuery<'a> {
    /// Case-insensitive substring over the name.
    pub search: Option<&'a str>,
    /// Only artists this user has favourited.
    pub favourites_of: Option<i64>,
    /// Albums that count; an artist with none left is not listed, and the
    /// counts are of what is left.
    pub filter: super::albums::AlbumFilter<'a>,
    pub order: ArtistOrder,
    /// `None` for the whole listing. A client that scrolls should page.
    pub limit: Option<u32>,
    pub offset: u32,
}

/// Artists with their album and track counts, narrowed, ordered and paged by
/// the database.
pub fn list_artists(conn: &Connection, q: &ArtistQuery) -> Result<Vec<ArtistRow>, DbError> {
    let mut sql = String::from(
        "SELECT a.id, a.name, a.sort_name, a.remote_id,
                COUNT(DISTINCT al.id), COUNT(t.id)
         FROM artists a
         INNER JOIN albums al ON al.artist_id = a.id
         LEFT JOIN tracks t ON t.album_id = al.id",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(user) = q.favourites_of {
        params.push(Box::new(super::auth::resolve_user(conn, user)?));
        sql.push_str(" JOIN favourite_artists f ON f.artist_name = a.name AND f.user_id = ?");
    }
    let mut wheres: Vec<String> = Vec::new();
    if let Some(query) = q.search {
        params.push(Box::new(format!("%{}%", escape_like(query))));
        wheres.push("a.name LIKE ? COLLATE NOCASE ESCAPE '\\'".into());
    }
    q.filter.push(&mut wheres, &mut params);
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(" GROUP BY a.id ORDER BY ");
    sql.push_str(q.order.clause());
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

/// One artist, with its counts. An artist credited only on other people's
/// albums (a feature, a compilation track) owns none, and is still an artist:
/// its track count is every track credited to it or on its albums.
pub fn get_artist(conn: &Connection, artist_id: i64) -> Result<Option<ArtistRow>, DbError> {
    Ok(conn
        .query_row(
            "SELECT a.id, a.name, a.sort_name, a.remote_id,
                    (SELECT COUNT(*) FROM albums WHERE artist_id = a.id),
                    (SELECT COUNT(*) FROM tracks
                      WHERE artist_id = a.id
                         OR album_id IN (SELECT id FROM albums WHERE artist_id = a.id))
             FROM artists a
             WHERE a.id = ?1",
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
        toggle_favourite_artist(&db.conn, crate::db::queries::LOCAL_USER, "Coil").unwrap();
        let rows = list_artists(
            &db.conn,
            &ArtistQuery {
                favourites_of: Some(crate::db::queries::LOCAL_USER),
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
    fn spellings_that_differ_only_in_case_are_one_artist() {
        let db = stocked_db();
        let a = get_or_create_artist(&db.conn, "The Squire of Gothos", None).unwrap();
        let b = get_or_create_artist(&db.conn, "The Squire Of Gothos", None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn an_artist_with_tracks_but_no_albums_is_still_an_artist() {
        let db = stocked_db();
        let guest = get_or_create_artist(&db.conn, "A Guest", None).unwrap();
        let album: i64 = db
            .conn
            .query_row("SELECT id FROM albums LIMIT 1", [], |r| r.get(0))
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO tracks (title, album_id, artist_id, path) VALUES ('Feature', ?1, ?2, '/f.flac')",
                params![album, guest],
            )
            .unwrap();
        let artist = get_artist(&db.conn, guest)
            .unwrap()
            .expect("credited on a track");
        assert_eq!((artist.album_count, artist.track_count), (0, 1));
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

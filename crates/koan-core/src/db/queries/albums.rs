use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::AlbumRow;

/// The album column list, read the same way by every query that selects it.
fn album_row(row: &rusqlite::Row) -> rusqlite::Result<AlbumRow> {
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
}

/// Get or create an album by title + artist. Returns the album ID.
#[allow(clippy::too_many_arguments)]
pub fn get_or_create_album(
    conn: &Connection,
    title: &str,
    artist_id: i64,
    date: Option<&str>,
    total_discs: Option<i32>,
    total_tracks: Option<i32>,
    codec: Option<&str>,
    label: Option<&str>,
    // `added_at`: remote sync passes the server's `created`, a local scan the
    // earliest mtime among the album's files. Both ISO 8601 UTC, so the two
    // sources sort against each other.
    remote_id: Option<&str>,
    added_at: Option<&str>,
) -> Result<i64, DbError> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM albums WHERE title = ?1 AND artist_id = ?2",
            params![title, artist_id],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        // Update mutable fields so rescans pick up format upgrades (e.g. MP3→FLAC),
        // corrected dates, or newly-added remote IDs.
        conn.execute(
            "UPDATE albums SET
                codec      = COALESCE(?1, codec),
                date       = COALESCE(?2, date),
                label      = COALESCE(?3, label),
                remote_id  = COALESCE(?4, remote_id),
                -- Earliest wins. A record acquired over months should date
                -- from its first file, not its last, and filling only would
                -- freeze whichever file the first scan happened to reach.
                added_at   = MIN(COALESCE(added_at, ?5), COALESCE(?5, added_at))
             WHERE id = ?6",
            params![codec, date, label, remote_id, added_at, id],
        )?;
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO albums (title, artist_id, date, total_discs, total_tracks, codec, label, remote_id, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![title, artist_id, date, total_discs, total_tracks, codec, label, remote_id, added_at],
    )?;
    Ok(conn.last_insert_rowid())
}

/// How a listing of albums is ordered.
///
/// In SQL rather than over the returned rows, because a listing that is read a
/// page at a time has to be ordered before it is cut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AlbumOrder {
    /// Artist, then release date, then title — how a shelf reads.
    #[default]
    ArtistThenDate,
    /// Release date, then title. A discography, in the order it happened.
    Date,
    /// Newest acquisition first. What a browser should open on: the record you
    /// just added is the one you were looking for.
    RecentlyAdded,
    Title,
    /// Newest release first.
    YearDesc,
    /// Seeded, so every page of one shuffle belongs to the same shuffle. A new
    /// seed is a new order — that is what the reshuffle button asks for.
    Random(i64),
}

impl AlbumOrder {
    fn clause(self) -> &'static str {
        match self {
            Self::ArtistThenDate => "a.name COLLATE LIBRARY, al.date, al.title COLLATE LIBRARY",
            Self::Date => "al.date, al.title COLLATE LIBRARY",
            // Albums predating the added_at column sort last rather than first,
            // which is what a NULL would do.
            Self::RecentlyAdded => {
                "COALESCE(al.added_at, '') DESC, a.name COLLATE LIBRARY, al.title COLLATE LIBRARY"
            }
            Self::Title => "al.title COLLATE LIBRARY, a.name COLLATE LIBRARY, al.date",
            Self::YearDesc => {
                "COALESCE(CAST(substr(al.date, 1, 4) AS INTEGER), 0) DESC, \
                               a.name COLLATE LIBRARY, al.title COLLATE LIBRARY"
            }
            Self::Random(_) => "koan_shuffle(al.id, ?)",
        }
    }
}

/// What to list. Everything optional, so one query answers the browser, the
/// search field, an artist's discography and the favourites page.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlbumQuery<'a> {
    pub artist_id: Option<i64>,
    /// Case-insensitive substring over the album title and the artist name.
    pub search: Option<&'a str>,
    pub order: AlbumOrder,
    /// Favourited records only.
    pub favourites_only: bool,
    /// `None` for the whole listing. A client that scrolls should page.
    pub limit: Option<u32>,
    pub offset: u32,
}

/// Albums, narrowed, ordered and paged by the database.
///
/// The narrowing belongs here rather than in each client: every front end wants
/// the same answer, and the ones that filtered a fully-loaded list in their own
/// language paid for reading the whole table to throw most of it away. Matching
/// is ASCII case-insensitive, like `find_artists` — SQLite's `NOCASE` does not
/// fold accented letters, so `MOTLEY` finds `Motley` but `MÖTLEY` does not find
/// `Mötley`.
pub fn list_albums(conn: &Connection, q: &AlbumQuery) -> Result<Vec<AlbumRow>, DbError> {
    let mut sql = String::from(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id",
    );
    if q.favourites_only {
        sql.push_str(
            " JOIN favourite_albums f
                ON f.artist_name = a.name AND f.album_title = al.title",
        );
    }

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut wheres: Vec<&str> = Vec::new();
    if let Some(id) = q.artist_id {
        params.push(Box::new(id));
        wheres.push("al.artist_id = ?");
    }
    if let Some(query) = q.search {
        let pattern = format!("%{}%", super::artists::escape_like(query));
        // Bound twice rather than once: positional parameters are cheaper to
        // keep straight than named ones across an assembled query.
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
        wheres.push(
            "(al.title LIKE ? COLLATE NOCASE ESCAPE '\\'
              OR a.name LIKE ? COLLATE NOCASE ESCAPE '\\')",
        );
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }

    sql.push_str(" ORDER BY ");
    if let AlbumOrder::Random(seed) = q.order {
        params.push(Box::new(seed));
    }
    sql.push_str(q.order.clause());

    if let Some(limit) = q.limit {
        params.push(Box::new(limit as i64));
        params.push(Box::new(q.offset as i64));
        sql.push_str(" LIMIT ? OFFSET ?");
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), album_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get albums for a specific artist, ordered chronologically.
pub fn albums_for_artist(conn: &Connection, artist_id: i64) -> Result<Vec<AlbumRow>, DbError> {
    list_albums(
        conn,
        &AlbumQuery {
            artist_id: Some(artist_id),
            order: AlbumOrder::Date,
            ..Default::default()
        },
    )
}

/// Get a single album by ID.
pub fn get_album(conn: &Connection, album_id: i64) -> Result<Option<AlbumRow>, DbError> {
    let result = conn
        .query_row(
            "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                    al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at
             FROM albums al
             LEFT JOIN artists a ON al.artist_id = a.id
             WHERE al.id = ?1",
            params![album_id],
            album_row,
        )
        .ok();
    Ok(result)
}

/// Get the date string for an album by ID.
pub fn album_date(conn: &Connection, album_id: i64) -> Result<Option<String>, DbError> {
    Ok(conn
        .query_row(
            "SELECT date FROM albums WHERE id = ?1",
            params![album_id],
            |row| row.get(0),
        )
        .ok()
        .flatten())
}

/// Albums whose title or artist matches, case-insensitive substring.
pub fn find_albums(conn: &Connection, query: &str) -> Result<Vec<AlbumRow>, DbError> {
    list_albums(
        conn,
        &AlbumQuery {
            search: Some(query),
            ..Default::default()
        },
    )
}

/// Get all albums with their artist name, sorted.
pub fn all_albums(conn: &Connection) -> Result<Vec<AlbumRow>, DbError> {
    list_albums(conn, &AlbumQuery::default())
}

/// Record what the server knows about an album beyond what a track carries.
///
/// `get_or_create_album` is reached through a track and only ever sees what a
/// file's tags say. Track totals, the record label and the MusicBrainz id are
/// properties of the release, and the server hands all three over in the same
/// response the sync already paged through.
///
/// Fills blanks rather than overwriting, so a locally-scanned album keeps what
/// its tags said.
pub fn enrich_remote_album(
    conn: &Connection,
    remote_id: &str,
    mbid: Option<&str>,
    sort_name: Option<&str>,
    total_tracks: Option<i32>,
    label: Option<&str>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE albums SET
             mbid         = COALESCE(mbid, ?2),
             sort_name    = COALESCE(sort_name, ?3),
             total_tracks = COALESCE(total_tracks, ?4),
             label        = COALESCE(label, ?5)
         WHERE remote_id = ?1",
        params![remote_id, mbid, sort_name, total_tracks, label],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::get_or_create_artist;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_album_create_and_dedup() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "Boards of Canada", None).unwrap();
        let a1 = get_or_create_album(
            &db.conn,
            "Music Has the Right to Children",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
            None,
        )
        .unwrap();
        let a2 = get_or_create_album(
            &db.conn,
            "Music Has the Right to Children",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(a1, a2);
    }

    #[test]
    fn test_album_codec_updated_on_format_upgrade() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "WAGDUG FUTURISTIC UNITY", None).unwrap();

        // First scan: album indexed as MP3.
        let id1 = get_or_create_album(
            &db.conn,
            "HAKAI",
            artist,
            Some("2008"),
            None,
            None,
            Some("MP3"),
            None,
            None,
            None,
        )
        .unwrap();

        let codec: Option<String> = db
            .conn
            .query_row(
                "SELECT codec FROM albums WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(codec.as_deref(), Some("MP3"));

        // Re-scan after upgrading MP3→FLAC: same album, new codec.
        let id2 = get_or_create_album(
            &db.conn,
            "HAKAI",
            artist,
            Some("2008"),
            None,
            None,
            Some("FLAC"),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(id1, id2, "should return the same album ID");

        let codec: Option<String> = db
            .conn
            .query_row(
                "SELECT codec FROM albums WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            codec.as_deref(),
            Some("FLAC"),
            "album codec should be updated after format upgrade"
        );
    }

    #[test]
    fn test_album_codec_not_nulled_by_missing_codec() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "Boards of Canada", None).unwrap();

        // First scan with codec.
        let id = get_or_create_album(
            &db.conn,
            "MHTRTC",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
            None,
        )
        .unwrap();

        // Re-encounter with no codec (e.g. remote sync without codec info).
        get_or_create_album(
            &db.conn,
            "MHTRTC",
            artist,
            Some("1998"),
            None,
            None,
            None, // no codec
            None, // no label
            None,
            None,
        )
        .unwrap();

        let (codec, label): (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT codec, label FROM albums WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            codec.as_deref(),
            Some("FLAC"),
            "codec should not be nulled by a None value"
        );
        assert_eq!(
            label.as_deref(),
            Some("Warp"),
            "label should not be nulled by a None value"
        );
    }

    /// Six albums across two artists, so a page is smaller than the listing.
    fn stocked_db() -> Database {
        use crate::db::queries::{sample_meta, upsert_track};
        let db = test_db();
        for (i, (artist, album)) in [
            ("Autechre", "Amber"),
            ("Autechre", "Tri Repetae"),
            ("Autechre", "Confield"),
            ("Boards of Canada", "Geogaddi"),
            ("Boards of Canada", "Twoism"),
            ("Coil", "Horse Rotorvator"),
        ]
        .iter()
        .enumerate()
        {
            let mut m = sample_meta("t", artist, album);
            m.path = Some(format!("/music/{album}/t.flac"));
            m.date = Some(format!("199{i}"));
            upsert_track(&db.conn, &m).unwrap();
        }
        db
    }

    #[test]
    fn paging_walks_the_listing_without_repeating() {
        let db = stocked_db();
        let page = |offset| {
            list_albums(
                &db.conn,
                &AlbumQuery {
                    limit: Some(2),
                    offset,
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|a| a.title)
            .collect::<Vec<_>>()
        };
        let whole = all_albums(&db.conn)
            .unwrap()
            .into_iter()
            .map(|a| a.title)
            .collect::<Vec<_>>();
        assert_eq!([page(0), page(2), page(4)].concat(), whole);
        assert!(
            page(6).is_empty(),
            "a page past the end is empty, not wrapped"
        );
    }

    #[test]
    fn search_narrows_on_title_or_artist() {
        let db = stocked_db();
        let titles = |q| {
            find_albums(&db.conn, q)
                .unwrap()
                .into_iter()
                .map(|a| a.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("geogaddi"), ["Geogaddi"]);
        assert_eq!(titles("autechre").len(), 3, "matched on the artist name");
    }

    /// The reason the seed exists: page two has to belong to the same shuffle
    /// as page one, or scrolling repeats and drops records.
    #[test]
    fn a_seeded_shuffle_pages_consistently() {
        let db = stocked_db();
        let shuffled = |seed, limit, offset| {
            list_albums(
                &db.conn,
                &AlbumQuery {
                    order: AlbumOrder::Random(seed),
                    limit,
                    offset,
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect::<Vec<_>>()
        };

        let whole = shuffled(42, None, 0);
        assert_eq!(
            [shuffled(42, Some(4), 0), shuffled(42, Some(4), 4)].concat(),
            whole
        );
        assert_ne!(shuffled(43, None, 0), whole, "a new seed is a new order");
        assert_eq!(whole.len(), 6, "a shuffle drops nothing");
    }

    #[test]
    fn favourites_only_lists_what_was_hearted() {
        use crate::db::queries::toggle_favourite_album;
        let db = stocked_db();
        toggle_favourite_album(&db.conn, "Coil", "Horse Rotorvator").unwrap();
        let rows = list_albums(
            &db.conn,
            &AlbumQuery {
                favourites_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            ["Horse Rotorvator"]
        );
    }

    #[test]
    fn test_all_albums_and_tracks() {
        use crate::db::queries::{sample_meta, tracks_for_album, upsert_track};

        let db = test_db();
        let mut m1 = sample_meta("Track1", "Artist1", "Album1");
        m1.track_number = Some(1);
        let mut m2 = sample_meta("Track2", "Artist1", "Album1");
        m2.track_number = Some(2);
        m2.path = Some("/music/Album1/Track2.flac".into());
        upsert_track(&db.conn, &m1).unwrap();
        upsert_track(&db.conn, &m2).unwrap();

        let albums = all_albums(&db.conn).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Album1");

        let tracks = tracks_for_album(&db.conn, albums[0].id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].track_number, Some(1));
        assert_eq!(tracks[1].track_number, Some(2));
    }
}

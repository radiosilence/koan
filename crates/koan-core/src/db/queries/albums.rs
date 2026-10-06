use rusqlite::{Connection, OptionalExtension, params};

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
        on_device: None,
    })
}

/// The album a track with these names belongs to, made if there is none.
///
/// An album is its title and album artist, compared as matching compares
/// names, and its release when one is known: two editions with the same title
/// and their own MusicBrainz release ids are two albums. A track naming no
/// release joins the album of its names that has none either, or else the
/// oldest; one naming a release joins the album with that release, or one
/// that has none yet, which takes it. The release id is never overwritten, so
/// files of two editions cannot trade it back and forth.
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
    release: Option<&str>,
    // `added_at`: remote sync passes the server's `created`, a local scan the
    // earliest mtime among the album's files. Both ISO 8601 UTC, so the two
    // sources sort against each other.
    added_at: Option<&str>,
) -> Result<i64, DbError> {
    let release = release.filter(|r| !r.is_empty());
    let key = super::sources::fold(title);
    type Stored = (
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let candidates: Vec<Stored> = conn
        .prepare_cached(
            "SELECT id, mbid, codec, date, label, added_at FROM albums
             WHERE title_key = ?1 AND artist_id = ?2 ORDER BY id",
        )?
        .query_map(params![key, artist_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let unclaimed = || candidates.iter().find(|c| c.1.is_none());
    let existing = match release {
        Some(release) => candidates
            .iter()
            .find(|c| c.1.as_deref() == Some(release))
            .or_else(unclaimed),
        None => unclaimed().or(candidates.first()),
    };

    if let Some((id, s_mbid, s_codec, s_date, s_label, s_added_at)) = existing {
        // Update mutable fields so rescans pick up format upgrades (e.g. MP3→FLAC)
        // or corrected dates. Every track of the album passes through here, so
        // the row is only written when one of them brings something new.
        // Earliest wins. A record acquired over months should date from its
        // first file, not its last, and filling only would freeze whichever
        // file the first scan happened to reach.
        let earliest = match (added_at, s_added_at.as_deref()) {
            (Some(new), Some(stored)) => Some(new.min(stored)),
            (new, stored) => new.or(stored),
        };
        let merged = (
            codec.or(s_codec.as_deref()),
            date.or(s_date.as_deref()),
            label.or(s_label.as_deref()),
            s_mbid.as_deref().or(release),
            earliest,
        );
        let stored = (
            s_codec.as_deref(),
            s_date.as_deref(),
            s_label.as_deref(),
            s_mbid.as_deref(),
            s_added_at.as_deref(),
        );
        if merged != stored {
            conn.prepare_cached(
                "UPDATE albums SET codec = ?1, date = ?2, label = ?3, mbid = ?4, added_at = ?5
                 WHERE id = ?6",
            )?
            .execute(params![
                merged.0, merged.1, merged.2, merged.3, merged.4, id
            ])?;
        }
        return Ok(*id);
    }

    conn.prepare_cached(
        "INSERT INTO albums (title, title_key, artist_id, date, total_discs, total_tracks, codec,
                             label, mbid, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?
    .execute(params![
        title,
        key,
        artist_id,
        date,
        total_discs,
        total_tracks,
        codec,
        label,
        release,
        added_at
    ])?;
    Ok(conn.last_insert_rowid())
}

/// Fold album `gone` into `keep`: its tracks, favourites, ratings and
/// shares move across, `keep` fills its gaps from it, and it is deleted. The koan server's
/// uid goes with the server's id, so the album stays one id on every device.
pub(crate) fn merge_albums(conn: &Connection, keep: i64, gone: i64) -> rusqlite::Result<()> {
    let server_uid: Option<String> = conn
        .query_row(
            "SELECT g.uid FROM albums g, albums k WHERE g.id = ?1 AND k.id = ?2
               AND g.uid = g.remote_id AND k.uid IS NOT k.remote_id",
            params![gone, keep],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute_batch("SAVEPOINT merge_albums")?;
    let merged = (|| {
        for sql in [
            "UPDATE tracks SET album_id = ?1 WHERE album_id = ?2",
            "UPDATE OR IGNORE favourite_albums SET album_id = ?1 WHERE album_id = ?2",
            "UPDATE OR IGNORE album_ratings SET album_id = ?1 WHERE album_id = ?2",
            "UPDATE shares SET subject_id = ?1 WHERE kind = 'album' AND subject_id = ?2",
            "UPDATE albums SET
                 remote_id = COALESCE(remote_id, (SELECT remote_id FROM albums WHERE id = ?2)),
                 mbid = COALESCE(mbid, (SELECT mbid FROM albums WHERE id = ?2)),
                 date = COALESCE(date, (SELECT date FROM albums WHERE id = ?2)),
                 label = COALESCE(label, (SELECT label FROM albums WHERE id = ?2)),
                 codec = COALESCE(codec, (SELECT codec FROM albums WHERE id = ?2)),
                 sort_name = COALESCE(sort_name, (SELECT sort_name FROM albums WHERE id = ?2)),
                 total_discs = COALESCE(total_discs, (SELECT total_discs FROM albums WHERE id = ?2)),
                 total_tracks = COALESCE(total_tracks, (SELECT total_tracks FROM albums WHERE id = ?2)),
                 added_at = MIN(COALESCE(added_at, (SELECT added_at FROM albums WHERE id = ?2)),
                                COALESCE((SELECT added_at FROM albums WHERE id = ?2), added_at))
               WHERE id = ?1",
        ] {
            conn.execute(sql, params![keep, gone])?;
        }
        for sql in [
            "DELETE FROM favourite_albums WHERE album_id = ?1",
            "DELETE FROM album_ratings WHERE album_id = ?1",
            "DELETE FROM albums WHERE id = ?1",
        ] {
            conn.execute(sql, params![gone])?;
        }
        if let Some(uid) = &server_uid {
            conn.execute(
                "UPDATE albums SET uid = ?1 WHERE id = ?2",
                params![uid, keep],
            )?;
        }
        Ok(())
    })();
    match merged {
        Ok(()) => conn.execute_batch("RELEASE merge_albums"),
        Err(e) => {
            conn.execute_batch("ROLLBACK TO merge_albums; RELEASE merge_albums")?;
            Err(e)
        }
    }
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
    /// Most recently played first. Only with `AlbumQuery::played`, which is
    /// what knows when; without it, `RecentlyAdded`.
    LastPlayed,
    /// Insertion order. The one order a new album cannot land in the middle
    /// of, which is what makes an offset walk over the whole list exact.
    Id,
    /// Fully on this device first, then by how much is, then the most
    /// recently downloaded.
    Downloaded,
    /// The closest match to `AlbumQuery::search` first, by title or artist
    /// (see `search::match_rank`), then as `ArtistThenDate`. Without a search,
    /// in the order of `AlbumQuery::ids`; without either, `RecentlyAdded`.
    Relevance,
}

impl AlbumOrder {
    fn clause(self) -> &'static str {
        match self {
            Self::ArtistThenDate => "a.name COLLATE LIBRARY, al.date, al.title COLLATE LIBRARY",
            Self::Date => "al.date, al.title COLLATE LIBRARY",
            // Albums predating the added_at column sort last rather than first,
            // which is what a NULL would do.
            Self::RecentlyAdded | Self::Relevance => {
                "COALESCE(al.added_at, '') DESC, a.name COLLATE LIBRARY, al.title COLLATE LIBRARY"
            }
            Self::Title => "al.title COLLATE LIBRARY, a.name COLLATE LIBRARY, al.date",
            Self::YearDesc => {
                "COALESCE(CAST(substr(al.date, 1, 4) AS INTEGER), 0) DESC, \
                               a.name COLLATE LIBRARY, al.title COLLATE LIBRARY"
            }
            Self::Random(_) => "koan_shuffle(al.id, ?)",
            Self::LastPlayed => "p.last DESC, p.last_id DESC",
            Self::Id => "al.id",
            Self::Downloaded => {
                "have = total DESC, CAST(have AS REAL) / total DESC, fetched DESC, al.id"
            }
        }
    }
}

/// Codecs that lose nothing, as the indexer names them.
pub const LOSSLESS_CODECS: [&str; 5] = ["FLAC", "ALAC", "WAV", "AIFF", "PCM"];

/// How much of a record is on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnDevice {
    /// Tracks with a file here: downloaded, or in the library.
    pub have: u32,
    pub total: u32,
}

/// What a listing narrowed to the device selects after the album columns, and
/// what `AlbumOrder::Downloaded` sorts by. Correlated rather than joined, so
/// only the records that pass the narrowing are counted.
const ON_DEVICE_COLUMNS: &str = ",
    (SELECT SUM(COALESCE(d.cached_path, d.path) IS NOT NULL) FROM tracks d WHERE d.album_id = al.id) AS have,
    (SELECT COUNT(*) FROM tracks d WHERE d.album_id = al.id) AS total,
    (SELECT MAX(COALESCE(d.cache_download_date, 0)) FROM tracks d WHERE d.album_id = al.id) AS fetched";

/// Narrowing by what the records are, shared by the album and artist listings:
/// an artist passes when any of their albums does.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlbumFilter<'a> {
    /// Only records in a codec from `LOSSLESS_CODECS`.
    pub lossless: bool,
    /// Only records in this codec, matched as the indexer names it.
    pub codec: Option<&'a str>,
    /// Release year bounds, inclusive. Records without a date are left out
    /// when either is set.
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    /// Records with at least one track tagged with this genre.
    pub genre: Option<&'a str>,
    /// Records with at least one track that can play here: downloaded, or a
    /// file in the library. What offline narrows to.
    pub on_device: bool,
}

impl AlbumFilter<'_> {
    /// Conditions on the album aliased `al`.
    pub(crate) fn push(
        &self,
        wheres: &mut Vec<String>,
        params: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) {
        if self.lossless {
            wheres.push(format!(
                "al.codec IN ({})",
                vec!["?"; LOSSLESS_CODECS.len()].join(",")
            ));
            params.extend(
                LOSSLESS_CODECS
                    .iter()
                    .map(|c| Box::new(*c) as Box<dyn rusqlite::ToSql>),
            );
        }
        if let Some(codec) = self.codec {
            wheres.push("al.codec = ? COLLATE NOCASE".into());
            params.push(Box::new(codec.to_owned()));
        }
        let year = "CAST(substr(al.date, 1, 4) AS INTEGER)";
        if let Some(from) = self.year_from {
            wheres.push(format!("{year} >= ?"));
            params.push(Box::new(from));
        }
        if let Some(to) = self.year_to {
            wheres.push(format!("al.date IS NOT NULL AND {year} <= ?"));
            params.push(Box::new(to));
        }
        if self.on_device {
            wheres.push(
                "EXISTS (SELECT 1 FROM tracks d WHERE d.album_id = al.id
                         AND COALESCE(d.cached_path, d.path) IS NOT NULL)"
                    .into(),
            );
        }
        if let Some(genre) = self.genre {
            wheres.push(
                "EXISTS (SELECT 1 FROM tracks g WHERE g.album_id = al.id AND g.genre = ? COLLATE NOCASE)"
                    .into(),
            );
            params.push(Box::new(genre.to_owned()));
        }
    }
}

/// What to list. Everything optional, so one query answers the browser, the
/// search field, an artist's discography and the favourites page.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlbumQuery<'a> {
    /// Only these albums.
    pub ids: Option<&'a [i64]>,
    pub artist_id: Option<i64>,
    /// Case-insensitive substring over the album title and the artist name.
    /// When no album holds it, the closest fuzzy matches instead
    /// (`search::fuzzy_ids`).
    pub search: Option<&'a str>,
    pub order: AlbumOrder,
    /// Only records this user has favourited.
    pub favourites_of: Option<i64>,
    /// Only records with a track played since then.
    pub played: Option<super::history::PlayedSince>,
    pub filter: AlbumFilter<'a>,
    /// `None` for the whole listing. A client that scrolls should page.
    pub limit: Option<u32>,
    pub offset: u32,
}

/// Albums, narrowed, ordered and paged by the database.
///
/// The narrowing belongs here rather than in each client: every front end wants
/// the same answer, and one filtering a fully-loaded list in its own language
/// pays for reading the whole table to throw most of it away. Matching
/// is ASCII case-insensitive, like `find_artists` — SQLite's `NOCASE` does not
/// fold accented letters, so `MOTLEY` finds `Motley` but `MÖTLEY` does not find
/// `Mötley`.
pub fn list_albums(conn: &Connection, q: &AlbumQuery) -> Result<Vec<AlbumRow>, DbError> {
    if let Some(ids) = fuzzy_fallback(conn, q)? {
        return list_albums(
            conn,
            &AlbumQuery {
                search: None,
                ids: Some(&ids),
                ..*q
            },
        );
    }
    let (body, mut params) = album_body(conn, q)?;
    let counted = q.filter.on_device || q.order == AlbumOrder::Downloaded;
    let mut sql = format!(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at{}
         {body} ORDER BY ",
        if counted { ON_DEVICE_COLUMNS } else { "" }
    );
    let order = match q.order {
        AlbumOrder::LastPlayed if q.played.is_none() => AlbumOrder::RecentlyAdded,
        order => order,
    };
    match (order, q.search, q.ids) {
        (AlbumOrder::Random(seed), ..) => {
            params.push(Box::new(seed));
            sql.push_str(order.clause());
        }
        (AlbumOrder::Relevance, Some(query), _) => {
            let binds = super::search::match_rank_binds(query);
            params.extend(
                binds
                    .iter()
                    .chain(&binds)
                    .map(|b| Box::new(b.clone()) as Box<dyn rusqlite::ToSql>),
            );
            sql.push_str(&format!(
                "min({}, {}), {}",
                super::search::match_rank("al.title"),
                super::search::match_rank("a.name"),
                AlbumOrder::ArtistThenDate.clause()
            ));
        }
        (AlbumOrder::Relevance, None, Some(ids)) => {
            params.push(Box::new(super::json_list(ids)));
            sql.push_str("(SELECT key FROM json_each(?) WHERE value = al.id)");
        }
        _ => sql.push_str(order.clause()),
    }

    if let Some(limit) = q.limit {
        params.push(Box::new(limit as i64));
        params.push(Box::new(q.offset as i64));
        sql.push_str(" LIMIT ? OFFSET ?");
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            let mut album = album_row(row)?;
            if counted {
                album.on_device = Some(OnDevice {
                    have: row.get(11)?,
                    total: row.get(12)?,
                });
            }
            Ok(album)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// How many albums `list_albums` would list for `q`, ignoring its paging: what
/// a "See all" says, from the same narrowing as the list it opens.
pub fn count_albums(conn: &Connection, q: &AlbumQuery) -> Result<u64, DbError> {
    if let Some(ids) = fuzzy_fallback(conn, q)? {
        return count_albums(
            conn,
            &AlbumQuery {
                search: None,
                ids: Some(&ids),
                ..*q
            },
        );
    }
    let (body, params) = album_body(conn, q)?;
    let n: i64 = conn.query_row(
        &format!("SELECT COUNT(*) {body}"),
        rusqlite::params_from_iter(params.iter()),
        |r| r.get(0),
    )?;
    Ok(n as u64)
}

/// What `q`'s search lists when no album holds it: the fuzzy matches, best
/// first. `None` when it has no search or the search finds something.
fn fuzzy_fallback(conn: &Connection, q: &AlbumQuery) -> Result<Option<Vec<i64>>, DbError> {
    let Some(query) = q.search else {
        return Ok(None);
    };
    let (body, params) = album_body(conn, q)?;
    let found: bool = conn.query_row(
        &format!("SELECT EXISTS (SELECT 1 {body})"),
        rusqlite::params_from_iter(params.iter()),
        |r| r.get(0),
    )?;
    if found {
        return Ok(None);
    }
    let mut ids = super::search::fuzzy_ids(conn, super::search::CorpusKind::Album, query)?;
    if let Some(only) = q.ids {
        ids.retain(|id| only.contains(id));
    }
    Ok(Some(ids))
}

/// The FROM and WHERE that `list_albums` and `count_albums` share, and their
/// parameters, in order.
fn album_body(
    conn: &Connection,
    q: &AlbumQuery,
) -> Result<(String, Vec<Box<dyn rusqlite::ToSql>>), DbError> {
    let mut sql = String::from(
        "FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(user) = q.favourites_of {
        params.push(Box::new(super::auth::resolve_user(conn, user)?));
        sql.push_str(" JOIN favourite_albums f ON f.album_id = al.id AND f.user_id = ?");
    }
    if let Some(played) = q.played {
        params.push(Box::new(super::auth::resolve_user(conn, played.user)?));
        params.push(Box::new(played.since));
        sql.push_str(
            " JOIN (SELECT t.album_id AS id, MAX(h.played_at) AS last, MAX(h.id) AS last_id
                      FROM play_history h JOIN tracks t ON t.id = h.track_id
                     WHERE h.user_id = ? AND h.played_at >= ? AND t.album_id IS NOT NULL
                     GROUP BY t.album_id) p ON p.id = al.id",
        );
    }
    let mut wheres: Vec<String> = Vec::new();
    if let Some(ids) = q.ids {
        params.push(Box::new(super::json_list(ids)));
        wheres.push("al.id IN (SELECT value FROM json_each(?))".into());
    }
    if let Some(id) = q.artist_id {
        params.push(Box::new(id));
        wheres.push("al.artist_id = ?".into());
    }
    if let Some(query) = q.search {
        let pattern = format!("%{}%", super::artists::escape_like(query));
        // Bound twice rather than once: positional parameters are cheaper to
        // keep straight than named ones across an assembled query.
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
        wheres.push(
            "(al.title LIKE ? COLLATE NOCASE ESCAPE '\\'
              OR a.name LIKE ? COLLATE NOCASE ESCAPE '\\')"
                .into(),
        );
    }
    q.filter.push(&mut wheres, &mut params);
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    Ok((sql, params))
}

/// How `played_albums` orders a user's listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayedOrder {
    /// Most recently played first.
    Recent,
    /// Most plays first, ties by most recent.
    Frequent,
}

/// Albums `user` has played, from play history, paged by the database.
/// Albums never played are not listed.
pub fn played_albums(
    conn: &Connection,
    user: i64,
    order: PlayedOrder,
    limit: u32,
    offset: u32,
) -> Result<Vec<AlbumRow>, DbError> {
    let order = match order {
        PlayedOrder::Recent => "p.last DESC",
        PlayedOrder::Frequent => "p.plays DESC, p.last DESC",
    };
    let sql = format!(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at
         FROM (SELECT t.album_id, MAX(h.played_at) AS last, COUNT(*) AS plays
                 FROM play_history h JOIN tracks t ON t.id = h.track_id
                WHERE h.user_id = ?1 AND t.album_id IS NOT NULL
                GROUP BY t.album_id) p
         JOIN albums al ON al.id = p.album_id
         LEFT JOIN artists a ON al.artist_id = a.id
         ORDER BY {order}, al.id
         LIMIT ?2 OFFSET ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(
            params![super::auth::resolve_user(conn, user)?, limit, offset],
            album_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// `user`'s rated albums, highest first and, within a rating, most recently
/// rated first: Subsonic's `highest` list.
pub fn highest_rated_albums(
    conn: &Connection,
    user: i64,
    limit: u32,
    offset: u32,
) -> Result<Vec<AlbumRow>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at
         FROM album_ratings r
         JOIN albums al ON al.id = r.album_id
         LEFT JOIN artists a ON al.artist_id = a.id
         WHERE r.user_id = ?1
         ORDER BY r.rating DESC, r.changed_at DESC, al.id
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt
        .query_map(
            params![super::auth::resolve_user(conn, user)?, limit, offset],
            album_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Genres by how many records carry them, most first: what a genre filter
/// offers. Blank tags are left out.
pub fn genres(conn: &Connection, limit: u32) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT genre FROM tracks
         WHERE genre IS NOT NULL AND TRIM(genre) != '' AND album_id IS NOT NULL
         GROUP BY genre COLLATE NOCASE
         ORDER BY COUNT(DISTINCT album_id) DESC, genre COLLATE NOCASE
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The codecs records are in, most common first.
pub fn album_codecs(conn: &Connection) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT codec FROM albums WHERE codec IS NOT NULL AND codec != ''
         GROUP BY codec ORDER BY COUNT(*) DESC, codec",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
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
        .prepare_cached(
            "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                    al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id,
                al.added_at
             FROM albums al
             LEFT JOIN artists a ON al.artist_id = a.id
             WHERE al.id = ?1",
        )
        .and_then(|mut stmt| stmt.query_row(params![album_id], album_row))
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

    /// Three records by two artists: a FLAC techno one from 1995, an MP3 rock
    /// one from 2005 and an ALAC techno one from 2010.
    fn filter_library() -> Database {
        let db = test_db();
        for (title, artist, album, codec, date, genre) in [
            ("A", "Rrose", "Early", "FLAC", "1995", "Techno"),
            ("B", "Band", "Middle", "MP3", "2005", "Rock"),
            ("C", "Rrose", "Late", "ALAC", "2010-03-01", "techno"),
        ] {
            let mut meta = crate::db::queries::sample_meta(title, artist, album);
            meta.codec = Some(codec.into());
            meta.date = Some(date.into());
            meta.genre = Some(genre.into());
            crate::db::queries::upsert_track(&db.conn, &meta).unwrap();
        }
        db
    }

    /// A record fully downloaded comes before one half there, and a record
    /// with nothing here is neither counted nor offered offline.
    #[test]
    fn on_device_counts_what_can_play_here() {
        let db = test_db();
        for (title, album) in [
            ("X1", "Whole"),
            ("X2", "Whole"),
            ("Y1", "Half"),
            ("Y2", "Half"),
            ("Z1", "None"),
        ] {
            let mut meta = crate::db::queries::sample_meta(title, "Artist", album);
            meta.path = Some(format!("/music/{title}.flac"));
            crate::db::queries::upsert_track(&db.conn, &meta).unwrap();
        }
        // Remote tracks, as a phone has them, two records' worth downloaded.
        db.conn
            .execute("UPDATE tracks SET path = NULL", [])
            .unwrap();
        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/cache/' || title WHERE title IN ('X1', 'X2', 'Y1')",
                [],
            )
            .unwrap();
        let album = |title: &str| -> i64 {
            db.conn
                .query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| {
                    r.get(0)
                })
                .unwrap()
        };

        let downloaded = list_albums(
            &db.conn,
            &AlbumQuery {
                filter: AlbumFilter {
                    on_device: true,
                    ..Default::default()
                },
                order: AlbumOrder::Downloaded,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            downloaded
                .iter()
                .map(|a| (a.id, a.on_device))
                .collect::<Vec<_>>(),
            vec![
                (album("Whole"), Some(OnDevice { have: 2, total: 2 })),
                (album("Half"), Some(OnDevice { have: 1, total: 2 })),
            ]
        );
        let offline = AlbumFilter {
            on_device: true,
            ..Default::default()
        };
        let mut listed = titles(&db, offline);
        listed.sort();
        assert_eq!(listed, vec!["Half", "Whole"]);
    }

    fn titles(db: &Database, filter: AlbumFilter) -> Vec<String> {
        list_albums(
            &db.conn,
            &AlbumQuery {
                filter,
                order: AlbumOrder::Date,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|a| a.title)
        .collect()
    }

    #[test]
    fn albums_filter_by_codec_year_and_genre_in_sql() {
        let db = filter_library();
        let lossless = AlbumFilter {
            lossless: true,
            ..Default::default()
        };
        assert_eq!(titles(&db, lossless), ["Early", "Late"]);
        let mp3 = AlbumFilter {
            codec: Some("mp3"),
            ..Default::default()
        };
        assert_eq!(titles(&db, mp3), ["Middle"]);
        let years = AlbumFilter {
            year_from: Some(2000),
            year_to: Some(2010),
            ..Default::default()
        };
        assert_eq!(titles(&db, years), ["Middle", "Late"]);
        let techno = AlbumFilter {
            genre: Some("TECHNO"),
            ..Default::default()
        };
        assert_eq!(titles(&db, techno), ["Early", "Late"]);
        let all = AlbumFilter {
            lossless: true,
            year_from: Some(2000),
            genre: Some("techno"),
            ..Default::default()
        };
        assert_eq!(titles(&db, all), ["Late"]);
        assert_eq!(
            genres(&db.conn, 10).unwrap().len(),
            2,
            "techno counted once"
        );
        assert_eq!(album_codecs(&db.conn).unwrap().len(), 3);
    }

    #[test]
    fn played_albums_follow_one_users_history() {
        use crate::db::queries::{LOCAL_USER, SOURCE_LOCAL, record_play_at};
        let db = filter_library();
        let track = |title: &str| -> i64 {
            db.conn
                .query_row("SELECT id FROM tracks WHERE title = ?1", [title], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        for (title, at) in [("A", 10), ("C", 30), ("A", 20)] {
            record_play_at(&db.conn, LOCAL_USER, track(title), at, None, SOURCE_LOCAL).unwrap();
        }
        let played = |order, limit, offset| {
            played_albums(&db.conn, LOCAL_USER, order, limit, offset)
                .unwrap()
                .into_iter()
                .map(|a| a.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(played(PlayedOrder::Recent, 10, 0), ["Late", "Early"]);
        assert_eq!(played(PlayedOrder::Frequent, 10, 0), ["Early", "Late"]);
        assert_eq!(played(PlayedOrder::Frequent, 1, 1), ["Late"]);
    }

    #[test]
    fn random_draws_narrow_in_sql() {
        use crate::db::queries::{RandomFilter, random_tracks_where};
        let db = filter_library();
        let draw = |filter: RandomFilter, count| {
            let mut titles: Vec<String> = random_tracks_where(&db.conn, count, &filter)
                .unwrap()
                .into_iter()
                .map(|t| t.title)
                .collect();
            titles.sort();
            titles
        };
        assert_eq!(draw(RandomFilter::default(), 10), ["A", "B", "C"]);
        assert_eq!(draw(RandomFilter::default(), 2).len(), 2);
        let techno = RandomFilter {
            genre: Some("TECHNO"),
            ..Default::default()
        };
        assert_eq!(draw(techno, 10), ["A", "C"]);
        let nineties = RandomFilter {
            year_from: Some(1990),
            year_to: Some(1999),
            ..Default::default()
        };
        assert_eq!(draw(nineties, 10), ["A"]);
    }

    #[test]
    fn artists_sort_and_count_what_the_filter_leaves() {
        use crate::db::queries::{ArtistOrder, ArtistQuery, list_artists};
        let db = filter_library();
        let names = |q: ArtistQuery| {
            list_artists(&db.conn, &q)
                .unwrap()
                .into_iter()
                .map(|a| (a.name, a.album_count))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(ArtistQuery {
                order: ArtistOrder::AlbumCount,
                ..Default::default()
            }),
            [("Rrose".to_string(), 2), ("Band".to_string(), 1)]
        );
        assert_eq!(
            names(ArtistQuery {
                filter: AlbumFilter {
                    year_from: Some(2000),
                    ..Default::default()
                },
                ..Default::default()
            }),
            [("Band".to_string(), 1), ("Rrose".to_string(), 1)]
        );
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
        let album: i64 = db
            .conn
            .query_row(
                "SELECT id FROM albums WHERE title = 'Horse Rotorvator'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        toggle_favourite_album(&db.conn, crate::db::queries::LOCAL_USER, album).unwrap();
        let rows = list_albums(
            &db.conn,
            &AlbumQuery {
                favourites_of: Some(crate::db::queries::LOCAL_USER),
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

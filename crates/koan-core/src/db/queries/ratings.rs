use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::auth::resolve_user;

// Ratings name rows, as favourites do, and follow them through merges. One to
// five; no rating is no row.

/// What a rating is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatingKind {
    Track,
    Album,
    Artist,
}

impl RatingKind {
    fn table(self) -> (&'static str, &'static str) {
        match self {
            Self::Track => ("track_ratings", "track_id"),
            Self::Album => ("album_ratings", "album_id"),
            Self::Artist => ("artist_ratings", "artist_id"),
        }
    }
}

/// Rate a row from 1 to 5, or clear its rating with 0. Values above 5 are
/// refused by the table.
pub fn set_rating(
    conn: &Connection,
    user: i64,
    kind: RatingKind,
    id: i64,
    rating: u8,
) -> rusqlite::Result<()> {
    let (table, column) = kind.table();
    let user = resolve_user(conn, user)?;
    if rating == 0 {
        conn.prepare_cached(&format!(
            "DELETE FROM {table} WHERE user_id = ?1 AND {column} = ?2"
        ))?
        .execute(params![user, id])?;
    } else {
        conn.prepare_cached(&format!(
            "INSERT INTO {table} (user_id, {column}, rating) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id, {column}) DO UPDATE SET
                 rating = excluded.rating, changed_at = datetime('now')"
        ))?
        .execute(params![user, id, rating])?;
    }
    Ok(())
}

/// `user`'s ratings of the given rows. Rows without one are absent.
pub fn ratings(
    conn: &Connection,
    user: i64,
    kind: RatingKind,
    ids: impl IntoIterator<Item = i64>,
) -> rusqlite::Result<HashMap<i64, u8>> {
    let ids: Vec<i64> = ids.into_iter().collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let (table, column) = kind.table();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {column}, rating FROM {table}
         WHERE user_id = ?1 AND {column} IN (SELECT value FROM json_each(?2))"
    ))?;
    let ids = serde_json::to_string(&ids).unwrap_or_default();
    let rows = stmt.query_map(params![resolve_user(conn, user)?, ids], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::LOCAL_USER;

    /// A library of one track, on one album by one artist, each with id 1.
    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Burial');
             INSERT INTO albums (id, title, artist_id) VALUES (1, 'Untrue', 1);
             INSERT INTO tracks (id, title, album_id, artist_id) VALUES (1, 'Archangel', 1, 1);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn set_replace_and_clear() {
        let conn = &conn();
        set_rating(conn, LOCAL_USER, RatingKind::Track, 1, 4).unwrap();
        set_rating(conn, LOCAL_USER, RatingKind::Album, 1, 5).unwrap();
        set_rating(conn, LOCAL_USER, RatingKind::Artist, 1, 2).unwrap();
        set_rating(conn, LOCAL_USER, RatingKind::Track, 1, 3).unwrap();
        assert_eq!(
            ratings(conn, LOCAL_USER, RatingKind::Track, [1, 99]).unwrap(),
            HashMap::from([(1, 3)])
        );
        assert_eq!(
            ratings(conn, LOCAL_USER, RatingKind::Album, [1]).unwrap()[&1],
            5
        );
        assert_eq!(
            ratings(conn, LOCAL_USER, RatingKind::Artist, [1]).unwrap()[&1],
            2
        );
        set_rating(conn, LOCAL_USER, RatingKind::Track, 1, 0).unwrap();
        assert!(
            ratings(conn, LOCAL_USER, RatingKind::Track, [1])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn out_of_range_is_refused() {
        assert!(set_rating(&conn(), LOCAL_USER, RatingKind::Track, 1, 6).is_err());
    }

    #[test]
    fn follows_the_row_out() {
        let conn = &conn();
        set_rating(conn, LOCAL_USER, RatingKind::Track, 1, 4).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM track_ratings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }
}

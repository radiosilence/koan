//! What the row types do not carry, read for a whole list of rows at once:
//! MusicBrainz ids, sort names, last plays, when a row was favourited. A
//! response that draws a list of 500 albums reads each of these once, not once
//! per album.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::auth::resolve_user;
use super::json_list;
use crate::db::connection::DbError;

/// Rows keyed by their first column, from a statement binding `params`.
fn keyed<T>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<(i64, T)>,
) -> Result<HashMap<i64, T>, DbError> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, row)?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

/// Each track's MusicBrainz recording id. Tracks without one are absent.
pub fn track_mbids(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>, DbError> {
    keyed(
        conn,
        "SELECT id, mbid FROM tracks
         WHERE id IN (SELECT value FROM json_each(?1)) AND mbid IS NOT NULL",
        [json_list(ids)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// Each album's MusicBrainz release id and sort name, either absent.
pub fn album_mbids_and_sort_names(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, (Option<String>, Option<String>)>, DbError> {
    keyed(
        conn,
        "SELECT id, mbid, sort_name FROM albums WHERE id IN (SELECT value FROM json_each(?1))",
        [json_list(ids)],
        |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
    )
}

/// Each artist's MusicBrainz id and sort name, either absent.
pub fn artist_mbids_and_sort_names(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, (Option<String>, Option<String>)>, DbError> {
    keyed(
        conn,
        "SELECT id, mbid, sort_name FROM artists WHERE id IN (SELECT value FROM json_each(?1))",
        [json_list(ids)],
        |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
    )
}

/// The genres each album's tracks are tagged with, as written, each once and
/// in order. Albums with none are absent.
pub fn album_genre_lists(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT album_id, genre FROM tracks
         WHERE album_id IN (SELECT value FROM json_each(?1))
           AND genre IS NOT NULL AND genre != ''
         ORDER BY album_id, genre",
    )?;
    let rows = stmt.query_map([json_list(ids)], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut genres: HashMap<i64, Vec<String>> = HashMap::new();
    for row in rows {
        let (id, genre) = row?;
        genres.entry(id).or_default().push(genre);
    }
    Ok(genres)
}

/// When `user` last played each track, seconds since the epoch. Tracks never
/// played are absent.
pub fn tracks_last_played(
    conn: &Connection,
    user: i64,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    keyed(
        conn,
        "SELECT track_id, MAX(played_at) FROM play_history
         WHERE track_id IN (SELECT value FROM json_each(?1)) AND user_id = ?2
         GROUP BY track_id",
        params![json_list(ids), resolve_user(conn, user)?],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// When `user` last played any track of each album, seconds since the epoch.
/// Albums never played are absent.
pub fn albums_last_played(
    conn: &Connection,
    user: i64,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    keyed(
        conn,
        "SELECT t.album_id, MAX(h.played_at) FROM play_history h
         JOIN tracks t ON t.id = h.track_id
         WHERE t.album_id IN (SELECT value FROM json_each(?1)) AND h.user_id = ?2
         GROUP BY t.album_id",
        params![json_list(ids), resolve_user(conn, user)?],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// When `user` favourited each of these tracks, seconds since the epoch.
/// Tracks that are not favourites are absent.
pub fn tracks_favourited_at(
    conn: &Connection,
    user: i64,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    favourited_at(conn, "favourites", "track_id", user, ids)
}

/// When `user` favourited each of these albums, as [`tracks_favourited_at`].
pub fn albums_favourited_at(
    conn: &Connection,
    user: i64,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    favourited_at(conn, "favourite_albums", "album_id", user, ids)
}

/// When `user` favourited each of these artists, as [`tracks_favourited_at`].
pub fn artists_favourited_at(
    conn: &Connection,
    user: i64,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    favourited_at(conn, "favourite_artists", "artist_id", user, ids)
}

fn favourited_at(
    conn: &Connection,
    table: &str,
    column: &str,
    user: i64,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    keyed(
        conn,
        &format!(
            "SELECT {column}, COALESCE(unixepoch(created_at), 0) FROM {table}
             WHERE {column} IN (SELECT value FROM json_each(?1)) AND user_id = ?2"
        ),
        params![json_list(ids), resolve_user(conn, user)?],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// The names of these artists, in id order. Ids naming no artist are left out.
pub fn artist_names(conn: &Connection, ids: &[i64]) -> Result<Vec<(i64, String)>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, name FROM artists WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id",
    )?;
    let rows = stmt.query_map([json_list(ids)], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{
        LOCAL_USER, TrackMeta, add_favourite, get_or_create_artist, record_play_at, sample_meta,
        set_favourite_album, set_favourite_artist, tracks_by_ids, upsert_track,
    };

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn track(conn: &Connection, meta: TrackMeta) -> (i64, i64, i64) {
        let id = upsert_track(conn, &meta).unwrap();
        let row = &tracks_by_ids(conn, &[id]).unwrap()[0];
        (id, row.album_id.unwrap(), row.artist_id.unwrap())
    }

    #[test]
    fn mbids_and_sort_names_are_read_for_the_rows_asked() {
        let conn = db();
        let mut meta = sample_meta("Roygbiv", "Boards of Canada", "MHTRTC");
        meta.mbid = Some("rec-1".into());
        let (named, album, artist) = track(&conn, meta);
        let (plain, _, _) = track(&conn, sample_meta("Other", "Someone", "Else"));
        conn.execute(
            "UPDATE albums SET mbid = 'rel-1', sort_name = 'Music Has' WHERE id = ?1",
            [album],
        )
        .unwrap();
        conn.execute("UPDATE artists SET mbid = 'art-1' WHERE id = ?1", [artist])
            .unwrap();

        let mbids = track_mbids(&conn, &[named, plain]).unwrap();
        assert_eq!(mbids, HashMap::from([(named, "rec-1".to_string())]));
        assert_eq!(
            album_mbids_and_sort_names(&conn, &[album]).unwrap()[&album],
            (Some("rel-1".into()), Some("Music Has".into()))
        );
        assert_eq!(
            artist_mbids_and_sort_names(&conn, &[artist]).unwrap()[&artist].0,
            Some("art-1".into())
        );
        assert!(track_mbids(&conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn an_albums_genres_are_listed_once_each_in_order() {
        let conn = db();
        let mut first = sample_meta("A", "Artist", "Record");
        first.genre = Some("Techno".into());
        let mut second = sample_meta("B", "Artist", "Record");
        second.track_number = Some(2);
        second.genre = Some("Ambient".into());
        let mut third = sample_meta("C", "Artist", "Record");
        third.track_number = Some(3);
        third.genre = Some("Techno".into());
        let (_, album, _) = track(&conn, first);
        track(&conn, second);
        track(&conn, third);
        assert_eq!(
            album_genre_lists(&conn, &[album]).unwrap()[&album],
            ["Ambient", "Techno"]
        );
    }

    #[test]
    fn last_plays_are_the_users_own_and_the_latest() {
        let conn = db();
        let (id, album, _) = track(&conn, sample_meta("A", "Artist", "Record"));
        record_play_at(&conn, LOCAL_USER, id, 100, None, "local").unwrap();
        record_play_at(&conn, LOCAL_USER, id, 200, None, "local").unwrap();
        assert_eq!(
            tracks_last_played(&conn, LOCAL_USER, &[id]).unwrap()[&id],
            200
        );
        assert_eq!(
            albums_last_played(&conn, LOCAL_USER, &[album]).unwrap()[&album],
            200
        );
        let other = crate::db::queries::auth::create_user(
            &conn,
            "other",
            "password",
            crate::auth::Role::User,
        )
        .unwrap();
        assert!(tracks_last_played(&conn, other, &[id]).unwrap().is_empty());
    }

    #[test]
    fn favourites_are_dated_per_kind() {
        let conn = db();
        let (id, album, artist) = track(&conn, sample_meta("A", "Artist", "Record"));
        add_favourite(&conn, LOCAL_USER, id).unwrap();
        set_favourite_album(&conn, LOCAL_USER, album, true).unwrap();
        assert!(tracks_favourited_at(&conn, LOCAL_USER, &[id]).unwrap()[&id] > 0);
        assert!(albums_favourited_at(&conn, LOCAL_USER, &[album]).unwrap()[&album] > 0);
        assert!(
            artists_favourited_at(&conn, LOCAL_USER, &[artist])
                .unwrap()
                .is_empty()
        );
        set_favourite_artist(&conn, LOCAL_USER, artist, true).unwrap();
        assert!(artists_favourited_at(&conn, LOCAL_USER, &[artist]).unwrap()[&artist] > 0);
    }

    #[test]
    fn artist_names_come_in_id_order_and_skip_unknown_ids() {
        let conn = db();
        let b = get_or_create_artist(&conn, "Burial", None).unwrap();
        let a = get_or_create_artist(&conn, "Arca", None).unwrap();
        let names = artist_names(&conn, &[a, b, 9999]).unwrap();
        let mut want = vec![(b, "Burial".to_string()), (a, "Arca".to_string())];
        want.sort();
        assert_eq!(names, want);
    }
}

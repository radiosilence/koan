use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};

use super::auth::resolve_user;

// Favourites name rows: a track, an album or an artist. They follow a row
// through merges, and a rebuilt index re-reads its sources into the rows it
// has, so they outlive that too.

/// A favourite changed on this device, waiting to be confirmed by a sync:
/// `kind` is `track`, `album` or `artist`, and `remote_id` the server's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavouriteChange {
    pub id: i64,
    pub kind: String,
    pub remote_id: String,
    pub star: bool,
    pub seq: i64,
}

/// Record a favourite changed here, replacing any earlier change to the same
/// item: only the latest is worth sending.
pub fn queue_favourite_change(
    conn: &Connection,
    kind: &str,
    remote_id: &str,
    star: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO favourite_outbox (kind, remote_id, star) VALUES (?1, ?2, ?3)
         ON CONFLICT(kind, remote_id) DO UPDATE SET star = excluded.star, seq = seq + 1",
        params![kind, remote_id, star],
    )?;
    Ok(())
}

/// Every favourite change the server is not yet known to have, oldest first.
pub fn favourite_changes(conn: &Connection) -> rusqlite::Result<Vec<FavouriteChange>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind, remote_id, star, seq FROM favourite_outbox ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FavouriteChange {
            id: row.get(0)?,
            kind: row.get(1)?,
            remote_id: row.get(2)?,
            star: row.get(3)?,
            seq: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// The server has answered `change`. A newer change to the same item, made
/// since it was read, stays.
pub fn forget_favourite_change(
    conn: &Connection,
    change: &FavouriteChange,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM favourite_outbox WHERE id = ?1 AND seq = ?2",
        params![change.id, change.seq],
    )?;
    Ok(())
}

/// `user`'s favourite tracks.
pub fn load_favourites(conn: &Connection, user: i64) -> rusqlite::Result<HashSet<i64>> {
    let user = resolve_user(conn, user)?;
    let mut stmt = conn.prepare_cached("SELECT track_id FROM favourites WHERE user_id = ?1")?;
    let rows = stmt.query_map([user], |row| row.get(0))?;
    rows.collect()
}

/// Make a track a favourite. Idempotent.
pub fn add_favourite(conn: &Connection, user: i64, track_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO favourites (user_id, track_id) VALUES (?1, ?2)",
        params![resolve_user(conn, user)?, track_id],
    )?;
    Ok(())
}

/// Stop a track being a favourite.
pub fn remove_favourite(conn: &Connection, user: i64, track_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM favourites WHERE user_id = ?1 AND track_id = ?2",
        params![resolve_user(conn, user)?, track_id],
    )?;
    Ok(())
}

/// Toggle a favourite. Returns true if the track is now a favourite.
pub fn toggle_favourite(conn: &Connection, user: i64, track_id: i64) -> rusqlite::Result<bool> {
    let user = resolve_user(conn, user)?;
    let removed = conn.execute(
        "DELETE FROM favourites WHERE user_id = ?1 AND track_id = ?2",
        params![user, track_id],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    add_favourite(conn, user, track_id)?;
    Ok(true)
}

/// Make an album a favourite, or stop it being one. Idempotent either way.
pub fn set_favourite_album(
    conn: &Connection,
    user: i64,
    album_id: i64,
    favourite: bool,
) -> rusqlite::Result<()> {
    let sql = if favourite {
        "INSERT OR IGNORE INTO favourite_albums (user_id, album_id) VALUES (?1, ?2)"
    } else {
        "DELETE FROM favourite_albums WHERE user_id = ?1 AND album_id = ?2"
    };
    conn.execute(sql, params![resolve_user(conn, user)?, album_id])?;
    Ok(())
}

/// Make an artist a favourite, or stop them being one. Idempotent either way.
pub fn set_favourite_artist(
    conn: &Connection,
    user: i64,
    artist_id: i64,
    favourite: bool,
) -> rusqlite::Result<()> {
    let sql = if favourite {
        "INSERT OR IGNORE INTO favourite_artists (user_id, artist_id) VALUES (?1, ?2)"
    } else {
        "DELETE FROM favourite_artists WHERE user_id = ?1 AND artist_id = ?2"
    };
    conn.execute(sql, params![resolve_user(conn, user)?, artist_id])?;
    Ok(())
}

/// Toggle an album favourite. Returns true if the album is now a favourite.
pub fn toggle_favourite_album(
    conn: &Connection,
    user: i64,
    album_id: i64,
) -> rusqlite::Result<bool> {
    let user = resolve_user(conn, user)?;
    let removed = conn.execute(
        "DELETE FROM favourite_albums WHERE user_id = ?1 AND album_id = ?2",
        params![user, album_id],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    set_favourite_album(conn, user, album_id, true)?;
    Ok(true)
}

/// Toggle an artist favourite. Returns true if the artist is now a favourite.
pub fn toggle_favourite_artist(
    conn: &Connection,
    user: i64,
    artist_id: i64,
) -> rusqlite::Result<bool> {
    let user = resolve_user(conn, user)?;
    let removed = conn.execute(
        "DELETE FROM favourite_artists WHERE user_id = ?1 AND artist_id = ?2",
        params![user, artist_id],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    set_favourite_artist(conn, user, artist_id, true)?;
    Ok(true)
}

/// Every album id that is a favourite.
pub fn favourite_album_id_set(conn: &Connection, user: i64) -> rusqlite::Result<HashSet<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT album_id FROM favourite_albums WHERE user_id = ?1")?;
    let rows = stmt.query_map([resolve_user(conn, user)?], |row| row.get(0))?;
    rows.collect()
}

/// Every artist id that is a favourite.
pub fn favourite_artist_id_set(conn: &Connection, user: i64) -> rusqlite::Result<HashSet<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT artist_id FROM favourite_artists WHERE user_id = ?1")?;
    let rows = stmt.query_map([resolve_user(conn, user)?], |row| row.get(0))?;
    rows.collect()
}

/// The remote id of an album, for starring it on the server.
pub fn album_remote_id(conn: &Connection, album_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT remote_id FROM albums WHERE id = ?1",
        [album_id],
        |row| row.get(0),
    )
    .optional()
    .map(Option::flatten)
}

/// The remote id of an artist, for starring it on the server.
pub fn artist_remote_id(conn: &Connection, artist_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT remote_id FROM artists WHERE id = ?1",
        [artist_id],
        |row| row.get(0),
    )
    .optional()
    .map(Option::flatten)
}

/// The remote id of a track, for starring it on the server. None for a track
/// only this device has.
pub fn track_remote_id(conn: &Connection, track_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT remote_id FROM tracks WHERE id = ?1",
        [track_id],
        |row| row.get(0),
    )
    .optional()
    .map(Option::flatten)
}

/// Favourited albums that exist on the server, as (album_id, remote_id).
pub fn favourite_albums_with_remote_id(
    conn: &Connection,
    user: i64,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.remote_id FROM albums al
         JOIN favourite_albums f ON f.album_id = al.id
         WHERE al.remote_id IS NOT NULL AND f.user_id = ?1",
    )?;
    let rows = stmt.query_map([resolve_user(conn, user)?], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    rows.collect()
}

/// Favourited artists that exist on the server, as (artist_id, remote_id).
pub fn favourite_artists_with_remote_id(
    conn: &Connection,
    user: i64,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT ar.id, ar.remote_id FROM artists ar
         JOIN favourite_artists f ON f.artist_id = ar.id
         WHERE ar.remote_id IS NOT NULL AND f.user_id = ?1",
    )?;
    let rows = stmt.query_map([resolve_user(conn, user)?], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    rows.collect()
}

/// Favourited tracks that exist on the server, as (track_id, remote_id).
pub fn favourites_with_remote_id(
    conn: &Connection,
    user: i64,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.remote_id FROM tracks t
         JOIN favourites f ON f.track_id = t.id
         WHERE t.remote_id IS NOT NULL AND f.user_id = ?1",
    )?;
    let rows = stmt.query_map([resolve_user(conn, user)?], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    rows.collect()
}

/// Star, as `user`, the rows of `table` the server knows by these ids.
/// Returns how many were new.
fn import_starred(
    conn: &Connection,
    user: i64,
    starred_remote_ids: &[String],
    find: &str,
    insert: &str,
) -> rusqlite::Result<usize> {
    let user = resolve_user(conn, user)?;
    super::atomically(conn, || {
        let mut find = conn.prepare_cached(find)?;
        let mut insert = conn.prepare_cached(insert)?;
        let mut count = 0;
        for rid in starred_remote_ids {
            let id: Option<i64> = find.query_row([rid], |row| row.get(0)).optional()?;
            if let Some(id) = id {
                count += insert.execute(params![user, id])?;
            }
        }
        Ok(count)
    })
}

/// Import starred tracks from the server, matched by remote id. Returns how
/// many were new.
pub fn import_remote_favourites(
    conn: &Connection,
    user: i64,
    starred_remote_ids: &[String],
) -> rusqlite::Result<usize> {
    import_starred(
        conn,
        user,
        starred_remote_ids,
        "SELECT track_id FROM remote_entries WHERE remote_id = ?1",
        "INSERT OR IGNORE INTO favourites (user_id, track_id) VALUES (?1, ?2)",
    )
}

/// Import starred albums from the server, matched by remote id. Returns how
/// many were new.
pub fn import_remote_favourite_albums(
    conn: &Connection,
    user: i64,
    starred_remote_ids: &[String],
) -> rusqlite::Result<usize> {
    import_starred(
        conn,
        user,
        starred_remote_ids,
        "SELECT id FROM albums WHERE remote_id = ?1 ORDER BY id LIMIT 1",
        "INSERT OR IGNORE INTO favourite_albums (user_id, album_id) VALUES (?1, ?2)",
    )
}

/// Import starred artists from the server, matched by remote id.
pub fn import_remote_favourite_artists(
    conn: &Connection,
    user: i64,
    starred_remote_ids: &[String],
) -> rusqlite::Result<usize> {
    import_starred(
        conn,
        user,
        starred_remote_ids,
        "SELECT id FROM artists WHERE remote_id = ?1 ORDER BY id LIMIT 1",
        "INSERT OR IGNORE INTO favourite_artists (user_id, artist_id) VALUES (?1, ?2)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{LOCAL_USER, sample_meta, upsert_track};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn local_track(conn: &Connection, title: &str) -> i64 {
        upsert_track(conn, &sample_meta(title, "Artist", "Album")).unwrap()
    }

    fn remote_track(conn: &Connection, title: &str, remote_id: &str) -> i64 {
        let mut meta = sample_meta(title, "Artist", "Album");
        meta.path = None;
        meta.remote_id = Some(remote_id.into());
        upsert_track(conn, &meta).unwrap()
    }

    #[test]
    fn only_tracks_on_the_server_are_pushed() {
        let conn = test_conn();
        let on_server = remote_track(&conn, "On the server", "r1");
        let local = local_track(&conn, "Local only");
        add_favourite(&conn, LOCAL_USER, on_server).unwrap();
        add_favourite(&conn, LOCAL_USER, local).unwrap();

        assert_eq!(
            favourites_with_remote_id(&conn, LOCAL_USER).unwrap(),
            vec![(on_server, "r1".to_string())]
        );
    }

    #[test]
    fn test_load_favourites_returns_empty_when_none_added() {
        let conn = test_conn();
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().is_empty());
    }

    #[test]
    fn test_add_and_remove_favourite() {
        let conn = test_conn();
        let id = local_track(&conn, "Track");

        add_favourite(&conn, LOCAL_USER, id).unwrap();
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().contains(&id));

        remove_favourite(&conn, LOCAL_USER, id).unwrap();
        assert!(!load_favourites(&conn, LOCAL_USER).unwrap().contains(&id));
    }

    #[test]
    fn test_add_favourite_is_idempotent() {
        let conn = test_conn();
        let id = local_track(&conn, "Track");
        add_favourite(&conn, LOCAL_USER, id).unwrap();
        add_favourite(&conn, LOCAL_USER, id).unwrap();
        assert_eq!(load_favourites(&conn, LOCAL_USER).unwrap().len(), 1);
    }

    #[test]
    fn test_toggle_favourite_on_then_off() {
        let conn = test_conn();
        let id = local_track(&conn, "Track");
        assert!(toggle_favourite(&conn, LOCAL_USER, id).unwrap());
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().contains(&id));
        assert!(!toggle_favourite(&conn, LOCAL_USER, id).unwrap());
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().is_empty());
    }

    #[test]
    fn a_favourite_names_a_track_that_exists() {
        let conn = test_conn();
        assert!(add_favourite(&conn, LOCAL_USER, 404).is_err());
    }

    #[test]
    fn a_favourite_goes_with_its_track() {
        let conn = test_conn();
        let id = local_track(&conn, "Track");
        add_favourite(&conn, LOCAL_USER, id).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = ?1", [id])
            .unwrap();
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().is_empty());
    }

    #[test]
    fn a_favourite_follows_its_track_into_a_merge() {
        let conn = test_conn();
        let remote = remote_track(&conn, "Track", "r1");
        add_favourite(&conn, LOCAL_USER, remote).unwrap();
        let mut file = sample_meta("Track", "Artist", "Album");
        file.track_number = Some(1);
        let merged = upsert_track(&conn, &file).unwrap();
        assert_eq!(merged, remote);
        assert!(
            load_favourites(&conn, LOCAL_USER)
                .unwrap()
                .contains(&merged)
        );
    }

    #[test]
    fn test_import_remote_favourites_adds_matching_tracks() {
        let conn = test_conn();
        let id = remote_track(&conn, "Track", "remote-001");
        let added =
            import_remote_favourites(&conn, LOCAL_USER, &["remote-001".to_string()]).unwrap();
        assert_eq!(added, 1);
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().contains(&id));
    }

    #[test]
    fn test_import_remote_favourites_skips_unknown_remote_ids() {
        let conn = test_conn();
        let added = import_remote_favourites(&conn, LOCAL_USER, &["unknown-remote-id".to_string()])
            .unwrap();
        assert_eq!(added, 0);
        assert!(load_favourites(&conn, LOCAL_USER).unwrap().is_empty());
    }

    fn insert_album(conn: &Connection, artist: &str, album: &str, remote_id: Option<&str>) -> i64 {
        conn.execute(
            "INSERT INTO artists (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            [artist],
        )
        .unwrap();
        let artist_id: i64 = conn
            .query_row("SELECT id FROM artists WHERE name = ?1", [artist], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO albums (title, artist_id, remote_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![album, artist_id, remote_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn an_album_favourite_toggles_on_and_off() {
        let conn = test_conn();
        let id = insert_album(&conn, "Russian Circles", "Enter", None);
        assert!(toggle_favourite_album(&conn, LOCAL_USER, id).unwrap());
        assert!(
            favourite_album_id_set(&conn, LOCAL_USER)
                .unwrap()
                .contains(&id)
        );
        assert!(!toggle_favourite_album(&conn, LOCAL_USER, id).unwrap());
        assert!(
            favourite_album_id_set(&conn, LOCAL_USER)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_artist_favourite_toggles_on_and_off() {
        let conn = test_conn();
        insert_album(&conn, "Godspeed", "Lift Your Skinny Fists", None);
        let artist_id: i64 = conn
            .query_row("SELECT id FROM artists WHERE name = 'Godspeed'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(toggle_favourite_artist(&conn, LOCAL_USER, artist_id).unwrap());
        assert!(
            favourite_artist_id_set(&conn, LOCAL_USER)
                .unwrap()
                .contains(&artist_id)
        );
        assert!(!toggle_favourite_artist(&conn, LOCAL_USER, artist_id).unwrap());
        assert!(
            favourite_artist_id_set(&conn, LOCAL_USER)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn starred_albums_import_by_remote_id() {
        let conn = test_conn();
        let id = insert_album(&conn, "Phace", "Mammoth", Some("remote-album-1"));
        let added =
            import_remote_favourite_albums(&conn, LOCAL_USER, &["remote-album-1".to_string()])
                .unwrap();
        assert_eq!(added, 1);
        assert!(
            favourite_album_id_set(&conn, LOCAL_USER)
                .unwrap()
                .contains(&id)
        );
        let again =
            import_remote_favourite_albums(&conn, LOCAL_USER, &["remote-album-1".to_string()])
                .unwrap();
        assert_eq!(again, 0, "re-importing should not add a second row");
    }

    #[test]
    fn only_favourites_the_server_knows_about_are_pushed() {
        let conn = test_conn();
        let local = insert_album(&conn, "Local Only", "Demo", None);
        let remote = insert_album(&conn, "On The Server", "Record", Some("remote-album-2"));
        toggle_favourite_album(&conn, LOCAL_USER, local).unwrap();
        toggle_favourite_album(&conn, LOCAL_USER, remote).unwrap();
        let pushable = favourite_albums_with_remote_id(&conn, LOCAL_USER).unwrap();
        assert_eq!(pushable, vec![(remote, "remote-album-2".to_string())]);
    }

    #[test]
    fn test_import_remote_favourites_is_idempotent() {
        let conn = test_conn();
        remote_track(&conn, "Track", "remote-002");
        import_remote_favourites(&conn, LOCAL_USER, &["remote-002".to_string()]).unwrap();
        let added =
            import_remote_favourites(&conn, LOCAL_USER, &["remote-002".to_string()]).unwrap();
        assert_eq!(added, 0);
        assert_eq!(load_favourites(&conn, LOCAL_USER).unwrap().len(), 1);
    }
}

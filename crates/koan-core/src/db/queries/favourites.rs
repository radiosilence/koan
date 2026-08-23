use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

/// Load all favourite track paths from the database.
pub fn load_favourites(conn: &Connection) -> rusqlite::Result<HashSet<PathBuf>> {
    let mut stmt = conn.prepare("SELECT track_path FROM favourites")?;
    let rows = stmt.query_map([], |row| {
        let p: String = row.get(0)?;
        Ok(PathBuf::from(p))
    })?;
    let mut set = HashSet::new();
    for path in rows.flatten() {
        set.insert(path);
    }
    Ok(set)
}

/// Add a track path to favourites. Idempotent.
pub fn add_favourite(conn: &Connection, path: &Path) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO favourites (track_path) VALUES (?1)",
        [path.to_string_lossy().as_ref()],
    )?;
    Ok(())
}

/// Remove a track path from favourites.
pub fn remove_favourite(conn: &Connection, path: &Path) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM favourites WHERE track_path = ?1",
        [path.to_string_lossy().as_ref()],
    )?;
    Ok(())
}

/// Toggle a favourite. Returns true if the track is now a favourite.
pub fn toggle_favourite(conn: &Connection, path: &Path) -> rusqlite::Result<bool> {
    let path_str = path.to_string_lossy();
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM favourites WHERE track_path = ?1)",
        [path_str.as_ref()],
        |row| row.get(0),
    )?;
    if exists {
        remove_favourite(conn, path)?;
        Ok(false)
    } else {
        add_favourite(conn, path)?;
        Ok(true)
    }
}

// --- Albums and artists ----------------------------------------------------
//
// Keyed by name rather than row id so a reindex does not wipe them. The
// consequence is that two albums sharing a title and an artist are one
// favourite, which is the same thing to anyone looking at the library.

/// Toggle an album favourite. Returns true if the album is now a favourite.
pub fn toggle_favourite_album(
    conn: &Connection,
    artist: &str,
    album: &str,
) -> rusqlite::Result<bool> {
    let removed = conn.execute(
        "DELETE FROM favourite_albums WHERE artist_name = ?1 AND album_title = ?2",
        [artist, album],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO favourite_albums (artist_name, album_title) VALUES (?1, ?2)",
        [artist, album],
    )?;
    Ok(true)
}

/// Toggle an artist favourite. Returns true if the artist is now a favourite.
pub fn toggle_favourite_artist(conn: &Connection, artist: &str) -> rusqlite::Result<bool> {
    let removed = conn.execute(
        "DELETE FROM favourite_artists WHERE artist_name = ?1",
        [artist],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO favourite_artists (artist_name) VALUES (?1)",
        [artist],
    )?;
    Ok(true)
}

/// Every album id that is a favourite, resolved through the album's title and
/// artist. One query per listing rather than one per row.
pub fn favourite_album_id_set(conn: &Connection) -> rusqlite::Result<HashSet<i64>> {
    let mut stmt = conn.prepare(
        "SELECT al.id FROM albums al
         JOIN artists ar ON al.artist_id = ar.id
         JOIN favourite_albums f
           ON f.artist_name = ar.name AND f.album_title = al.title",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    rows.collect()
}

/// Every artist id that is a favourite.
pub fn favourite_artist_id_set(conn: &Connection) -> rusqlite::Result<HashSet<i64>> {
    let mut stmt = conn.prepare(
        "SELECT ar.id FROM artists ar
         JOIN favourite_artists f ON f.artist_name = ar.name",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
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

/// The (artist, title) pair an album is favourited under.
pub fn album_favourite_key(
    conn: &Connection,
    album_id: i64,
) -> rusqlite::Result<Option<(String, String)>> {
    conn.query_row(
        "SELECT ar.name, al.title FROM albums al
         JOIN artists ar ON al.artist_id = ar.id
         WHERE al.id = ?1",
        [album_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// The name an artist is favourited under.
pub fn artist_favourite_key(conn: &Connection, artist_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT name FROM artists WHERE id = ?1",
        [artist_id],
        |row| row.get(0),
    )
    .optional()
}

/// Favourited albums that exist on the server, as (album_id, remote_id).
pub fn favourite_albums_with_remote_id(conn: &Connection) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.remote_id FROM albums al
         JOIN artists ar ON al.artist_id = ar.id
         JOIN favourite_albums f
           ON f.artist_name = ar.name AND f.album_title = al.title
         WHERE al.remote_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

/// Favourited artists that exist on the server, as (artist_id, remote_id).
pub fn favourite_artists_with_remote_id(conn: &Connection) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT ar.id, ar.remote_id FROM artists ar
         JOIN favourite_artists f ON f.artist_name = ar.name
         WHERE ar.remote_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

/// Import starred albums from the server, matched by remote id. Returns how
/// many were new.
pub fn import_remote_favourite_albums(
    conn: &Connection,
    starred_remote_ids: &[String],
) -> rusqlite::Result<usize> {
    let mut count = 0;
    for rid in starred_remote_ids {
        let names: Option<(String, String)> = conn
            .query_row(
                "SELECT ar.name, al.title FROM albums al
                 JOIN artists ar ON al.artist_id = ar.id
                 WHERE al.remote_id = ?1",
                [rid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((artist, album)) = names {
            count += conn.execute(
                "INSERT OR IGNORE INTO favourite_albums (artist_name, album_title) VALUES (?1, ?2)",
                [&artist, &album],
            )?;
        }
    }
    Ok(count)
}

/// Import starred artists from the server, matched by remote id.
pub fn import_remote_favourite_artists(
    conn: &Connection,
    starred_remote_ids: &[String],
) -> rusqlite::Result<usize> {
    let mut count = 0;
    for rid in starred_remote_ids {
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM artists WHERE remote_id = ?1",
                [rid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(name) = name {
            count += conn.execute(
                "INSERT OR IGNORE INTO favourite_artists (artist_name) VALUES (?1)",
                [&name],
            )?;
        }
    }
    Ok(count)
}

/// Look up the remote_id for a track by its path. Returns None for local-only tracks.
pub fn remote_id_for_path(conn: &Connection, path: &Path) -> rusqlite::Result<Option<String>> {
    let path_str = path.to_string_lossy();
    conn.query_row(
        "SELECT remote_id FROM tracks WHERE path = ?1 OR cached_path = ?1 OR remote_url = ?1",
        [path_str.as_ref()],
        |row| row.get(0),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Look up the album's remote_id for a track identified by its path.
/// Returns None if the track or album has no remote_id.
pub fn album_remote_id_for_path(
    conn: &Connection,
    path: &Path,
) -> rusqlite::Result<Option<String>> {
    let path_str = path.to_string_lossy();
    conn.query_row(
        "SELECT al.remote_id FROM tracks t
         JOIN albums al ON t.album_id = al.id
         WHERE (t.path = ?1 OR t.cached_path = ?1 OR t.remote_url = ?1)
         AND al.remote_id IS NOT NULL",
        [path_str.as_ref()],
        |row| row.get(0),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Load all favourite track paths that have a remote_id, returning (path, remote_id) pairs.
pub fn favourites_with_remote_id(conn: &Connection) -> rusqlite::Result<Vec<(PathBuf, String)>> {
    let mut stmt = conn.prepare(
        "SELECT f.track_path, t.remote_id FROM favourites f
         JOIN tracks t ON (t.path = f.track_path OR t.cached_path = f.track_path OR t.remote_url = f.track_path)
         WHERE t.remote_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let rid: String = row.get(1)?;
        Ok((PathBuf::from(path), rid))
    })?;
    let mut result = Vec::new();
    for pair in rows.flatten() {
        result.push(pair);
    }
    Ok(result)
}

/// Sync favourites from the remote server into the local database.
/// Adds any remote-starred tracks as local favourites (by matching remote_id → path).
/// Returns the number of new favourites added.
pub fn import_remote_favourites(
    conn: &Connection,
    starred_remote_ids: &[String],
) -> rusqlite::Result<usize> {
    let mut count = 0;
    for rid in starred_remote_ids {
        // Find the local path for this remote_id.
        let path: Option<String> = conn
            .query_row(
                "SELECT COALESCE(cached_path, path, remote_url) FROM tracks WHERE remote_id = ?1",
                [rid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(p) = path {
            let inserted: usize = conn.execute(
                "INSERT OR IGNORE INTO favourites (track_path) VALUES (?1)",
                [&p],
            )?;
            count += inserted;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    /// Insert a minimal track row so `import_remote_favourites` can resolve remote_id → path.
    fn insert_track_with_remote_id(conn: &Connection, path: &str, remote_id: &str) {
        conn.execute(
            "INSERT INTO artists (name) VALUES ('Artist') ON CONFLICT(name) DO NOTHING",
            [],
        )
        .unwrap();
        let artist_id: i64 = conn
            .query_row("SELECT id FROM artists WHERE name = 'Artist'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO albums (title, artist_id) VALUES ('Album', ?1) ON CONFLICT(title, artist_id) DO NOTHING",
            [artist_id],
        )
        .unwrap();
        let album_id: i64 = conn
            .query_row(
                "SELECT id FROM albums WHERE title = 'Album' AND artist_id = ?1",
                [artist_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO tracks (title, artist_id, album_id, source, path, remote_id)
             VALUES ('Track', ?1, ?2, 'local', ?3, ?4)",
            rusqlite::params![artist_id, album_id, path, remote_id],
        )
        .unwrap();
    }

    #[test]
    fn test_load_favourites_returns_empty_when_none_added() {
        let conn = test_conn();
        let favs = load_favourites(&conn).unwrap();
        assert!(
            favs.is_empty(),
            "expected no favourites in a fresh database"
        );
    }

    #[test]
    fn test_add_and_remove_favourite() {
        let conn = test_conn();
        let path = Path::new("/music/track.flac");

        add_favourite(&conn, path).unwrap();
        let favs = load_favourites(&conn).unwrap();
        assert!(
            favs.contains(path),
            "track should be in favourites after add"
        );

        remove_favourite(&conn, path).unwrap();
        let favs = load_favourites(&conn).unwrap();
        assert!(
            !favs.contains(path),
            "track should not be in favourites after remove"
        );
    }

    #[test]
    fn test_add_favourite_is_idempotent() {
        let conn = test_conn();
        let path = Path::new("/music/idempotent.flac");

        add_favourite(&conn, path).unwrap();
        add_favourite(&conn, path).unwrap(); // second call must not error
        let favs = load_favourites(&conn).unwrap();
        assert_eq!(favs.len(), 1, "duplicate add should not create two rows");
    }

    #[test]
    fn test_toggle_favourite_on_then_off() {
        let conn = test_conn();
        let path = Path::new("/music/toggle.flac");

        // First toggle: not present → should be added, returns true.
        let now_fav = toggle_favourite(&conn, path).unwrap();
        assert!(
            now_fav,
            "toggle on empty should add the favourite and return true"
        );

        let favs = load_favourites(&conn).unwrap();
        assert!(
            favs.contains(path),
            "track should be in favourites after first toggle"
        );

        // Second toggle: present → should be removed, returns false.
        let now_fav = toggle_favourite(&conn, path).unwrap();
        assert!(
            !now_fav,
            "second toggle should remove the favourite and return false"
        );

        let favs = load_favourites(&conn).unwrap();
        assert!(
            !favs.contains(path),
            "track should not be in favourites after second toggle"
        );
    }

    #[test]
    fn test_toggle_favourite_nonexistent_track_path_handled_gracefully() {
        // The favourites table stores paths directly; there is no FK constraint
        // to the tracks table. A path that has no corresponding track row should
        // be toggled in/out without error.
        let conn = test_conn();
        let path = Path::new("/music/does-not-exist-in-tracks.flac");

        let result = toggle_favourite(&conn, path);
        assert!(
            result.is_ok(),
            "toggling a path with no track row should not error"
        );
        assert!(
            result.unwrap(),
            "non-existent path should be added on first toggle"
        );

        let favs = load_favourites(&conn).unwrap();
        assert!(
            favs.contains(path),
            "path should appear in favourites even without a matching track row"
        );
    }

    #[test]
    fn test_import_remote_favourites_adds_matching_tracks() {
        let conn = test_conn();
        insert_track_with_remote_id(&conn, "/music/remote-track.flac", "remote-001");

        let added = import_remote_favourites(&conn, &["remote-001".to_string()]).unwrap();
        assert_eq!(added, 1, "should have imported one favourite");

        let favs = load_favourites(&conn).unwrap();
        assert!(
            favs.contains(Path::new("/music/remote-track.flac")),
            "imported track path should be in favourites"
        );
    }

    #[test]
    fn test_import_remote_favourites_skips_unknown_remote_ids() {
        let conn = test_conn();

        let added = import_remote_favourites(&conn, &["unknown-remote-id".to_string()]).unwrap();
        assert_eq!(added, 0, "unknown remote_id should not add any favourites");

        let favs = load_favourites(&conn).unwrap();
        assert!(favs.is_empty());
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
            "INSERT INTO albums (title, artist_id, remote_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(title, artist_id) DO NOTHING",
            rusqlite::params![album, artist_id, remote_id],
        )
        .unwrap();
        conn.query_row(
            "SELECT id FROM albums WHERE title = ?1 AND artist_id = ?2",
            rusqlite::params![album, artist_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn an_album_favourite_toggles_on_and_off() {
        let conn = test_conn();
        let id = insert_album(&conn, "Russian Circles", "Enter", None);

        assert!(toggle_favourite_album(&conn, "Russian Circles", "Enter").unwrap());
        assert!(favourite_album_id_set(&conn).unwrap().contains(&id));

        assert!(!toggle_favourite_album(&conn, "Russian Circles", "Enter").unwrap());
        assert!(favourite_album_id_set(&conn).unwrap().is_empty());
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

        assert!(toggle_favourite_artist(&conn, "Godspeed").unwrap());
        assert!(favourite_artist_id_set(&conn).unwrap().contains(&artist_id));

        assert!(!toggle_favourite_artist(&conn, "Godspeed").unwrap());
        assert!(favourite_artist_id_set(&conn).unwrap().is_empty());
    }

    /// Favourites are keyed by name so a reindex cannot lose them. Rebuilding
    /// the library assigns new row ids, and resolving through the name has to
    /// still find the album.
    #[test]
    fn an_album_favourite_survives_new_row_ids() {
        let conn = test_conn();
        insert_album(&conn, "Boards of Canada", "Geogaddi", None);
        toggle_favourite_album(&conn, "Boards of Canada", "Geogaddi").unwrap();

        conn.execute("DELETE FROM albums", []).unwrap();
        conn.execute("DELETE FROM artists", []).unwrap();
        let new_id = insert_album(&conn, "Boards of Canada", "Geogaddi", None);

        assert!(
            favourite_album_id_set(&conn).unwrap().contains(&new_id),
            "the favourite should resolve to the rebuilt album row"
        );
    }

    #[test]
    fn starred_albums_import_by_remote_id() {
        let conn = test_conn();
        let id = insert_album(&conn, "Phace", "Mammoth", Some("remote-album-1"));

        let added = import_remote_favourite_albums(&conn, &["remote-album-1".to_string()]).unwrap();
        assert_eq!(added, 1);
        assert!(favourite_album_id_set(&conn).unwrap().contains(&id));

        let again = import_remote_favourite_albums(&conn, &["remote-album-1".to_string()]).unwrap();
        assert_eq!(again, 0, "re-importing should not add a second row");
    }

    #[test]
    fn only_favourites_the_server_knows_about_are_pushed() {
        let conn = test_conn();
        insert_album(&conn, "Local Only", "Demo", None);
        insert_album(&conn, "On The Server", "Record", Some("remote-album-2"));
        toggle_favourite_album(&conn, "Local Only", "Demo").unwrap();
        toggle_favourite_album(&conn, "On The Server", "Record").unwrap();

        let pushable = favourite_albums_with_remote_id(&conn).unwrap();
        assert_eq!(pushable.len(), 1);
        assert_eq!(pushable[0].1, "remote-album-2");
    }

    #[test]
    fn test_import_remote_favourites_is_idempotent() {
        let conn = test_conn();
        insert_track_with_remote_id(&conn, "/music/idempotent-remote.flac", "remote-002");

        import_remote_favourites(&conn, &["remote-002".to_string()]).unwrap();
        let added = import_remote_favourites(&conn, &["remote-002".to_string()]).unwrap();

        assert_eq!(
            added, 0,
            "re-importing an already-favourited track should add 0 rows"
        );
        assert_eq!(load_favourites(&conn).unwrap().len(), 1);
    }
}

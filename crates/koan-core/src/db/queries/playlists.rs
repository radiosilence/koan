//! Playlists: named, ordered lists of library tracks.
//!
//! A playlist holds what Subsonic holds — a name, a comment, an owner, a public
//! flag and an ordered list of songs — so one made here and one made on the
//! server are the same object and can be reconciled without inventing fields
//! the server has nowhere to put. The two exceptions are local by nature: where
//! a playlist sits in your sidebar, and whether you like to look at it grouped
//! by album.
//!
//! Order is stored as an explicit `position` rather than implied by rowid,
//! because the same track may appear twice and both copies have to keep their
//! place.

use rusqlite::{Connection, params};

use super::TrackRow;
use crate::db::connection::DbError;

/// A playlist and what can be known about it without reading its tracks.
#[derive(Debug, Clone)]
pub struct PlaylistRow {
    pub id: i64,
    pub name: String,
    pub comment: Option<String>,
    pub public: bool,
    pub owner: Option<String>,
    pub remote_id: Option<String>,
    pub created_at: String,
    pub changed_at: String,
    /// Where it sits in the sidebar. Local only.
    pub sort_order: i64,
    /// Grouped by album, one row per track, or `None` to follow the default.
    /// Local only — a view preference is about this machine, not the playlist.
    pub grouped: Option<bool>,
    pub track_count: i64,
    pub duration_ms: i64,
}

const SELECT: &str = "SELECT p.id, p.name, p.comment, p.public, p.owner, p.remote_id,
            p.created_at, p.changed_at, p.sort_order, p.grouped,
            COUNT(pt.track_id), COALESCE(SUM(t.duration_ms), 0)
     FROM playlists p
     LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
     LEFT JOIN tracks t ON t.id = pt.track_id";

fn row_to_playlist(row: &rusqlite::Row) -> rusqlite::Result<PlaylistRow> {
    Ok(PlaylistRow {
        id: row.get(0)?,
        name: row.get(1)?,
        comment: row.get(2)?,
        public: row.get::<_, i64>(3)? != 0,
        owner: row.get(4)?,
        remote_id: row.get(5)?,
        created_at: row.get(6)?,
        changed_at: row.get(7)?,
        sort_order: row.get(8)?,
        grouped: row.get::<_, Option<i64>>(9)?.map(|g| g != 0),
        track_count: row.get(10)?,
        duration_ms: row.get(11)?,
    })
}

/// Every playlist, in sidebar order.
pub fn list_playlists(conn: &Connection) -> Result<Vec<PlaylistRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "{SELECT} GROUP BY p.id ORDER BY p.sort_order, p.name COLLATE LIBRARY"
    ))?;
    let rows = stmt
        .query_map([], row_to_playlist)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_playlist(conn: &Connection, id: i64) -> Result<Option<PlaylistRow>, DbError> {
    let result = conn.query_row(
        &format!("{SELECT} WHERE p.id = ?1 GROUP BY p.id"),
        params![id],
        row_to_playlist,
    );
    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn playlist_by_remote_id(
    conn: &Connection,
    remote_id: &str,
) -> Result<Option<PlaylistRow>, DbError> {
    let result = conn.query_row(
        &format!("{SELECT} WHERE p.remote_id = ?1 GROUP BY p.id"),
        params![remote_id],
        row_to_playlist,
    );
    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Create an empty playlist and return its id.
///
/// New playlists go to the top of the sidebar: you just made it, so it is the
/// one you are about to use.
pub fn create_playlist(
    conn: &Connection,
    name: &str,
    comment: Option<&str>,
) -> Result<i64, DbError> {
    let top: i64 = conn
        .query_row(
            "SELECT COALESCE(MIN(sort_order), 0) - 1 FROM playlists",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO playlists (name, comment, sort_order) VALUES (?1, ?2, ?3)",
        params![name, comment, top],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_playlist(conn: &Connection, id: i64) -> Result<bool, DbError> {
    Ok(conn.execute("DELETE FROM playlists WHERE id = ?1", params![id])? > 0)
}

pub fn rename_playlist(conn: &Connection, id: i64, name: &str) -> Result<bool, DbError> {
    Ok(conn.execute(
        "UPDATE playlists SET name = ?2, changed_at = datetime('now') WHERE id = ?1",
        params![id, name],
    )? > 0)
}

/// Set the sidebar order from the ids in the order they should appear.
pub fn reorder_playlists(conn: &Connection, ids: &[i64]) -> Result<(), DbError> {
    for (position, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE playlists SET sort_order = ?2 WHERE id = ?1",
            params![id, position as i64],
        )?;
    }
    Ok(())
}

/// Remember how this playlist is looked at. `None` follows the app default.
pub fn set_playlist_grouped(
    conn: &Connection,
    id: i64,
    grouped: Option<bool>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET grouped = ?2 WHERE id = ?1",
        params![id, grouped.map(i64::from)],
    )?;
    Ok(())
}

/// Attach a server id, and record the server's own timestamps with it.
pub fn set_playlist_remote(
    conn: &Connection,
    id: i64,
    remote_id: &str,
    owner: Option<&str>,
    public: bool,
    changed_at: Option<&str>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET remote_id = ?2, owner = ?3, public = ?4,
                changed_at = COALESCE(NULLIF(?5, ''), changed_at)
         WHERE id = ?1",
        params![
            id,
            remote_id,
            owner,
            public as i64,
            changed_at.unwrap_or("")
        ],
    )?;
    Ok(())
}

/// One entry: a place in a playlist, and the track sitting in it.
///
/// The id is the entry's, not the track's. It survives a reorder, and it is
/// what a queue item remembers — which is how the two copies of a song in one
/// playlist are told apart when one of them is playing.
#[derive(Debug, Clone)]
pub struct PlaylistEntry {
    pub id: i64,
    pub position: i64,
    pub track: TrackRow,
}

/// The entries of this playlist, in order, with their tracks.
pub fn playlist_entries(conn: &Connection, id: i64) -> Result<Vec<PlaylistEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT pt.id, pt.position,
                t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM playlist_tracks pt
         JOIN tracks t ON t.id = pt.track_id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE pt.playlist_id = ?1
         ORDER BY pt.position",
    )?;
    let rows = stmt
        .query_map(params![id], |row| {
            Ok(PlaylistEntry {
                id: row.get(0)?,
                position: row.get(1)?,
                // The track's own columns start at 2; the mapper counts from
                // whatever offset it is given.
                track: super::tracks::row_to_track_row_at(row, 2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The entry ids of this playlist, in order. The cheap half of
/// [`playlist_entries`], for the times only identity and order matter.
pub fn playlist_entry_ids(conn: &Connection, id: i64) -> Result<Vec<i64>, DbError> {
    let mut stmt =
        conn.prepare("SELECT id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")?;
    let rows = stmt
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Which playlist an entry belongs to.
pub fn playlist_of_entry(conn: &Connection, entry_id: i64) -> Result<Option<i64>, DbError> {
    let found = conn
        .query_row(
            "SELECT playlist_id FROM playlist_tracks WHERE id = ?1",
            params![entry_id],
            |row| row.get(0),
        )
        .ok();
    Ok(found)
}

/// The track ids in this playlist, in order. Duplicates kept.
pub fn playlist_track_ids(conn: &Connection, id: i64) -> Result<Vec<i64>, DbError> {
    let mut stmt = conn
        .prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")?;
    let rows = stmt
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The tracks in this playlist, in order, with full metadata.
pub fn playlist_tracks(conn: &Connection, id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM playlist_tracks pt
         JOIN tracks t ON t.id = pt.track_id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE pt.playlist_id = ?1
         ORDER BY pt.position",
    )?;
    let rows = stmt
        .query_map(params![id], super::tracks::row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Append tracks to the end. Returns how many landed.
///
/// A track already in the playlist is added again rather than skipped: putting
/// a song in twice is a thing people do on purpose, and silently refusing it is
/// worse than the duplicate.
pub fn add_tracks(conn: &Connection, id: i64, track_ids: &[i64]) -> Result<Vec<i64>, DbError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    let mut added = Vec::new();
    for track_id in track_ids {
        // A track that no longer exists would fail the foreign key and take the
        // whole add down with it.
        let inserted = conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?3)",
            params![id, next, track_id],
        )?;
        if inserted > 0 {
            next += 1;
            added.push(conn.last_insert_rowid());
        }
    }
    touch(conn, id)?;
    Ok(added)
}

/// Put the entries in this order. Ids are kept, so nothing holding a reference
/// to an entry loses it because the playlist was rearranged.
///
/// Entries not named are left where they are relative to each other and pushed
/// to the end — a caller that names all of them, which every caller does, never
/// meets that case.
pub fn reorder_entries(conn: &Connection, id: i64, entry_ids: &[i64]) -> Result<(), DbError> {
    // Positions are unique per playlist, so they cannot be rewritten in place
    // without colliding on the way through. Negative numbers are out of the way
    // of anything the table holds.
    for (position, entry) in entry_ids.iter().enumerate() {
        conn.execute(
            "UPDATE playlist_tracks SET position = ?3
             WHERE id = ?1 AND playlist_id = ?2",
            params![entry, id, -(position as i64) - 1],
        )?;
    }
    conn.execute(
        "UPDATE playlist_tracks SET position = -position - 1
         WHERE playlist_id = ?1 AND position < 0",
        params![id],
    )?;
    touch(conn, id)
}

/// Drop entries by id. Everything after them closes up.
pub fn remove_entries(conn: &Connection, id: i64, entry_ids: &[i64]) -> Result<usize, DbError> {
    let mut removed = 0;
    for entry in entry_ids {
        removed += conn.execute(
            "DELETE FROM playlist_tracks WHERE id = ?1 AND playlist_id = ?2",
            params![entry, id],
        )?;
    }
    if removed > 0 {
        renumber(conn, id)?;
        touch(conn, id)?;
    }
    Ok(removed)
}

/// Close the gaps left by a removal, keeping the order and the ids.
fn renumber(conn: &Connection, id: i64) -> Result<(), DbError> {
    let order: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")?;
        stmt.query_map(params![id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (position, entry) in order.iter().enumerate() {
        conn.execute(
            "UPDATE playlist_tracks SET position = ?2 WHERE id = ?1",
            params![entry, -(position as i64) - 1],
        )?;
    }
    conn.execute(
        "UPDATE playlist_tracks SET position = -position - 1
         WHERE playlist_id = ?1 AND position < 0",
        params![id],
    )?;
    Ok(())
}

/// Replace the whole contents in one go.
///
/// Entry ids do not survive this, so it is only for the case where they cannot
/// mean anything anyway: the server handing over its copy of the playlist,
/// which knows nothing about them. Editing goes through [`reorder_entries`] and
/// [`remove_entries`].
///
/// Positions are rewritten from zero, so nothing depends on what was there
/// before.
pub fn set_playlist_tracks(conn: &Connection, id: i64, track_ids: &[i64]) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
        params![id],
    )?;
    let mut next = 0i64;
    for track_id in track_ids {
        let inserted = conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?3)",
            params![id, next, track_id],
        )?;
        if inserted > 0 {
            next += 1;
        }
    }
    touch(conn, id)
}

/// Up to four covers for the playlist's tile, one per album.
///
/// Album ids, because art is stored and cached per record: naming a track here
/// asks for the same sleeve under a second key, which is a second round trip on
/// a remote library and a second copy on disk — and it hides the record from
/// whatever the caller knows about albums whose art is really the server's
/// placeholder. Distinct albums, in playlist order: four copies of the same
/// sleeve is not a mosaic.
pub fn playlist_cover_album_ids(conn: &Connection, id: i64) -> Result<Vec<i64>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT MIN(pt.position), t.album_id
         FROM playlist_tracks pt
         JOIN tracks t ON t.id = pt.track_id
         WHERE pt.playlist_id = ?1 AND t.album_id IS NOT NULL
         GROUP BY t.album_id
         ORDER BY MIN(pt.position)
         LIMIT 4",
    )?;
    let rows = stmt
        .query_map(params![id], |row| row.get::<_, i64>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Playlists that have never been pushed to a server.
pub fn playlists_without_remote(conn: &Connection) -> Result<Vec<PlaylistRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "{SELECT} WHERE p.remote_id IS NULL GROUP BY p.id ORDER BY p.sort_order"
    ))?;
    let rows = stmt
        .query_map([], row_to_playlist)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Local track ids for a list of the server's song ids, in the order given.
///
/// `None` where the library has never seen that song — a playlist can name
/// tracks a partial sync has not reached yet, and dropping them silently would
/// reorder everything after them.
pub fn track_ids_for_remote_ids(
    conn: &Connection,
    remote_ids: &[String],
) -> Result<Vec<Option<i64>>, DbError> {
    let mut stmt = conn.prepare("SELECT id FROM tracks WHERE remote_id = ?1")?;
    let mut out = Vec::with_capacity(remote_ids.len());
    for remote_id in remote_ids {
        out.push(
            stmt.query_row(params![remote_id], |row| row.get::<_, i64>(0))
                .ok(),
        );
    }
    Ok(out)
}

/// The server's song ids for a playlist's tracks, in order, skipping any the
/// server does not know about.
pub fn remote_ids_for_playlist(conn: &Connection, id: i64) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.remote_id FROM playlist_tracks pt
         JOIN tracks t ON t.id = pt.track_id
         WHERE pt.playlist_id = ?1 AND t.remote_id IS NOT NULL
         ORDER BY pt.position",
    )?;
    let rows = stmt
        .query_map(params![id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Mark the playlist as changed now. Callers that rewrite the track list go
/// through this so the reconciler can tell whose copy is newer.
fn touch(conn: &Connection, id: i64) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET changed_at = datetime('now') WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{sample_meta, upsert_track};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn track(conn: &Connection, title: &str, album: &str) -> i64 {
        upsert_track(conn, &sample_meta(title, "Artist", album)).unwrap()
    }

    #[test]
    fn create_add_and_read_back_in_order() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let id = create_playlist(&conn, "Evening", None).unwrap();

        assert_eq!(add_tracks(&conn, id, &[b, a]).unwrap().len(), 2);
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![b, a]);

        let row = get_playlist(&conn, id).unwrap().unwrap();
        assert_eq!(row.name, "Evening");
        assert_eq!(row.track_count, 2);
        assert_eq!(row.duration_ms, 480_000);
    }

    #[test]
    fn the_same_track_can_appear_twice() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let id = create_playlist(&conn, "Repeat", None).unwrap();

        add_tracks(&conn, id, &[a, a]).unwrap();
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![a, a]);
        assert_eq!(playlist_tracks(&conn, id).unwrap().len(), 2);
    }

    #[test]
    fn entry_ids_survive_a_reorder() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let id = create_playlist(&conn, "Mix", None).unwrap();
        let made = add_tracks(&conn, id, &[a, b]).unwrap();

        reorder_entries(&conn, id, &[made[1], made[0]]).unwrap();

        let entries = playlist_entries(&conn, id).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![made[1], made[0]],
            "the rows swapped places and kept their identities"
        );
        assert_eq!(
            entries.iter().map(|e| e.position).collect::<Vec<_>>(),
            vec![0, 1],
            "positions are renumbered from zero"
        );
    }

    /// The case the whole entry id exists for: a queue item pointing at one of
    /// two copies of the same track has to keep pointing at that one.
    #[test]
    fn the_two_copies_of_a_track_are_different_entries() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let id = create_playlist(&conn, "Repeat", None).unwrap();
        let made = add_tracks(&conn, id, &[a, b, a]).unwrap();
        assert_eq!(made.len(), 3);
        assert_ne!(made[0], made[2], "same track, different rows");

        // Move the second copy to the front; the first copy stays where it is.
        reorder_entries(&conn, id, &[made[2], made[0], made[1]]).unwrap();
        let entries = playlist_entries(&conn, id).unwrap();
        assert_eq!(entries[0].id, made[2]);
        assert_eq!(entries[0].track.id, a);
        assert_eq!(entries[1].id, made[0]);
    }

    #[test]
    fn removing_an_entry_closes_the_gap_and_leaves_the_rest_alone() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let c = track(&conn, "Three", "Album");
        let id = create_playlist(&conn, "Mix", None).unwrap();
        let made = add_tracks(&conn, id, &[a, b, c]).unwrap();

        assert_eq!(remove_entries(&conn, id, &[made[1]]).unwrap(), 1);

        let entries = playlist_entries(&conn, id).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.id, e.position))
                .collect::<Vec<_>>(),
            vec![(made[0], 0), (made[2], 1)],
            "the survivors keep their ids and close up"
        );
    }

    #[test]
    fn setting_the_tracks_rewrites_positions() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let c = track(&conn, "Three", "Album");
        let id = create_playlist(&conn, "Mix", None).unwrap();
        add_tracks(&conn, id, &[a, b, c]).unwrap();

        set_playlist_tracks(&conn, id, &[c, a]).unwrap();
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![c, a]);
    }

    #[test]
    fn tracks_that_no_longer_exist_are_left_out_rather_than_failing() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let id = create_playlist(&conn, "Mix", None).unwrap();

        assert_eq!(add_tracks(&conn, id, &[a, 9999]).unwrap().len(), 1);
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![a]);
    }

    #[test]
    fn deleting_a_playlist_takes_its_members_with_it() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let id = create_playlist(&conn, "Doomed", None).unwrap();
        add_tracks(&conn, id, &[a]).unwrap();

        assert!(delete_playlist(&conn, id).unwrap());
        assert!(!delete_playlist(&conn, id).unwrap());
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn deleting_a_track_removes_it_from_every_playlist() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let id = create_playlist(&conn, "Mix", None).unwrap();
        add_tracks(&conn, id, &[a, b]).unwrap();

        conn.execute("DELETE FROM tracks WHERE id = ?1", params![a])
            .unwrap();
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![b]);
    }

    #[test]
    fn covers_are_one_per_album_in_playlist_order() {
        let conn = test_conn();
        let a1 = track(&conn, "A1", "First");
        let a2 = track(&conn, "A2", "First");
        let b1 = track(&conn, "B1", "Second");
        let id = create_playlist(&conn, "Mix", None).unwrap();
        add_tracks(&conn, id, &[a1, a2, b1]).unwrap();

        let album_of = |track_id: i64| {
            conn.query_row(
                "SELECT album_id FROM tracks WHERE id = ?1",
                params![track_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
        };
        assert_eq!(
            playlist_cover_album_ids(&conn, id).unwrap(),
            vec![album_of(a1), album_of(b1)]
        );
    }

    #[test]
    fn sidebar_order_is_what_reorder_was_given() {
        let conn = test_conn();
        let a = create_playlist(&conn, "A", None).unwrap();
        let b = create_playlist(&conn, "B", None).unwrap();
        let c = create_playlist(&conn, "C", None).unwrap();

        reorder_playlists(&conn, &[c, a, b]).unwrap();
        let order: Vec<i64> = list_playlists(&conn)
            .unwrap()
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(order, vec![c, a, b]);
    }

    #[test]
    fn the_view_preference_survives_a_round_trip() {
        let conn = test_conn();
        let id = create_playlist(&conn, "Mix", None).unwrap();
        assert_eq!(get_playlist(&conn, id).unwrap().unwrap().grouped, None);

        set_playlist_grouped(&conn, id, Some(true)).unwrap();
        assert_eq!(
            get_playlist(&conn, id).unwrap().unwrap().grouped,
            Some(true)
        );
        set_playlist_grouped(&conn, id, None).unwrap();
        assert_eq!(get_playlist(&conn, id).unwrap().unwrap().grouped, None);
    }
}

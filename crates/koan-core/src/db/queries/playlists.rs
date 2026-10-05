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
use super::auth::resolve_user;
use crate::db::connection::DbError;

/// A playlist and what can be known about it without reading its tracks.
#[derive(Debug, Clone)]
pub struct PlaylistRow {
    pub id: i64,
    /// What every surface publishes as its id; see `queries::uids`.
    pub uid: String,
    pub name: String,
    pub comment: Option<String>,
    pub public: bool,
    /// The server's name for whoever owns it, or the owning account's.
    pub owner: Option<String>,
    /// The user it belongs to, resolved (see `queries::auth::resolve_user`).
    pub user_id: i64,
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
    /// Bumped by every local edit.
    pub revision: i64,
    /// The `revision` the last push or pull covered.
    pub synced_revision: Option<i64>,
    /// The server's `changed` as of the last push or pull.
    pub remote_changed: Option<String>,
    /// A smart playlist's rules, as JSON (see `crate::smart`).
    pub rules: Option<String>,
    /// The file in a library folder it was read from. The file decides its
    /// name and what it holds, so neither is edited here.
    pub source_path: Option<String>,
    /// Its contents are not for editing: a smart playlist here, or one the
    /// server says is read-only.
    pub readonly: bool,
}

const SELECT: &str = "SELECT p.id, p.name, p.comment, p.public, COALESCE(p.owner, u.username),
            p.remote_id, p.created_at, p.changed_at, p.sort_order, p.grouped,
            COUNT(pt.track_id), COALESCE(SUM(t.duration_ms), 0), p.user_id,
            COALESCE(p.uid, CAST(p.id AS TEXT)), p.revision, p.synced_revision, p.remote_changed,
            p.rules, p.readonly, p.source_path
     FROM playlists p
     LEFT JOIN users u ON u.id = p.user_id
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
        user_id: row.get(12)?,
        uid: row.get(13)?,
        revision: row.get(14)?,
        synced_revision: row.get(15)?,
        remote_changed: row.get(16)?,
        readonly: row.get::<_, Option<String>>(17)?.is_some() || row.get::<_, i64>(18)? != 0,
        rules: row.get(17)?,
        source_path: row.get(19)?,
    })
}

impl PlaylistRow {
    /// Whether `user` (resolved) may see it: their own, or anyone's public one.
    pub fn readable_by(&self, user: i64) -> bool {
        self.user_id == user || self.public
    }

    /// Whether `user` (resolved) may change it: their own only.
    pub fn editable_by(&self, user: i64) -> bool {
        self.user_id == user
    }

    /// Whether `user` (resolved) may change what it holds: their own, unless
    /// its contents come from rules or the server will not take edits.
    pub fn contents_editable_by(&self, user: i64) -> bool {
        self.editable_by(user) && !self.readonly
    }

    /// Whether it has local edits the server has not had.
    pub fn unsynced(&self) -> bool {
        self.synced_revision != Some(self.revision)
    }
}

/// `user`'s playlists and everyone's public ones, in sidebar order.
pub fn list_playlists(conn: &Connection, user: i64) -> Result<Vec<PlaylistRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "{SELECT} WHERE p.user_id = ?1 OR p.public = 1
         GROUP BY p.id ORDER BY p.sort_order, p.name COLLATE LIBRARY"
    ))?;
    let rows = stmt
        .query_map([resolve_user(conn, user)?], row_to_playlist)?
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

/// Create an empty playlist of `user`'s and return its id.
///
/// New playlists go to the top of the sidebar: you just made it, so it is the
/// one you are about to use.
pub fn create_playlist(
    conn: &Connection,
    user: i64,
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
        "INSERT INTO playlists (name, comment, sort_order, user_id) VALUES (?1, ?2, ?3, ?4)",
        params![name, comment, top, resolve_user(conn, user)?],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_playlist(conn: &Connection, id: i64) -> Result<bool, DbError> {
    Ok(conn.execute("DELETE FROM playlists WHERE id = ?1", params![id])? > 0)
}

pub fn rename_playlist(conn: &Connection, id: i64, name: &str) -> Result<bool, DbError> {
    Ok(conn.execute(
        "UPDATE playlists SET name = ?2, changed_at = datetime('now'), revision = revision + 1
         WHERE id = ?1",
        params![id, name],
    )? > 0)
}

/// Set the sidebar order from the ids in the order they should appear.
pub fn reorder_playlists(conn: &Connection, ids: &[i64]) -> Result<(), DbError> {
    super::atomically(conn, || {
        let mut update =
            conn.prepare_cached("UPDATE playlists SET sort_order = ?2 WHERE id = ?1")?;
        for (position, id) in ids.iter().enumerate() {
            update.execute(params![id, position as i64])?;
        }
        Ok(())
    })
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

/// Attach a server id, and the account on that server it belongs to.
pub fn set_playlist_remote(
    conn: &Connection,
    id: i64,
    remote_id: &str,
    owner: Option<&str>,
    public: bool,
    account: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET remote_id = ?2, owner = ?3, public = ?4, remote_account = ?5
         WHERE id = ?1",
        params![id, remote_id, owner, public as i64, account],
    )?;
    super::adopt_uid(conn, super::UidKind::Playlist, id, remote_id)?;
    Ok(())
}

/// Record whether the server will take edits to this playlist's contents
/// (OpenSubsonic's `readonly`: a smart playlist there).
pub fn set_playlist_readonly(conn: &Connection, id: i64, readonly: bool) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET readonly = ?2 WHERE id = ?1",
        params![id, readonly as i64],
    )?;
    Ok(())
}

/// Record that the server and this copy agree: the server's `changed` stamp
/// as it now stands, and the local revision that was sent or received —
/// `None` for the current one. A push passes the revision it read before
/// sending, so an edit made while it was in flight still counts as unsynced.
pub fn mark_playlist_synced(
    conn: &Connection,
    id: i64,
    remote_changed: Option<&str>,
    revision: Option<i64>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET remote_changed = ?2, synced_revision = COALESCE(?3, revision)
         WHERE id = ?1",
        params![id, remote_changed, revision],
    )?;
    Ok(())
}

/// Turn every playlist tied to a server account other than `account` back
/// into a local one, which the next sync pushes as new. A playlist with a
/// server id but no recorded account is taken to be `account`'s.
///
/// Without this, signing in somewhere else reads every playlist the old
/// server had as deleted on the new one.
pub fn detach_playlists_from_other_accounts(
    conn: &Connection,
    account: &str,
) -> Result<usize, DbError> {
    let detached = conn.execute(
        "UPDATE playlists SET remote_id = NULL, owner = NULL, remote_changed = NULL,
                synced_revision = NULL, remote_account = NULL
         WHERE remote_id IS NOT NULL AND remote_account IS NOT NULL AND remote_account != ?1",
        params![account],
    )?;
    conn.execute(
        "UPDATE playlists SET remote_account = ?1
         WHERE remote_id IS NOT NULL AND remote_account IS NULL",
        params![account],
    )?;
    Ok(detached)
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
    super::atomically(conn, || {
        let mut next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let mut insert = conn.prepare_cached(INSERT_ENTRY)?;
        let mut added = Vec::new();
        for track_id in track_ids {
            if insert.execute(params![id, next, track_id])? > 0 {
                next += 1;
                added.push(conn.last_insert_rowid());
            }
        }
        touch(conn, id)?;
        Ok(added)
    })
}

/// Insert one entry, unless its track has gone: a track that no longer exists
/// would fail the foreign key and take the whole edit down with it.
const INSERT_ENTRY: &str = "INSERT INTO playlist_tracks (playlist_id, position, track_id)
     SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?3)";

/// Positions are unique per playlist, so they cannot be rewritten in place
/// without colliding on the way through. Entries are first moved to negative
/// positions, out of the way of anything the table holds, then flipped back.
const FLIP_NEGATIVE_POSITIONS: &str = "UPDATE playlist_tracks SET position = -position - 1
     WHERE playlist_id = ?1 AND position < 0";

/// Put the entries in this order. Ids are kept, so nothing holding a reference
/// to an entry loses it because the playlist was rearranged.
///
/// Entries not named are left where they are relative to each other and pushed
/// to the end — a caller that names all of them, which every caller does, never
/// meets that case.
pub fn reorder_entries(conn: &Connection, id: i64, entry_ids: &[i64]) -> Result<(), DbError> {
    super::atomically(conn, || {
        let mut update = conn.prepare_cached(
            "UPDATE playlist_tracks SET position = ?3 WHERE id = ?1 AND playlist_id = ?2",
        )?;
        for (position, entry) in entry_ids.iter().enumerate() {
            update.execute(params![entry, id, -(position as i64) - 1])?;
        }
        conn.execute(FLIP_NEGATIVE_POSITIONS, params![id])?;
        touch(conn, id)
    })
}

/// Drop entries by id. Everything after them closes up.
pub fn remove_entries(conn: &Connection, id: i64, entry_ids: &[i64]) -> Result<usize, DbError> {
    super::atomically(conn, || {
        let mut delete =
            conn.prepare_cached("DELETE FROM playlist_tracks WHERE id = ?1 AND playlist_id = ?2")?;
        let mut removed = 0;
        for entry in entry_ids {
            removed += delete.execute(params![entry, id])?;
        }
        if removed > 0 {
            renumber(conn, id)?;
            touch(conn, id)?;
        }
        Ok(removed)
    })
}

/// Close the gaps left by a removal, keeping the order and the ids.
fn renumber(conn: &Connection, id: i64) -> Result<(), DbError> {
    super::atomically(conn, || {
        let order: Vec<i64> = conn
            .prepare_cached(
                "SELECT id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
            )?
            .query_map(params![id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut update =
            conn.prepare_cached("UPDATE playlist_tracks SET position = ?2 WHERE id = ?1")?;
        for (position, entry) in order.iter().enumerate() {
            update.execute(params![entry, -(position as i64) - 1])?;
        }
        conn.execute(FLIP_NEGATIVE_POSITIONS, params![id])?;
        Ok(())
    })
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
    super::atomically(conn, || {
        conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![id],
        )?;
        let mut insert = conn.prepare_cached(INSERT_ENTRY)?;
        let mut next = 0i64;
        for track_id in track_ids {
            if insert.execute(params![id, next, track_id])? > 0 {
                next += 1;
            }
        }
        touch(conn, id)
    })
}

/// Make the playlist hold exactly these tracks, in this order, keeping the
/// ids of entries whose track stays. Whether anything changed: a list that
/// already matches writes nothing and is not marked changed.
///
/// What a smart playlist's evaluation goes through, so a queue following it
/// keeps the items whose tracks are still selected.
pub(crate) fn replace_entries(
    conn: &Connection,
    id: i64,
    track_ids: &[i64],
) -> Result<bool, DbError> {
    super::atomically(conn, || {
        let existing = entry_snapshot(conn, id)?;
        if existing.iter().map(|e| e.1).eq(track_ids.iter().copied()) {
            return Ok(false);
        }
        let mut spare: std::collections::HashMap<i64, std::collections::VecDeque<i64>> =
            std::collections::HashMap::new();
        for &(entry, track) in &existing {
            spare.entry(track).or_default().push_back(entry);
        }
        let merged: Vec<(Option<i64>, i64)> = track_ids
            .iter()
            .map(|&t| (spare.get_mut(&t).and_then(|q| q.pop_front()), t))
            .collect();
        let mut delete = conn.prepare_cached("DELETE FROM playlist_tracks WHERE id = ?1")?;
        for entry in spare.into_values().flatten() {
            delete.execute(params![entry])?;
        }
        let mut update =
            conn.prepare_cached("UPDATE playlist_tracks SET position = ?2 WHERE id = ?1")?;
        let mut insert = conn.prepare_cached(INSERT_ENTRY)?;
        for (position, (entry, track)) in merged.iter().enumerate() {
            let position = -(position as i64) - 1;
            match entry {
                Some(entry) => update.execute(params![entry, position])?,
                None => insert.execute(params![id, position, track])?,
            };
        }
        conn.execute(FLIP_NEGATIVE_POSITIONS, params![id])?;
        touch(conn, id)?;
        Ok(true)
    })
}

/// A playlist's entries, in order: each one's id and its track. What an undo
/// puts back — see [`restore_entries`].
pub fn entry_snapshot(conn: &Connection, id: i64) -> Result<Vec<(i64, i64)>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Put a playlist back exactly as an [`entry_snapshot`] found it.
///
/// Unlike [`set_playlist_tracks`], entries keep their ids, so a queue
/// following the playlist stays locked to it through an undo. An id another
/// playlist has taken since is given a new one; a track gone from the library
/// is left out, as adding it would be refused.
pub fn restore_entries(conn: &Connection, id: i64, entries: &[(i64, i64)]) -> Result<(), DbError> {
    super::atomically(conn, || {
        conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![id],
        )?;
        let mut insert = conn.prepare_cached(
            "INSERT INTO playlist_tracks (id, playlist_id, position, track_id)
             SELECT CASE WHEN EXISTS (SELECT 1 FROM playlist_tracks WHERE id = ?1) THEN NULL ELSE ?1 END,
                    ?2, ?3, ?4
             WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?4)",
        )?;
        let mut next = 0i64;
        for (entry_id, track_id) in entries {
            if insert.execute(params![entry_id, id, next, track_id])? > 0 {
                next += 1;
            }
        }
        touch(conn, id)
    })
}

/// Take the server's copy of the contents, keeping what only this copy can hold.
///
/// Entries whose track has no server id never went to the server, so its copy
/// cannot mention them; they stay, at their old positions as near as the new
/// length allows. Entries for tracks still present keep their ids, so a queue
/// following the playlist stays locked to it. Does not mark the playlist
/// changed: this is the server's copy, not an edit.
pub fn merge_server_tracks(
    conn: &Connection,
    id: i64,
    server_track_ids: &[i64],
) -> Result<(), DbError> {
    super::atomically(conn, || {
        merge_server_tracks_inner(conn, id, server_track_ids)
    })
}

fn merge_server_tracks_inner(
    conn: &Connection,
    id: i64,
    server_track_ids: &[i64],
) -> Result<(), DbError> {
    let existing: Vec<(i64, i64, bool)> = {
        let mut stmt = conn.prepare(
            "SELECT pt.id, pt.track_id, t.remote_id IS NULL FROM playlist_tracks pt
             JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = ?1 ORDER BY pt.position",
        )?;
        stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut merged: Vec<(Option<i64>, i64)> = server_track_ids.iter().map(|&t| (None, t)).collect();
    for (index, &(entry, track, local_only)) in existing.iter().enumerate() {
        if local_only {
            merged.insert(index.min(merged.len()), (Some(entry), track));
        }
    }
    let mut spare: Vec<(i64, i64)> = existing
        .iter()
        .filter(|(_, _, local_only)| !local_only)
        .map(|&(entry, track, _)| (entry, track))
        .collect();
    for (entry, track) in merged.iter_mut().filter(|(e, _)| e.is_none()) {
        if let Some(i) = spare.iter().position(|&(_, t)| t == *track) {
            *entry = Some(spare.remove(i).0);
        }
    }
    let mut delete = conn.prepare_cached("DELETE FROM playlist_tracks WHERE id = ?1")?;
    for (entry, _) in spare {
        delete.execute(params![entry])?;
    }

    let mut update =
        conn.prepare_cached("UPDATE playlist_tracks SET position = ?2 WHERE id = ?1")?;
    let mut insert = conn.prepare_cached(INSERT_ENTRY)?;
    for (position, (entry, track)) in merged.iter().enumerate() {
        let position = -(position as i64) - 1;
        match entry {
            Some(entry) => update.execute(params![entry, position])?,
            None => insert.execute(params![id, position, track])?,
        };
    }
    conn.execute(FLIP_NEGATIVE_POSITIONS, params![id])?;
    renumber(conn, id)
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

/// `user`'s playlists that have never been pushed to a server. Smart ones
/// are never pushed: the server would hold a copy of today's selection that
/// nothing keeps up to date.
pub fn playlists_without_remote(conn: &Connection, user: i64) -> Result<Vec<PlaylistRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "{SELECT} WHERE p.remote_id IS NULL AND p.user_id = ?1 AND p.rules IS NULL
         GROUP BY p.id ORDER BY p.sort_order"
    ))?;
    let rows = stmt
        .query_map([resolve_user(conn, user)?], row_to_playlist)?
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
        "UPDATE playlists SET changed_at = datetime('now'), revision = revision + 1 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::LOCAL_USER;
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

    /// Edits are their own transaction, and nest inside a caller's: rolling
    /// the caller back takes the edit with it.
    #[test]
    fn an_edit_inside_a_callers_transaction_rolls_back_with_it() {
        let conn = test_conn();
        let (a, b) = (track(&conn, "A", "X"), track(&conn, "B", "X"));
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
        add_tracks(&conn, id, &[a]).unwrap();
        assert!(conn.is_autocommit(), "committed on its own");

        let tx = conn.unchecked_transaction().unwrap();
        add_tracks(&tx, id, &[b]).unwrap();
        let entries = playlist_entry_ids(&tx, id).unwrap();
        reorder_entries(&tx, id, &[entries[1], entries[0]]).unwrap();
        assert_eq!(playlist_track_ids(&tx, id).unwrap(), [b, a]);
        drop(tx);

        assert_eq!(playlist_track_ids(&conn, id).unwrap(), [a]);
    }

    #[test]
    fn create_add_and_read_back_in_order() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let id = create_playlist(&conn, LOCAL_USER, "Evening", None).unwrap();

        assert_eq!(add_tracks(&conn, id, &[b, a]).unwrap().len(), 2);
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![b, a]);

        let row = get_playlist(&conn, id).unwrap().unwrap();
        assert_eq!(row.name, "Evening");
        assert_eq!(row.track_count, 2);
        assert_eq!(row.duration_ms, 480_000);
    }

    #[test]
    fn restore_puts_entries_back_with_their_ids() {
        let conn = test_conn();
        let (a, b, c) = (
            track(&conn, "A", "X"),
            track(&conn, "B", "X"),
            track(&conn, "C", "X"),
        );
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
        add_tracks(&conn, id, &[a, b, c]).unwrap();
        let before = entry_snapshot(&conn, id).unwrap();
        assert_eq!(
            before.iter().map(|e| e.1).collect::<Vec<_>>(),
            vec![a, b, c]
        );

        remove_entries(&conn, id, &[before[1].0]).unwrap();
        reorder_entries(&conn, id, &[before[2].0, before[0].0]).unwrap();
        restore_entries(&conn, id, &before).unwrap();

        assert_eq!(entry_snapshot(&conn, id).unwrap(), before);
    }

    #[test]
    fn the_same_track_can_appear_twice() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let id = create_playlist(&conn, LOCAL_USER, "Repeat", None).unwrap();

        add_tracks(&conn, id, &[a, a]).unwrap();
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![a, a]);
        assert_eq!(playlist_tracks(&conn, id).unwrap().len(), 2);
    }

    #[test]
    fn entry_ids_survive_a_reorder() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let b = track(&conn, "Two", "Album");
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
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
        let id = create_playlist(&conn, LOCAL_USER, "Repeat", None).unwrap();
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
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
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
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
        add_tracks(&conn, id, &[a, b, c]).unwrap();

        set_playlist_tracks(&conn, id, &[c, a]).unwrap();
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![c, a]);
    }

    #[test]
    fn tracks_that_no_longer_exist_are_left_out_rather_than_failing() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();

        assert_eq!(add_tracks(&conn, id, &[a, 9999]).unwrap().len(), 1);
        assert_eq!(playlist_track_ids(&conn, id).unwrap(), vec![a]);
    }

    #[test]
    fn deleting_a_playlist_takes_its_members_with_it() {
        let conn = test_conn();
        let a = track(&conn, "One", "Album");
        let id = create_playlist(&conn, LOCAL_USER, "Doomed", None).unwrap();
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
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
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
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
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
        let a = create_playlist(&conn, LOCAL_USER, "A", None).unwrap();
        let b = create_playlist(&conn, LOCAL_USER, "B", None).unwrap();
        let c = create_playlist(&conn, LOCAL_USER, "C", None).unwrap();

        reorder_playlists(&conn, &[c, a, b]).unwrap();
        let order: Vec<i64> = list_playlists(&conn, LOCAL_USER)
            .unwrap()
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(order, vec![c, a, b]);
    }

    #[test]
    fn the_view_preference_survives_a_round_trip() {
        let conn = test_conn();
        let id = create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
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

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
    for row in rows {
        if let Ok(path) = row {
            set.insert(path);
        }
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

/// Look up the remote_id for a track by its path. Returns None for local-only tracks.
pub fn remote_id_for_path(conn: &Connection, path: &Path) -> rusqlite::Result<Option<String>> {
    let path_str = path.to_string_lossy();
    conn.query_row(
        "SELECT remote_id FROM tracks WHERE path = ?1 OR cached_path = ?1",
        [path_str.as_ref()],
        |row| row.get(0),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Load all favourite track paths that have a remote_id, returning (path, remote_id) pairs.
pub fn favourites_with_remote_id(
    conn: &Connection,
) -> rusqlite::Result<Vec<(PathBuf, String)>> {
    let mut stmt = conn.prepare(
        "SELECT f.track_path, t.remote_id FROM favourites f
         JOIN tracks t ON (t.path = f.track_path OR t.cached_path = f.track_path)
         WHERE t.remote_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let rid: String = row.get(1)?;
        Ok((PathBuf::from(path), rid))
    })?;
    let mut result = Vec::new();
    for row in rows {
        if let Ok(pair) = row {
            result.push(pair);
        }
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
                "SELECT COALESCE(cached_path, path) FROM tracks WHERE remote_id = ?1",
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

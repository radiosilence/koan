//! Cover images beside the tracks, as the last scan saw them (`cover_files`).
//!
//! An image added, replaced or removed changes no track row, so nothing a
//! scan stores says the records under it now look different. The folder
//! watcher's directory rescans name every record under the directory anyway
//! (`queries::evict_art_under`); a full scan, which is all a library on a
//! network share or in a container gets, compares the images it walked with
//! the ones it saw last and names the records under each that differ.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::db::connection::DbError;
use crate::db::queries;

/// A file's modification time, in seconds, and length.
fn stamp(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    Some((mtime, meta.len() as i64))
}

/// Store the images `found` under `dir`, read in full, in place of what was
/// stored for it, and return the directories holding an image that was
/// added, replaced or removed.
fn store(conn: &Connection, dir: &Path, found: &[PathBuf]) -> Result<Vec<PathBuf>, DbError> {
    let (lower, upper) = queries::folder_prefix_range(dir);
    let stored: HashMap<String, (i64, i64)> = conn
        .prepare("SELECT path, mtime, size FROM cover_files WHERE path >= ?1 AND path < ?2")?
        .query_map(params![lower, upper], |r| {
            Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))
        })?
        .collect::<Result<_, _>>()?;
    let mut seen = HashSet::new();
    let mut changed: HashSet<PathBuf> = HashSet::new();
    let mut put = conn.prepare(
        "INSERT INTO cover_files (path, mtime, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime, size = excluded.size",
    )?;
    for image in found.iter().filter(|p| p.starts_with(dir)) {
        let (Some(path), Some(now)) = (image.to_str(), stamp(image)) else {
            continue;
        };
        seen.insert(path);
        if stored.get(path) != Some(&now) {
            put.execute(params![path, now.0, now.1])?;
            changed.extend(image.parent().map(Path::to_path_buf));
        }
    }
    let mut gone = conn.prepare("DELETE FROM cover_files WHERE path = ?1")?;
    for path in stored.keys().filter(|p| !seen.contains(p.as_str())) {
        gone.execute([path])?;
        changed.extend(Path::new(path).parent().map(Path::to_path_buf));
    }
    Ok(changed.into_iter().collect())
}

/// After a full scan of the library folder `root`, read in full: name the
/// records under every directory whose images changed. The first look at a
/// root only records what is there, or upgrading would refresh every cover
/// in the library at once; the root's own row marks that it has been looked
/// at.
pub(super) fn settle_root(conn: &Connection, root: &Path, found: &[PathBuf]) {
    let Some(marker) = root.to_str() else { return };
    let run = || -> Result<usize, DbError> {
        let looked = conn
            .query_row(
                "SELECT 1 FROM cover_files WHERE path = ?1",
                [marker],
                |_| Ok(()),
            )
            .is_ok();
        let changed = store(conn, root, found)?;
        if !looked {
            conn.execute(
                "INSERT INTO cover_files (path, mtime, size) VALUES (?1, 0, 0)",
                [marker],
            )?;
            return Ok(0);
        }
        let mut named = 0;
        for dir in &changed {
            named += queries::evict_art_under(conn, dir)?;
        }
        Ok(named)
    };
    match queries::atomically(conn, run) {
        Ok(0) => {}
        Ok(n) => log::info!(
            "covers beside the tracks changed under {}: {n} records refreshed",
            root.display()
        ),
        Err(e) => log::warn!("cover images under {} not compared: {e}", root.display()),
    }
}

/// After a rescan of `dir`, which names every record under it by itself:
/// keep what a later full scan compares against.
pub(super) fn settle_dir(conn: &Connection, dir: &Path, found: &[PathBuf]) {
    if let Err(e) = queries::atomically(conn, || store(conn, dir, found)) {
        log::warn!("cover images under {} not recorded: {e}", dir.display());
    }
}

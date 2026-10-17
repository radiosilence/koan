//! What the TUI does to playlists, over the same calls the apps make: every
//! edit is written here and pushed to the server in the background, and a
//! playlist that the library or the server owns is not edited.

use std::sync::{Arc, LazyLock, OnceLock};

use crossbeam_channel::Sender;
use koan_core::db::connection::Database;
use koan_core::db::pool;
use koan_core::db::queries::{self, PlaylistEntry, PlaylistRow};
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueMode, SharedPlayerState};

/// Earlier states of each playlist edited here, for undo and redo. Lasts the
/// session, as the apps' does.
static HISTORY: LazyLock<koan_core::playlists::PlaylistHistory> = LazyLock::new(Default::default);

/// The queue an edit is carried to while the queue is the playlist.
static QUEUE: OnceLock<(Arc<SharedPlayerState>, Sender<PlayerCommand>)> = OnceLock::new();

/// Names the player an edit to a playlist can reach. Without it an edit only
/// reaches the database.
pub fn attach(state: Arc<SharedPlayerState>, tx: Sender<PlayerCommand>) {
    QUEUE.set((state, tx)).ok();
}

fn locked(db: &Database, id: i64) -> bool {
    QUEUE
        .get()
        .is_some_and(|(state, _)| koan_core::playlists::locked_to(db, state, id))
}

fn follow(db: &Database, id: i64, was_locked: bool) {
    if let Some((state, tx)) = QUEUE.get() {
        koan_core::playlists::follow_edit(db, state, id, was_locked, &|cmd| {
            tx.send(cmd).ok();
        });
    }
}

/// An edit to the contents of a playlist: refused for one that is not the
/// user's to fill, recorded for undo, carried to the queue if it is the
/// playlist, and pushed to the server.
fn edit<T>(id: i64, change: impl FnOnce(&Database) -> Result<T, String>) -> Result<T, String> {
    let db = db()?;
    fillable(&db, id)?;
    let before =
        koan_core::playlists::PlaylistHistory::before(&db.conn, id).map_err(|e| e.to_string())?;
    let was_locked = locked(&db, id);
    let out = change(&db)?;
    HISTORY.record(before);
    follow(&db, id, was_locked);
    koan_core::playlists::push_to_remote(id);
    Ok(out)
}

/// Undoes (or redoes) the last edit to the playlist; whether there was one.
pub fn step(id: i64, undo: bool) -> Result<bool, String> {
    let db = db()?;
    fillable(&db, id)?;
    let was_locked = locked(&db, id);
    let moved = if undo {
        HISTORY.undo(&db.conn, id)
    } else {
        HISTORY.redo(&db.conn, id)
    }
    .map_err(|e| e.to_string())?;
    if moved {
        follow(&db, id, was_locked);
        koan_core::playlists::push_to_remote(id);
    }
    Ok(moved)
}

/// Every playlist, smart ones evaluated first if they are due.
pub fn list() -> Vec<PlaylistRow> {
    let Ok(db) = pool::shared().get() else {
        return Vec::new();
    };
    queries::smart::refresh_due(&db.conn, queries::LOCAL_USER).ok();
    queries::list_playlists(&db.conn, queries::LOCAL_USER).unwrap_or_default()
}

pub fn get(id: i64) -> Option<PlaylistRow> {
    let db = pool::shared().get().ok()?;
    queries::get_playlist(&db.conn, id).ok().flatten()
}

/// The playlist's entries in order. Duplicates are kept: the same song twice
/// in a row is something people do on purpose.
pub fn entries(id: i64) -> Vec<PlaylistEntry> {
    let Ok(db) = pool::shared().get() else {
        return Vec::new();
    };
    queries::smart::refresh_if_due(&db.conn, id).ok();
    queries::playlist_entries(&db.conn, id).unwrap_or_default()
}

fn db() -> Result<pool::Handle<'static>, String> {
    pool::shared().get().map_err(|e| e.to_string())
}

/// Why the playlist `name` takes no edits to what it holds.
pub fn read_only(name: &str) -> String {
    format!("'{name}' is read-only: its rules, its file or its server decide what it holds")
}

/// The playlist, if its contents are the user's to change.
fn fillable(db: &Database, id: i64) -> Result<PlaylistRow, String> {
    match queries::get_playlist(&db.conn, id).map_err(|e| e.to_string())? {
        Some(list) if list.readonly => Err(read_only(&list.name)),
        Some(list) => Ok(list),
        None => Err("that playlist is gone".into()),
    }
}

/// A new playlist holding `track_ids`; its id.
pub fn create(name: &str, track_ids: &[i64]) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a playlist needs a name".into());
    }
    let db = db()?;
    let id = queries::create_playlist(&db.conn, queries::LOCAL_USER, name, None)
        .map_err(|e| e.to_string())?;
    if !track_ids.is_empty() {
        queries::add_tracks(&db.conn, id, track_ids).map_err(|e| e.to_string())?;
    }
    koan_core::playlists::push_to_remote(id);
    Ok(id)
}

/// Whether the playlist's name is the user's to change: one read from a file
/// is named by it.
pub fn renamable(list: &PlaylistRow) -> Result<(), String> {
    match list.source_path {
        Some(_) => Err(format!(
            "'{}' is named by its file in the library",
            list.name
        )),
        None => Ok(()),
    }
}

pub fn rename(id: i64, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a playlist needs a name".into());
    }
    let db = db()?;
    if let Some(list) = queries::get_playlist(&db.conn, id).map_err(|e| e.to_string())? {
        renamable(&list)?;
    }
    queries::rename_playlist(&db.conn, id, name).map_err(|e| e.to_string())?;
    koan_core::playlists::push_to_remote(id);
    Ok(())
}

/// Deletes the playlist here and, when it has a server's copy, there; the
/// server id is read first or the next sync would bring it back.
pub fn delete(id: i64) -> Result<(), String> {
    let db = db()?;
    let remote_id = queries::get_playlist(&db.conn, id)
        .ok()
        .flatten()
        .and_then(|p| p.remote_id);
    if queries::delete_playlist(&db.conn, id).map_err(|e| e.to_string())?
        && let Some(remote_id) = remote_id
    {
        koan_core::playlists::delete_on_remote(remote_id);
    }
    Ok(())
}

/// Appends tracks; how many landed.
pub fn add(id: i64, track_ids: &[i64]) -> Result<usize, String> {
    edit(id, |db| {
        queries::add_tracks(&db.conn, id, track_ids)
            .map(|added| added.len())
            .map_err(|e| e.to_string())
    })
}

pub fn remove(id: i64, entry_ids: &[i64]) -> Result<usize, String> {
    edit(id, |db| {
        queries::remove_entries(&db.conn, id, entry_ids).map_err(|e| e.to_string())
    })
}

/// The entry ids with the one at `from` moved to `to`.
pub fn moved(order: &[i64], from: usize, to: usize) -> Vec<i64> {
    let mut order = order.to_vec();
    if from < order.len() {
        let id = order.remove(from);
        order.insert(to.min(order.len()), id);
    }
    order
}

/// Puts the entries in `order`. Ids survive, so a queue item that holds one
/// still knows which row it means.
pub fn reorder(id: i64, order: &[i64]) -> Result<(), String> {
    edit(id, |db| {
        queries::reorder_entries(&db.conn, id, order).map_err(|e| e.to_string())
    })
}

/// Shuffles the playlist itself, for good: its entries in a new order.
pub fn shuffle(id: i64) -> Result<(), String> {
    edit(id, |db| {
        let mut entries = queries::playlist_entries(&db.conn, id).map_err(|e| e.to_string())?;
        koan_core::helpers::shuffle(&mut entries);
        let order: Vec<i64> = entries.iter().map(|e| e.id).collect();
        queries::reorder_entries(&db.conn, id, &order).map_err(|e| e.to_string())
    })
}

/// Remembers whether the playlist is looked at grouped by album.
pub fn set_grouped(id: i64, grouped: bool) -> Result<(), String> {
    let db = db()?;
    queries::set_playlist_grouped(&db.conn, id, Some(grouped)).map_err(|e| e.to_string())
}

/// Writes the playlist as an M3U8 at `path`; what was written, in words.
pub fn export(id: i64, path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("a file to write to, please".into());
    }
    let path = match path.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME").map_or(path.into(), |home| format!("{home}/{rest}")),
        None => path.to_string(),
    };
    let db = db()?;
    let done = koan_core::playlists::export_m3u8(&db, id, std::path::Path::new(&path))
        .map_err(|e| format!("couldn't write {path}: {e}"))?;
    let tracks = super::track_rows::count(done.written, "track");
    Ok(match done.skipped {
        0 => format!("exported {tracks} to {path}"),
        n => format!(
            "exported {tracks} to {path}; {n} aren't downloaded, so they have no file to point at"
        ),
    })
}

/// The queue from the playlist's entries, as the player is told to play it.
/// Each item remembers the row it came from, which says which of two copies
/// of a song is playing.
pub fn queue_command(
    entries: &[PlaylistEntry],
    start_entry: Option<i64>,
    shuffled: bool,
) -> Option<PlayerCommand> {
    if entries.is_empty() {
        return None;
    }
    let db = pool::shared().get().ok()?;
    let tracks: Vec<_> = entries.iter().map(|e| e.track.clone()).collect();
    // One item per track, in order: the entries already hold their tracks.
    let mut items = koan_core::helpers::playlist_items_for_tracks(&db, &tracks);
    for (item, entry) in items.iter_mut().zip(entries) {
        item.playlist_entry_id = Some(entry.id);
    }
    let start = start_entry
        .filter(|_| !shuffled)
        .and_then(|entry| entries.iter().position(|e| e.id == entry))
        .unwrap_or(0);
    Some(PlayerCommand::ReplacePlaylist {
        items,
        start,
        position_ms: 0,
        play: true,
        mode: if shuffled {
            QueueMode::Shuffled
        } else {
            QueueMode::InOrder
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_moves_to_where_it_is_sent() {
        assert_eq!(moved(&[1, 2, 3, 4], 0, 2), vec![2, 3, 1, 4]);
        assert_eq!(moved(&[1, 2, 3, 4], 3, 0), vec![4, 1, 2, 3]);
        assert_eq!(moved(&[1, 2, 3], 1, 9), vec![1, 3, 2]);
        assert_eq!(moved(&[1, 2], 5, 0), vec![1, 2]);
    }

    #[test]
    fn a_playlist_needs_a_name() {
        assert!(create("   ", &[]).is_err());
        assert!(rename(1, "").is_err());
    }
}

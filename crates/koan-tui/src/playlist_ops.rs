//! What the TUI does to playlists, over the same calls the apps make: every
//! edit is written here and pushed to the server in the background, and a
//! playlist that the library or the server owns is not edited.

use koan_core::db::pool;
use koan_core::db::queries::{self, PlaylistEntry, PlaylistRow};
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::QueueMode;

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

fn db() -> Result<koan_core::db::pool::Handle<'static>, String> {
    pool::shared().get().map_err(|e| e.to_string())
}

/// The playlist, if its contents are the user's to change.
fn fillable(db: &koan_core::db::connection::Database, id: i64) -> Result<PlaylistRow, String> {
    match queries::get_playlist(&db.conn, id).map_err(|e| e.to_string())? {
        Some(list) if list.readonly => Err(format!(
            "'{}' is read-only: its rules, its file or its server decide what it holds",
            list.name
        )),
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

pub fn rename(id: i64, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a playlist needs a name".into());
    }
    let db = db()?;
    if let Some(list) = queries::get_playlist(&db.conn, id).map_err(|e| e.to_string())?
        && list.source_path.is_some()
    {
        return Err(format!(
            "'{}' is named by its file in the library",
            list.name
        ));
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
    let db = db()?;
    fillable(&db, id)?;
    let added = queries::add_tracks(&db.conn, id, track_ids).map_err(|e| e.to_string())?;
    koan_core::playlists::push_to_remote(id);
    Ok(added.len())
}

pub fn remove(id: i64, entry_ids: &[i64]) -> Result<usize, String> {
    let db = db()?;
    fillable(&db, id)?;
    let removed = queries::remove_entries(&db.conn, id, entry_ids).map_err(|e| e.to_string())?;
    koan_core::playlists::push_to_remote(id);
    Ok(removed)
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
    let db = db()?;
    fillable(&db, id)?;
    queries::reorder_entries(&db.conn, id, order).map_err(|e| e.to_string())?;
    koan_core::playlists::push_to_remote(id);
    Ok(())
}

/// The queue from the playlist's entries, as the player is told to play it.
/// Each item remembers the row it came from, which says which of two copies
/// of a song is playing.
pub fn queue_command(
    entries: &[PlaylistEntry],
    start_entry: Option<i64>,
    shuffled: bool,
) -> Option<PlayerCommand> {
    let db = pool::shared().get().ok()?;
    let tracks: Vec<_> = entries.iter().map(|e| e.track.clone()).collect();
    let mut items = koan_core::helpers::playlist_items_for_tracks(&db, &tracks);
    // Zipped against the entries whose track resolved, not against all of
    // them: a playlist naming a track the library has lost yields fewer items
    // than entries, and zipping the two directly would hand every entry after
    // the gap the wrong id.
    let resolved: std::collections::HashSet<i64> = items.iter().filter_map(|i| i.db_id).collect();
    let kept = entries.iter().filter(|e| resolved.contains(&e.track.id));
    for (item, entry) in items.iter_mut().zip(kept) {
        item.playlist_entry_id = Some(entry.id);
    }
    if items.is_empty() {
        return None;
    }
    let start = match start_entry.filter(|_| !shuffled) {
        Some(entry) => entries
            .iter()
            .skip_while(|e| e.id != entry)
            .find_map(|e| items.iter().position(|i| i.playlist_entry_id == Some(e.id)))
            .unwrap_or(0),
        None => 0,
    };
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

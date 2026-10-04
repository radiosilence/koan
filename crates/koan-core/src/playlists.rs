//! Playlists beyond the database: keeping them in step with the server, and
//! writing them out as files.
//!
//! The database module owns what a playlist *is*. This owns what happens to it
//! next — which is either a Subsonic call or an M3U8 on disk.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::connection::{Database, DbError};
use crate::db::queries;
use crate::helpers::subsonic_client;
use crate::player::state::SharedPlayerState;
use crate::remote::client::SubsonicClient;

/// Each playlist's earlier states, for undo and redo.
///
/// The queue keeps its own (`player::undo`); this is the same for playlists,
/// held by whoever edits them. An edit records the playlist as it was first;
/// undoing puts that back exactly, entry ids included, so a queue following
/// the playlist stays locked to it. Per playlist, so undoing in one never
/// reaches into another, and in memory: it lasts the session, as the queue's
/// does.
#[derive(Default)]
pub struct PlaylistHistory {
    steps: parking_lot::Mutex<HashMap<i64, Steps>>,
}

#[derive(Default)]
struct Steps {
    undo: Vec<Vec<(i64, i64)>>,
    redo: Vec<Vec<(i64, i64)>>,
}

/// How many edits back a playlist can go, as the queue's history.
const HISTORY_DEPTH: usize = 100;

impl PlaylistHistory {
    /// Before an edit: the playlist as it is now. A new edit forgets what had
    /// been undone.
    pub fn record(&self, conn: &rusqlite::Connection, id: i64) -> Result<(), DbError> {
        let before = queries::entry_snapshot(conn, id)?;
        let mut steps = self.steps.lock();
        let steps = steps.entry(id).or_default();
        steps.undo.push(before);
        if steps.undo.len() > HISTORY_DEPTH {
            steps.undo.remove(0);
        }
        steps.redo.clear();
        Ok(())
    }

    /// Back one edit. Whether there was one.
    pub fn undo(&self, conn: &rusqlite::Connection, id: i64) -> Result<bool, DbError> {
        self.step(conn, id, true)
    }

    /// Forward one undone edit. Whether there was one.
    pub fn redo(&self, conn: &rusqlite::Connection, id: i64) -> Result<bool, DbError> {
        self.step(conn, id, false)
    }

    fn step(&self, conn: &rusqlite::Connection, id: i64, back: bool) -> Result<bool, DbError> {
        let mut all = self.steps.lock();
        let steps = all.entry(id).or_default();
        let target = if back {
            steps.undo.pop()
        } else {
            steps.redo.pop()
        };
        let Some(target) = target else {
            return Ok(false);
        };
        let now = queries::entry_snapshot(conn, id)?;
        queries::restore_entries(conn, id, &target)?;
        if back {
            steps.redo.push(now)
        } else {
            steps.undo.push(now)
        }
        Ok(true)
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::db::queries::{LOCAL_USER, sample_meta, upsert_track};

    #[test]
    fn undo_and_redo_walk_a_playlists_edits() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let (a, b) = (
            upsert_track(&conn, &sample_meta("A", "Artist", "X")).unwrap(),
            upsert_track(&conn, &sample_meta("B", "Artist", "X")).unwrap(),
        );
        let id = queries::create_playlist(&conn, LOCAL_USER, "Mix", None).unwrap();
        let history = PlaylistHistory::default();

        history.record(&conn, id).unwrap();
        queries::add_tracks(&conn, id, &[a, b]).unwrap();
        let full = queries::entry_snapshot(&conn, id).unwrap();
        history.record(&conn, id).unwrap();
        queries::remove_entries(&conn, id, &[full[0].0]).unwrap();

        assert!(history.undo(&conn, id).unwrap());
        assert_eq!(queries::entry_snapshot(&conn, id).unwrap(), full);
        assert!(history.undo(&conn, id).unwrap());
        assert!(queries::entry_snapshot(&conn, id).unwrap().is_empty());
        assert!(!history.undo(&conn, id).unwrap(), "nothing further back");

        assert!(history.redo(&conn, id).unwrap());
        assert_eq!(queries::entry_snapshot(&conn, id).unwrap(), full);
    }
}

/// The queue, and the playlist or record it is still exactly.
///
/// While the two match, the queue *follows* a playlist: an edit there is an
/// edit here, quietly. The moment you rearrange the queue yourself or add to
/// it, they stop matching and the playlist becomes a document you are editing
/// rather than the thing you are listening to.
///
/// A record cannot be edited, so locking to one buys no following — only the
/// ability to say what you are listening to, which is worth saying.
///
/// Derived rather than tracked: there is no flag to keep in sync, nothing to
/// persist and nothing to migrate, and it cannot get stuck, because a queue
/// that stops matching stops being locked and one that happens to match again
/// is locked again. Playing a playlist shuffled scrambles the order on purpose,
/// so that queue is not locked.
pub fn queue_lock(db: &Database, state: &SharedPlayerState) -> Option<QueueLock> {
    let (items, _) = state.snapshot_playlist();
    if items.is_empty() {
        return None;
    }

    // Every item has to have come from the same playlist. One that did not —
    // played next, dropped in — is the queue having diverged.
    let entry_ids: Vec<i64> = items.iter().filter_map(|i| i.playlist_entry_id).collect();
    if entry_ids.len() == items.len()
        && let Ok(Some(playlist_id)) = queries::playlist_of_entry(&db.conn, entry_ids[0])
        && queries::playlist_entry_ids(&db.conn, playlist_id).is_ok_and(|ids| ids == entry_ids)
    {
        return Some(QueueLock::Playlist(playlist_id));
    }

    // A record needs no provenance of its own: an album *is* an ordered set of
    // tracks in the library, so the queue being that album is a question about
    // the tracks it holds. Which means this works for a queue restored from a
    // previous session, where nothing remembers where it came from.
    let track_ids: Vec<i64> = items.iter().filter_map(|i| i.db_id).collect();
    if track_ids.len() != items.len() {
        return None;
    }
    let album_id = queries::get_track_row(&db.conn, track_ids[0])
        .ok()
        .flatten()?
        .album_id?;
    let album: Vec<i64> = queries::tracks_for_album(&db.conn, album_id)
        .ok()?
        .into_iter()
        .map(|t| t.id)
        .collect();
    (album == track_ids).then_some(QueueLock::Album(album_id))
}

/// What the queue still is, when it is still something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueLock {
    Playlist(i64),
    Album(i64),
}

/// What a reconciliation did.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaylistSync {
    /// Playlists taken from the server, new or updated.
    pub pulled: usize,
    /// Playlists sent to the server, new or updated.
    pub pushed: usize,
}

/// Reconcile playlists with the server, both directions.
///
/// Unlike favourites, a playlist has an order, so this is not a union. Each
/// side's change is judged against its own record: the server's `changed`
/// against the stamp it had at the last sync, the local `changed_at` against
/// the one that sync covered. Only one side moved: that side wins. Both moved:
/// last writer wins on `changed`. Neither: nothing is fetched at all.
///
/// Playlists that have never been to the server are created there. Ones the
/// server no longer has are dropped locally: deleting a playlist on Navidrome
/// and having it reappear on the next sync would make deletion impossible.
/// Playlists tied to another server or account are first made local again, so
/// signing in somewhere else never reads them as deleted.
pub fn reconcile_playlists(
    db: &Database,
    client: &SubsonicClient,
    url: &str,
    username: &str,
) -> PlaylistSync {
    let mut out = PlaylistSync::default();
    let account = account_key(url, username);

    let remote = match client.get_playlists() {
        Ok(lists) => lists,
        Err(e) => {
            log::warn!("could not fetch playlists from the server: {e}");
            return out;
        }
    };

    match queries::detach_playlists_from_other_accounts(&db.conn, &account) {
        Ok(0) => {}
        Ok(n) => log::info!("{n} playlists belonged to another server; keeping them as local"),
        Err(e) => {
            log::warn!("could not check which server playlists belong to: {e}");
            return out;
        }
    }

    // Only playlists this user owns are ours to write back. A public playlist
    // belonging to someone else is still worth having locally, but pushing our
    // copy of it would be editing their playlist.
    let mut seen_remote_ids = Vec::new();

    for summary in &remote {
        seen_remote_ids.push(summary.id.clone());
        let local = queries::playlist_by_remote_id(&db.conn, &summary.id)
            .ok()
            .flatten();
        let ours = summary
            .owner
            .as_deref()
            .is_none_or(|owner| owner == username);

        if let Some(local) = &local {
            let server_moved = summary.changed.as_deref() != local.remote_changed.as_deref();
            let local_moved = ours && local.unsynced();
            if local_moved
                && (!server_moved || newer(&local.changed_at, summary.changed.as_deref()))
            {
                if push(db, client, &account, local.id).is_ok() {
                    out.pushed += 1;
                }
                continue;
            }
            if !server_moved && !local_moved {
                continue;
            }
        }

        let full = match client.get_playlist(&summary.id) {
            Ok(full) => full,
            Err(e) => {
                log::warn!("could not fetch playlist {}: {e}", summary.id);
                continue;
            }
        };

        let id = match local {
            Some(local) => local.id,
            None => {
                match queries::create_playlist(
                    &db.conn,
                    queries::LOCAL_USER,
                    &summary.name,
                    summary.comment.as_deref(),
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        log::warn!("could not store playlist {}: {e}", summary.name);
                        continue;
                    }
                }
            }
        };

        let _ = queries::rename_playlist(&db.conn, id, &summary.name);
        let _ = queries::set_playlist_remote(
            &db.conn,
            id,
            &summary.id,
            summary.owner.as_deref(),
            summary.public,
            &account,
        );

        let remote_song_ids: Vec<String> = full.entry.iter().map(|s| s.id.clone()).collect();
        let track_ids: Vec<i64> = queries::track_ids_for_remote_ids(&db.conn, &remote_song_ids)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        if let Err(e) = queries::merge_server_tracks(&db.conn, id, &track_ids) {
            log::warn!(
                "could not store playlist contents for {}: {e}",
                summary.name
            );
            continue;
        }
        // The rename stamped the local copy as changed; this is the server's
        // copy, so what is here now is what the server has.
        let _ = queries::mark_playlist_synced(&db.conn, id, summary.changed.as_deref(), None);
        out.pulled += 1;
    }

    // A playlist we hold a server id for that the server no longer lists was
    // deleted there.
    for local in queries::list_playlists(&db.conn, queries::LOCAL_USER).unwrap_or_default() {
        if let Some(remote_id) = &local.remote_id
            && !seen_remote_ids.contains(remote_id)
        {
            let _ = queries::delete_playlist(&db.conn, local.id);
        }
    }

    for local in
        queries::playlists_without_remote(&db.conn, queries::LOCAL_USER).unwrap_or_default()
    {
        if push(db, client, &account, local.id).is_ok() {
            out.pushed += 1;
        }
    }

    out
}

/// What a playlist's `remote_id` is relative to: one account on one server.
fn account_key(url: &str, username: &str) -> String {
    format!("{username}@{}", url.trim_end_matches('/'))
}

/// One lock per playlist, held for the length of a push.
///
/// Pushes are fired from every edit, and two in flight at once would race: both
/// could see no server id and create two playlists there, or land out of
/// order and leave the server holding the older contents. Held, each push
/// reads the row only once the one before it has written its answer back.
fn push_lock(id: i64) -> std::sync::Arc<parking_lot::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<i64, std::sync::Arc<parking_lot::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .entry(id)
        .or_default()
        .clone()
}

/// Send a playlist's name and contents to the server, in order.
///
/// `createPlaylist` with a `playlistId` replaces the contents wholesale, which
/// is the only Subsonic call that can express a reorder — so a push is always
/// the whole list rather than a diff.
fn push(db: &Database, client: &SubsonicClient, account: &str, id: i64) -> Result<(), ()> {
    let lock = push_lock(id);
    let _held = lock.lock();
    // A server id from another account names someone else's playlist here.
    if queries::detach_playlists_from_other_accounts(&db.conn, account).is_err() {
        return Err(());
    }

    let Ok(Some(local)) = queries::get_playlist(&db.conn, id) else {
        return Err(());
    };
    let remote_id = local.remote_id.as_deref();
    let song_ids = queries::remote_ids_for_playlist(&db.conn, id).unwrap_or_default();

    // A playlist made entirely of local files has nothing the server could
    // point at. Creating an empty one there would be worse than not creating it.
    if remote_id.is_none() && song_ids.is_empty() {
        return Err(());
    }

    // The name has to travel on its own. Navidrome's `createPlaylist` with a
    // `playlistId` replaces the songs and ignores the `name` it is handed, so a
    // rename pushed that way changes nothing. `updatePlaylist` is the call that
    // carries metadata; `createPlaylist` is the one that carries order. A push
    // needs both.
    if let Some(remote_id) = remote_id
        && let Err(e) = client.update_playlist(
            remote_id,
            Some(&local.name),
            local.comment.as_deref(),
            Some(local.public),
        )
    {
        log::warn!(
            "could not rename playlist '{}' on the server: {e}",
            local.name
        );
    }

    match client.create_playlist(remote_id, &local.name, &song_ids) {
        Ok(created) => {
            let new_id = created
                .as_ref()
                .map(|c| c.playlist.id.clone())
                .or_else(|| remote_id.map(str::to_string));
            if let Some(new_id) = new_id {
                let changed = created.as_ref().and_then(|c| c.playlist.changed.clone());
                let owner = created.as_ref().and_then(|c| c.playlist.owner.clone());
                let _ = queries::set_playlist_remote(
                    &db.conn,
                    id,
                    &new_id,
                    owner.as_deref(),
                    local.public,
                    account,
                );
                let _ = queries::mark_playlist_synced(
                    &db.conn,
                    id,
                    changed.as_deref(),
                    Some(local.revision),
                );
            }
            Ok(())
        }
        Err(e) => {
            log::warn!(
                "could not push playlist '{}' to the server: {e}",
                local.name
            );
            Err(())
        }
    }
}

/// Whether `local` was changed after the server's copy was.
///
/// Only consulted when both sides changed since the last sync. Both are
/// ISO 8601 in UTC — SQLite's `datetime('now')` on our side, the server's own
/// stamp on theirs — near enough that comparing the digits works, once
/// SQLite's space is made a `T`. A server that sends no timestamp at all
/// cannot be shown to be newer, so ours wins and the push settles it.
fn newer(local: &str, remote: Option<&str>) -> bool {
    let Some(remote) = remote else { return true };
    let normalise = |s: &str| s.replace(' ', "T").trim_end_matches('Z').to_string();
    normalise(local) > normalise(remote)
}

/// Push a playlist to the server in the background, if there is one.
///
/// Fire and forget on its own thread, the way favourites are: the local copy is
/// already written, and a slow server should not hold up the edit that caused
/// this. A failure leaves the local copy unsynced, which is exactly what
/// [`reconcile_playlists`] resolves on the next sync.
///
/// The thread takes its own connection rather than borrowing the caller's:
/// a `rusqlite::Connection` is not `Sync`, and the answer has to be written
/// back — the new server id — so it needs one of its own.
pub fn push_to_remote(id: i64) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Some(client) = subsonic_client(&cfg) else {
        return;
    };
    let account = account_key(&cfg.remote.url, &cfg.remote.username);
    std::thread::Builder::new()
        .name("koan-playlist-sync".into())
        .spawn(move || {
            let Ok(db) = crate::db::pool::shared().get() else {
                return;
            };
            let Ok(Some(list)) = queries::get_playlist(&db.conn, id) else {
                return;
            };
            // The upstream server has one account, and it is the local user's.
            if !matches!(
                queries::auth::is_local_user(&db.conn, list.user_id),
                Ok(true)
            ) {
                return;
            }
            let _ = push(&db, &client, &account, id);
        })
        .ok();
}

/// Delete a playlist on the server. Nothing to do for one that never went.
pub fn delete_on_remote(remote_id: String) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Some(client) = subsonic_client(&cfg) else {
        return;
    };
    std::thread::Builder::new()
        .name("koan-playlist-sync".into())
        .spawn(move || {
            if let Err(e) = client.delete_playlist(&remote_id) {
                log::warn!("could not delete playlist {remote_id} on the server: {e}");
            }
        })
        .ok();
}

/// What an export wrote, and what it could not.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExportSummary {
    pub written: usize,
    /// Tracks with no file on this machine. A playlist file is a list of
    /// paths, and a remote track that has never been downloaded has none.
    pub skipped: usize,
}

/// Write a playlist as an extended M3U8.
///
/// Absolute paths, UTF-8, `#EXTINF` per entry — the format every player still
/// reads. Remote tracks that have not been downloaded are left out rather than
/// written as stream URLs: a Subsonic stream URL carries the credentials that
/// authorise it, and a playlist file is something people mail to each other.
pub fn export_m3u8(
    db: &Database,
    playlist_id: i64,
    dest: &Path,
) -> Result<ExportSummary, std::io::Error> {
    let name = queries::get_playlist(&db.conn, playlist_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();
    let tracks = queries::playlist_tracks(&db.conn, playlist_id).unwrap_or_default();

    let mut out = ExportSummary::default();
    let mut file = std::fs::File::create(dest)?;
    writeln!(file, "#EXTM3U")?;
    if !name.is_empty() {
        writeln!(file, "#PLAYLIST:{name}")?;
    }

    for track in &tracks {
        let path = track
            .path
            .as_deref()
            .or(track.cached_path.as_deref())
            .map(PathBuf::from)
            .filter(|p| p.exists());
        let Some(path) = path else {
            out.skipped += 1;
            continue;
        };
        let seconds = track.duration_ms.unwrap_or(0) / 1000;
        writeln!(
            file,
            "#EXTINF:{seconds},{} - {}",
            track.artist_name, track.title
        )?;
        writeln!(file, "{}", path.display())?;
        out.written += 1;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{TrackMeta, upsert_track};

    fn meta(title: &str, path: &Path) -> TrackMeta {
        TrackMeta {
            title: title.into(),
            artist: "Artist".into(),
            album_artist: Some("Artist".into()),
            album: "Album".into(),
            date: None,
            disc: None,
            track_number: None,
            genre: None,
            label: None,
            duration_ms: Some(185_000),
            codec: Some("FLAC".into()),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            size_bytes: None,
            mtime: None,
            path: Some(path.to_string_lossy().into_owned()),
            source: "local".into(),
            remote_id: None,
            remote_url: None,
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_mbid: None,
            album_added_at: None,
        }
    }

    /// The queue is locked while it is still exactly the playlist, and stops
    /// being the moment it is not. Everything else about following follows from
    /// this one answer.
    #[test]
    fn a_queue_is_locked_only_while_it_is_still_the_playlist() {
        use crate::player::state::{ItemState, PlaylistItem, QueueItemId, SharedPlayerState};

        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let a = upsert_track(&db.conn, &meta("A", &dir.path().join("a.flac"))).unwrap();
        let b = upsert_track(&db.conn, &meta("B", &dir.path().join("b.flac"))).unwrap();

        let id =
            queries::create_playlist(&db.conn, crate::db::queries::LOCAL_USER, "Evening", None)
                .unwrap();
        let entries = queries::add_tracks(&db.conn, id, &[a, b]).unwrap();

        let state = SharedPlayerState::new();
        let queued = |entry: Option<i64>| PlaylistItem {
            id: QueueItemId::new(),
            db_id: Some(a),
            playlist_entry_id: entry,
            path: dir.path().join("a.flac"),
            title: "A".into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: ItemState::Ready,
            pre_shuffle: None,
        };

        assert_eq!(
            queue_lock(&db, &state),
            None,
            "an empty queue is not locked"
        );

        state.add_items(vec![queued(Some(entries[0])), queued(Some(entries[1]))]);
        assert_eq!(
            queue_lock(&db, &state),
            Some(QueueLock::Playlist(id)),
            "the queue is the playlist"
        );

        // Something that never came from the playlist — played next, dropped
        // in.
        state.add_items(vec![queued(None)]);
        assert_eq!(queue_lock(&db, &state), None);
    }

    /// Reordering the queue by hand ends the lock, which is the whole point of
    /// deriving it: there is no flag anyone has to remember to clear.
    #[test]
    fn rearranging_the_queue_ends_the_lock() {
        use crate::player::state::{ItemState, PlaylistItem, QueueItemId, SharedPlayerState};

        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let a = upsert_track(&db.conn, &meta("A", &dir.path().join("a.flac"))).unwrap();
        let b = upsert_track(&db.conn, &meta("B", &dir.path().join("b.flac"))).unwrap();
        let id =
            queries::create_playlist(&db.conn, crate::db::queries::LOCAL_USER, "Evening", None)
                .unwrap();
        let entries = queries::add_tracks(&db.conn, id, &[a, b]).unwrap();

        let state = SharedPlayerState::new();
        let items: Vec<PlaylistItem> = entries
            .iter()
            .map(|entry| PlaylistItem {
                id: QueueItemId::new(),
                db_id: Some(a),
                playlist_entry_id: Some(*entry),
                path: dir.path().join("a.flac"),
                title: "A".into(),
                artist: "Artist".into(),
                album_artist: "Artist".into(),
                album: "Album".into(),
                year: None,
                codec: None,
                track_number: None,
                disc: None,
                duration_ms: None,
                state: ItemState::Ready,
                pre_shuffle: None,
            })
            .collect();
        let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
        state.add_items(items);
        assert_eq!(queue_lock(&db, &state), Some(QueueLock::Playlist(id)));

        state.reorder_to(&[ids[1], ids[0]]);
        assert_eq!(
            queue_lock(&db, &state),
            None,
            "same tracks, different order — no longer the playlist"
        );

        // And the playlist catching up locks it again. Nothing had to be reset.
        queries::reorder_entries(&db.conn, id, &[entries[1], entries[0]]).unwrap();
        assert_eq!(queue_lock(&db, &state), Some(QueueLock::Playlist(id)));
    }

    /// A record needs no provenance: it *is* an ordered set of tracks, so the
    /// queue being that record is a question about what the queue holds. Which
    /// is why it survives a relaunch, where nothing remembers what was played.
    #[test]
    fn a_queue_holding_exactly_one_record_is_locked_to_it() {
        use crate::player::state::{ItemState, PlaylistItem, QueueItemId, SharedPlayerState};

        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let a = upsert_track(&db.conn, &meta("A", &dir.path().join("a.flac"))).unwrap();
        let b = upsert_track(&db.conn, &meta("B", &dir.path().join("b.flac"))).unwrap();
        let album_id = queries::get_track_row(&db.conn, a)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();

        let state = SharedPlayerState::new();
        let queued = |track: i64| PlaylistItem {
            id: QueueItemId::new(),
            db_id: Some(track),
            playlist_entry_id: None,
            path: dir.path().join("a.flac"),
            title: "A".into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: ItemState::Ready,
            pre_shuffle: None,
        };

        state.add_items(vec![queued(a)]);
        assert_eq!(
            queue_lock(&db, &state),
            None,
            "half a record is not the record"
        );

        state.add_items(vec![queued(b)]);
        assert_eq!(queue_lock(&db, &state), Some(QueueLock::Album(album_id)));
    }

    #[test]
    fn export_writes_what_is_on_disk_and_counts_what_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();

        let present = dir.path().join("here.flac");
        std::fs::write(&present, b"x").unwrap();
        let here = upsert_track(&db.conn, &meta("Here", &present)).unwrap();
        let gone = upsert_track(&db.conn, &meta("Gone", &dir.path().join("gone.flac"))).unwrap();

        let id =
            queries::create_playlist(&db.conn, crate::db::queries::LOCAL_USER, "Evening", None)
                .unwrap();
        queries::add_tracks(&db.conn, id, &[here, gone]).unwrap();

        let dest = dir.path().join("evening.m3u8");
        let summary = export_m3u8(&db, id, &dest).unwrap();
        assert_eq!((summary.written, summary.skipped), (1, 1));

        let written = std::fs::read_to_string(&dest).unwrap();
        assert!(written.starts_with("#EXTM3U\n#PLAYLIST:Evening\n"));
        assert!(written.contains("#EXTINF:185,Artist - Here"));
        assert!(written.contains(&present.display().to_string()));
        assert!(!written.contains("gone.flac"));
    }

    /// A Subsonic server holding playlists and nothing else, one request per
    /// connection. Each `changed` is a counter, so every write moves it.
    #[derive(Default)]
    struct Server {
        lists: Vec<(String, Vec<String>, u32)>,
        fetches: usize,
        creates_without_id: usize,
    }

    fn serve(server: std::sync::Arc<parking_lot::Mutex<Server>>) -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let server = server.clone();
                std::thread::spawn(move || {
                    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    reader.read_line(&mut request).unwrap();
                    let mut line = String::new();
                    while reader.read_line(&mut line).unwrap_or(0) > 2 {
                        line.clear();
                    }
                    let target = request.split_whitespace().nth(1).unwrap_or("");
                    let (path, query) = target.split_once('?').unwrap_or((target, ""));
                    let params: Vec<(&str, &str)> = query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .collect();
                    let param = |k: &str| params.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
                    let body = respond(&server, path.rsplit('/').next().unwrap(), &params, param);
                    let mut stream = stream;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        url
    }

    fn respond<'a>(
        server: &parking_lot::Mutex<Server>,
        endpoint: &str,
        params: &[(&str, &'a str)],
        param: impl Fn(&str) -> Option<&'a str>,
    ) -> String {
        let summary = |(id, songs, changed): &(String, Vec<String>, u32)| {
            format!(
                r#""id":"{id}","name":"{id}","owner":"u","songCount":{},"changed":"{changed}""#,
                songs.len()
            )
        };
        let full = |list: &(String, Vec<String>, u32)| {
            let entries: Vec<String> = list
                .1
                .iter()
                .map(|s| format!(r#"{{"id":"{s}","title":"{s}"}}"#))
                .collect();
            format!(r#"{{{},"entry":[{}]}}"#, summary(list), entries.join(","))
        };
        let ok = |inner: String| format!(r#"{{"subsonic-response":{{"status":"ok"{inner}}}}}"#);
        match endpoint {
            "getPlaylists" => {
                let lists: Vec<String> = server
                    .lock()
                    .lists
                    .iter()
                    .map(|l| format!("{{{}}}", summary(l)))
                    .collect();
                ok(format!(
                    r#","playlists":{{"playlist":[{}]}}"#,
                    lists.join(",")
                ))
            }
            "getPlaylist" => {
                let mut server = server.lock();
                server.fetches += 1;
                let list = server
                    .lists
                    .iter()
                    .find(|l| Some(l.0.as_str()) == param("id"));
                ok(format!(r#","playlist":{}"#, full(list.unwrap())))
            }
            "createPlaylist" => {
                let songs: Vec<String> = params
                    .iter()
                    .filter(|(k, _)| *k == "songId")
                    .map(|(_, v)| v.to_string())
                    .collect();
                // Wide enough for two pushes to overlap if nothing stops them.
                std::thread::sleep(std::time::Duration::from_millis(50));
                let mut server = server.lock();
                let id = match param("playlistId") {
                    Some(id) => id.to_string(),
                    None => {
                        server.creates_without_id += 1;
                        format!("p{}", server.lists.len() + 1)
                    }
                };
                server.lists.retain(|l| l.0 != id);
                let changed = server.lists.iter().map(|l| l.2).max().unwrap_or(0) + 100;
                server.lists.push((id, songs, changed));
                ok(format!(
                    r#","playlist":{}"#,
                    full(server.lists.last().unwrap())
                ))
            }
            _ => ok(String::new()),
        }
    }

    fn remote_meta(title: &str, remote_id: &str) -> TrackMeta {
        TrackMeta {
            path: None,
            source: "remote".into(),
            remote_id: Some(remote_id.into()),
            album: title.into(),
            ..meta(title, Path::new(""))
        }
    }

    fn entries(db: &Database, id: i64) -> Vec<(i64, i64)> {
        queries::playlist_entries(&db.conn, id)
            .unwrap()
            .into_iter()
            .map(|e| (e.id, e.track.id))
            .collect()
    }

    #[test]
    fn a_sync_keeps_local_only_entries_and_fetches_nothing_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let r1 = upsert_track(&db.conn, &remote_meta("One", "s1")).unwrap();
        let r2 = upsert_track(&db.conn, &remote_meta("Two", "s2")).unwrap();
        let local = upsert_track(&db.conn, &meta("Here", &dir.path().join("l.flac"))).unwrap();

        let server = std::sync::Arc::new(parking_lot::Mutex::new(Server {
            lists: vec![("p1".into(), vec!["s1".into(), "s2".into()], 1)],
            ..Default::default()
        }));
        let url = serve(server.clone());
        let client = SubsonicClient::new(&url, "u", "pw");

        let first = reconcile_playlists(&db, &client, &url, "u");
        assert_eq!(first.pulled, 1);
        let id = queries::playlist_by_remote_id(&db.conn, "p1")
            .unwrap()
            .unwrap()
            .id;
        assert_eq!(
            entries(&db, id).iter().map(|e| e.1).collect::<Vec<_>>(),
            [r1, r2]
        );

        // A local file joins; the push carries only what the server can name.
        queries::add_tracks(&db.conn, id, &[local]).unwrap();
        let second = reconcile_playlists(&db, &client, &url, "u");
        assert_eq!((second.pushed, second.pulled), (1, 0));
        assert_eq!(server.lock().lists[0].1, ["s1", "s2"]);

        let before = entries(&db, id);
        assert_eq!(before.len(), 3);
        let fetches = server.lock().fetches;
        let third = reconcile_playlists(&db, &client, &url, "u");
        assert_eq!((third.pushed, third.pulled), (0, 0));
        assert_eq!(
            server.lock().fetches,
            fetches,
            "nothing moved, nothing fetched"
        );
        assert_eq!(entries(&db, id), before);

        // An edit on the server comes down without the local file or the
        // surviving entry's id.
        {
            let mut server = server.lock();
            server.lists[0].1 = vec!["s2".into()];
            server.lists[0].2 += 1;
        }
        let fourth = reconcile_playlists(&db, &client, &url, "u");
        assert_eq!(fourth.pulled, 1);
        assert_eq!(entries(&db, id), [before[1], before[2]]);
    }

    #[test]
    fn another_account_keeps_the_playlists_and_pushes_them_as_new() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let r1 = upsert_track(&db.conn, &remote_meta("One", "s1")).unwrap();
        let id = queries::create_playlist(&db.conn, queries::LOCAL_USER, "Road", None).unwrap();
        queries::add_tracks(&db.conn, id, &[r1]).unwrap();
        queries::set_playlist_remote(&db.conn, id, "old-1", Some("u"), false, "u@http://old")
            .unwrap();

        let server = std::sync::Arc::new(parking_lot::Mutex::new(Server::default()));
        let url = serve(server.clone());
        let client = SubsonicClient::new(&url, "u", "pw");
        let sync = reconcile_playlists(&db, &client, &url, "u");

        let list = queries::get_playlist(&db.conn, id).unwrap().expect("kept");
        assert_eq!(list.track_count, 1);
        assert_eq!(sync.pushed, 1);
        assert_eq!(list.remote_id.as_deref(), Some("p1"));
    }

    #[test]
    fn concurrent_pushes_create_one_server_playlist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = Database::open(&path).unwrap();
        let r1 = upsert_track(&db.conn, &remote_meta("One", "s1")).unwrap();
        let id = queries::create_playlist(&db.conn, queries::LOCAL_USER, "Road", None).unwrap();
        queries::add_tracks(&db.conn, id, &[r1]).unwrap();

        let server = std::sync::Arc::new(parking_lot::Mutex::new(Server::default()));
        let url = serve(server.clone());
        let pushes: Vec<_> = (0..2)
            .map(|_| {
                let (path, url) = (path.clone(), url.clone());
                std::thread::spawn(move || {
                    let db = Database::open(&path).unwrap();
                    let client = SubsonicClient::new(&url, "u", "pw");
                    push(&db, &client, &account_key(&url, "u"), id)
                })
            })
            .collect();
        for p in pushes {
            p.join().unwrap().unwrap();
        }
        assert_eq!(server.lock().creates_without_id, 1);
        assert_eq!(server.lock().lists.len(), 1);
    }
}

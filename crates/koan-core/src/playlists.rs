//! Playlists beyond the database: keeping them in step with the server, and
//! writing them out as files.
//!
//! The database module owns what a playlist *is*. This owns what happens to it
//! next — which is either a Subsonic call or an M3U8 on disk.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::helpers::subsonic_client;
use crate::player::state::SharedPlayerState;
use crate::remote::client::SubsonicClient;

/// The queue, and the playlist or record it is still exactly.
///
/// While the two match, the queue *follows* a playlist: an edit there is an
/// edit here, quietly. The moment you rearrange the queue yourself, add to it,
/// or let radio extend it, they stop matching and the playlist becomes a
/// document you are editing rather than the thing you are listening to.
///
/// A record cannot be edited, so locking to one buys no following — only the
/// ability to say what you are listening to, which is worth saying.
///
/// Derived rather than tracked, which is the whole reason it is simple. There
/// is no flag to keep in sync, nothing to persist and nothing to migrate — and
/// it cannot get stuck, because a queue that stops matching stops being locked
/// and one that happens to match again is locked again. Playing a playlist
/// shuffled scrambles the order on purpose, so that queue is not locked, which
/// is the right answer rather than a special case.
pub fn queue_lock(db: &Database, state: &SharedPlayerState) -> Option<QueueLock> {
    let (items, _) = state.snapshot_playlist();
    if items.is_empty() {
        return None;
    }

    // Every item has to have come from the same playlist. One that did not —
    // played next, dropped in, found by radio — is the queue having diverged.
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
/// Unlike favourites, a playlist has an order and a server that records when it
/// last changed — so this is last-writer-wins on `changed`, not a union. Local
/// edits push the moment they happen, so a local copy ahead of the server's
/// means a push that never got out (koan was offline, the server was down), and
/// that is exactly the case where ours should win.
///
/// Playlists that have never been to the server are created there. Ones the
/// server no longer has are dropped locally: deleting a playlist on Navidrome
/// and having it reappear on the next sync would make deletion impossible.
pub fn reconcile_playlists(db: &Database, client: &SubsonicClient, username: &str) -> PlaylistSync {
    let mut out = PlaylistSync::default();

    let remote = match client.get_playlists() {
        Ok(lists) => lists,
        Err(e) => {
            log::warn!("could not fetch playlists from the server: {e}");
            return out;
        }
    };

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

        if let Some(local) = &local
            && ours
            && newer(&local.changed_at, summary.changed.as_deref())
        {
            if push(db, client, local.id, Some(&summary.id)).is_ok() {
                out.pushed += 1;
            }
            continue;
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
                match queries::create_playlist(&db.conn, &summary.name, summary.comment.as_deref())
                {
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
            summary.changed.as_deref(),
        );

        let remote_song_ids: Vec<String> = full.entry.iter().map(|s| s.id.clone()).collect();
        let track_ids: Vec<i64> = queries::track_ids_for_remote_ids(&db.conn, &remote_song_ids)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        if let Err(e) = queries::set_playlist_tracks(&db.conn, id, &track_ids) {
            log::warn!(
                "could not store playlist contents for {}: {e}",
                summary.name
            );
            continue;
        }
        // `set_playlist_tracks` stamps the local copy as changed, which would
        // make the next sync think we were ahead of the server. We are not:
        // this *is* the server's copy.
        let _ = queries::set_playlist_remote(
            &db.conn,
            id,
            &summary.id,
            summary.owner.as_deref(),
            summary.public,
            summary.changed.as_deref(),
        );
        out.pulled += 1;
    }

    // A playlist we hold a server id for that the server no longer lists was
    // deleted there.
    for local in queries::list_playlists(&db.conn).unwrap_or_default() {
        if let Some(remote_id) = &local.remote_id
            && !seen_remote_ids.contains(remote_id)
        {
            let _ = queries::delete_playlist(&db.conn, local.id);
        }
    }

    for local in queries::playlists_without_remote(&db.conn).unwrap_or_default() {
        if push(db, client, local.id, None).is_ok() {
            out.pushed += 1;
        }
    }

    out
}

/// Send a playlist's name and contents to the server, in order.
///
/// `createPlaylist` with a `playlistId` replaces the contents wholesale, which
/// is the only Subsonic call that can express a reorder — so a push is always
/// the whole list rather than a diff.
fn push(
    db: &Database,
    client: &SubsonicClient,
    id: i64,
    remote_id: Option<&str>,
) -> Result<(), ()> {
    let Ok(Some(local)) = queries::get_playlist(&db.conn, id) else {
        return Err(());
    };
    let song_ids = queries::remote_ids_for_playlist(&db.conn, id).unwrap_or_default();

    // A playlist made entirely of local files has nothing the server could
    // point at. Creating an empty one there would be worse than not creating it.
    if remote_id.is_none() && song_ids.is_empty() {
        return Err(());
    }

    // The name has to travel on its own. Navidrome's `createPlaylist` with a
    // `playlistId` replaces the songs and ignores the `name` it is handed, so a
    // rename pushed that way reached the server and changed nothing — which is
    // exactly what it looked like from the outside. `updatePlaylist` is the
    // call that carries metadata; `createPlaylist` is the one that carries
    // order. A push needs both.
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
                    changed.as_deref(),
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
/// Both are ISO 8601 in UTC — SQLite's `datetime('now')` on our side, the
/// server's own stamp on theirs — near enough that comparing the digits works,
/// once SQLite's space is made a `T`. A server that sends no timestamp at all
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
/// this. A failure leaves the local copy newer than the server's, which is
/// exactly what [`reconcile_playlists`] resolves on the next sync.
///
/// The thread opens its own database handle rather than borrowing the caller's:
/// a `rusqlite::Connection` is neither `Send` nor `Sync`, and the answer has to
/// be written back — the new server id — so it needs one of its own.
pub fn push_to_remote(id: i64) {
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
            let Ok(db) = Database::open_default() else {
                return;
            };
            let remote_id = queries::get_playlist(&db.conn, id)
                .ok()
                .flatten()
                .and_then(|p| p.remote_id);
            let _ = push(&db, &client, id, remote_id.as_deref());
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
            album_added_at: None,
        }
    }

    /// The queue is locked while it is still exactly the playlist, and stops
    /// being the moment it is not. Everything else about following follows from
    /// this one answer.
    #[test]
    fn a_queue_is_locked_only_while_it_is_still_the_playlist() {
        use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};

        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let a = upsert_track(&db.conn, &meta("A", &dir.path().join("a.flac"))).unwrap();
        let b = upsert_track(&db.conn, &meta("B", &dir.path().join("b.flac"))).unwrap();

        let id = queries::create_playlist(&db.conn, "Evening", None).unwrap();
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
            load_state: LoadState::Ready,
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
        // in, found by radio.
        state.add_items(vec![queued(None)]);
        assert_eq!(queue_lock(&db, &state), None);
    }

    /// Reordering the queue by hand ends the lock, which is the whole point of
    /// deriving it: there is no flag anyone has to remember to clear.
    #[test]
    fn rearranging_the_queue_ends_the_lock() {
        use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};

        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let a = upsert_track(&db.conn, &meta("A", &dir.path().join("a.flac"))).unwrap();
        let b = upsert_track(&db.conn, &meta("B", &dir.path().join("b.flac"))).unwrap();
        let id = queries::create_playlist(&db.conn, "Evening", None).unwrap();
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
                load_state: LoadState::Ready,
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
        use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};

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
            load_state: LoadState::Ready,
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

        let id = queries::create_playlist(&db.conn, "Evening", None).unwrap();
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
}

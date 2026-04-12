//! Enqueue helpers for the TUI.
//!
//! Builds PlaylistItems from track IDs and enqueues them according to the
//! requested action (append, append+play, replace queue).

use koan_core::config;
use koan_core::db::queries;
use koan_core::helpers::{playlist_item_from_track, resolve_item_path};
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{LoadState, QueueItemId};

use crate::app::PickerAction;
use crate::download_queue::DownloadQueue;

/// Build PlaylistItems from track IDs and enqueue according to the action:
/// - Append: add to end of queue, don't play.
/// - AppendAndPlay: add to end, play the first added track.
/// - ReplaceQueue: clear queue, add tracks, play from top.
///
/// Pending remote tracks are submitted to the persistent `DownloadQueue`.
pub fn enqueue_playlist(
    ids: Vec<i64>,
    action: PickerAction,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    download_queue: DownloadQueue,
) {
    let db = match koan_core::db::connection::Database::open_default() {
        Ok(db) => db,
        Err(e) => {
            log::error!("db error: {}", e);
            return;
        }
    };
    let cfg = config::Config::load().unwrap_or_default();

    let mut items: Vec<koan_core::player::state::PlaylistItem> = Vec::new();
    let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();

    for &id in &ids {
        let Some(track) = queries::get_track_row(&db.conn, id).ok().flatten() else {
            continue;
        };
        let album_date: Option<String> = track
            .album_id
            .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

        let (dest, load_state) = resolve_item_path(&db, &cfg, id, &track, album_date.as_deref());

        let item = playlist_item_from_track(&track, album_date.as_deref(), dest, load_state);
        if matches!(item.load_state, LoadState::Pending) {
            pending_downloads.push((id, item.id));
        }
        items.push(item);
    }

    if items.is_empty() {
        return;
    }

    let first_id = items[0].id;

    if action == PickerAction::ReplaceQueue && tx.send(PlayerCommand::ClearPlaylist).is_err() {
        return;
    }

    if tx.send(PlayerCommand::AddToPlaylist(items)).is_err() {
        return;
    }

    if matches!(
        action,
        PickerAction::AppendAndPlay | PickerAction::ReplaceQueue
    ) && tx.send(PlayerCommand::Play(first_id)).is_err()
    {
        return;
    }

    download_queue.enqueue(pending_downloads);
}

//! Enqueue helpers for the TUI.
//!
//! Builds PlaylistItems from track IDs and enqueues them according to the
//! requested action (append, append+play, replace queue).

use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueItemId, QueueMode};

use crate::app::PickerAction;

/// Build PlaylistItems from track IDs and enqueue according to the action:
/// - Append: add to end of queue, don't play.
/// - AppendAndPlay: add to end, play the first added track.
/// - ReplaceQueue: replace the queue in one command, play from top.
/// - PlayNext: insert straight after `playing`, the item at the cursor; with
///   none, append.
///
/// Remote tracks download once the player has them: the download queue
/// follows the playlist.
pub fn enqueue_playlist(
    ids: Vec<i64>,
    action: PickerAction,
    playing: Option<QueueItemId>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
) {
    let db = match koan_core::db::pool::shared().get() {
        Ok(db) => db,
        Err(e) => {
            log::error!("db error: {}", e);
            return;
        }
    };
    let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap_or_default();
    let items = koan_core::helpers::playlist_items_for_tracks(&db, &rows);

    if items.is_empty() {
        return;
    }

    // One command, so the playlist is never empty in between: the download
    // queue reads an empty playlist as nothing wanted.
    if action == PickerAction::ReplaceQueue {
        tx.send(PlayerCommand::ReplacePlaylist {
            items,
            start: 0,
            position_ms: 0,
            play: true,
            mode: QueueMode::InOrder,
        })
        .ok();
        return;
    }

    if action == PickerAction::PlayNext
        && let Some(after) = playing
    {
        tx.send(PlayerCommand::InsertInPlaylist { items, after })
            .ok();
        return;
    }

    // One command, so a controlled device is told to play what it appended
    // rather than an entry only this one knows.
    if action == PickerAction::AppendAndPlay {
        tx.send(PlayerCommand::AddAndPlay(items)).ok();
    } else {
        tx.send(PlayerCommand::AddToPlaylist(items)).ok();
    }
}

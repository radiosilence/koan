//! Enqueue helpers for the TUI.
//!
//! Builds PlaylistItems from track IDs and enqueues them according to the
//! requested action (append, append+play, replace queue).

use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;

use crate::app::PickerAction;

/// Build PlaylistItems from track IDs and enqueue according to the action:
/// - Append: add to end of queue, don't play.
/// - AppendAndPlay: add to end, play the first added track.
/// - ReplaceQueue: clear queue, add tracks, play from top.
///
/// Remote tracks download once the player has them: the download queue
/// follows the playlist.
pub fn enqueue_playlist(
    ids: Vec<i64>,
    action: PickerAction,
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
    ) {
        tx.send(PlayerCommand::Play(first_id)).ok();
    }
}

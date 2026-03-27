use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{LoadState, QueueItemId, SharedPlayerState};
use owo_colors::OwoColorize;

use super::download_queue::DownloadQueue;
use super::{cache_path_for_track, open_db, playlist_item_from_track};
use crate::tui::app::PickerAction;

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
    let db = open_db();
    let cfg = config::Config::load().unwrap_or_default();

    // Build all PlaylistItems from DB (fast, no downloads).
    let mut items: Vec<koan_core::player::state::PlaylistItem> = Vec::new();
    let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();

    for &id in &ids {
        let Some(track) = queries::get_track_row(&db.conn, id).ok().flatten() else {
            continue;
        };
        let album_date: Option<String> = track
            .album_id
            .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

        // Resolve the path — local, cached, or needs download.
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

    // Apply the requested action.
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

    // Submit pending downloads to the persistent queue.
    // Workers + cursor watcher handle prioritization automatically.
    download_queue.enqueue(pending_downloads);
}

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, LoadState::Ready) for local/cached, (cache_path, LoadState::Pending) for remote.
pub(crate) fn resolve_item_path(
    db: &Database,
    cfg: &config::Config,
    id: i64,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> (PathBuf, LoadState) {
    match queries::resolve_playback_path(&db.conn, id) {
        Ok(Some(queries::PlaybackSource::Local(p))) => (p, LoadState::Ready),
        Ok(Some(queries::PlaybackSource::Cached(p))) => (p, LoadState::Ready),
        Ok(Some(queries::PlaybackSource::Remote(_))) => {
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            if dest.exists() {
                (dest, LoadState::Ready)
            } else {
                (dest, LoadState::Pending)
            }
        }
        _ => {
            // Fallback: construct a cache path and mark pending.
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            (dest, LoadState::Pending)
        }
    }
}

/// Download a single track and update playlist state.
pub(crate) fn download_track(
    db_id: i64,
    queue_id: QueueItemId,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    log_buf: &Arc<Mutex<Vec<String>>>,
    state: &Arc<SharedPlayerState>,
    cfg: &config::Config,
) {
    let db = open_db();
    let track = match queries::get_track_row(&db.conn, db_id) {
        Ok(Some(t)) => t,
        _ => {
            state.update_load_state(queue_id, LoadState::Failed("track not found".into()));
            return;
        }
    };

    let remote_id = match &track.remote_id {
        Some(rid) => rid.clone(),
        None => {
            state.update_load_state(queue_id, LoadState::Failed("no remote_id".into()));
            return;
        }
    };

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());

    // Already downloaded (race with another batch).
    if dest.exists() {
        state.update_load_state(queue_id, LoadState::Ready);
        if state.is_cursor(queue_id) {
            tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        }
        return;
    }

    let client = match super::subsonic_client(cfg) {
        Some(c) => c,
        None => {
            log::warn!(
                "remote not configured — skipping download for {}",
                remote_id
            );
            return;
        }
    };

    // Shared counter: download thread writes, StreamingSource reads.
    let bytes_written: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let progress_state = state.clone();
    let progress_qid = queue_id;
    let bytes_written_progress = bytes_written.clone();
    let progress_tx = tx.clone();
    let stream_ready_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stream_ready_flag = stream_ready_sent.clone();
    let result = client.download_with_progress(&remote_id, &dest, move |downloaded, total| {
        bytes_written_progress.store(downloaded, Ordering::Release);
        progress_state.update_load_state(
            progress_qid,
            LoadState::Downloading {
                downloaded,
                total,
                bytes_written: bytes_written_progress.clone(),
            },
        );
        // Signal the player once when enough data is buffered for streaming.
        if !stream_ready_flag.load(Ordering::Relaxed)
            && downloaded >= koan_core::player::state::STREAM_THRESHOLD
        {
            stream_ready_flag.store(true, Ordering::Relaxed);
            progress_tx
                .send(PlayerCommand::TrackStreamReady(progress_qid))
                .ok();
        }
    });

    if let Err(e) = result {
        state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
        let msg = format!("{} {} \u{2014} {}", "x".red().bold(), track.title, e);
        log_buf.lock().unwrap().push(msg);
        return;
    }

    // Download succeeded — mark ready.
    state.update_load_state(queue_id, LoadState::Ready);
    let _ = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy());

    let msg = format!(
        "{} {} \u{2014} {}",
        "+".green(),
        track.title,
        track.artist_name.dimmed(),
    );
    log_buf.lock().unwrap().push(msg);

    // If cursor is waiting on this track, tell the player.
    if state.is_cursor(queue_id) {
        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
    }
}

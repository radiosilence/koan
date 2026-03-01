use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{LoadState, QueueItemId, SharedPlayerState};
use owo_colors::OwoColorize;

use super::{cache_path_for_track, get_remote_password, open_db, playlist_item_from_track};
use crate::tui::app::PickerAction;

/// Build PlaylistItems from track IDs and enqueue according to the action:
/// - Append: add to end of queue, don't play.
/// - AppendAndPlay: add to end, play the first added track.
/// - ReplaceQueue: clear queue, add tracks, play from top.
pub fn enqueue_playlist(
    ids: Vec<i64>,
    action: PickerAction,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buf: Arc<Mutex<Vec<String>>>,
    state: Arc<SharedPlayerState>,
) {
    let db = open_db();
    let cfg = config::Config::load().unwrap_or_default();

    // Phase 1: Build all PlaylistItems from DB (fast, no downloads).
    let mut items: Vec<koan_core::player::state::PlaylistItem> = Vec::new();
    let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();

    for &id in &ids {
        let Some(track) = queries::get_track_row(&db.conn, id).ok().flatten() else {
            continue;
        };
        let album_date: Option<String> = track.album_id.and_then(|aid| {
            db.conn
                .query_row(
                    "SELECT date FROM albums WHERE id = ?1",
                    rusqlite::params![aid],
                    |row| row.get(0),
                )
                .ok()
                .flatten()
        });

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

    // Phase 2: Download pending items.
    //
    // N worker threads drain the shared queue in order. When the cursor changes
    // to a pending track, we yank it (+ the next track) OUT of the queue and
    // spin up extra threads immediately — no waiting for a worker to free up.
    let num_workers = cfg.remote.download_workers.max(1);

    let work_queue: Arc<Mutex<std::collections::VecDeque<(i64, QueueItemId)>>> =
        Arc::new(Mutex::new(pending_downloads.into()));

    std::thread::scope(|s| {
        // Watcher: detect cursor changes -> spawn priority download threads.
        let wq = work_queue.clone();
        let state_ref = &state;
        let tx_ref = &tx;
        let log_ref = &log_buf;
        let cfg_ref = &cfg;
        s.spawn(move || {
            let mut last_cursor: Option<QueueItemId> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(30));

                // Exit when queue is fully drained.
                if wq.lock().unwrap().is_empty() {
                    break;
                }

                let current = state_ref.cursor();
                if current == last_cursor {
                    continue;
                }
                last_cursor = current;

                let Some(cursor_id) = current else {
                    continue;
                };

                // Pull cursor track (and the one after it) from the queue
                // so workers don't also download them.
                let mut priority_items = Vec::new();
                {
                    let mut q = wq.lock().unwrap();
                    if let Some(pos) = q.iter().position(|(_, qid)| *qid == cursor_id) {
                        priority_items.push(q.remove(pos).unwrap());
                        // Also grab the next track (for gapless lookahead).
                        if let Some(next) = q.front().copied() {
                            priority_items.push(q.pop_front().unwrap());
                            let _ = next; // suppress unused warning
                        }
                    }
                }

                // Fire off immediate download threads for priority items.
                for (db_id, queue_id) in priority_items {
                    log::info!("priority: spawning immediate download for {:?}", queue_id);
                    s.spawn(move || {
                        download_single_track(db_id, queue_id, tx_ref, log_ref, state_ref, cfg_ref);
                    });
                }
            }
        });

        // Worker pool: drain the queue in order.
        for _ in 0..num_workers {
            let wq = work_queue.clone();
            let tx_ref = &tx;
            let log_ref = &log_buf;
            let state_ref = &state;
            let cfg_ref = &cfg;
            s.spawn(move || {
                loop {
                    let item = wq.lock().unwrap().pop_front();
                    let Some((db_id, queue_id)) = item else {
                        break;
                    };
                    download_single_track(db_id, queue_id, tx_ref, log_ref, state_ref, cfg_ref);
                }
            });
        }
    });
}

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, LoadState::Ready) for local/cached, (cache_path, LoadState::Pending) for remote.
fn resolve_item_path(
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
fn download_single_track(
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

    let album_date: Option<String> = track.album_id.and_then(|aid| {
        db.conn
            .query_row(
                "SELECT date FROM albums WHERE id = ?1",
                rusqlite::params![aid],
                |row| row.get(0),
            )
            .ok()
            .flatten()
    });

    let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());

    // Already downloaded (race with another batch).
    if dest.exists() {
        state.update_load_state(queue_id, LoadState::Ready);
        if state.is_cursor(queue_id) {
            tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        }
        return;
    }

    let password = get_remote_password(cfg);
    let client = koan_core::remote::client::SubsonicClient::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    );

    let progress_state = state.clone();
    let progress_qid = queue_id;
    let result = client.download_with_progress(&remote_id, &dest, move |downloaded, total| {
        progress_state
            .update_load_state(progress_qid, LoadState::Downloading { downloaded, total });
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

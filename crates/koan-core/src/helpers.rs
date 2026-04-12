//! Shared helpers used by downstream crates (koan-tui, koan-server, koan-cli).
//!
//! These functions provide common functionality for building playlist items,
//! resolving track paths, downloading remote tracks, and building Subsonic clients.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::commands::PlayerCommand;
use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};
use crate::remote::client::SubsonicClient;

// ---------------------------------------------------------------------------
// Subsonic client builder
// ---------------------------------------------------------------------------

/// Get the remote password from config, falling back to Keychain for backwards compat.
pub fn get_remote_password(cfg: &Config) -> Option<String> {
    if !cfg.remote.password.is_empty() {
        return Some(cfg.remote.password.clone());
    }
    // Fallback to Keychain for users who set up before the config change.
    crate::credentials::get_password(&cfg.remote.url).ok()
}

/// Build a `SubsonicClient` from the merged config, returning `None` if remote
/// is disabled or has no URL configured.
pub fn subsonic_client(cfg: &Config) -> Option<SubsonicClient> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    let password = get_remote_password(cfg)?;
    Some(SubsonicClient::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    ))
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
pub fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    // Truncate on a char boundary to stay under 240 bytes.
    if cleaned.len() <= 240 {
        return cleaned;
    }
    let mut end = 240;
    while !cleaned.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    cleaned[..end].trim_end().to_string()
}

/// Build a structured cache path for a track:
///   cache_dir/Album Artist/(Year) Album [Codec]/01. Track Artist - Title.ext
pub fn cache_path_for_track(
    cache_dir: &Path,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> PathBuf {
    let artist_dir = sanitise_filename(&track.artist_name);

    let year = album_date
        .and_then(|d| if d.len() >= 4 { Some(&d[..4]) } else { None })
        .map(|y| format!("({}) ", y))
        .unwrap_or_default();
    let codec = track
        .codec
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default();
    let album_dir = sanitise_filename(&format!("{}{}{}", year, track.album_title, codec));

    let disc_prefix = match track.disc {
        Some(d) if d > 1 => format!("{}-", d),
        _ => String::new(),
    };
    let track_num = track
        .track_number
        .map(|n| format!("{:02}. ", n))
        .unwrap_or_default();

    let ext = track
        .codec
        .as_deref()
        .map(|c| c.to_lowercase())
        .unwrap_or_else(|| "flac".into());

    let filename = sanitise_filename(&format!(
        "{}{}{} - {}",
        disc_prefix, track_num, track.artist_name, track.title
    ));

    cache_dir
        .join(artist_dir)
        .join(album_dir)
        .join(format!("{}.{}", filename, ext))
}

// ---------------------------------------------------------------------------
// Track resolution
// ---------------------------------------------------------------------------

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, LoadState::Ready) for local/cached, (cache_path, LoadState::Pending) for remote.
pub fn resolve_item_path(
    db: &Database,
    cfg: &Config,
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

/// Build a PlaylistItem from a TrackRow + album date + resolved path + load state.
pub fn playlist_item_from_track(
    track: &queries::TrackRow,
    album_date: Option<&str>,
    dest: PathBuf,
    load_state: LoadState,
) -> PlaylistItem {
    let year = album_date.and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });
    PlaylistItem {
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path: dest,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

/// Build a PlaylistItem from a TrackRow, resolving its path automatically.
pub fn track_to_playlist_item(track: &queries::TrackRow, db: &Database) -> PlaylistItem {
    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cfg = Config::load().unwrap_or_default();
    let (path, load_state) = resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());

    let year = album_date.as_deref().and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });

    PlaylistItem {
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Resolve a track to a playable file, downloading from remote if needed.
///
/// Resolution order:
/// 1. Local library path (DB `path` field) -- use directly if file exists
/// 2. Cache path -- use if already downloaded
/// 3. Download from remote to cache -- stream while downloading
pub fn download_track(
    db_id: i64,
    queue_id: QueueItemId,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    log_buf: &Arc<Mutex<Vec<String>>>,
    state: &Arc<SharedPlayerState>,
    cfg: &Config,
) {
    let db = match Database::open_default() {
        Ok(db) => db,
        Err(e) => {
            state.update_load_state(queue_id, LoadState::Failed(format!("db error: {}", e)));
            return;
        }
    };
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
            // No remote_id -- check if the local file exists.
            if let Some(ref path) = track.path {
                let p = std::path::PathBuf::from(path);
                if p.exists() {
                    state.update_paths(&[(queue_id, p)]);
                    state.update_load_state(queue_id, LoadState::Ready);
                    if state.is_cursor(queue_id) {
                        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
                    }
                    return;
                }
            }
            state.update_load_state(queue_id, LoadState::Failed("no remote_id".into()));
            return;
        }
    };

    // 1. Check if the local library file exists.
    if let Some(ref local_path) = track.path {
        let p = std::path::PathBuf::from(local_path);
        if p.exists() {
            log::info!("download_track: local file exists, using {}", p.display());
            state.update_paths(&[(queue_id, p)]);
            state.update_load_state(queue_id, LoadState::Ready);
            if state.is_cursor(queue_id) {
                tx.send(PlayerCommand::TrackReady(queue_id)).ok();
            }
            return;
        }
    }

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());

    // 2. Already cached.
    if dest.exists() {
        state.update_paths(&[(queue_id, dest)]);
        state.update_load_state(queue_id, LoadState::Ready);
        if state.is_cursor(queue_id) {
            tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        }
        return;
    }

    // 3. Download from remote.
    let part_path = dest.with_extension("part");
    state.update_paths(&[(queue_id, part_path)]);

    let client = match subsonic_client(cfg) {
        Some(c) => c,
        None => {
            log::warn!(
                "remote not configured -- skipping download for {}",
                remote_id
            );
            return;
        }
    };

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
        if !stream_ready_flag.load(Ordering::Relaxed)
            && downloaded >= crate::player::state::STREAM_THRESHOLD
        {
            stream_ready_flag.store(true, Ordering::Relaxed);
            progress_tx
                .send(PlayerCommand::TrackStreamReady(progress_qid))
                .ok();
        }
    });

    if let Err(e) = result {
        state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
        let msg = format!("x {} — {}", track.title, e);
        log_buf.lock().unwrap().push(msg);
        return;
    }

    // Download succeeded.
    state.update_paths(&[(queue_id, dest.clone())]);
    state.update_load_state(queue_id, LoadState::Ready);
    let _ = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy());

    let msg = format!("+ {} — {}", track.title, track.artist_name);
    log_buf.lock().unwrap().push(msg);

    if state.is_cursor(queue_id) {
        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
    }
}

/// Spawn background downloads for remote tracks with LoadState::Pending.
pub fn spawn_downloads(
    pending: Vec<(i64, QueueItemId)>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    let log_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    std::thread::Builder::new()
        .name("koan-download".into())
        .spawn(move || {
            let cfg = Config::load().unwrap_or_default();
            for (db_id, queue_id) in pending {
                download_track(db_id, queue_id, &tx, &log_buf, &state, &cfg);
            }
        })
        .ok();
}

//! Downloading a remote track into the cache.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::db::queries;
use crate::player::commands::PlayerCommand;
use crate::player::state::SharedPlayerState;
use crate::remote::client::{SubsonicClient, SubsonicError};
use crate::remote::download::DownloadError;

use super::*;

/// Whether a cached file plausibly holds audio.
///
/// A stored Subsonic error is a few hundred bytes of JSON or XML; no real
/// encoded track comes close to that, so the size check alone settles almost
/// every case and the leading byte covers the rest.
pub(super) fn is_cached_audio(path: &std::path::Path) -> bool {
    const MIN_PLAUSIBLE_BYTES: u64 = 4096;
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() >= MIN_PLAUSIBLE_BYTES => true,
        Ok(_) => {
            let mut first = [0u8; 1];
            match std::fs::File::open(path)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut first).map(|_| first[0]))
            {
                Ok(b) => b != b'{' && b != b'<',
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// Resolve a track to a playable file, downloading from remote if needed.
///
/// Resolution order:
/// 1. Local library path (DB `path` field) -- use directly if file exists
/// 2. Cache path -- use if already downloaded
/// 3. Download from remote to cache -- stream while downloading
///
/// Runs the transfer the download queue claimed for this track in the
/// player's store. `cancelled` says when nothing wants it any more; `None` is
/// returned then. Says nothing to the queue entries waiting on it: the caller
/// settles them, all at once, with [`crate::remote::downloads::settle`].
pub(crate) fn download_track(
    db_id: i64,
    cancelled: &dyn Fn() -> bool,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    state: &SharedPlayerState,
    cfg: &Config,
    client: &SubsonicClient,
) -> Option<Result<PathBuf, String>> {
    // From the pool. This runs once per track fetched, and opening a
    // connection runs the schema DDL and a WAL checkpoint — with several
    // transfers going, several init cycles would contend with each other and
    // with library reads.
    let db = match crate::db::pool::shared().get() {
        Ok(db) => db,
        Err(e) => return Some(Err(format!("db error: {e}"))),
    };
    let Ok(Some(track)) = queries::get_track_row(&db.conn, db_id) else {
        return Some(Err("track not found".into()));
    };

    // 1. The library's own file, when there is one.
    if let Some(p) = track.path.as_deref().map(PathBuf::from)
        && p.exists()
    {
        log::info!("download_track: local file exists, using {}", p.display());
        return Some(Ok(p));
    }
    let Some(remote_id) = track.remote_id.clone() else {
        return Some(Err(
            "not in the library folder, and no remote copy to fetch".into(),
        ));
    };

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cache_dir = cfg.cache_dir();
    let dest = cache_path_for_track(&cache_dir, &track, album_date.as_deref());
    if !path_within(&cache_dir, &dest) {
        return Some(Err(format!(
            "cache path escapes the cache: {}",
            dest.display()
        )));
    }

    // 2. Already cached.
    //
    // Older builds could write a Subsonic error body here as if it were audio,
    // leaving a tiny JSON file that reports Ready and then fails to decode
    // forever. Treat those as absent so they get re-fetched.
    if dest.exists() && !is_cached_audio(&dest) {
        log::warn!(
            "discarding non-audio cache entry {} (likely a stored server error)",
            dest.display()
        );
        let _ = std::fs::remove_file(&dest);
    }
    if dest.exists() {
        return Some(Ok(dest));
    }

    // 3. Download from remote, into the `.part` file the decoder streams from
    // while the bytes land.
    let store = state.downloads();
    let bytes_written = store.announce(
        db_id,
        track.title.clone(),
        track.artist_name.clone(),
        crate::remote::download::part_path(&dest),
        dest.clone(),
    );

    let progress_tx = tx.clone();
    let stream_ready_flag = std::sync::atomic::AtomicBool::new(false);
    // A retry restarts the byte count from zero, so a changed total re-announces.
    let announced_total = AtomicU64::new(u64::MAX);
    let result =
        client.download_with_progress(&remote_id, &dest, cancelled, |downloaded, total| {
            bytes_written.set(downloaded);
            // What knows a transfer moved is the code moving it. Held to a reading
            // every 250ms inside, so a chunk landing costs an atomic and a compare.
            store.progressed();
            if announced_total.swap(total, Ordering::Relaxed) != total {
                store.started(db_id, total);
            }
            if !stream_ready_flag.load(Ordering::Relaxed)
                && downloaded >= crate::player::state::STREAM_THRESHOLD
            {
                stream_ready_flag.store(true, Ordering::Relaxed);
                // Every entry waiting on it: whichever is under the cursor is the
                // one the player starts streaming.
                for id in store.waiters(db_id) {
                    progress_tx.send(PlayerCommand::TrackStreamReady(id)).ok();
                }
            }
        });

    match result {
        Err(SubsonicError::Download(DownloadError::Cancelled)) => None,
        Err(e) => {
            log::warn!("x {} — {}", track.title, e);
            Some(Err(e.to_string()))
        }
        Ok(()) => {
            cache_grew(std::fs::metadata(&dest).map_or(0, |m| m.len()));
            // Without this row the file is invisible to cache eviction and never reclaimed.
            if let Err(e) = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy()) {
                log::warn!(
                    "cached {} but failed to record it ({}) — it will not be evicted",
                    dest.display(),
                    e
                );
            }
            log::info!("+ {} — {}", track.title, track.artist_name);
            Some(Ok(dest))
        }
    }
}

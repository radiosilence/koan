//! Library tracks as queue items.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::state::{ItemState, PlaylistItem, QueueItemId};

use super::*;

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, `ItemState::Ready`) for local/cached, (cache path, `ItemState::Pending`)
/// for remote — a track with no copy here yet has to be fetched before it plays.
/// A copy found in the cache that the row does not name is added to `found`,
/// for the caller to record.
fn resolve_item_path(
    found: &mut Vec<(i64, PathBuf)>,
    cfg: &Config,
    track: &queries::TrackRow,
    remote_url: Option<&str>,
    album_date: Option<&str>,
) -> (PathBuf, ItemState) {
    match queries::choose_playback_source(
        track.path.as_deref(),
        track.cached_path.as_deref(),
        remote_url,
    ) {
        Some(queries::PlaybackSource::Local(p)) => (p, ItemState::Ready),
        // A cache entry is only as good as its contents. Older builds could
        // store a Subsonic error body here, which reports Ready and then fails
        // to decode forever; treating it as Pending sends it back through the
        // download path, which discards it and re-fetches.
        Some(queries::PlaybackSource::Cached(p)) => {
            let state = if is_cached_audio(&p) {
                ItemState::Ready
            } else {
                ItemState::Pending
            };
            (p, state)
        }
        Some(queries::PlaybackSource::Remote(_)) => {
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            if dest.exists() && is_cached_audio(&dest) {
                if track.cached_path.as_deref() != Some(&*dest.to_string_lossy()) {
                    found.push((track.id, dest.clone()));
                }
                (dest, ItemState::Ready)
            } else {
                (dest, ItemState::Pending)
            }
        }
        _ => {
            // Fallback: construct a cache path and mark pending.
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            (dest, ItemState::Pending)
        }
    }
}

/// Build a PlaylistItem from a TrackRow + album date + resolved path + load state.
pub fn playlist_item_from_track(
    track: &queries::TrackRow,
    album_date: Option<&str>,
    dest: PathBuf,
    state: ItemState,
) -> PlaylistItem {
    let year = album_date.and_then(year_of).map(str::to_string);
    PlaylistItem {
        playlist_entry_id: None,
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
        state,
        played: false,
    }
}

/// Build playlist items for many tracks at once.
///
/// One config read and one query for the whole batch, whatever its size: the
/// rows already say where each track's file and download are, and what they
/// leave out — the stream URL and the album's date — is read for all of them
/// together.
pub fn playlist_items_for_tracks(db: &Database, tracks: &[queries::TrackRow]) -> Vec<PlaylistItem> {
    playlist_items_with(db, &Config::load().unwrap_or_default(), tracks)
}

pub(super) fn playlist_items_with(
    db: &Database,
    cfg: &Config,
    tracks: &[queries::TrackRow],
) -> Vec<PlaylistItem> {
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let extras = queries::queue_item_extras(&db.conn, &ids).unwrap_or_default();

    let mut found = Vec::new();
    let items = tracks
        .iter()
        .map(|track| {
            let extra = extras.get(&track.id);
            let remote_url = extra.and_then(|e| e.remote_url.as_deref());
            let album_date = extra.and_then(|e| e.album_date.as_deref());
            let (path, state) = resolve_item_path(&mut found, cfg, track, remote_url, album_date);
            playlist_item_from_track(track, album_date, path, state)
        })
        .collect();
    // Bookkeeping an enqueue should not wait behind a scan for: skipped while
    // a writer holds the lock, and found again the next time the track is
    // queued, or at launch (`adopt_cached_files`).
    if !found.is_empty() {
        let recorded = crate::db::connection::without_waiting(&db.conn, |conn| {
            queries::atomically(conn, || {
                found.iter().try_for_each(|(id, path)| {
                    queries::set_cached_path(conn, *id, &path.to_string_lossy())
                })
            })
        });
        if let Err(e) = recorded {
            log::debug!(
                "left {} cached file(s) unrecorded for now: {e}",
                found.len()
            );
        }
    }
    items
}

/// Record a file found already in the cache as the track's download, unless
/// it already is. A file can outlive its record — a row re-derived or merged
/// without it, a crash between the rename and the write — and an unrecorded
/// one reads as not on this machine and is never evicted.
pub(super) fn adopt_cached(conn: &rusqlite::Connection, track: &queries::TrackRow, dest: &Path) {
    let dest = dest.to_string_lossy();
    if track.cached_path.as_deref() == Some(&*dest) {
        return;
    }
    if let Err(e) = queries::set_cached_path(conn, track.id, &dest) {
        log::warn!("found {dest} in the cache but failed to record it ({e})");
    }
}

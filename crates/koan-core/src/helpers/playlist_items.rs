//! Library tracks as queue items.

use std::path::PathBuf;

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::state::{ItemState, PlaylistItem, QueueItemId};

use super::*;

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, `ItemState::Ready`) for local/cached, (cache path, `ItemState::Pending`)
/// for remote — a track with no copy here yet has to be fetched before it plays.
fn resolve_item_path(
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
        pre_shuffle: None,
    }
}

/// Build playlist items for many tracks at once.
///
/// One config read and one query for the whole batch, whatever its size: the
/// rows already say where each track's file and download are, and what they
/// leave out — the stream URL and the album's date — is read for all of them
/// together.
pub fn playlist_items_for_tracks(db: &Database, tracks: &[queries::TrackRow]) -> Vec<PlaylistItem> {
    let cfg = Config::load().unwrap_or_default();
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let extras = queries::queue_item_extras(&db.conn, &ids).unwrap_or_default();

    tracks
        .iter()
        .map(|track| {
            let extra = extras.get(&track.id);
            let remote_url = extra.and_then(|e| e.remote_url.as_deref());
            let album_date = extra.and_then(|e| e.album_date.as_deref());
            let (path, state) = resolve_item_path(&cfg, track, remote_url, album_date);
            playlist_item_from_track(track, album_date, path, state)
        })
        .collect()
}

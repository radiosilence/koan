use std::sync::Arc;

use async_graphql::connection::Edge;
use crossbeam_channel::Sender;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueItemId, SharedPlayerState};

use super::types::Conn;

// ---------------------------------------------------------------------------
// Pagination helper — uses usize as cursor (async-graphql has built-in impl)
// ---------------------------------------------------------------------------

pub(super) fn paginate<T: async_graphql::OutputType>(
    items: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> async_graphql::Result<Conn<T>> {
    let total = items.len();

    let start = if let Some(ref cursor) = after {
        cursor.parse::<usize>().unwrap_or(0) + 1
    } else {
        0
    };

    let end = if let Some(f) = first {
        (start + f as usize).min(total)
    } else {
        total
    };

    let mut conn = Conn::new(start > 0, end < total);
    for (i, item) in items.into_iter().enumerate().skip(start).take(end - start) {
        conn.edges.push(Edge::new(i, item));
    }
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Year extraction from date strings ("2024", "2024-01-15", etc)
// ---------------------------------------------------------------------------

pub(super) fn extract_year(date: &str) -> Option<i32> {
    date.get(..4).and_then(|s| s.parse().ok())
}

/// Get album year from its date field.
pub(super) fn album_year(album: &queries::AlbumRow) -> Option<i32> {
    album.date.as_deref().and_then(extract_year)
}

/// Get the year for a track via its album's date.
pub(super) fn track_year(db: &Database, track: &queries::TrackRow) -> Option<i32> {
    track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten())
        .as_deref()
        .and_then(extract_year)
}

// ---------------------------------------------------------------------------
// Favourite sync
// ---------------------------------------------------------------------------

pub(super) fn sync_favourite_to_remote(db: &Database, path: &str, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let remote_id = queries::remote_id_for_path(&db.conn, std::path::Path::new(path))
        .ok()
        .flatten();
    if let Some(rid) = remote_id {
        let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
            return;
        };
        std::thread::Builder::new()
            .name("koan-fav-sync".into())
            .spawn(move || {
                let result = if star {
                    client.star(&rid)
                } else {
                    client.unstar(&rid)
                };
                if let Err(e) = result {
                    log::warn!("failed to sync favourite to remote: {}", e);
                }
            })
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Download spawning — delegates to koan-core helpers
// ---------------------------------------------------------------------------

pub(super) fn spawn_downloads(
    pending: Vec<(i64, QueueItemId)>,
    tx: Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    koan_core::helpers::spawn_downloads(pending, tx, state);
}

// ---------------------------------------------------------------------------
// TrackRow -> PlaylistItem — delegates to koan-core helpers
// ---------------------------------------------------------------------------

pub fn track_to_playlist_item(
    track: &queries::TrackRow,
    db: &Database,
) -> koan_core::player::state::PlaylistItem {
    koan_core::helpers::track_to_playlist_item(track, db)
}

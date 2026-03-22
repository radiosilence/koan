use std::collections::HashSet;
use std::sync::Arc;

use async_graphql::connection::{Connection, Edge, EmptyFields};
use crossbeam_channel::Sender;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueItemId, SharedPlayerState};

// ---------------------------------------------------------------------------
// Pagination helper — uses usize as cursor (async-graphql has built-in impl)
// ---------------------------------------------------------------------------

pub(super) fn paginate<T: async_graphql::OutputType>(
    items: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> async_graphql::Result<Connection<usize, T, EmptyFields, EmptyFields>> {
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

    let mut conn = Connection::new(start > 0, end < total);
    for (i, item) in items.into_iter().enumerate().skip(start).take(end - start) {
        conn.edges.push(Edge::new(i, item));
    }
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Helper: year extraction from date strings ("2024", "2024-01-15", etc)
// ---------------------------------------------------------------------------

pub(super) fn extract_year(date: &str) -> Option<i32> {
    date.get(..4).and_then(|s| s.parse().ok())
}

/// Get album year from its date field.
pub(super) fn album_year(album: &queries::AlbumRow) -> Option<i32> {
    album.date.as_deref().and_then(extract_year)
}

/// Get genres for an artist (distinct genres from their tracks).
pub(super) fn artist_genres(db: &Database, artist_id: i64) -> HashSet<String> {
    queries::tracks_for_artist(&db.conn, artist_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t.genre)
        .map(|g| g.to_lowercase())
        .collect()
}

/// Get genres for an album (distinct genres from its tracks).
pub(super) fn album_genres(db: &Database, album_id: i64) -> HashSet<String> {
    queries::tracks_for_album(&db.conn, album_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t.genre)
        .map(|g| g.to_lowercase())
        .collect()
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
// Helper: favourite ID sets for filtering
// ---------------------------------------------------------------------------

/// Get the set of artist IDs that have at least one favourited track.
pub(super) fn favourite_artist_ids(db: &Database) -> async_graphql::Result<HashSet<i64>> {
    let fav_paths =
        queries::load_favourites(&db.conn).map_err(|e| async_graphql::Error::new(e.to_string()))?;
    let mut ids = HashSet::new();
    for path in &fav_paths {
        let path_str = path.to_string_lossy();
        if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &path_str)
            && let Ok(Some(row)) = queries::get_track_row(&db.conn, tid)
            && let Some(aid) = row.artist_id
        {
            ids.insert(aid);
        }
    }
    Ok(ids)
}

/// Get the set of album IDs that have at least one favourited track.
pub(super) fn favourite_album_ids(db: &Database) -> async_graphql::Result<HashSet<i64>> {
    let fav_paths =
        queries::load_favourites(&db.conn).map_err(|e| async_graphql::Error::new(e.to_string()))?;
    let mut ids = HashSet::new();
    for path in &fav_paths {
        let path_str = path.to_string_lossy();
        if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &path_str)
            && let Ok(Some(row)) = queries::get_track_row(&db.conn, tid)
            && let Some(aid) = row.album_id
        {
            ids.insert(aid);
        }
    }
    Ok(ids)
}

pub(super) fn sync_favourite_to_remote(db: &Database, path: &str, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let remote_id = queries::remote_id_for_path(&db.conn, std::path::Path::new(path))
        .ok()
        .flatten();
    if let Some(rid) = remote_id {
        let password = super::super::get_remote_password(&cfg);
        let client = koan_core::remote::client::SubsonicClient::new(
            &cfg.remote.url,
            &cfg.remote.username,
            &password,
        );
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
// Helper: TrackRow -> PlaylistItem (for queue mutations via GraphQL)
// ---------------------------------------------------------------------------

/// Spawn background downloads for remote tracks that were added to the queue
/// with LoadState::Pending. Uses the same download pipeline as the TUI.
pub(super) fn spawn_downloads(
    pending: Vec<(i64, QueueItemId)>,
    tx: Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    use std::sync::Mutex;

    let log_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    std::thread::Builder::new()
        .name("koan-gql-download".into())
        .spawn(move || {
            let cfg = Config::load().unwrap_or_default();
            for (db_id, queue_id) in pending {
                super::super::enqueue::download_track(db_id, queue_id, &tx, &log_buf, &state, &cfg);
            }
        })
        .ok();
}

pub fn track_to_playlist_item(
    track: &queries::TrackRow,
    db: &Database,
) -> koan_core::player::state::PlaylistItem {
    use koan_core::player::state::PlaylistItem;

    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cfg = Config::load().unwrap_or_default();
    let (path, load_state) =
        super::super::enqueue::resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());

    let year = album_date.as_deref().and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });

    PlaylistItem {
        id: QueueItemId::new(),
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

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
// Pagination — uses usize as cursor (async-graphql has built-in impl)
// ---------------------------------------------------------------------------

/// Page size when the client asks for no `first`.
///
/// The old behaviour was "everything": one `{ tracks { edges { node { ... } } } }`
/// materialised the whole library as rows, as GraphQL values and as serialised
/// JSON at the same time.
pub(super) const DEFAULT_PAGE: usize = 50;

/// Ceiling on `first`, so a client cannot ask for the library in one response.
pub(super) const MAX_PAGE: usize = 500;

/// Clamp a client-supplied `first` into the servable range. Negative values are
/// floored at zero — as a raw `as usize` they wrapped to a request for the
/// entire library.
pub(super) fn page_size(first: Option<i32>) -> usize {
    match first {
        Some(f) => (f.max(0) as usize).min(MAX_PAGE),
        None => DEFAULT_PAGE,
    }
}

/// Index of the first row after the given cursor.
pub(super) fn page_offset(after: Option<&str>) -> usize {
    after
        .and_then(|c| c.parse::<usize>().ok())
        .map(|i| i.saturating_add(1))
        .unwrap_or(0)
}

/// Paginate an already-materialised list.
pub(super) fn paginate<T: async_graphql::OutputType>(
    items: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> async_graphql::Result<Conn<T>> {
    let total = items.len();
    let start = page_offset(after.as_deref()).min(total);
    let end = start.saturating_add(page_size(first)).min(total);

    let mut conn = Conn::new(start > 0, end < total);
    for (i, item) in items.into_iter().enumerate().skip(start).take(end - start) {
        conn.edges.push(Edge::new(i, item));
    }
    Ok(conn)
}

/// Wrap a page that SQL already windowed. `rows` may hold one extra row beyond
/// `limit`, which is how the next-page flag is derived without a `COUNT(*)`.
pub(super) fn paginate_window<T: async_graphql::OutputType>(
    mut rows: Vec<T>,
    offset: usize,
    limit: usize,
) -> Conn<T> {
    let has_next = rows.len() > limit;
    rows.truncate(limit);
    let mut conn = Conn::new(offset > 0, has_next);
    for (i, item) in rows.into_iter().enumerate() {
        conn.edges.push(Edge::new(offset + i, item));
    }
    conn
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

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};
use crate::remote::client::{SubsonicAlbum, SubsonicAlbumFull, SubsonicClient};

use rayon::prelude::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("subsonic error: {0}")]
    Subsonic(#[from] super::client::SubsonicError),
    #[error("db error: {0}")]
    Db(#[from] crate::db::connection::DbError),
}

#[derive(Debug, Default)]
pub struct SyncResult {
    pub artists_synced: usize,
    pub albums_synced: usize,
    pub tracks_synced: usize,
    pub matched_local: usize,
}

/// Pull the entire Navidrome/Subsonic library into the local DB as remote tracks.
///
/// Pipeline: paginate album list → fetch album details in parallel (rayon) →
/// batch-write each page in a single transaction. Local tracks are never touched.
/// After sync, matches remote tracks against local (same artist+album+title+track#)
/// so local files take playback priority.
pub fn sync_library(db: &Database, client: &SubsonicClient) -> Result<SyncResult, SyncError> {
    let mut result = SyncResult::default();

    // Clear old remote-only tracks. Doesn't touch local or cached.
    let removed = queries::remove_tracks_by_source(&db.conn, "remote")?;
    if removed > 0 {
        log::info!("cleared {} stale remote tracks", removed);
    }

    // Count artists for the result (cheap call).
    let artists = client.get_artists()?;
    result.artists_synced = artists.len();
    log::info!("syncing {} artists from remote", artists.len());

    // Paginate through all albums, fetch details in parallel, batch write.
    let mut offset = 0u32;
    let page_size = 500u32;

    loop {
        let album_summaries = client.get_album_list("alphabeticalByName", page_size, offset)?;
        if album_summaries.is_empty() {
            break;
        }

        let page_count = album_summaries.len();

        // Parallel fetch: get full album details (with tracks) concurrently.
        let fetched: Vec<(SubsonicAlbum, SubsonicAlbumFull)> = album_summaries
            .into_par_iter()
            .filter_map(|summary| match client.get_album(&summary.id) {
                Ok(full) => Some((summary, full)),
                Err(e) => {
                    log::warn!("failed to fetch album {}: {}", summary.id, e);
                    None
                }
            })
            .collect();

        // Batch write in a single transaction.
        db.conn
            .execute_batch("BEGIN")
            .map_err(crate::db::connection::DbError::from)?;

        for (_, album) in &fetched {
            result.albums_synced += 1;
            let artist_name = album.artist.as_deref().unwrap_or("Unknown Artist");

            for song in &album.song {
                let meta = TrackMeta {
                    title: song.title.clone(),
                    artist: song
                        .artist
                        .clone()
                        .unwrap_or_else(|| artist_name.to_string()),
                    album_artist: album.artist.clone(),
                    album: album.name.clone(),
                    date: album.year.map(|y| y.to_string()),
                    disc: song.disc_number,
                    track_number: song.track,
                    genre: song.genre.clone().or_else(|| album.genre.clone()),
                    label: None,
                    duration_ms: song.duration.map(|d| d * 1000),
                    codec: song.suffix.clone(),
                    sample_rate: None,
                    bit_depth: None,
                    channels: None,
                    bitrate: song.bit_rate,
                    size_bytes: None,
                    mtime: None,
                    path: None,
                    source: "remote".to_string(),
                    remote_id: Some(song.id.clone()),
                    remote_url: Some(client.stream_url(&song.id)),
                };

                match queries::upsert_track(&db.conn, &meta) {
                    Ok(_) => result.tracks_synced += 1,
                    Err(e) => log::warn!("failed to insert remote track {}: {}", song.title, e),
                }
            }
        }

        db.conn
            .execute_batch("COMMIT")
            .map_err(crate::db::connection::DbError::from)?;

        offset += page_count as u32;
        log::info!(
            "synced {} albums ({} tracks) so far...",
            result.albums_synced,
            result.tracks_synced
        );
    }

    // Match remote tracks against local ones.
    let matched = match_remote_to_local(&db.conn);
    result.matched_local = matched;

    log::info!(
        "sync complete: {} artists, {} albums, {} tracks, {} matched to local",
        result.artists_synced,
        result.albums_synced,
        result.tracks_synced,
        result.matched_local
    );

    Ok(result)
}

/// For remote tracks that have a matching local track (same artist, album, title, track#),
/// copy the local path onto the remote track so playback uses the local file.
fn match_remote_to_local(conn: &rusqlite::Connection) -> usize {
    // Find remote tracks that match a local track.
    let result = conn.execute(
        "UPDATE tracks SET path = (
            SELECT l.path FROM tracks l
            WHERE l.source = 'local'
              AND l.title = tracks.title
              AND l.artist_id = tracks.artist_id
              AND l.album_id = tracks.album_id
              AND COALESCE(l.track_number, -1) = COALESCE(tracks.track_number, -1)
            LIMIT 1
        )
        WHERE source = 'remote'
          AND path IS NULL
          AND EXISTS (
            SELECT 1 FROM tracks l
            WHERE l.source = 'local'
              AND l.title = tracks.title
              AND l.artist_id = tracks.artist_id
              AND l.album_id = tracks.album_id
              AND COALESCE(l.track_number, -1) = COALESCE(tracks.track_number, -1)
        )",
        [],
    );

    match result {
        Ok(count) => {
            if count > 0 {
                log::info!("{} remote tracks matched to local files", count);
            }
            count
        }
        Err(e) => {
            log::error!("failed to match remote tracks to local: {}", e);
            0
        }
    }
}

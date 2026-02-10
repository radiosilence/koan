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
}

/// Pull the entire Navidrome/Subsonic library into the local DB.
///
/// Pipeline: paginate album list → fetch album details in parallel (rayon) →
/// batch-write each page in a single transaction.
///
/// Deduplication happens in `upsert_track` — if a local track already exists
/// with the same artist + album + title + track#, the remote_id and remote_url
/// are merged onto the existing row instead of creating a duplicate.
pub fn sync_library(db: &Database, client: &SubsonicClient) -> Result<SyncResult, SyncError> {
    let mut result = SyncResult::default();

    let artists = client.get_artists()?;
    result.artists_synced = artists.len();
    log::info!("syncing {} artists from remote", artists.len());

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

    log::info!(
        "sync complete: {} artists, {} albums, {} tracks",
        result.artists_synced,
        result.albums_synced,
        result.tracks_synced,
    );

    Ok(result)
}

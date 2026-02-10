use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};
use crate::remote::client::SubsonicClient;

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
/// Existing remote tracks are cleared first (full re-sync). Local tracks are
/// never touched. After sync, we try to match remote tracks against local ones
/// — if a local file has the same artist + album + title + track#, the local
/// path takes priority for playback.
pub fn sync_library(db: &Database, client: &SubsonicClient) -> Result<SyncResult, SyncError> {
    let mut result = SyncResult::default();

    // Clear old remote-only tracks. This doesn't touch local or cached tracks.
    let removed = queries::remove_tracks_by_source(&db.conn, "remote")?;
    if removed > 0 {
        log::info!("cleared {} stale remote tracks", removed);
    }

    db.conn
        .execute_batch("BEGIN")
        .map_err(crate::db::connection::DbError::from)?;

    // Fetch all artists from the server.
    let artists = client.get_artists()?;
    result.artists_synced = artists.len();
    log::info!("syncing {} artists from remote", artists.len());

    // We don't need to iterate artists individually — getAlbumList2 gives us
    // everything with full track info. Artists get created via upsert_track.

    // Paginate through all albums on the server.
    let mut offset = 0u32;
    let page_size = 500u32;

    loop {
        let albums = client.get_album_list("alphabeticalByName", page_size, offset)?;
        if albums.is_empty() {
            break;
        }

        for album_summary in &albums {
            // Fetch full album with tracks.
            let album = match client.get_album(&album_summary.id) {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("failed to fetch album {}: {}", album_summary.id, e);
                    continue;
                }
            };

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
                    duration_ms: song.duration.map(|d| d * 1000), // Subsonic returns seconds
                    codec: song.suffix.clone(),
                    sample_rate: None,
                    bit_depth: None,
                    channels: None,
                    bitrate: song.bit_rate,
                    size_bytes: None,
                    mtime: None,
                    path: None, // No local path — it's remote.
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

        offset += albums.len() as u32;
        log::info!("synced {} albums so far...", result.albums_synced);
    }

    // Match remote tracks against local ones.
    // If a local track has the same artist + album + title + track#, mark it
    // so resolve_playback_path will prefer the local file.
    let matched = match_remote_to_local(&db.conn);
    result.matched_local = matched;

    db.conn
        .execute_batch("COMMIT")
        .map_err(crate::db::connection::DbError::from)?;

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

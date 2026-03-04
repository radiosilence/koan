mod albums;
mod artists;
pub mod lyrics;
mod scan_cache;
mod search;
mod stats;
pub mod tracks;

use std::path::PathBuf;

// Re-export all public items so `use queries::*` still works.
pub use albums::*;
pub use artists::*;
pub use lyrics::*;
pub use scan_cache::*;
pub use search::*;
pub use stats::*;
pub use tracks::*;

// --- Row types ---

#[derive(Debug, Clone)]
pub struct ArtistRow {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub remote_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlbumRow {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: String,
    pub date: Option<String>,
    pub total_discs: Option<i32>,
    pub total_tracks: Option<i32>,
    pub codec: Option<String>,
    pub label: Option<String>,
    pub remote_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackRow {
    pub id: i64,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub artist_name: String,
    pub album_artist_name: String,
    pub album_title: String,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub title: String,
    pub duration_ms: Option<i64>,
    pub path: Option<String>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub genre: Option<String>,
    pub source: String,
    pub remote_id: Option<String>,
    pub cached_path: Option<String>,
}

/// Where to get audio data for playback. Local always wins.
#[derive(Debug, Clone)]
pub enum PlaybackSource {
    Local(PathBuf),
    Cached(PathBuf),
    Remote(String),
}

#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
}

/// Metadata for inserting/updating a track.
#[derive(Debug, Clone)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    pub album: String,
    pub date: Option<String>,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub size_bytes: Option<i64>,
    pub mtime: Option<i64>,
    pub path: Option<String>,
    pub source: String,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
}

/// Test helper: build a sample TrackMeta for use in tests across sub-modules.
#[cfg(test)]
pub fn sample_meta(title: &str, artist: &str, album: &str) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: artist.into(),
        album_artist: Some(artist.into()),
        album: album.into(),
        date: Some("2024".into()),
        disc: Some(1),
        track_number: Some(1),
        genre: Some("Electronic".into()),
        label: None,
        duration_ms: Some(240_000),
        codec: Some("FLAC".into()),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(2),
        bitrate: Some(1000),
        size_bytes: Some(30_000_000),
        mtime: Some(1700000000),
        path: Some(format!("/music/{}/{}.flac", album, title)),
        source: "local".into(),
        remote_id: None,
        remote_url: None,
    }
}

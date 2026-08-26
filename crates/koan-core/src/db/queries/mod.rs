mod albums;
mod artists;
pub mod auth;
pub mod batch;
mod favourites;
pub mod history;
pub mod lyrics;
pub mod playback_state;
pub mod playlists;
pub mod radio;
mod scan_cache;
mod search;
mod stats;
pub mod tracks;
pub mod vectors;

use std::path::PathBuf;

// Re-export all public items so `use queries::*` still works.
pub use albums::*;
pub use artists::*;
pub use batch::*;
pub use favourites::*;
pub use history::*;
pub use lyrics::*;
pub use playback_state::*;
pub use playlists::*;
pub use radio::*;
pub use scan_cache::*;
pub use search::*;
pub use stats::*;
pub use tracks::*;
pub use vectors::*;

/// The half-open range of paths under a folder, for `path >= .0 AND path < .1`.
///
/// A prefix match on an indexed column, rather than `LIKE 'folder/%'` — which
/// SQLite answers by reading every row, because a pattern is opaque to an
/// index until it has been evaluated. It also takes the pattern out of the
/// path: `LIKE` reads `_` as "any character" and folds ASCII case, so
/// `/Volumes/My_Music` matched `/Volumes/My Music` and `/volumes/my_music`
/// alike.
///
/// The trailing separator is what keeps `/Volumes/Music` out of
/// `/Volumes/Music Backup`; the upper bound is the highest code point, so
/// every path under the folder sorts below it.
pub fn folder_prefix_range(folder: &std::path::Path) -> (String, String) {
    let prefix = format!(
        "{}{}",
        folder
            .to_string_lossy()
            .trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );
    let upper = format!("{prefix}\u{10FFFF}");
    (prefix, upper)
}

// --- Row types ---

#[derive(Debug, Clone)]
pub struct ArtistRow {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub remote_id: Option<String>,
    /// Albums credited to this artist, and tracks across them. Aggregated in
    /// the same query as the row itself — a count per artist would be one
    /// query per row in a list thousands long.
    pub album_count: i64,
    pub track_count: i64,
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
    /// When the album entered the library — the server's `created` for remote
    /// albums, otherwise the time it was first indexed.
    pub added_at: Option<String>,
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
    /// The server's ids for the album and its artist.
    ///
    /// Carried alongside the track's own, because the server keys stars,
    /// shares and cover art off them — a library synced without these has
    /// albums and artists it can name but cannot refer to.
    pub album_remote_id: Option<String>,
    pub artist_remote_id: Option<String>,
    /// MusicBrainz recording id. From the server today; a local scan could
    /// read it from `MUSICBRAINZ_TRACKID` too.
    pub mbid: Option<String>,
    /// When the album this track belongs to entered the library. Remote sync
    /// supplies the server's `created`; anything else leaves it and the album
    /// is stamped with the time it was first seen.
    pub album_added_at: Option<String>,
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
        album_remote_id: None,
        artist_remote_id: None,
        mbid: None,
        remote_url: None,
        album_added_at: None,
    }
}

use async_graphql::{Enum, SimpleObject};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum PlaybackStateEnum {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LoadStateEnum {
    Ready,
    Pending,
    Downloading,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TrackSource {
    Local,
    Remote,
    Cached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ArtistSortField {
    Name,
    TrackCount,
    AlbumCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum AlbumSortField {
    Title,
    Date,
    ArtistThenDate,
    TrackCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TrackSortField {
    Title,
    Artist,
    Album,
    Duration,
    ArtistAlbumDiscTrack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum SortDirection {
    Asc,
    Desc,
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A track in the library.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub genre: Option<String>,
    pub source: TrackSource,
    pub remote_id: Option<String>,
    pub path: Option<String>,
    pub cached_path: Option<String>,
}

/// Currently playing track info.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlNowPlaying {
    pub state: PlaybackStateEnum,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub track: Option<GqlNowPlayingTrack>,
    pub queue_item_id: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlNowPlayingTrack {
    pub queue_item_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub duration_ms: Option<i64>,
}

/// A single entry in the play queue.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlQueueEntry {
    pub queue_item_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<i64>,
    pub is_current: bool,
}

/// Library-wide statistics.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlLibraryStats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
}

/// Audio output device.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlDevice {
    pub name: String,
    pub sample_rates: Vec<f64>,
}

/// Generic mutation result.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlStatus {
    pub success: bool,
    pub message: String,
}

impl GqlStatus {
    pub fn ok(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: msg.into(),
        }
    }

    #[allow(dead_code)]
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: msg.into(),
        }
    }
}

/// Queue mutation result with added item info.
#[derive(Debug, Clone, SimpleObject)]
pub struct GqlQueueMutationResult {
    pub ok: bool,
    pub message: String,
    pub added_count: i32,
    pub queue_item_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Connection types (Relay spec)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlPageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlArtistEdge {
    pub node: GqlArtistFlat,
    pub cursor: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlArtistFlat {
    pub id: i64,
    pub name: String,
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlArtistConnection {
    pub edges: Vec<GqlArtistEdge>,
    pub page_info: GqlPageInfo,
    pub total_count: i32,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlAlbumEdge {
    pub node: GqlAlbumFlat,
    pub cursor: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlAlbumFlat {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: String,
    pub date: Option<String>,
    pub codec: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlAlbumConnection {
    pub edges: Vec<GqlAlbumEdge>,
    pub page_info: GqlPageInfo,
    pub total_count: i32,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlTrackEdge {
    pub node: GqlTrack,
    pub cursor: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlTrackConnection {
    pub edges: Vec<GqlTrackEdge>,
    pub page_info: GqlPageInfo,
    pub total_count: i32,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlQueueConnection {
    pub edges: Vec<GqlQueueEdge>,
    pub total_count: i32,
    pub current_index: Option<i32>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GqlQueueEdge {
    pub node: GqlQueueEntry,
    pub cursor: String,
}

// ---------------------------------------------------------------------------
// Conversions from koan-core types
// ---------------------------------------------------------------------------

impl GqlTrack {
    pub fn from_row(t: &koan_core::db::queries::TrackRow) -> Self {
        Self {
            id: t.id,
            title: t.title.clone(),
            artist: t.artist_name.clone(),
            album_artist: t.album_artist_name.clone(),
            album: t.album_title.clone(),
            album_id: t.album_id,
            artist_id: t.artist_id,
            disc: t.disc,
            track_number: t.track_number,
            duration_ms: t.duration_ms,
            codec: t.codec.clone(),
            sample_rate: t.sample_rate,
            bit_depth: t.bit_depth,
            channels: t.channels,
            bitrate: t.bitrate,
            genre: t.genre.clone(),
            source: match t.source.as_str() {
                "remote" => TrackSource::Remote,
                "cached" => TrackSource::Cached,
                _ => TrackSource::Local,
            },
            remote_id: t.remote_id.clone(),
            path: t.path.clone(),
            cached_path: t.cached_path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Cursor encoding/decoding
// ---------------------------------------------------------------------------

pub fn encode_cursor(offset: usize) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("cursor:{offset}"))
}

pub fn decode_cursor(cursor: &str) -> Option<usize> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cursor)
        .ok()?;
    let s = String::from_utf8(bytes).ok()?;
    s.strip_prefix("cursor:")?.parse().ok()
}

/// Apply Relay-style pagination to a slice. Returns (page_slice, start_offset, has_prev, has_next).
pub fn paginate<T: Clone>(
    items: &[T],
    first: Option<i32>,
    after: Option<&str>,
    last: Option<i32>,
    before: Option<&str>,
) -> (Vec<T>, usize, bool, bool) {
    let total = items.len();

    // Determine start from `after` cursor.
    let start = after.and_then(decode_cursor).map(|c| c + 1).unwrap_or(0);

    // Determine end from `before` cursor.
    let end = before.and_then(decode_cursor).unwrap_or(total);

    let start = start.min(total);
    let end = end.min(total).max(start);

    let slice = &items[start..end];

    // Apply first/last limits.
    let (result, result_start) = if let Some(first) = first {
        let first = first.max(0) as usize;
        let taken = &slice[..slice.len().min(first)];
        (taken.to_vec(), start)
    } else if let Some(last) = last {
        let last = last.max(0) as usize;
        let skip = slice.len().saturating_sub(last);
        (slice[skip..].to_vec(), start + skip)
    } else {
        (slice.to_vec(), start)
    };

    let has_prev = result_start > 0;
    let has_next = result_start + result.len() < total;

    (result, result_start, has_prev, has_next)
}

pub fn make_page_info(
    start_offset: usize,
    count: usize,
    has_prev: bool,
    has_next: bool,
) -> GqlPageInfo {
    GqlPageInfo {
        has_previous_page: has_prev,
        has_next_page: has_next,
        start_cursor: if count > 0 {
            Some(encode_cursor(start_offset))
        } else {
            None
        },
        end_cursor: if count > 0 {
            Some(encode_cursor(start_offset + count - 1))
        } else {
            None
        },
    }
}

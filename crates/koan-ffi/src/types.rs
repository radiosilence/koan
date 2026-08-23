//! Plain-data mirrors of koan-core types, shaped for the FFI boundary.
//!
//! uniffi needs owned records with concrete field types — no lifetimes, no
//! `PathBuf`, no `Arc`. These conversions are the only place that translation
//! happens; everything above and below speaks its own native vocabulary.

use koan_core::db::queries::{self, AlbumRow, ArtistRow, LibraryStats, TrackRow};
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueEntry, QueueEntryStatus, TrackInfo,
};

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

impl From<PlaybackState> for PlayState {
    fn from(s: PlaybackState) -> Self {
        match s {
            PlaybackState::Stopped => Self::Stopped,
            PlaybackState::Playing => Self::Playing,
            PlaybackState::Paused => Self::Paused,
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStatus {
    Queued,
    Playing,
    Played,
    Downloading,
    PriorityPending,
    Failed,
}

impl From<QueueEntryStatus> for EntryStatus {
    fn from(s: QueueEntryStatus) -> Self {
        match s {
            QueueEntryStatus::Queued => Self::Queued,
            QueueEntryStatus::Playing => Self::Playing,
            QueueEntryStatus::Played => Self::Played,
            QueueEntryStatus::Downloading => Self::Downloading,
            QueueEntryStatus::PriorityPending => Self::PriorityPending,
            QueueEntryStatus::Failed => Self::Failed,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
}

impl From<ArtistRow> for Artist {
    fn from(r: ArtistRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            sort_name: r.sort_name,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: String,
    pub year: Option<i32>,
    pub codec: Option<String>,
    pub label: Option<String>,
    pub total_discs: Option<i32>,
    pub total_tracks: Option<i32>,
    /// When it entered the library. Sortable text — the server's ISO `created`
    /// for remote albums, SQLite's `datetime('now')` for locally scanned ones.
    pub added_at: Option<String>,
}

impl From<AlbumRow> for Album {
    fn from(r: AlbumRow) -> Self {
        let year = r.date.as_deref().and_then(year_of);
        Self {
            id: r.id,
            title: r.title,
            artist_id: r.artist_id,
            artist_name: r.artist_name,
            year,
            codec: r.codec,
            label: r.label,
            total_discs: r.total_discs,
            total_tracks: r.total_tracks,
            added_at: r.added_at,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist_name: String,
    pub album_artist_name: String,
    pub album_title: String,
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
    /// `"local"` or `"remote"` — remote tracks download on demand.
    pub source: String,
    /// Present once the file exists on disk, locally or in the cache.
    pub path: Option<String>,
    pub is_favourite: bool,
}

impl Track {
    pub(crate) fn from_row(r: TrackRow, is_favourite: bool) -> Self {
        let path = r.path.clone().or_else(|| r.cached_path.clone());
        Self {
            id: r.id,
            title: r.title,
            artist_name: r.artist_name,
            album_artist_name: r.album_artist_name,
            album_title: r.album_title,
            album_id: r.album_id,
            artist_id: r.artist_id,
            disc: r.disc,
            track_number: r.track_number,
            duration_ms: r.duration_ms,
            codec: r.codec,
            sample_rate: r.sample_rate,
            bit_depth: r.bit_depth,
            channels: r.channels,
            bitrate: r.bitrate,
            genre: r.genre,
            source: r.source,
            path,
            is_favourite,
        }
    }
}

/// Audio format of the track on the wire right now — what the DAC is actually
/// being fed, as opposed to what the database claims.
#[derive(uniffi::Record, Debug, Clone)]
pub struct StreamFormat {
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<u32>,
    pub channels: u16,
}

impl From<&TrackInfo> for StreamFormat {
    fn from(t: &TrackInfo) -> Self {
        Self {
            codec: t.codec.clone(),
            sample_rate: t.sample_rate,
            bit_depth: t.bit_depth,
            bitrate_kbps: t.bitrate_kbps,
            channels: t.channels,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct NowPlaying {
    pub state: PlayState,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Queue item currently under the cursor, if any.
    pub queue_item_id: Option<String>,
    pub entry: Option<QueueItem>,
    pub format: Option<StreamFormat>,
    /// Bumped on every queue mutation — cheap change detection for the UI.
    pub playlist_version: u64,
    pub radio_enabled: bool,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct QueueItem {
    pub queue_item_id: String,
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub year: Option<String>,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub status: EntryStatus,
    /// 0.0–1.0 while downloading, `None` otherwise.
    pub download_progress: Option<f64>,
}

impl QueueItem {
    /// Build directly from a playlist item, skipping `derive_visible_queue()`.
    /// The transport bar polls several times a second and only ever wants the
    /// item under the cursor — deriving the whole queue for that is waste.
    pub(crate) fn from_cursor_item(item: &PlaylistItem, state: PlaybackState) -> Self {
        let status = match (&item.load_state, state) {
            (LoadState::Failed(_), _) => EntryStatus::Failed,
            (LoadState::Downloading { .. }, _) => EntryStatus::Downloading,
            (_, PlaybackState::Playing) => EntryStatus::Playing,
            (_, PlaybackState::Paused) => EntryStatus::Playing,
            (_, PlaybackState::Stopped) => EntryStatus::Queued,
        };
        let download_progress = match &item.load_state {
            LoadState::Downloading {
                total,
                bytes_written,
                ..
            } if *total > 0 => {
                let done = bytes_written.load(std::sync::atomic::Ordering::Relaxed);
                Some((done as f64 / *total as f64).clamp(0.0, 1.0))
            }
            _ => None,
        };
        Self {
            queue_item_id: item.id.0.to_string(),
            track_id: item.db_id,
            title: item.title.clone(),
            artist: item.artist.clone(),
            album_artist: item.album_artist.clone(),
            album: item.album.clone(),
            year: item.year.clone(),
            codec: item.codec.clone(),
            track_number: item.track_number,
            disc: item.disc,
            duration_ms: item.duration_ms,
            status,
            download_progress,
        }
    }
}

impl From<&QueueEntry> for QueueItem {
    fn from(e: &QueueEntry) -> Self {
        let download_progress = e.download_progress.and_then(|(done, total)| {
            (total > 0).then(|| (done as f64 / total as f64).clamp(0.0, 1.0))
        });
        Self {
            queue_item_id: e.id.0.to_string(),
            track_id: e.db_id,
            title: e.title.clone(),
            artist: e.artist.clone(),
            album_artist: e.album_artist.clone(),
            album: e.album.clone(),
            year: e.year.clone(),
            codec: e.codec.clone(),
            track_number: e.track_number,
            disc: e.disc,
            duration_ms: e.duration_ms,
            status: e.status.into(),
            download_progress,
        }
    }
}

/// A created share link, and how much of the request it actually covers.
///
/// `skipped` is the point of this being a record rather than a bare string: a
/// selection mixing local-only files with server-backed ones produces a link
/// that is genuinely partial, and the UI has to be able to say so.
#[derive(uniffi::Record, Debug, Clone)]
pub struct Share {
    pub url: String,
    pub shared: u32,
    pub skipped: u32,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Stats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
}

impl From<LibraryStats> for Stats {
    fn from(s: LibraryStats) -> Self {
        Self {
            total_tracks: s.total_tracks,
            local_tracks: s.local_tracks,
            remote_tracks: s.remote_tracks,
            cached_tracks: s.cached_tracks,
            total_albums: s.total_albums,
            total_artists: s.total_artists,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Device {
    pub name: String,
    pub sample_rates: Vec<f64>,
}

/// Cover art as raw bytes. The GraphQL surface base64s this because JSON has to;
/// across FFI it stays binary and lands straight in an `NSImage`.
#[derive(uniffi::Record, Debug, Clone)]
pub struct CoverArt {
    pub data: Vec<u8>,
    pub mime: String,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct LyricLine {
    pub time_secs: f64,
    pub text: String,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Lyrics {
    pub content: String,
    /// LRC with timestamps, as opposed to a plain text dump.
    pub synced: bool,
    pub source: String,
    /// Parsed LRC lines, empty when unsynced. Parsing lives here so clients
    /// don't each reimplement the timestamp format.
    pub lines: Vec<LyricLine>,
}

impl From<koan_core::lyrics::Lyrics> for Lyrics {
    fn from(l: koan_core::lyrics::Lyrics) -> Self {
        let lines = if l.synced {
            koan_core::lyrics::parse_lrc(&l.content)
                .into_iter()
                .map(|line| LyricLine {
                    time_secs: line.time_secs,
                    text: line.text,
                })
                .collect()
        } else {
            Vec::new()
        };
        // `LyricsSource::as_str` is private to koan-core, so name them here.
        let source = match l.source {
            koan_core::lyrics::LyricsSource::Embedded => "embedded",
            koan_core::lyrics::LyricsSource::Sidecar => "sidecar",
            koan_core::lyrics::LyricsSource::Lrclib => "lrclib",
            koan_core::lyrics::LyricsSource::Cache => "cache",
        };
        Self {
            content: l.content,
            synced: l.synced,
            source: source.into(),
            lines,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Snapshot {
    pub name: String,
    pub track_count: u32,
    pub position_ms: u64,
    pub created_at: String,
}

impl From<queries::QueueSnapshotSummary> for Snapshot {
    fn from(s: queries::QueueSnapshotSummary) -> Self {
        Self {
            name: s.name,
            track_count: s.track_count as u32,
            position_ms: s.position_ms,
            created_at: s.created_at,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct ScanSummary {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub skipped: u32,
    pub errors: Vec<String>,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct SyncSummary {
    pub artists: u32,
    pub albums: u32,
    pub tracks: u32,
    /// Non-zero means the run was incomplete and the next one will retry those
    /// albums — worth saying so rather than reporting a clean sync.
    pub albums_failed: u32,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct SimilarArtist {
    pub artist_id: i64,
    pub name: String,
    pub score: f64,
    pub source: String,
}

/// Sort orders the library browser offers. Applied after the DB read, so it
/// works uniformly across every listing regardless of which query produced it.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumSort {
    /// Newest first. What a library browser should open on — the thing you
    /// just added is the thing you want.
    RecentlyAdded,
    Title,
    Artist,
    Year,
    /// Reshuffled on every call, so asking again gives a different order —
    /// which is the point: it's for turning up records you'd forgotten.
    Random,
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackSort {
    /// Disc, then track number — album running order.
    Album,
    Title,
    Artist,
    Duration,
}

#[derive(uniffi::Error, Debug, thiserror::Error)]
pub enum KoanError {
    #[error("database: {message}")]
    Database { message: String },
    #[error("audio device: {message}")]
    Audio { message: String },
    #[error("player is not accepting commands: {message}")]
    Player { message: String },
    #[error("not found: {message}")]
    NotFound { message: String },
    #[error("bad argument: {message}")]
    BadArgument { message: String },
}

pub(crate) fn year_of(date: &str) -> Option<i32> {
    date.get(..4).and_then(|s| s.parse().ok())
}

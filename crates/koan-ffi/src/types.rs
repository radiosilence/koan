//! Plain-data mirrors of koan-core types, shaped for the FFI boundary.
//!
//! uniffi needs owned records with concrete field types — no lifetimes, no
//! `PathBuf`, no `Arc`. These conversions are the only place that translation
//! happens; everything above and below speaks its own native vocabulary.

use std::collections::HashMap;

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
    pub album_count: i64,
    pub track_count: i64,
}

impl From<ArtistRow> for Artist {
    fn from(r: ArtistRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            album_count: r.album_count,
            track_count: r.track_count,
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
    /// The server knows about this track. Independent of `on_disk`: a record
    /// held both locally and on the server is one row that is both, and a UI
    /// that treats source as a single value cannot say so.
    pub on_server: bool,
    /// The bytes are on this machine — an indexed file or a finished download.
    pub on_disk: bool,
    /// Present once the file exists on disk, locally or in the cache.
    pub path: Option<String>,
    pub is_favourite: bool,
}

/// One play, with the track it played.
#[derive(uniffi::Record, Debug, Clone)]
pub struct PlayHistoryEntry {
    /// Identifies this play, not the track — the same track played twice is
    /// two entries with two ids.
    pub id: i64,
    pub track: Track,
    /// Unix seconds.
    pub played_at: i64,
    /// How long the track was listened to, where that was recorded.
    pub listened_ms: Option<i64>,
    /// `local` for koan's own playback, `subsonic` for a client scrobbling in.
    pub source: String,
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
            on_server: r.remote_id.is_some(),
            on_disk: path.is_some(),
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
    /// What the output device settled at, where it is known. Equal to
    /// `sample_rate` means koan handed the device the samples as they are;
    /// anything else means something resampled to reach it. What happens
    /// past the device — other clients, the volume stage — is the system's,
    /// and this says nothing about it.
    pub output_sample_rate: Option<u32>,
}

impl StreamFormat {
    pub(crate) fn of(t: &TrackInfo, output_sample_rate: Option<u32>) -> Self {
        Self {
            output_sample_rate,
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
    /// The record this came off, where the library still knows. Carried so a
    /// client can draw one sleeve per album rather than asking for artwork once
    /// per track and fetching the same image a dozen times.
    pub album_id: Option<i64>,
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
    /// The playlist row this came from, when it came from one. Survives the
    /// queue being shuffled or cut about — the queue is a view onto the
    /// playlist, not a copy of it.
    pub playlist_entry_id: Option<i64>,
    /// 0.0–1.0 while downloading, `None` otherwise.
    pub download_progress: Option<f64>,
    /// Why this item cannot play, when `status` is `Failed`.
    pub failure_reason: Option<String>,
}

/// How far one in-flight download has got.
///
/// Its own record rather than a queue refetch: progress moves several times a
/// second and the queue does not, so the two travel separately.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct DownloadProgress {
    pub queue_item_id: String,
    /// 0.0–1.0, or `None` when the server sent no Content-Length.
    pub progress: Option<f64>,
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
            playlist_entry_id: item.playlist_entry_id,
            // The transport polls this and there is no connection here to ask.
            // `PlayerModel` resolves the album for what is playing on its own.
            album_id: None,
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
            failure_reason: match &item.load_state {
                LoadState::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
        }
    }
}

impl QueueItem {
    /// Build from a derived queue entry, taking album IDs from a map resolved
    /// for the whole queue in one query — one statement per queue read rather
    /// than one per row.
    pub(crate) fn from_entry(e: &QueueEntry, album_ids: &HashMap<i64, i64>) -> Self {
        let download_progress = e.download_progress.and_then(|(done, total)| {
            (total > 0).then(|| (done as f64 / total as f64).clamp(0.0, 1.0))
        });
        Self {
            queue_item_id: e.id.0.to_string(),
            track_id: e.db_id,
            playlist_entry_id: e.playlist_entry_id,
            album_id: e.db_id.and_then(|id| album_ids.get(&id).copied()),
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
            failure_reason: e.error.clone(),
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
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub comment: Option<String>,
    /// Set once the playlist exists on the server. Its absence is what tells a
    /// client the playlist is local-only.
    pub remote_id: Option<String>,
    pub public: bool,
    pub owner: Option<String>,
    pub track_count: u32,
    pub duration_ms: i64,
    pub created_at: String,
    pub changed_at: String,
    /// How this machine likes to look at it. `None` follows the app default.
    pub grouped: Option<bool>,
}

impl From<queries::PlaylistRow> for Playlist {
    fn from(p: queries::PlaylistRow) -> Self {
        Self {
            id: p.id,
            name: p.name,
            comment: p.comment,
            remote_id: p.remote_id,
            public: p.public,
            owner: p.owner,
            track_count: p.track_count as u32,
            duration_ms: p.duration_ms,
            created_at: p.created_at,
            changed_at: p.changed_at,
            grouped: p.grouped,
        }
    }
}

/// One row of a playlist: the track, and the entry it sits in.
///
/// The id is the entry's. It is what a queue item remembers, so a client can
/// tell which of two copies of a song is the one playing.
#[derive(uniffi::Record, Debug, Clone)]
pub struct PlaylistEntry {
    pub id: i64,
    pub track: Track,
}

/// What writing a playlist out to a file managed.
#[derive(uniffi::Record, Debug, Clone)]
pub struct PlaylistExport {
    pub written: u32,
    /// Tracks with no file on this machine — a playlist file is a list of
    /// paths, and an undownloaded remote track has none.
    pub skipped: u32,
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
    pub favourites_pushed: u32,
    pub favourites_imported: u32,
    pub playlists_pulled: u32,
    pub playlists_pushed: u32,
}

/// A named pattern from `[organize.patterns]`.
#[derive(uniffi::Record, Debug, Clone)]
pub struct OrganizePattern {
    pub name: String,
    pub pattern: String,
    /// The one `[organize] default` names. Preselected when the sheet opens.
    pub is_default: bool,
}

/// What the pattern means for one file.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOutcome {
    /// Will be moved, or was.
    Move,
    /// Already exactly where the pattern puts it.
    Unchanged,
    /// Something holds the destination. Nothing is ever overwritten, so this
    /// file stays where it is.
    Conflict,
    /// The pattern produced nothing usable, or the move failed.
    Error,
}

/// One file's row in the plan: where it is, where the pattern puts it, and
/// whether that can happen.
#[derive(uniffi::Record, Debug, Clone)]
pub struct OrganizeEntry {
    /// `None` for a file the library holds no row for.
    pub track_id: Option<i64>,
    pub from_path: String,
    /// `None` only when the pattern failed before producing a path at all.
    pub to_path: Option<String>,
    pub outcome: PlanOutcome,
    /// Why this file isn't moving. `None` when it is.
    pub reason: Option<String>,
    /// Names of the cover art, cue sheets and logs travelling with this file.
    /// Named rather than counted: "+1 file" tells you something is coming
    /// along without telling you whether you want it to.
    pub ancillary: Vec<String>,
}

/// Every selected file and what happens to it, in plan order.
///
/// Preview and execute answer in the same shape, so the table the user
/// confirmed is the table that reports what happened.
#[derive(uniffi::Record, Debug, Clone)]
pub struct OrganizePlan {
    pub entries: Vec<OrganizeEntry>,
    pub moved_count: u32,
    pub unchanged_count: u32,
    pub conflict_count: u32,
    pub error_count: u32,
    /// Selected tracks with no local file to move — remote-only, or gone from
    /// disk. Counted so a selection of 20 yielding 12 rows says why.
    pub unresolved: u32,
}

impl OrganizePlan {
    /// Build from a core result. `requested` is how many tracks were asked
    /// for, when the caller named a set; the shortfall is what never resolved
    /// to a local file.
    pub(crate) fn build(
        result: koan_core::organize::OrganizeResult,
        requested: Option<usize>,
    ) -> Self {
        use koan_core::organize::PlanOutcome as Core;
        let conflict_count = result.conflicts().count() as u32;
        Self {
            moved_count: result.moved_count() as u32,
            unchanged_count: result.unchanged_count() as u32,
            conflict_count,
            error_count: result.failures().count() as u32 - conflict_count,
            unresolved: requested
                .map(|n| n.saturating_sub(result.entries.len()) as u32)
                .unwrap_or(0),
            entries: result
                .entries
                .into_iter()
                .map(|e| OrganizeEntry {
                    track_id: e.track_id,
                    from_path: e.from.to_string_lossy().into_owned(),
                    to_path: e.to.map(|t| t.to_string_lossy().into_owned()),
                    reason: e.outcome.reason().map(str::to_owned),
                    ancillary: e
                        .ancillary
                        .iter()
                        .map(|(from, _)| {
                            from.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })
                        .collect(),
                    outcome: match e.outcome {
                        Core::Move => PlanOutcome::Move,
                        Core::Unchanged => PlanOutcome::Unchanged,
                        Core::Conflict(_) => PlanOutcome::Conflict,
                        Core::Error(_) => PlanOutcome::Error,
                    },
                })
                .collect(),
        }
    }
}

/// What importing files from outside the library produced.
#[derive(uniffi::Record, Debug, Clone)]
pub struct ImportSummary {
    /// Library rows for the imported files, in walk order — what the caller
    /// queues.
    pub track_ids: Vec<i64>,
    pub added: u32,
    pub updated: u32,
    pub errors: Vec<String>,
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
    /// The server could not be reached, or gave up part way. Distinct from a
    /// server that answered and said no — this one is worth retrying.
    #[error("remote: {message}")]
    Remote { message: String },
}

pub(crate) fn year_of(date: &str) -> Option<i32> {
    date.get(..4).and_then(|s| s.parse().ok())
}

/// Everything the settings window reads and writes.
///
/// One record rather than a getter per field: the window shows the whole
/// configuration at once, and a single read keeps it consistent with itself.
/// The remote password is deliberately absent — it lives in the platform
/// credential store and is written through `sign_in_remote`, never read back.
#[derive(uniffi::Record, Debug, Clone)]
pub struct Settings {
    /// Folders and how many tracks each accounts for — a folder is easier to
    /// judge by what it contributed than by its path.
    pub library_folders: Vec<LibraryFolder>,

    pub remote_enabled: bool,
    pub remote_url: String,
    pub remote_username: String,
    /// A password is stored for this server. Not the password.
    pub remote_signed_in: bool,
    /// Tracks the server accounts for.
    pub remote_tracks: u64,
    pub download_workers: u32,
    /// Human-readable, e.g. "50GB". Empty means unlimited.
    pub cache_limit: String,
    pub cache_dir: String,
    pub cache_bytes: u64,
    pub auto_sync: bool,
    pub auto_sync_interval_mins: u64,

    /// `off`, `track` or `album`.
    pub replaygain: String,
    pub pre_amp_db: f64,

    pub radio_lookahead: u32,
    pub radio_batch_size: u32,
    pub radio_discovery_weight: f64,
}

/// A scanned folder, and what it contributed.
#[derive(uniffi::Record, Debug, Clone)]
pub struct LibraryFolder {
    pub path: String,
    pub tracks: u64,
}

/// What a library rebuild removed.
#[derive(uniffi::Record, Debug, Clone)]
pub struct RebuildSummary {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

/// What clearing the download cache removed.
#[derive(uniffi::Record, Debug, Clone)]
pub struct CacheCleared {
    pub files: u64,
    pub bytes: u64,
}

/// The spectrum reduced to three numbers, for indicators that move with the
/// music without drawing it.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq)]
pub struct VizLevels {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

impl From<koan_core::audio::viz::VizLevels> for VizLevels {
    fn from(l: koan_core::audio::viz::VizLevels) -> Self {
        Self {
            low: l.low,
            mid: l.mid,
            high: l.high,
        }
    }
}

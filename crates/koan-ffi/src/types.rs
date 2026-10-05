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

#[derive(uniffi::Record, Debug, Clone, PartialEq)]
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
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
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
    /// What DSP is doing to the audio. `None` is the untouched path.
    pub dsp: Option<DspInfo>,
}

/// The output device's DSP profile, while one is running.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct DspInfo {
    pub profile: String,
    pub eq: bool,
    /// The rate the impulse response in use was designed at, which the output
    /// runs at. A source at another rate is resampled to reach it.
    pub convolution_rate: Option<u32>,
}

impl From<koan_core::audio::dsp::DspStatus> for DspInfo {
    fn from(d: koan_core::audio::dsp::DspStatus) -> Self {
        Self {
            profile: d.profile,
            eq: d.eq,
            convolution_rate: d.convolution_rate,
        }
    }
}

impl StreamFormat {
    pub(crate) fn of(
        t: &TrackInfo,
        output_sample_rate: Option<u32>,
        dsp: Option<koan_core::audio::dsp::DspStatus>,
    ) -> Self {
        Self {
            output_sample_rate,
            dsp: dsp.map(Into::into),
            codec: t.codec.clone(),
            sample_rate: t.sample_rate,
            bit_depth: t.bit_depth,
            bitrate_kbps: t.bitrate_kbps,
            channels: t.channels,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub state: PlayState,
    /// Waiting for the track under the cursor to arrive. With `Stopped` it
    /// opens playing, with `Paused` it opens paused.
    pub waiting: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Queue item currently under the cursor, if any.
    pub queue_item_id: Option<String>,
    pub entry: Option<QueueItem>,
    pub format: Option<StreamFormat>,
    /// Bumped on every queue mutation — cheap change detection for the UI.
    pub playlist_version: u64,
    /// Shuffle is on: the queue after the current track was reordered at
    /// random, and turning it off puts it back.
    pub shuffle: bool,
    pub repeat_mode: RepeatMode,
}

/// What follows a track at its end.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    /// The last track runs on into the first.
    Queue,
    /// The track plays again. Next and previous still move on.
    One,
}

impl From<koan_core::player::state::Repeat> for RepeatMode {
    fn from(r: koan_core::player::state::Repeat) -> Self {
        use koan_core::player::state::Repeat;
        match r {
            Repeat::Off => Self::Off,
            Repeat::Queue => Self::Queue,
            Repeat::One => Self::One,
        }
    }
}

impl From<RepeatMode> for koan_core::player::state::Repeat {
    fn from(r: RepeatMode) -> Self {
        match r {
            RepeatMode::Off => Self::Off,
            RepeatMode::Queue => Self::Queue,
            RepeatMode::One => Self::One,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone, PartialEq)]
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
    /// Why this item cannot play, when `status` is `Failed`.
    pub failure_reason: Option<String>,
    /// The server knows about this track. False for a queue item with no
    /// library row behind it, which has nowhere to have come from.
    pub on_server: bool,
    /// The bytes are on this machine — an indexed file or a finished download.
    pub on_disk: bool,
}

/// One transfer, as the download store has it — everything about it that does
/// not move while the bytes land.
///
/// The numbers are in `TransferFigure`, deliberately. A transfer appears,
/// changes state a handful of times and settles; its byte count moves ten times
/// a second for as long as it runs. Carrying both in one value would mean a
/// list rebuilding at the rate a download writes.
///
/// Identified by its track: a track is fetched once, however many queue entries
/// want it, and a queue row finds its transfer by the track it plays.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct Transfer {
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub state: TransferState,
    /// Why it stopped, when it failed.
    pub failure_reason: Option<String>,
}

/// What one transfer is doing right now.
///
/// The volatile half of a `Transfer`, split out so the two travel at their own
/// rates. Serves both the figure on a queue row and the row on the downloads
/// page — one reading of one fact, so they cannot disagree.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct TransferFigure {
    pub track_id: i64,
    /// 0.0–1.0, or `None` when the server sent no Content-Length — a bar drawn
    /// at zero for a transfer that is going fine reads as stuck.
    pub progress: Option<f64>,
    pub bytes_written: u64,
    pub total_bytes: u64,
    /// Smoothed. Zero for a transfer that has settled, and for one that has
    /// stopped moving — which is the case worth seeing.
    pub bytes_per_second: u64,
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running,
    Done,
    Failed,
}

impl TransferState {
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

impl Transfer {
    pub(crate) fn of(d: &koan_core::remote::downloads::Download) -> Self {
        use koan_core::remote::downloads::DownloadState;
        Self {
            track_id: d.track_id,
            title: d.title.clone(),
            artist: d.artist.clone(),
            state: match &d.state {
                DownloadState::Queued => TransferState::Queued,
                DownloadState::Running => TransferState::Running,
                DownloadState::Done => TransferState::Done,
                DownloadState::Failed(_) => TransferState::Failed,
            },
            failure_reason: match &d.state {
                DownloadState::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
        }
    }
}

impl TransferFigure {
    pub(crate) fn of(d: &koan_core::remote::downloads::Download) -> Self {
        Self {
            track_id: d.track_id,
            progress: d.fraction(),
            bytes_written: d.bytes_written(),
            total_bytes: d.total,
            bytes_per_second: d.bytes_per_second,
        }
    }

    pub(crate) fn reading(r: &koan_core::remote::downloads::Reading) -> Self {
        Self {
            track_id: r.track_id,
            progress: r.fraction(),
            bytes_written: r.written,
            total_bytes: r.total,
            bytes_per_second: r.bytes_per_second,
        }
    }
}

impl QueueItem {
    /// Build directly from a playlist item, skipping `derive_visible_queue()`.
    /// The state watcher builds this on every wake and only ever wants the
    /// item under the cursor — deriving the whole queue for that is waste.
    pub(crate) fn from_cursor_item(
        item: &PlaylistItem,
        state: PlaybackState,
        downloads: &koan_core::remote::downloads::DownloadStore,
    ) -> Self {
        // The item's own state and any transfer against it, as one answer.
        let load = LoadState::of(item, downloads);
        // The queue row's mapping, so the transport and the row agree; a
        // playable track the player is not on yet is merely queued.
        let transferring = matches!(load, LoadState::Downloading { .. });
        let status = match QueueEntryStatus::at_cursor(&item.state, transferring) {
            QueueEntryStatus::Playing if state == PlaybackState::Stopped => EntryStatus::Queued,
            status => status.into(),
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
            failure_reason: match &load {
                LoadState::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
            // The transport polls this and there is no connection here to ask.
            // Nothing it draws needs them; the queue's own rows carry the real
            // reading.
            on_server: false,
            on_disk: false,
        }
    }
}

impl QueueItem {
    /// Build from a derived queue entry, taking album IDs from a map resolved
    /// for the whole queue in one query — one statement per queue read rather
    /// than one per row.
    pub(crate) fn from_entry(
        e: &QueueEntry,
        album_ids: &HashMap<i64, i64>,
        sources: &HashMap<i64, (bool, bool)>,
    ) -> Self {
        let (on_server, on_disk) = e
            .db_id
            .and_then(|id| sources.get(&id))
            .copied()
            .unwrap_or((false, false));
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
            failure_reason: e.error.clone(),
            on_server,
            on_disk,
        }
    }
}

/// A record and its tracks, as one answer.
///
/// The page wants both and wants them together, so they are one call: one hop
/// across the boundary, one connection out of the pool, one lock taken.
#[derive(uniffi::Record, Debug, Clone)]
pub struct AlbumPage {
    pub album: Option<Album>,
    pub tracks: Vec<Track>,
}

/// A created share link, and how much of the request it covers.
///
/// `skipped` is the point of this being a record rather than a bare string: a
/// selection mixing local-only files with server-backed ones produces a link
/// that is partial, and the UI has to be able to say so.
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
    /// How it is connected: `builtin`, `usb`, `bluetooth`, `airplay`,
    /// `display`, `virtual` or `other`. For its icon.
    pub kind: String,
}

/// Cover art as raw bytes. The GraphQL surface base64s this because JSON has to;
/// across FFI it stays binary.
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

#[derive(uniffi::Record, Debug, Clone, PartialEq)]
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
    /// Its contents are not for editing: rules or a playlist file decide
    /// them, here or on the server. Adds, removals and reorders are refused.
    pub readonly: bool,
    /// Rules here decide its contents (a smart playlist on this machine,
    /// rather than one mirrored from a server).
    pub smart: bool,
    /// Read from a file in the library, which decides its name and contents.
    pub from_file: bool,
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
            readonly: p.readonly,
            smart: p.rules.is_some(),
            from_file: p.source_path.is_some(),
        }
    }
}

/// What the queue still is, when it is still something.
///
/// A queue that came from a playlist or a record and has not been touched since
/// is still that thing, and saying so is what makes following legible — you can
/// see why an edit to the playlist moved something in the queue, and you can
/// see the moment it stops.
#[derive(uniffi::Enum, Debug, Clone, PartialEq)]
pub enum QueueLock {
    Playlist { playlist: Playlist },
    Album { album: Album },
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
    /// Pages of tracks that could not be fetched. Non-zero means the same.
    pub pages_failed: u32,
    pub favourites_pushed: u32,
    pub favourites_imported: u32,
    pub playlists_pulled: u32,
    pub playlists_pushed: u32,
}

/// Which part of a remote sync is running.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPhase {
    /// Paging through the server's album list. `done` counts albums.
    Albums,
    /// Fetching and writing tracks. `done` counts tracks.
    Tracks,
    /// Recording artist metadata. `total` is the number of artists.
    Artists,
    /// Relinking and tidying up. No counts.
    Finishing,
}

/// How far a remote sync has got. `total` is absent where the server gives no
/// way to know it in advance.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncProgress {
    pub phase: SyncPhase,
    pub done: u64,
    pub total: Option<u64>,
}

impl From<koan_core::remote::sync::SyncProgress> for SyncProgress {
    fn from(p: koan_core::remote::sync::SyncProgress) -> Self {
        use koan_core::remote::sync::SyncPhase as Core;
        Self {
            phase: match p.phase {
                Core::Albums => SyncPhase::Albums,
                Core::Tracks => SyncPhase::Tracks,
                Core::Artists => SyncPhase::Artists,
                Core::Finishing => SyncPhase::Finishing,
            },
            done: p.done,
            total: p.total,
        }
    }
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

/// An artist beyond the library: the opening of their Wikipedia article and
/// whether there is a photograph to ask for.
#[derive(uniffi::Record, Debug, Clone)]
pub struct ArtistInfo {
    pub bio: Option<String>,
    /// The article the biography opens, for reading on and for credit.
    pub bio_url: Option<String>,
    pub has_image: bool,
    /// Photographer and licence.
    pub image_credit: Option<String>,
}

impl From<koan_core::artist_info::ArtistInfo> for ArtistInfo {
    fn from(info: koan_core::artist_info::ArtistInfo) -> Self {
        Self {
            bio: info.bio,
            bio_url: info.bio_url,
            has_image: info.image_url.is_some(),
            image_credit: info.image_credit,
        }
    }
}

/// Sort orders the library browser offers. Applied by the database, because a
/// listing that is read a page at a time has to be ordered before it is cut.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumSort {
    /// Newest first. What a library browser should open on — the thing you
    /// just added is the thing you want.
    RecentlyAdded,
    Title,
    Artist,
    Year,
    /// Shuffled by the seed passed alongside it. The same seed is the same
    /// order, page after page; a new seed is a new order — which is the point,
    /// it's for turning up records you'd forgotten.
    Random,
}

/// Narrowing the album and artist browsers by what the records are. The web
/// UI's filters, applied by the same queries: an artist passes when any of
/// their albums does.
#[derive(uniffi::Record, Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowseFilter {
    pub favourites: bool,
    /// Only records in a lossless codec.
    pub lossless: bool,
    /// Only records in this codec, as `BrowseChoices::codecs` names it.
    pub codec: Option<String>,
    /// Release year bounds, inclusive. Records without a date are left out
    /// when either is set.
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub genre: Option<String>,
}

/// What the codec and genre filters offer: the codecs records are in, most
/// common first, and the genres most records carry.
#[derive(uniffi::Record, Debug, Clone)]
pub struct BrowseChoices {
    pub codecs: Vec<String>,
    pub genres: Vec<String>,
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
    /// An import of coefficients that say nothing of their rate. Ask, and
    /// import again with one.
    #[error("{message}")]
    NeedsSampleRate { message: String },
}

/// The DSP profiles, and which the current output plays through.
#[derive(uniffi::Record, Debug, Clone)]
pub struct DspOverview {
    /// Off bypasses every profile.
    pub enabled: bool,
    /// The output device playback goes to, which profiles are chosen by. A
    /// renderer is named by its UDN.
    pub device: Option<String>,
    pub active: Option<String>,
    pub profiles: Vec<DspProfileSummary>,
    /// What to call the devices named by a UDN, where the renderer is known.
    pub names: std::collections::HashMap<String, String>,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct DspProfileSummary {
    pub name: String,
    pub devices: Vec<String>,
    pub bands: u32,
    /// Rates there are impulse responses for.
    pub rates: Vec<u32>,
    /// Why it would not load, if it would not.
    pub problem: Option<String>,
}

/// Everything in one profile, for its page in Settings.
#[derive(uniffi::Record, Debug, Clone)]
pub struct DspProfileDetail {
    pub name: String,
    pub devices: Vec<String>,
    /// The files it was imported from.
    pub source: Vec<String>,
    pub bands: Vec<DspBand>,
    pub impulses: Vec<DspImpulse>,
    /// Gain ahead of the filters at `preamp_rate`, derived unless `preamp_set`.
    pub preamp_db: f64,
    pub preamp_rate: u32,
    pub preamp_set: bool,
    pub problem: Option<String>,
}

/// One of a profile's filters, in the order they run.
#[derive(uniffi::Record, Debug, Clone)]
pub struct DspBand {
    /// A band — `peaking`, `low_shelf`, `high_shelf`, `low_pass`,
    /// `high_pass`, `notch`, `band_pass`, `all_pass`, `gain`, or one of the
    /// shelves and passes with `_first_order` — or `delay`, `mix` or `graphic`.
    pub kind: String,
    pub freq: f64,
    pub gain_db: f64,
    pub q: f64,
    /// From 0. Empty is every channel.
    pub channels: Vec<u16>,
    /// A delay's length: `delay_ms` and `delay_samples` added together.
    pub delay_ms: f64,
    pub delay_samples: f64,
    /// A mix's outputs, from channel 0.
    pub mix: Vec<DspMixOutput>,
    /// A graphic EQ's points.
    pub curve: Vec<DspPoint>,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct DspMixOutput {
    /// `(channel, linear gain)` summed into this output. Empty is silence.
    pub sources: Vec<DspMixSource>,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct DspMixSource {
    pub channel: u16,
    pub gain: f64,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct DspPoint {
    pub hz: f64,
    pub db: f64,
}

impl From<koan_core::config::DspFilter> for DspBand {
    fn from(f: koan_core::config::DspFilter) -> Self {
        use koan_core::config::DspFilter;
        let mut band = Self {
            kind: String::new(),
            freq: 0.0,
            gain_db: 0.0,
            q: 0.0,
            channels: f.channels().to_vec(),
            delay_ms: 0.0,
            delay_samples: 0.0,
            mix: Vec::new(),
            curve: Vec::new(),
        };
        match f {
            DspFilter::Band(b) => {
                band.kind = b.kind.name().to_string();
                band.freq = b.freq;
                band.gain_db = b.gain_db;
                band.q = b.q;
            }
            DspFilter::Delay(d) => {
                band.kind = "delay".into();
                band.delay_ms = d.ms;
                band.delay_samples = d.samples;
            }
            DspFilter::Mix(m) => {
                band.kind = "mix".into();
                band.mix = m
                    .outputs
                    .into_iter()
                    .map(|row| DspMixOutput {
                        sources: row
                            .into_iter()
                            .map(|(channel, gain)| DspMixSource { channel, gain })
                            .collect(),
                    })
                    .collect();
            }
            DspFilter::Graphic(g) => {
                band.kind = "graphic".into();
                band.curve = g
                    .points
                    .into_iter()
                    .map(|(hz, db)| DspPoint { hz, db })
                    .collect();
            }
        }
        band
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct DspImpulse {
    pub file: String,
    pub rate: u32,
    /// `None` for one response applied to every channel.
    pub channels: Option<u32>,
    pub taps: u32,
    pub routes: u32,
    /// Feeds channels into each other, as crossfeed does.
    pub mixes: bool,
    /// Delays channels against each other.
    pub delayed: bool,
    /// Where the response peaks: the delay trimmed from playback.
    pub peak_ms: f64,
}

impl From<koan_core::audio::dsp::profiles::Detail> for DspProfileDetail {
    fn from(d: koan_core::audio::dsp::profiles::Detail) -> Self {
        Self {
            name: d.name,
            devices: d.devices,
            source: d.source,
            bands: d.filters.into_iter().map(DspBand::from).collect(),
            impulses: d
                .impulses
                .into_iter()
                .map(|i| DspImpulse {
                    file: i.file,
                    rate: i.rate,
                    channels: i.channels.map(|c| c as u32),
                    taps: i.taps as u32,
                    routes: i.routes as u32,
                    mixes: i.mixes,
                    delayed: i.delayed,
                    peak_ms: i.peak_ms,
                })
                .collect(),
            preamp_db: d.preamp_db,
            preamp_rate: d.preamp_rate,
            preamp_set: d.preamp_set,
            problem: d.problem,
        }
    }
}

impl From<koan_core::audio::dsp::profiles::Overview> for DspOverview {
    fn from(o: koan_core::audio::dsp::profiles::Overview) -> Self {
        Self {
            enabled: o.enabled,
            names: Default::default(),
            device: o.device,
            active: o.active,
            profiles: o
                .profiles
                .into_iter()
                .map(|p| DspProfileSummary {
                    name: p.name,
                    devices: p.devices,
                    bands: p.bands as u32,
                    rates: p.rates,
                    problem: p.problem,
                })
                .collect(),
        }
    }
}

pub(crate) fn year_of(date: &str) -> Option<i32> {
    date.get(..4).and_then(|s| s.parse().ok())
}

/// Everything the settings window reads and writes.
///
/// One record rather than a getter per field: the window shows the whole
/// configuration at once, and a single read keeps it consistent with itself.
/// The remote password is deliberately absent — it lives in config.local.toml
/// and is written through `sign_in_remote`, never read back.
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
    pub fade_on_pause: bool,

    /// Open to control from any koan app on the local network.
    pub devices_discoverable: bool,
    /// Devices to reach by address where Bonjour does not: `host:port`.
    pub devices_addresses: Vec<String>,
    /// What devices on the local network may do with this one: `full`
    /// (playback, outputs, presets, volume, hand-off) or `playback` (play and
    /// the queue only).
    pub devices_nearby_control: String,
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

/// See `KoanEngine::art_evictions`.
#[derive(uniffi::Record, Debug, Clone, Default)]
pub struct ArtEvictions {
    /// Pass back as `after` next time.
    pub seq: i64,
    pub albums: Vec<i64>,
    pub artists: Vec<i64>,
    pub tracks: Vec<i64>,
}

/// An account on a server, as one link. What a tapped invite carries and what
/// an admin sends: `link` for koan, and the password for any other Subsonic
/// app when the account was just made or given a new one.
#[derive(uniffi::Record, Debug, Clone)]
pub struct Invite {
    pub server: String,
    pub username: String,
    /// What koan trades for an API key; absent from a pasted address with the
    /// account in it.
    pub token: Option<String>,
    pub password: Option<String>,
    pub link: String,
    pub email_subject: String,
    pub email_text: String,
    pub email_html: String,
    pub mailto: String,
}

impl From<koan_core::invite::Invite> for Invite {
    fn from(i: koan_core::invite::Invite) -> Self {
        Self {
            link: i.link(),
            email_subject: i.email_subject(),
            email_text: i.email_text(),
            email_html: i.email_html(),
            mailto: i.mailto(),
            server: i.server,
            username: i.username,
            token: i.token,
            password: i.password,
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountRole {
    /// Listens.
    Readonly,
    /// Also edits playlists and favourites.
    User,
    /// Also manages the server and its accounts.
    Admin,
}

impl AccountRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Readonly => "readonly",
            Self::User => "user",
            Self::Admin => "admin",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "admin" => Self::Admin,
            "user" => Self::User,
            _ => Self::Readonly,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct ServerAccount {
    pub username: String,
    pub role: AccountRole,
}

/// Another device koan can play on: one on the same account, or one on the
/// local network.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// `ios`, `macos` or `linux`.
    pub platform: String,
    /// Signed in to the same account: reachable from anywhere, through the
    /// server.
    pub account: bool,
    /// The account that owns it, for a device shared with this one.
    pub owner: Option<String>,
    /// Found on the local network.
    pub nearby: bool,
    /// Reachable at once. False for a phone iOS has suspended, which a
    /// command wakes and music reaches as a notification to tap.
    pub awake: bool,
    /// Not heard from for longer than a heartbeat.
    pub asleep: bool,
    /// Can be woken from asleep. An asleep device that cannot is shown and
    /// cannot be chosen.
    pub wakeable: bool,
    /// Unix seconds when it was last reachable.
    pub last_seen: Option<i64>,
    /// While it is being woken, the stage: `network`, `push` or
    /// `notification`.
    pub waking: Option<String>,
    /// Why the last attempt to wake it failed.
    pub wake_failed: Option<String>,
    /// Plays from the same library, so music can be handed between the two.
    pub same_library: bool,
    pub state: PlayState,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// This library's row for what it is playing, when it has one: for its
    /// artwork and its heart.
    pub track_id: Option<i64>,
    pub album_id: Option<i64>,
    /// As reported; the client runs it on from arrival while `state` is
    /// playing.
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Why it cannot be reached, for a device found but not connected or
    /// the one being controlled while it is out of reach.
    pub problem: Option<String>,
}

/// A UPnP renderer on the network: an amplifier or streamer that plays a
/// file it is handed a URL to.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct RendererInfo {
    pub udn: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    /// Takes the next track before this one ends, so albums play without gaps.
    pub gapless: bool,
    /// Playing or paused when last asked, by whatever drives it. Picking it
    /// takes it over.
    pub busy: bool,
}

/// What the device in view plays through: this one's outputs, or those of the
/// device it controls, as that device published them.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct OutputsInfo {
    /// The device they belong to, by name, when it is another one.
    pub owner: Option<String>,
    /// Its own audio devices: on a phone, the route the system chose.
    pub devices: Vec<OutputInfo>,
    pub renderers: Vec<OutputInfo>,
    pub current: OutputChoice,
    /// The volume of the renderer it plays to, when it has one.
    pub volume: Option<u8>,
    /// Its DSP profiles, and whether processing is on there.
    pub profiles: Vec<String>,
    pub dsp_enabled: bool,
}

/// One output: an audio device by name, or a renderer by UDN.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct OutputInfo {
    pub id: String,
    pub name: String,
    /// How it is connected: `usb`, `bluetooth`, `upnp` and the rest.
    pub kind: String,
    /// A renderer's make and model.
    pub detail: String,
    /// Playing or paused for something else.
    pub busy: bool,
    pub preset: Option<String>,
}

/// An output to play through.
#[derive(uniffi::Enum, Debug, Clone, PartialEq)]
pub enum OutputChoice {
    /// The system's default output.
    Default,
    Device {
        name: String,
    },
    Renderer {
        udn: String,
    },
}

impl From<koan_core::remote::outputs::OutputChoice> for OutputChoice {
    fn from(c: koan_core::remote::outputs::OutputChoice) -> Self {
        use koan_core::remote::outputs::OutputChoice as C;
        match c {
            C::Default => Self::Default,
            C::Device { name } => Self::Device { name },
            C::Renderer { udn } => Self::Renderer { udn },
        }
    }
}

impl From<OutputChoice> for koan_core::remote::outputs::OutputChoice {
    fn from(c: OutputChoice) -> Self {
        match c {
            OutputChoice::Default => Self::Default,
            OutputChoice::Device { name } => Self::Device { name },
            OutputChoice::Renderer { udn } => Self::Renderer { udn },
        }
    }
}

impl OutputsInfo {
    pub(crate) fn of(owner: Option<String>, o: koan_core::remote::outputs::LinkOutputs) -> Self {
        let output = |o: koan_core::remote::outputs::LinkOutput| OutputInfo {
            id: o.id,
            name: o.name,
            kind: o.kind,
            detail: o.detail,
            busy: o.busy,
            preset: o.preset,
        };
        Self {
            owner,
            devices: o.devices.into_iter().map(output).collect(),
            renderers: o.renderers.into_iter().map(output).collect(),
            current: o.current.into(),
            volume: o.volume,
            profiles: o.profiles,
            dsp_enabled: o.dsp_enabled,
        }
    }
}

/// The renderer this koan is playing to in place of its own output.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct RendererOutput {
    pub udn: String,
    pub name: String,
    /// 0–100, when the renderer has a volume control.
    pub volume: Option<u8>,
    /// Why the last track was skipped there, until one plays.
    pub problem: Option<String>,
}

/// What the server this app signs in to turned out to be, and what it and
/// this device offer. Settings shows it.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct ConnectionInfo {
    /// OpenSubsonic's `type`: `koan`, `navidrome`. `None` from an older
    /// server, or before one has answered.
    pub server_kind: Option<String>,
    pub server_version: Option<String>,
    pub open_subsonic: bool,
    /// Each extension and the versions the server offers, e.g. `koanLink 1`.
    pub extensions: Vec<ServerExtension>,
    /// The server can pass commands between this account's devices.
    pub devices: bool,
    /// The link to the server is up.
    pub linked: bool,
    /// The port this device listens on for others on the network, while it
    /// is discoverable.
    pub listening_port: Option<u16>,
    /// iOS has not let this app onto the local network, so nothing there can
    /// be found or reached until the person allows it in Settings.
    pub local_network_blocked: bool,
    /// This device, as others see it.
    pub this_device: String,
    /// The server lets this device be shared with other accounts on it.
    pub sharing: bool,
    /// The accounts this device is shared with.
    pub shared_with: Vec<String>,
    /// Why the server refused the last change to them.
    pub share_error: Option<String>,
    /// The server's other accounts, to share with.
    pub share_accounts: Vec<String>,
}

#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct ServerExtension {
    pub name: String,
    pub versions: Vec<i64>,
}

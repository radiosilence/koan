use std::sync::Arc;

use async_graphql::connection::{DisableNodesField, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, InputObject, Object, SimpleObject};
use koan_core::db::queries;
use koan_core::player::state::{PlaybackState, QueueEntryStatus, SharedPlayerState};

use super::helpers::paginate;
use super::jobs::{Job, JobState};
use super::loaders::{
    AlbumStatsOf, AlbumTracks, ArtistAlbums, ArtistStatsOf, ArtistTracks, DbLoader, FavouritePath,
};

/// Connection type alias — standard async-graphql Connection with `nodes` field disabled.
/// Exposes `edges` + `pageInfo` only (proper Relay spec).
pub(super) type Conn<T> = async_graphql::connection::Connection<
    usize,
    T,
    EmptyFields,
    EmptyFields,
    async_graphql::connection::DefaultConnectionName,
    async_graphql::connection::DefaultEdgeName,
    DisableNodesField,
>;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum PlaybackStateEnum {
    Stopped,
    Playing,
    Paused,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum TrackSource {
    Local,
    Remote,
    Cached,
}

impl TrackSource {
    pub(super) fn as_db_value(self) -> &'static str {
        match self {
            TrackSource::Local => "local",
            TrackSource::Remote => "remote",
            TrackSource::Cached => "cached",
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum ArtistSortField {
    Name,
    TrackCount,
    AlbumCount,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum AlbumSortField {
    Title,
    Date,
    ArtistThenDate,
    TrackCount,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum TrackSortField {
    Title,
    Artist,
    Album,
    Duration,
    ArtistAlbumDiscTrack,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum SortDirection {
    Asc,
    Desc,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum FuzzySearchKind {
    Track,
    Album,
    Artist,
}

/// Queue entry status — mirrors `QueueEntryStatus` from koan-core.
/// Derived from cursor position + load state.
#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(super) enum GqlQueueEntryStatus {
    /// After the cursor — waiting to play.
    Queued,
    /// At the cursor and loaded — currently playing.
    Playing,
    /// Before the cursor — already played.
    Played,
    /// Downloading (not at cursor).
    Downloading,
    /// At the cursor but not yet loaded — priority pending.
    PriorityPending,
    /// Download or load failed.
    Failed,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

pub(super) struct GqlArtist {
    pub row: queries::ArtistRow,
}

#[Object(name = "Artist")]
impl GqlArtist {
    async fn id(&self) -> i64 {
        self.row.id
    }

    async fn name(&self) -> &str {
        &self.row.name
    }

    async fn albums(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Conn<GqlAlbum>> {
        let all = loader(ctx)?
            .load_one(ArtistAlbums(self.row.id))
            .await?
            .unwrap_or_default();
        paginate(
            all.into_iter().map(|row| GqlAlbum { row }).collect(),
            after,
            first,
        )
    }

    async fn tracks(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Conn<GqlTrack>> {
        let all = loader(ctx)?
            .load_one(ArtistTracks(self.row.id))
            .await?
            .unwrap_or_default();
        paginate(
            all.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn album_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let stats = loader(ctx)?
            .load_one(ArtistStatsOf(self.row.id))
            .await?
            .unwrap_or_default();
        Ok(stats.album_count as i32)
    }

    async fn track_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let stats = loader(ctx)?
            .load_one(ArtistStatsOf(self.row.id))
            .await?
            .unwrap_or_default();
        Ok(stats.track_count as i32)
    }
}

pub(super) struct GqlAlbum {
    pub row: queries::AlbumRow,
}

#[Object(name = "Album")]
impl GqlAlbum {
    async fn id(&self) -> i64 {
        self.row.id
    }

    async fn title(&self) -> &str {
        &self.row.title
    }

    async fn artist_id(&self) -> i64 {
        self.row.artist_id
    }

    async fn artist_name(&self) -> &str {
        &self.row.artist_name
    }

    async fn date(&self) -> Option<&str> {
        self.row.date.as_deref()
    }

    async fn codec(&self) -> Option<&str> {
        self.row.codec.as_deref()
    }

    async fn label(&self) -> Option<&str> {
        self.row.label.as_deref()
    }

    async fn disc_count(&self) -> Option<i32> {
        self.row.total_discs
    }

    async fn tracks(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Conn<GqlTrack>> {
        let all = loader(ctx)?
            .load_one(AlbumTracks(self.row.id))
            .await?
            .unwrap_or_default();
        paginate(
            all.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn track_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let stats = loader(ctx)?
            .load_one(AlbumStatsOf(self.row.id))
            .await?
            .unwrap_or_default();
        Ok(stats.track_count as i32)
    }

    async fn total_duration_ms(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        let stats = loader(ctx)?
            .load_one(AlbumStatsOf(self.row.id))
            .await?
            .unwrap_or_default();
        Ok(stats.total_duration_ms)
    }
}

pub(super) struct GqlTrack {
    pub row: queries::TrackRow,
}

#[Object(name = "Track")]
impl GqlTrack {
    async fn id(&self) -> i64 {
        self.row.id
    }

    async fn title(&self) -> &str {
        &self.row.title
    }

    async fn artist(&self) -> &str {
        &self.row.artist_name
    }

    async fn album_artist(&self) -> &str {
        &self.row.album_artist_name
    }

    async fn album(&self) -> &str {
        &self.row.album_title
    }

    async fn album_id(&self) -> Option<i64> {
        self.row.album_id
    }

    async fn artist_id(&self) -> Option<i64> {
        self.row.artist_id
    }

    async fn disc(&self) -> Option<i32> {
        self.row.disc
    }

    async fn track_number(&self) -> Option<i32> {
        self.row.track_number
    }

    async fn duration_ms(&self) -> Option<i64> {
        self.row.duration_ms
    }

    async fn codec(&self) -> Option<&str> {
        self.row.codec.as_deref()
    }

    async fn sample_rate(&self) -> Option<i32> {
        self.row.sample_rate
    }

    async fn bit_depth(&self) -> Option<i32> {
        self.row.bit_depth
    }

    async fn channels(&self) -> Option<i32> {
        self.row.channels
    }

    async fn bitrate(&self) -> Option<i32> {
        self.row.bitrate
    }

    async fn genre(&self) -> Option<&str> {
        self.row.genre.as_deref()
    }

    async fn source(&self) -> TrackSource {
        match self.row.source.as_str() {
            "local" => TrackSource::Local,
            "cached" => TrackSource::Cached,
            _ => TrackSource::Remote,
        }
    }

    async fn remote_id(&self) -> Option<&str> {
        self.row.remote_id.as_deref()
    }

    async fn path(&self) -> Option<&str> {
        self.row.path.as_deref()
    }

    async fn cached_path(&self) -> Option<&str> {
        self.row.cached_path.as_deref()
    }

    async fn is_favourite(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let Some(path) = self.row.path.as_ref().or(self.row.cached_path.as_ref()) else {
            return Ok(false);
        };
        Ok(loader(ctx)?
            .load_one(FavouritePath(path.clone()))
            .await?
            .unwrap_or(false))
    }
}

fn loader<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a DataLoader<DbLoader>> {
    ctx.data::<DataLoader<DbLoader>>()
}

#[derive(SimpleObject)]
#[graphql(name = "NowPlaying")]
pub(super) struct GqlNowPlaying {
    pub state: PlaybackStateEnum,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub track: Option<GqlNowPlayingTrack>,
    pub queue_item_id: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(name = "NowPlayingTrack")]
pub(super) struct GqlNowPlayingTrack {
    /// Library row id, when the queue entry came from the database. The remote
    /// bridge streams `/rest/stream?id=<trackId>`; without it a client had no
    /// way to name the track the server is playing.
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<u32>,
    pub channels: u16,
    pub duration_ms: u64,
    /// What the output device settled at, where it is known. Equal to
    /// `sampleRate` means koan handed the device the samples as they are;
    /// anything else means something resampled to reach it.
    pub output_sample_rate: Option<u32>,
}

impl GqlNowPlaying {
    /// Read the player's current track. Shared by the `nowPlaying` query and
    /// the subscription of the same name, which must not drift apart.
    pub(super) fn capture(state: &Arc<SharedPlayerState>) -> Self {
        let playback_state = match state.playback_state() {
            PlaybackState::Stopped => PlaybackStateEnum::Stopped,
            PlaybackState::Playing => PlaybackStateEnum::Playing,
            PlaybackState::Paused => PlaybackStateEnum::Paused,
        };
        let position_ms = state.position_ms();

        let Some(info) = state.track_info() else {
            return Self {
                state: playback_state,
                position_ms,
                duration_ms: None,
                track: None,
                queue_item_id: None,
            };
        };

        let (items, _cursor) = state.snapshot_playlist();
        let playlist_item = items.iter().find(|i| i.id == info.id);
        Self {
            state: playback_state,
            position_ms,
            duration_ms: Some(info.duration_ms),
            track: Some(GqlNowPlayingTrack {
                track_id: playlist_item.and_then(|i| i.db_id),
                title: playlist_item.map(|i| i.title.clone()).unwrap_or_default(),
                artist: playlist_item.map(|i| i.artist.clone()).unwrap_or_default(),
                album: playlist_item.map(|i| i.album.clone()).unwrap_or_default(),
                codec: info.codec.clone(),
                sample_rate: info.sample_rate,
                bit_depth: info.bit_depth,
                bitrate_kbps: info.bitrate_kbps,
                channels: info.channels,
                duration_ms: info.duration_ms,
                output_sample_rate: state.output_sample_rate(),
            }),
            queue_item_id: Some(info.id.0.to_string()),
        }
    }
}

pub(super) struct GqlQueueEntry {
    pub queue_item_id: String,
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub is_current: bool,
    pub status: GqlQueueEntryStatus,
    pub download_progress: Option<GqlDownloadProgress>,
    pub failure_reason: Option<String>,
}

#[Object(name = "QueueEntry")]
impl GqlQueueEntry {
    async fn queue_item_id(&self) -> &str {
        &self.queue_item_id
    }

    /// Library row id — see `NowPlayingTrack::trackId`.
    async fn track_id(&self) -> Option<i64> {
        self.track_id
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn artist(&self) -> &str {
        &self.artist
    }

    async fn album(&self) -> &str {
        &self.album
    }

    async fn codec(&self) -> Option<&str> {
        self.codec.as_deref()
    }

    async fn track_number(&self) -> Option<i64> {
        self.track_number
    }

    async fn disc(&self) -> Option<i64> {
        self.disc
    }

    async fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    async fn is_current(&self) -> bool {
        self.is_current
    }

    /// Derived status: Queued, Playing, Played, Downloading, PriorityPending, Failed.
    async fn status(&self) -> GqlQueueEntryStatus {
        self.status
    }

    /// Download progress — present only when the track is being downloaded.
    async fn download_progress(&self) -> Option<&GqlDownloadProgress> {
        self.download_progress.as_ref()
    }

    /// Why the entry cannot play — present only when `status` is `FAILED`.
    async fn failure_reason(&self) -> Option<&str> {
        self.failure_reason.as_deref()
    }
}

#[derive(SimpleObject)]
#[graphql(name = "LibraryStats")]
pub(super) struct GqlLibraryStats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
}

#[derive(SimpleObject)]
#[graphql(name = "Device")]
pub(super) struct GqlDevice {
    pub name: String,
    pub sample_rates: Vec<f64>,
}

#[derive(SimpleObject)]
#[graphql(name = "SimilarArtist")]
pub(super) struct GqlSimilarArtist {
    pub artist: GqlSimilarArtistInfo,
    pub score: f64,
    pub source: String,
    pub relationship: String,
}

#[derive(SimpleObject)]
#[graphql(name = "SimilarArtistInfo")]
pub(super) struct GqlSimilarArtistInfo {
    pub id: i64,
    pub name: String,
}

#[derive(SimpleObject)]
#[graphql(name = "PlayHistoryEntry")]
pub(super) struct GqlPlayHistoryEntry {
    pub track_id: i64,
    pub played_at: i64,
    pub duration_ms: Option<i64>,
    pub track: Option<GqlPlayHistoryTrack>,
}

#[derive(SimpleObject)]
#[graphql(name = "PlayHistoryTrack")]
pub(super) struct GqlPlayHistoryTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(SimpleObject)]
#[graphql(name = "Snapshot")]
pub(super) struct GqlSnapshot {
    pub name: String,
    pub track_count: i32,
    pub position_ms: u64,
    pub created_at: String,
}

#[derive(SimpleObject)]
#[graphql(name = "RadioStatus")]
pub(super) struct GqlRadioStatus {
    pub enabled: bool,
}

#[derive(SimpleObject)]
#[graphql(name = "FuzzyMatch")]
pub(super) struct GqlFuzzyMatch {
    pub id: i64,
    pub name: String,
    pub rank: i32,
    pub kind: FuzzySearchKind,
}

#[derive(SimpleObject)]
#[graphql(name = "Lyrics")]
pub(super) struct GqlLyrics {
    pub content: String,
    pub synced: bool,
    pub source: String,
}

#[derive(SimpleObject)]
#[graphql(name = "CoverArt")]
pub(super) struct GqlCoverArt {
    pub data_base64: String,
    pub mime: String,
}

/// What the pattern means for one file.
#[derive(async_graphql::Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "PlanOutcome")]
pub(super) enum GqlPlanOutcome {
    /// Will be moved, or was.
    Move,
    /// Already exactly where the pattern puts it.
    Unchanged,
    /// Something holds the destination. Nothing is ever overwritten, so the
    /// file stays where it is.
    Conflict,
    /// The pattern produced nothing usable, or the move failed.
    Error,
}

/// One file's place in a plan.
#[derive(SimpleObject)]
#[graphql(name = "FileMove")]
pub(super) struct GqlFileMove {
    /// Null for a file the library doesn't hold a row for.
    pub track_id: Option<i64>,
    pub from_path: String,
    /// Null only when the pattern failed before producing a path at all.
    pub to_path: Option<String>,
    pub outcome: GqlPlanOutcome,
    /// Why this file isn't moving. Null when it is.
    pub reason: Option<String>,
}

/// Every selected file and what happens to it, in plan order. Preview and
/// execute answer in the same shape, so one table can render either.
#[derive(SimpleObject)]
#[graphql(name = "OrganizePlan")]
pub(super) struct GqlOrganizePlan {
    pub entries: Vec<GqlFileMove>,
    pub moved_count: i32,
    pub unchanged_count: i32,
    pub conflict_count: i32,
    pub error_count: i32,
}

impl From<koan_core::organize::OrganizeResult> for GqlOrganizePlan {
    fn from(result: koan_core::organize::OrganizeResult) -> Self {
        use koan_core::organize::PlanOutcome;
        let conflict_count = result.conflicts().count() as i32;
        Self {
            moved_count: result.moved_count() as i32,
            unchanged_count: result.unchanged_count() as i32,
            conflict_count,
            error_count: result.failures().count() as i32 - conflict_count,
            entries: result
                .entries
                .into_iter()
                .map(|e| GqlFileMove {
                    track_id: e.track_id,
                    from_path: e.from.to_string_lossy().into_owned(),
                    to_path: e.to.map(|t| t.to_string_lossy().into_owned()),
                    reason: e.outcome.reason().map(str::to_owned),
                    outcome: match e.outcome {
                        PlanOutcome::Move => GqlPlanOutcome::Move,
                        PlanOutcome::Unchanged => GqlPlanOutcome::Unchanged,
                        PlanOutcome::Conflict(_) => GqlPlanOutcome::Conflict,
                        PlanOutcome::Error(_) => GqlPlanOutcome::Error,
                    },
                })
                .collect(),
        }
    }
}

/// A handle to work running on a detached thread. Poll it with the `job` query.
#[derive(SimpleObject)]
#[graphql(name = "Job")]
pub(super) struct GqlJob {
    pub id: String,
    /// What the job is doing — `scan` or `remoteSync`.
    pub kind: String,
    pub state: JobState,
    /// Human-readable progress or outcome.
    pub message: String,
}

impl From<Job> for GqlJob {
    fn from(job: Job) -> Self {
        Self {
            id: job.id,
            kind: job.kind,
            state: job.state,
            message: job.message,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "Share")]
pub(super) struct GqlShare {
    pub url: Option<String>,
    pub id: String,
    /// Tracks the server knows about, which went into the link.
    pub shared: i32,
    /// Tracks with no copy on the server, left out of it.
    pub skipped: i32,
}

pub(super) struct GqlSimilarTrack {
    pub row: queries::TrackRow,
    pub distance: f64,
}

#[Object(name = "SimilarTrack")]
impl GqlSimilarTrack {
    async fn track_id(&self) -> i64 {
        self.row.id
    }

    async fn title(&self) -> &str {
        &self.row.title
    }

    async fn artist(&self) -> &str {
        &self.row.artist_name
    }

    async fn album(&self) -> &str {
        &self.row.album_title
    }

    async fn distance(&self) -> f64 {
        self.distance
    }

    async fn duration_ms(&self) -> Option<i64> {
        self.row.duration_ms
    }

    async fn genre(&self) -> Option<&str> {
        self.row.genre.as_deref()
    }
}

/// Mutation/query result status.
pub(super) struct GqlStatus {
    pub success: bool,
    pub message: String,
}

#[Object(name = "Status")]
impl GqlStatus {
    async fn ok(&self) -> bool {
        self.success
    }

    async fn message(&self) -> &str {
        &self.message
    }
}

impl GqlStatus {
    pub fn success(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: msg.into(),
        }
    }
}

/// Download progress for a queue entry.
#[derive(SimpleObject, Clone)]
#[graphql(name = "DownloadProgress")]
pub(super) struct GqlDownloadProgress {
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total bytes expected (0 if unknown).
    pub total: u64,
}

/// Queue snapshot with version for change detection.
#[derive(SimpleObject)]
#[graphql(name = "QueueSnapshot")]
pub(super) struct GqlQueueSnapshot {
    /// Monotonically increasing version — changes on every playlist mutation.
    pub version: u64,
    /// Queue entries with derived status.
    pub entries: Vec<GqlQueueEntry>,
    /// Number of entries before the cursor (already played).
    pub finished_count: i32,
    /// Whether any entry is currently playing.
    pub has_playing: bool,
    /// Number of entries after the cursor (queued).
    pub queue_count: i32,
}

impl GqlQueueSnapshot {
    /// Read the derived queue. Shared by the `queue` query and the
    /// `queueUpdated` subscription, which must not drift apart.
    pub(super) fn capture(state: &Arc<SharedPlayerState>) -> Self {
        let version = state.playlist_version();
        let snap = state.derive_visible_queue();

        let entries = snap
            .entries
            .iter()
            .map(|entry| {
                let status = match entry.status {
                    QueueEntryStatus::Queued => GqlQueueEntryStatus::Queued,
                    QueueEntryStatus::Playing => GqlQueueEntryStatus::Playing,
                    QueueEntryStatus::Played => GqlQueueEntryStatus::Played,
                    QueueEntryStatus::Downloading => GqlQueueEntryStatus::Downloading,
                    QueueEntryStatus::PriorityPending => GqlQueueEntryStatus::PriorityPending,
                    QueueEntryStatus::Failed => GqlQueueEntryStatus::Failed,
                };
                GqlQueueEntry {
                    queue_item_id: entry.id.0.to_string(),
                    track_id: entry.db_id,
                    title: entry.title.clone(),
                    artist: entry.artist.clone(),
                    album: entry.album.clone(),
                    codec: entry.codec.clone(),
                    track_number: entry.track_number,
                    disc: entry.disc,
                    duration_ms: entry.duration_ms,
                    is_current: entry.status == QueueEntryStatus::Playing,
                    status,
                    download_progress: entry
                        .download_progress
                        .map(|(downloaded, total)| GqlDownloadProgress { downloaded, total }),
                    failure_reason: entry.error.clone(),
                }
            })
            .collect();

        Self {
            version,
            entries,
            finished_count: snap.finished_count as i32,
            has_playing: snap.has_playing,
            queue_count: snap.queue_count as i32,
        }
    }
}

/// A single frame of visualizer data.
#[derive(SimpleObject, Clone)]
#[graphql(name = "VizFrame")]
pub(super) struct GqlVizFrame {
    /// Spectrum bar heights (0.0..1.0), 48 bars.
    pub spectrum: Vec<f32>,
    /// Peak hold values (slowly decaying maxima), 48 bars.
    pub peaks: Vec<f32>,
    /// RMS VU levels: [left, right], each 0.0..1.0.
    pub vu_levels: Vec<f32>,
    /// Beat energy (0.0..1.0). Spikes on transients.
    pub beat_energy: f32,
    /// Raw waveform samples (interleaved stereo). Empty when disabled or no audio playing.
    /// Opt-in: only populated when the client requests it.
    pub waveform: Vec<f32>,
}

/// Top-level config as exposed via GraphQL.
#[derive(SimpleObject)]
#[graphql(name = "Config")]
pub(super) struct GqlConfig {
    pub library_folders: Vec<String>,
    pub replaygain_mode: String,
    pub pre_amp_db: f64,
    pub output_device: Option<String>,
    pub target_fps: i32,
    pub art_size: i32,
    pub remote_enabled: bool,
    pub remote_url: String,
    pub remote_username: String,
    pub transcode_quality: String,
    pub cache_limit: Option<String>,
    pub visualizer_fps: i32,
    pub radio_enabled: bool,
    pub graphql_port: i32,
    pub graphql_playground: bool,
}

/// Input for updating config fields. All optional — only provided fields are written.
#[derive(InputObject)]
#[graphql(name = "ConfigInput")]
pub(super) struct GqlConfigInput {
    pub library_folders: Option<Vec<String>>,
    pub replaygain_mode: Option<String>,
    pub pre_amp_db: Option<f64>,
    pub output_device: Option<String>,
    pub target_fps: Option<i32>,
    pub art_size: Option<i32>,
    pub remote_enabled: Option<bool>,
    pub remote_url: Option<String>,
    pub remote_username: Option<String>,
    pub transcode_quality: Option<String>,
    pub cache_limit: Option<String>,
    pub visualizer_fps: Option<i32>,
    pub graphql_port: Option<i32>,
    pub graphql_playground: Option<bool>,
}

pub(super) struct GqlQueueMutationResult {
    pub success: bool,
    pub message: String,
    pub added_count: i32,
    pub queue_item_ids: Vec<String>,
}

#[Object(name = "QueueMutationResult")]
impl GqlQueueMutationResult {
    async fn ok(&self) -> bool {
        self.success
    }

    async fn message(&self) -> &str {
        &self.message
    }

    async fn added_count(&self) -> i32 {
        self.added_count
    }

    async fn queue_item_ids(&self) -> &[String] {
        &self.queue_item_ids
    }
}

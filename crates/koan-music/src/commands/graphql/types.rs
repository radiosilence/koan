use async_graphql::{Context, Enum, Object, SimpleObject};

use async_graphql::connection::{Connection, EmptyFields};
use koan_core::db::queries;

use super::DbHandle;
use super::helpers::paginate;

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

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

pub(super) struct GqlArtist {
    pub row: queries::ArtistRow,
}

#[Object]
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
    ) -> async_graphql::Result<Connection<usize, GqlAlbum, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let all = queries::albums_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
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
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let all = queries::tracks_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        paginate(
            all.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn album_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let albums = queries::albums_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(albums.len() as i32)
    }

    async fn track_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tracks = queries::tracks_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(tracks.len() as i32)
    }
}

pub(super) struct GqlAlbum {
    pub row: queries::AlbumRow,
}

#[Object]
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
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let all = queries::tracks_for_album(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        paginate(
            all.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn track_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tracks = queries::tracks_for_album(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(tracks.len() as i32)
    }

    async fn total_duration_ms(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tracks = queries::tracks_for_album(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(tracks.iter().filter_map(|t| t.duration_ms).sum())
    }
}

pub(super) struct GqlTrack {
    pub row: queries::TrackRow,
}

#[Object]
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
        let db = ctx.data::<DbHandle>()?.open()?;
        let favs = queries::load_favourites(&db.conn)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let path = self
            .row
            .path
            .as_ref()
            .or(self.row.cached_path.as_ref())
            .map(std::path::PathBuf::from);
        Ok(path.map(|p| favs.contains(&p)).unwrap_or(false))
    }
}

#[derive(SimpleObject)]
pub(super) struct GqlNowPlaying {
    pub state: PlaybackStateEnum,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub track: Option<GqlNowPlayingTrack>,
    pub queue_item_id: Option<String>,
}

#[derive(SimpleObject)]
pub(super) struct GqlNowPlayingTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration_ms: u64,
}

pub(super) struct GqlQueueEntry {
    pub queue_item_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub is_current: bool,
}

#[Object]
impl GqlQueueEntry {
    async fn queue_item_id(&self) -> &str {
        &self.queue_item_id
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
}

#[derive(SimpleObject)]
pub(super) struct GqlLibraryStats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
    /// Whether the neural-discovery feature was compiled in.
    pub neural_embeddings_available: bool,
    /// Number of tracks with neural (DCLAP) embeddings.
    pub neural_embedding_count: i64,
    /// Number of tracks with acoustic (bliss) embeddings.
    pub acoustic_embedding_count: i64,
}

#[derive(SimpleObject)]
pub(super) struct GqlDevice {
    pub name: String,
    pub sample_rates: Vec<f64>,
}

#[derive(SimpleObject)]
pub(super) struct GqlSimilarArtist {
    pub artist: GqlSimilarArtistInfo,
    pub score: f64,
    pub source: String,
    pub relationship: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlSimilarArtistInfo {
    pub id: i64,
    pub name: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlPlayHistoryEntry {
    pub track_id: i64,
    pub played_at: i64,
    pub duration_ms: Option<i64>,
    pub track: Option<GqlPlayHistoryTrack>,
}

#[derive(SimpleObject)]
pub(super) struct GqlPlayHistoryTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlSnapshot {
    pub name: String,
    pub track_count: i32,
    pub position_ms: u64,
    pub created_at: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlRadioStatus {
    pub enabled: bool,
}

#[derive(SimpleObject)]
pub(super) struct GqlFuzzyMatch {
    pub id: i64,
    pub name: String,
    pub rank: i32,
    pub kind: FuzzySearchKind,
}

#[derive(SimpleObject)]
pub(super) struct GqlLyrics {
    pub content: String,
    pub synced: bool,
    pub source: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlCoverArt {
    pub data_base64: String,
    pub mime: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlOrganizePreview {
    pub moves: Vec<GqlFileMove>,
    pub errors: Vec<String>,
    pub skipped: i32,
}

#[derive(SimpleObject)]
pub(super) struct GqlFileMove {
    pub track_id: i64,
    pub from_path: String,
    pub to_path: String,
}

#[derive(SimpleObject)]
pub(super) struct GqlOrganizeResult {
    pub moved_count: i32,
    pub errors: Vec<String>,
    pub skipped: i32,
}

#[derive(SimpleObject)]
pub(super) struct GqlScanResult {
    pub tracks_added: i64,
    pub tracks_updated: i64,
    pub tracks_unchanged: i64,
}

#[derive(SimpleObject)]
pub(super) struct GqlShare {
    pub url: Option<String>,
    pub id: String,
}

pub(super) struct GqlSimilarTrack {
    pub row: queries::TrackRow,
    pub distance: f64,
}

#[Object]
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

#[Object]
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

pub(super) struct GqlQueueMutationResult {
    pub success: bool,
    pub message: String,
    pub added_count: i32,
    pub queue_item_ids: Vec<String>,
}

#[Object]
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

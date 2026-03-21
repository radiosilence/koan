use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::audio::device;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueItemId, SharedPlayerState};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Json;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::open_db;

// ---------------------------------------------------------------------------
// Parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlayParams {
    #[schemars(description = "Queue item ID (UUID string) to start playing")]
    pub queue_item_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SeekParams {
    #[schemars(description = "Position in milliseconds to seek to")]
    pub position_ms: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddToQueueParams {
    #[schemars(description = "Track IDs from the library database to add to the queue")]
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
pub struct InsertInQueueParams {
    #[schemars(description = "Track IDs from the library database to insert")]
    pub track_ids: Vec<i64>,
    #[schemars(
        description = "Queue item ID (UUID string) to insert after (not yet used — currently appends)"
    )]
    pub after_queue_item_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveFromQueueParams {
    #[schemars(description = "Queue item IDs (UUID strings) to remove")]
    pub queue_item_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplaceQueueParams {
    #[schemars(description = "Track IDs from the library database for the new queue")]
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReorderQueueParams {
    #[schemars(description = "Queue item IDs (UUID strings) to move")]
    pub queue_item_ids: Vec<String>,
    #[schemars(description = "Queue item ID (UUID string) to move items relative to")]
    pub target_queue_item_id: String,
    #[schemars(description = "If true, insert after the target; if false, insert before")]
    pub after: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Search query — matches against track title, artist, album, genre")]
    pub query: String,
    #[schemars(description = "Max results to return (default 500)")]
    pub limit: Option<u32>,
    #[schemars(description = "Offset for pagination (default 0)")]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListAlbumsParams {
    #[schemars(description = "Optional artist ID to filter albums by")]
    pub artist_id: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTracksParams {
    #[schemars(description = "Optional album ID to list tracks for")]
    pub album_id: Option<i64>,
    #[schemars(description = "Optional single artist ID to list tracks for")]
    pub artist_id: Option<i64>,
    #[schemars(
        description = "Optional array of artist IDs — returns tracks for ALL listed artists in one call. Use this to batch-fetch tracks for multiple artists at once."
    )]
    pub artist_ids: Option<Vec<i64>>,
    #[schemars(description = "Max results (default 500)")]
    pub limit: Option<u32>,
    #[schemars(description = "Offset for pagination (default 0)")]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RandomTracksParams {
    #[schemars(description = "Number of random tracks to return (default 20, max 100)")]
    pub count: Option<u32>,
    #[schemars(description = "Optional artist ID to filter by")]
    pub artist_id: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTrackParams {
    #[schemars(description = "Track ID from the library database")]
    pub track_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetDeviceParams {
    #[schemars(description = "Audio output device name to switch to")]
    pub device_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FavouriteParams {
    #[schemars(description = "Track ID from the library database")]
    pub track_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SnapshotNameParams {
    #[schemars(description = "Name for the snapshot (e.g. 'techno', 'chill morning')")]
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphqlParams {
    #[schemars(
        description = "GraphQL query string — supports queries and mutations against the koan schema"
    )]
    pub query: String,
    #[schemars(description = "Optional JSON object of query variables")]
    pub variables: Option<serde_json::Value>,
}

/// GraphQL execution result wrapper — MCP spec requires outputSchema to be an object type.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GraphqlResponse {
    /// The GraphQL response JSON (contains data and/or errors fields).
    pub result: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Generic status response — MCP spec requires tool outputs to be objects, not bare strings.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StatusResponse {
    pub message: String,
}

impl StatusResponse {
    fn ok(msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            message: msg.into(),
        })
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct NowPlayingResponse {
    state: String,
    position_ms: u64,
    track: Option<NowPlayingTrack>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct NowPlayingTrack {
    queue_item_id: String,
    title: String,
    artist: String,
    album: String,
    codec: Option<String>,
    sample_rate: Option<u32>,
    bit_depth: Option<u16>,
    channels: Option<u16>,
    duration_ms: Option<u64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct QueueResponse {
    pub items: Vec<QueueEntryResponse>,
    pub count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct QueueEntryResponse {
    queue_item_id: String,
    title: String,
    artist: String,
    album: String,
    codec: Option<String>,
    track_number: Option<i64>,
    disc: Option<i64>,
    duration_ms: Option<u64>,
    is_current: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TrackListResponse {
    pub tracks: Vec<TrackResponse>,
    pub count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TrackResponse {
    id: i64,
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    disc: Option<i32>,
    track_number: Option<i32>,
    duration_ms: Option<i64>,
    codec: Option<String>,
    sample_rate: Option<i32>,
    bit_depth: Option<i32>,
    channels: Option<i32>,
    genre: Option<String>,
    source: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ArtistListResponse {
    pub artists: Vec<ArtistResponse>,
    pub count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ArtistResponse {
    id: i64,
    name: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AlbumListResponse {
    pub albums: Vec<AlbumResponse>,
    pub count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AlbumResponse {
    id: i64,
    title: String,
    artist_id: i64,
    artist_name: String,
    date: Option<String>,
    codec: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct LibraryStatsResponse {
    total_tracks: i64,
    local_tracks: i64,
    remote_tracks: i64,
    cached_tracks: i64,
    total_albums: i64,
    total_artists: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceResponse>,
    pub count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DeviceResponse {
    name: String,
    sample_rates: Vec<f64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SnapshotListResponse {
    pub snapshots: Vec<SnapshotSummaryResponse>,
    pub count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SnapshotSummaryResponse {
    name: String,
    track_count: usize,
    position_ms: u64,
    created_at: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RadioStatusResponse {
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Helper conversions
// ---------------------------------------------------------------------------

fn track_row_to_response(t: &queries::TrackRow) -> TrackResponse {
    TrackResponse {
        id: t.id,
        title: t.title.clone(),
        artist: t.artist_name.clone(),
        album_artist: t.album_artist_name.clone(),
        album: t.album_title.clone(),
        disc: t.disc,
        track_number: t.track_number,
        duration_ms: t.duration_ms,
        codec: t.codec.clone(),
        sample_rate: t.sample_rate,
        bit_depth: t.bit_depth,
        channels: t.channels,
        genre: t.genre.clone(),
        source: t.source.clone(),
    }
}

fn parse_queue_item_id(s: &str) -> Result<QueueItemId, String> {
    Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|e| format!("invalid queue item ID '{}': {}", s, e))
}

use crate::tui::app::PickerAction;
use std::sync::Mutex;

/// Enqueue tracks using the same pipeline as the TUI — handles local, cached,
/// and remote tracks with background downloading. Spawns on a background thread
/// so the MCP response returns immediately.
fn enqueue_tracks_bg(
    ids: Vec<i64>,
    action: PickerAction,
    tx: Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    let log_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    std::thread::Builder::new()
        .name("koan-mcp-enqueue".into())
        .spawn(move || {
            super::enqueue_playlist(ids, action, tx, log_buf, state);
        })
        .expect("failed to spawn enqueue thread");
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct KoanMcpServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    graphql_schema: super::graphql::KoanSchema,
}

impl KoanMcpServer {
    pub fn new(
        state: Arc<SharedPlayerState>,
        cmd_tx: Sender<PlayerCommand>,
        db_path: PathBuf,
    ) -> Self {
        let graphql_schema =
            super::graphql::build_schema(state.clone(), cmd_tx.clone(), db_path.clone());
        Self {
            tool_router: Self::tool_router(),
            state,
            cmd_tx,
            db_path,
            graphql_schema,
        }
    }

    fn open_db(&self) -> Result<Database, String> {
        Database::open(&self.db_path).map_err(|e| format!("db error: {}", e))
    }
}

use rmcp::handler::server::wrapper::Parameters;

#[tool_router]
impl KoanMcpServer {
    // -----------------------------------------------------------------------
    // Playback control
    // -----------------------------------------------------------------------

    #[tool(description = "Play a specific queue item by its queue item ID")]
    fn play(
        &self,
        Parameters(params): Parameters<PlayParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let id = parse_queue_item_id(&params.queue_item_id)?;
        self.cmd_tx
            .send(PlayerCommand::Play(id))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("playing"))
    }

    #[tool(description = "Pause playback")]
    fn pause(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::Pause)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("paused"))
    }

    #[tool(description = "Resume playback")]
    fn resume(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::Resume)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("resumed"))
    }

    #[tool(description = "Stop playback")]
    fn stop(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::Stop)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("stopped"))
    }

    #[tool(description = "Skip to the next track in the queue")]
    fn next(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::NextTrack)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("skipped to next"))
    }

    #[tool(description = "Skip to the previous track in the queue")]
    fn previous(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::PrevTrack)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("skipped to previous"))
    }

    #[tool(description = "Seek to a position in the current track (milliseconds)")]
    fn seek(
        &self,
        Parameters(params): Parameters<SeekParams>,
    ) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::Seek(params.position_ms))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok(format!(
            "seeked to {}ms",
            params.position_ms
        )))
    }

    // -----------------------------------------------------------------------
    // Queue management
    // -----------------------------------------------------------------------

    #[tool(
        description = "Add tracks to the end of the queue by their library track IDs. Remote tracks are downloaded automatically in the background."
    )]
    fn add_to_queue(
        &self,
        Parameters(params): Parameters<AddToQueueParams>,
    ) -> Json<StatusResponse> {
        let count = params.track_ids.len();
        enqueue_tracks_bg(
            params.track_ids,
            PickerAction::Append,
            self.cmd_tx.clone(),
            self.state.clone(),
        );
        StatusResponse::ok(format!("queueing {} tracks (downloading if needed)", count))
    }

    #[tool(
        description = "Insert tracks into the queue after a specific queue item, by their library track IDs. Remote tracks are downloaded automatically."
    )]
    fn insert_in_queue(
        &self,
        Parameters(params): Parameters<InsertInQueueParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let after_id = parse_queue_item_id(&params.after_queue_item_id)?;
        let db = self.open_db()?;
        let mut items = Vec::new();
        let mut remote_ids = Vec::new();

        for &tid in &params.track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = super::graphql::track_to_playlist_item(&track, &db);
                if matches!(
                    item.load_state,
                    koan_core::player::state::LoadState::Pending
                ) {
                    remote_ids.push(tid);
                }
                items.push(item);
            }
        }

        let count = items.len();
        if !items.is_empty() {
            self.cmd_tx
                .send(PlayerCommand::InsertInPlaylist {
                    items,
                    after: after_id,
                })
                .map_err(|e| format!("send error: {}", e))?;
        }

        // Kick off background downloads for any remote tracks.
        if !remote_ids.is_empty() {
            enqueue_tracks_bg(
                remote_ids,
                PickerAction::Append,
                self.cmd_tx.clone(),
                self.state.clone(),
            );
        }

        Ok(StatusResponse::ok(format!(
            "inserted {} tracks after queue item",
            count
        )))
    }

    #[tool(description = "Remove tracks from the queue by their queue item IDs")]
    fn remove_from_queue(
        &self,
        Parameters(params): Parameters<RemoveFromQueueParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let ids: Vec<QueueItemId> = params
            .queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let count = ids.len();
        self.cmd_tx
            .send(PlayerCommand::RemoveFromPlaylistBatch(ids))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok(format!(
            "removed {} items from queue",
            count
        )))
    }

    #[tool(description = "Clear the entire queue and stop playback")]
    fn clear_queue(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::ClearPlaylist)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("queue cleared"))
    }

    #[tool(
        description = "Replace the entire queue with new tracks and start playing. Takes library track IDs. Remote tracks are downloaded automatically."
    )]
    fn replace_queue(
        &self,
        Parameters(params): Parameters<ReplaceQueueParams>,
    ) -> Json<StatusResponse> {
        let count = params.track_ids.len();
        enqueue_tracks_bg(
            params.track_ids,
            PickerAction::ReplaceQueue,
            self.cmd_tx.clone(),
            self.state.clone(),
        );
        StatusResponse::ok(format!(
            "replacing queue with {} tracks and playing (downloading if needed)",
            count
        ))
    }

    #[tool(description = "Get the current queue with track info and playback status")]
    fn get_queue(&self) -> Json<QueueResponse> {
        let (items, cursor) = self.state.snapshot_playlist();
        let entries: Vec<QueueEntryResponse> = items
            .iter()
            .map(|item| QueueEntryResponse {
                queue_item_id: item.id.0.to_string(),
                title: item.title.clone(),
                artist: item.artist.clone(),
                album: item.album.clone(),
                codec: item.codec.clone(),
                track_number: item.track_number,
                disc: item.disc,
                duration_ms: item.duration_ms,
                is_current: cursor == Some(item.id),
            })
            .collect();
        let count = entries.len();
        Json(QueueResponse {
            items: entries,
            count,
        })
    }

    #[tool(description = "Reorder items within the queue — move items to before/after a target")]
    fn reorder_queue(
        &self,
        Parameters(params): Parameters<ReorderQueueParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let ids: Vec<QueueItemId> = params
            .queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let target = parse_queue_item_id(&params.target_queue_item_id)?;
        self.cmd_tx
            .send(PlayerCommand::MoveItemsInPlaylist {
                ids,
                target,
                after: params.after,
            })
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("queue reordered"))
    }

    // -----------------------------------------------------------------------
    // Library discovery
    // -----------------------------------------------------------------------

    #[tool(
        description = "Full-text search across the music library — matches title, artist, album, genre"
    )]
    fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TrackListResponse>, String> {
        let db = self.open_db()?;
        let limit = params.limit.unwrap_or(500);
        let offset = params.offset.unwrap_or(0);
        let tracks = queries::search_tracks_paged(&db.conn, &params.query, limit, offset)
            .map_err(|e| format!("search error: {}", e))?;
        let items: Vec<TrackResponse> = tracks.iter().map(track_row_to_response).collect();
        let count = items.len();
        Ok(Json(TrackListResponse {
            tracks: items,
            count,
        }))
    }

    #[tool(description = "List all artists in the library (album artists only, sorted by name)")]
    fn list_artists(&self) -> Result<Json<ArtistListResponse>, String> {
        let db = self.open_db()?;
        let artists = queries::all_artists(&db.conn).map_err(|e| format!("db error: {}", e))?;
        let items: Vec<ArtistResponse> = artists
            .iter()
            .map(|a| ArtistResponse {
                id: a.id,
                name: a.name.clone(),
            })
            .collect();
        let count = items.len();
        Ok(Json(ArtistListResponse {
            artists: items,
            count,
        }))
    }

    #[tool(
        description = "List albums — all albums or filtered by artist ID. Sorted chronologically."
    )]
    fn list_albums(
        &self,
        Parameters(params): Parameters<ListAlbumsParams>,
    ) -> Result<Json<AlbumListResponse>, String> {
        let db = self.open_db()?;
        let albums = if let Some(artist_id) = params.artist_id {
            queries::albums_for_artist(&db.conn, artist_id)
                .map_err(|e| format!("db error: {}", e))?
        } else {
            queries::all_albums(&db.conn).map_err(|e| format!("db error: {}", e))?
        };
        let items: Vec<AlbumResponse> = albums
            .iter()
            .map(|a| AlbumResponse {
                id: a.id,
                title: a.title.clone(),
                artist_id: a.artist_id,
                artist_name: a.artist_name.clone(),
                date: a.date.clone(),
                codec: a.codec.clone(),
                label: a.label.clone(),
            })
            .collect();
        let count = items.len();
        Ok(Json(AlbumListResponse {
            albums: items,
            count,
        }))
    }

    #[tool(
        description = "List tracks — by album ID, artist ID, or all. Ordered by disc/track number. Supports pagination."
    )]
    fn list_tracks(
        &self,
        Parameters(params): Parameters<ListTracksParams>,
    ) -> Result<Json<TrackListResponse>, String> {
        let db = self.open_db()?;
        let tracks = if let Some(album_id) = params.album_id {
            queries::tracks_for_album(&db.conn, album_id).map_err(|e| format!("db error: {}", e))?
        } else if let Some(ref artist_ids) = params.artist_ids {
            // Batch fetch tracks for multiple artists in one call.
            let mut all = Vec::new();
            for &aid in artist_ids {
                let mut t = queries::tracks_for_artist(&db.conn, aid)
                    .map_err(|e| format!("db error: {}", e))?;
                all.append(&mut t);
            }
            all
        } else if let Some(artist_id) = params.artist_id {
            queries::tracks_for_artist(&db.conn, artist_id)
                .map_err(|e| format!("db error: {}", e))?
        } else {
            let limit = params.limit.unwrap_or(500);
            let offset = params.offset.unwrap_or(0);
            queries::all_tracks_paged(&db.conn, limit, offset)
                .map_err(|e| format!("db error: {}", e))?
        };
        let items: Vec<TrackResponse> = tracks.iter().map(track_row_to_response).collect();
        let count = items.len();
        Ok(Json(TrackListResponse {
            tracks: items,
            count,
        }))
    }

    #[tool(
        description = "Get random tracks from the library. Useful for discovering what's in the collection. Optionally filter by artist."
    )]
    fn random_tracks(
        &self,
        Parameters(params): Parameters<RandomTracksParams>,
    ) -> Result<Json<TrackListResponse>, String> {
        let db = self.open_db()?;
        let count = params.count.unwrap_or(20).min(100);
        let tracks = queries::random_tracks(&db.conn, count, params.artist_id)
            .map_err(|e| format!("db error: {}", e))?;
        let items: Vec<TrackResponse> = tracks.iter().map(track_row_to_response).collect();
        let len = items.len();
        Ok(Json(TrackListResponse {
            tracks: items,
            count: len,
        }))
    }

    #[tool(description = "Get full metadata for a specific track by ID")]
    fn get_track(
        &self,
        Parameters(params): Parameters<GetTrackParams>,
    ) -> Result<Json<TrackResponse>, String> {
        let db = self.open_db()?;
        let track = queries::get_track_row(&db.conn, params.track_id)
            .map_err(|e| format!("db error: {}", e))?
            .ok_or_else(|| format!("track {} not found", params.track_id))?;
        Ok(Json(track_row_to_response(&track)))
    }

    #[tool(description = "Get library statistics — total tracks, albums, artists by source")]
    fn library_stats(&self) -> Result<Json<LibraryStatsResponse>, String> {
        let db = self.open_db()?;
        let stats = queries::library_stats(&db.conn).map_err(|e| format!("db error: {}", e))?;
        Ok(Json(LibraryStatsResponse {
            total_tracks: stats.total_tracks,
            local_tracks: stats.local_tracks,
            remote_tracks: stats.remote_tracks,
            cached_tracks: stats.cached_tracks,
            total_albums: stats.total_albums,
            total_artists: stats.total_artists,
        }))
    }

    // -----------------------------------------------------------------------
    // State queries
    // -----------------------------------------------------------------------

    #[tool(
        description = "Get the currently playing track, playback state (stopped/playing/paused), and position"
    )]
    fn now_playing(&self) -> Json<NowPlayingResponse> {
        let playback_state = self.state.playback_state();
        let state_str = match playback_state {
            PlaybackState::Stopped => "stopped",
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
        };
        let position_ms = self.state.position_ms();
        let track = self.state.track_info().map(|info| {
            let (items, _cursor) = self.state.snapshot_playlist();
            let playlist_item = items.iter().find(|i| i.id == info.id);
            NowPlayingTrack {
                queue_item_id: info.id.0.to_string(),
                title: playlist_item.map(|i| i.title.clone()).unwrap_or_default(),
                artist: playlist_item.map(|i| i.artist.clone()).unwrap_or_default(),
                album: playlist_item.map(|i| i.album.clone()).unwrap_or_default(),
                codec: Some(info.codec.clone()),
                sample_rate: Some(info.sample_rate),
                bit_depth: Some(info.bit_depth),
                channels: Some(info.channels),
                duration_ms: Some(info.duration_ms),
            }
        });
        Json(NowPlayingResponse {
            state: state_str.into(),
            position_ms,
            track,
        })
    }

    #[tool(description = "List available audio output devices")]
    fn list_devices(&self) -> Result<Json<DeviceListResponse>, String> {
        let devices = device::list_output_devices().map_err(|e| format!("device error: {}", e))?;
        let items: Vec<DeviceResponse> = devices
            .iter()
            .map(|d| DeviceResponse {
                name: d.name.clone(),
                sample_rates: d.sample_rates.clone(),
            })
            .collect();
        let count = items.len();
        Ok(Json(DeviceListResponse {
            devices: items,
            count,
        }))
    }

    #[tool(description = "Switch audio output to a different device by name")]
    fn set_device(
        &self,
        Parameters(params): Parameters<SetDeviceParams>,
    ) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::SetOutputDevice(params.device_name.clone()))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok(format!(
            "switched to device '{}'",
            params.device_name
        )))
    }

    // -----------------------------------------------------------------------
    // Favourites
    // -----------------------------------------------------------------------

    #[tool(
        description = "Star/favourite a track by its library track ID. Automatically syncs to the remote server if configured."
    )]
    fn favourite(
        &self,
        Parameters(params): Parameters<FavouriteParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let db = self.open_db()?;
        let track = queries::get_track_row(&db.conn, params.track_id)
            .map_err(|e| format!("db error: {}", e))?
            .ok_or_else(|| format!("track {} not found", params.track_id))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| format!("track {} has no path", params.track_id))?;
        queries::add_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| format!("db error: {}", e))?;
        super::graphql::sync_favourite_to_remote_from_path(&db, path, true);
        Ok(StatusResponse::ok(format!(
            "favourited track {}",
            params.track_id
        )))
    }

    #[tool(
        description = "Unstar/unfavourite a track by its library track ID. Automatically syncs to the remote server if configured."
    )]
    fn unfavourite(
        &self,
        Parameters(params): Parameters<FavouriteParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let db = self.open_db()?;
        let track = queries::get_track_row(&db.conn, params.track_id)
            .map_err(|e| format!("db error: {}", e))?
            .ok_or_else(|| format!("track {} not found", params.track_id))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| format!("track {} has no path", params.track_id))?;
        queries::remove_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| format!("db error: {}", e))?;
        super::graphql::sync_favourite_to_remote_from_path(&db, path, false);
        Ok(StatusResponse::ok(format!(
            "unfavourited track {}",
            params.track_id
        )))
    }

    #[tool(
        description = "Execute a GraphQL query against the koan music library and player. \
        Supports nested queries (artists→albums→tracks), cursor pagination, mutations for \
        playback/queue control, and more. Use introspection or the schema docs to explore. \
        This single tool can replace most other library/discovery tools."
    )]
    fn graphql(
        &self,
        Parameters(params): Parameters<GraphqlParams>,
    ) -> Result<Json<GraphqlResponse>, String> {
        let schema = self.graphql_schema.clone();
        let rt =
            tokio::runtime::Handle::try_current().map_err(|_| "no tokio runtime".to_string())?;
        let result = rt.block_on(super::graphql::execute_in_process(
            &schema,
            &params.query,
            params.variables,
        ));
        Ok(Json(GraphqlResponse { result }))
    }

    #[tool(description = "List all favourited/starred tracks")]
    fn list_favourites(&self) -> Result<Json<TrackListResponse>, String> {
        let db = self.open_db()?;
        let fav_paths =
            queries::load_favourites(&db.conn).map_err(|e| format!("db error: {}", e))?;

        let mut tracks = Vec::new();
        for path in &fav_paths {
            let path_str = path.to_string_lossy();
            if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &path_str)
                && let Ok(Some(row)) = queries::get_track_row(&db.conn, tid)
            {
                tracks.push(track_row_to_response(&row));
            }
        }
        let count = tracks.len();
        Ok(Json(TrackListResponse { tracks, count }))
    }

    // -----------------------------------------------------------------------
    // Device control
    // -----------------------------------------------------------------------

    #[tool(description = "Reset audio output to the system default device")]
    fn clear_device(&self) -> Result<Json<StatusResponse>, String> {
        self.cmd_tx
            .send(PlayerCommand::ClearOutputDevice)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(StatusResponse::ok("device cleared, using system default"))
    }

    // -----------------------------------------------------------------------
    // Snapshots — save/restore/list/delete named queue states
    // -----------------------------------------------------------------------

    #[tool(
        description = "Save the current queue, cursor position, and playback position as a named snapshot. \
        Overwrites if a snapshot with the same name already exists. \
        Use this to bank a curated mix and restore it later."
    )]
    fn save_snapshot(
        &self,
        Parameters(params): Parameters<SnapshotNameParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let db = self.open_db()?;
        let (items, cursor) = self.state.snapshot_playlist();
        let position_ms = self.state.position_ms();

        let persisted: Vec<koan_core::db::queries::playback_state::PersistedQueueItem> = items
            .iter()
            .map(koan_core::db::queries::playback_state::PersistedQueueItem::from_playlist_item)
            .collect();
        let cursor_path = cursor.and_then(|cid| {
            items
                .iter()
                .find(|i| i.id == cid)
                .map(|i| i.path.to_string_lossy().into_owned())
        });

        queries::save_snapshot(
            &db.conn,
            &params.name,
            &persisted,
            cursor_path.as_deref(),
            position_ms,
        )
        .map_err(|e| format!("db error: {}", e))?;

        Ok(StatusResponse::ok(format!(
            "saved snapshot '{}' ({} tracks)",
            params.name,
            items.len()
        )))
    }

    #[tool(
        description = "Restore a named snapshot — replaces the current queue and resumes playback \
        at the saved cursor position. Use list_snapshots to see available names."
    )]
    fn restore_snapshot(
        &self,
        Parameters(params): Parameters<SnapshotNameParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let db = self.open_db()?;
        let snap = queries::load_snapshot(&db.conn, &params.name)
            .map_err(|e| format!("db error: {}", e))?
            .ok_or_else(|| format!("snapshot '{}' not found", params.name))?;

        let items: Vec<koan_core::player::state::PlaylistItem> =
            snap.items.iter().map(|i| i.to_playlist_item()).collect();

        self.cmd_tx
            .send(PlayerCommand::ClearPlaylist)
            .map_err(|e| format!("send error: {}", e))?;

        let cursor_item_id = snap.cursor_path.as_ref().and_then(|cp| {
            items
                .iter()
                .find(|i| i.path.to_string_lossy() == cp.as_str())
                .map(|i| i.id)
        });

        if !items.is_empty() {
            let first_id = cursor_item_id.unwrap_or(items[0].id);
            self.cmd_tx
                .send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| format!("send error: {}", e))?;
            let _ = self.cmd_tx.send(PlayerCommand::Play(first_id));
            if snap.position_ms > 0 {
                let _ = self.cmd_tx.send(PlayerCommand::Seek(snap.position_ms));
            }
        }

        Ok(StatusResponse::ok(format!(
            "restored snapshot '{}'",
            params.name
        )))
    }

    #[tool(
        description = "List all saved queue snapshots with name, track count, and creation date"
    )]
    fn list_snapshots(&self) -> Result<Json<SnapshotListResponse>, String> {
        let db = self.open_db()?;
        let list = queries::list_snapshots(&db.conn).map_err(|e| format!("db error: {}", e))?;
        let snapshots: Vec<SnapshotSummaryResponse> = list
            .into_iter()
            .map(|s| SnapshotSummaryResponse {
                name: s.name,
                track_count: s.track_count,
                position_ms: s.position_ms,
                created_at: s.created_at,
            })
            .collect();
        let count = snapshots.len();
        Ok(Json(SnapshotListResponse { snapshots, count }))
    }

    #[tool(description = "Delete a named snapshot")]
    fn delete_snapshot(
        &self,
        Parameters(params): Parameters<SnapshotNameParams>,
    ) -> Result<Json<StatusResponse>, String> {
        let db = self.open_db()?;
        let deleted = queries::delete_snapshot(&db.conn, &params.name)
            .map_err(|e| format!("db error: {}", e))?;
        if deleted {
            Ok(StatusResponse::ok(format!(
                "deleted snapshot '{}'",
                params.name
            )))
        } else {
            Err(format!("snapshot '{}' not found", params.name))
        }
    }

    // -----------------------------------------------------------------------
    // Radio mode
    // -----------------------------------------------------------------------

    #[tool(
        description = "Enable radio mode — automatically discovers and queues similar tracks \
        when the queue runs low. Uses ListenBrainz, MusicBrainz, genre/era matching, \
        and Subsonic (if configured) for multi-signal discovery."
    )]
    fn enable_radio(&self) -> Json<StatusResponse> {
        self.state.set_radio_mode(true);
        StatusResponse::ok("radio mode enabled")
    }

    #[tool(description = "Disable radio mode — stop auto-queueing tracks")]
    fn disable_radio(&self) -> Json<StatusResponse> {
        self.state.set_radio_mode(false);
        StatusResponse::ok("radio mode disabled")
    }

    #[tool(description = "Check whether radio mode is currently enabled")]
    fn radio_status(&self) -> Json<RadioStatusResponse> {
        Json(RadioStatusResponse {
            enabled: self.state.radio_mode(),
        })
    }
}

#[rmcp::tool_handler]
impl ServerHandler for KoanMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "koan is a bit-perfect macOS music player. You can control playback, \
                 manage the queue, search the music library, and manage favourites. \
                 Use search and list_artists/list_albums/list_tracks to discover music, \
                 then add_to_queue or replace_queue to play it. Track IDs are library \
                 database IDs (integers). Queue item IDs are UUIDs assigned when tracks \
                 are added to the queue.",
        )
    }
}

/// Entry point for `koan mcp` — starts a headless player with an MCP server on stdio.
pub fn cmd_mcp() {
    // Validate DB is accessible before starting the server.
    let _db = open_db();
    let db_path = koan_core::config::db_path();

    // Spawn the player engine (headless — no TUI).
    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    let server = KoanMcpServer::new(state, cmd_tx, db_path);

    // Run the MCP server on the tokio runtime (blocking the main thread).
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let transport = rmcp::transport::io::stdio();
        let service = server
            .serve(transport)
            .await
            .expect("failed to start MCP server");
        let _ = service.waiting().await;
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::db::connection::Database;
    use koan_core::db::queries;
    use koan_core::player::commands::CommandChannel;
    use tempfile::TempDir;

    /// Create a test server with an in-memory-like temp DB and a channel we can drain.
    fn test_server() -> (
        KoanMcpServer,
        crossbeam_channel::Receiver<PlayerCommand>,
        TempDir,
    ) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = SharedPlayerState::new();
        let ch = CommandChannel::new();
        let tx = ch.tx.clone();
        let rx = ch.rx.clone();

        let server = KoanMcpServer::new(state, tx, db_path);
        (server, rx, tmp)
    }

    /// Insert a test track into the DB, returns track_id.
    fn insert_test_track(db_path: &std::path::Path, title: &str, artist: &str, album: &str) -> i64 {
        let db = Database::open(db_path).unwrap();
        let meta = queries::TrackMeta {
            title: title.to_string(),
            artist: artist.to_string(),
            album_artist: Some(artist.to_string()),
            album: album.to_string(),
            track_number: Some(1),
            disc: Some(1),
            date: Some("2024".into()),
            genre: Some("Electronic".into()),
            duration_ms: Some(240000),
            path: Some(format!(
                "/tmp/test/{}.flac",
                title.to_lowercase().replace(' ', "_")
            )),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1411),
            size_bytes: Some(42_000_000),
            mtime: Some(1700000000),
            source: "local".into(),
            remote_id: None,
            remote_url: None,
            label: None,
        };
        queries::upsert_track(&db.conn, &meta).unwrap()
    }

    // --- parse_queue_item_id ---

    #[test]
    fn parse_valid_uuid() {
        let uuid = Uuid::now_v7();
        let result = parse_queue_item_id(&uuid.to_string());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, uuid);
    }

    #[test]
    fn parse_invalid_uuid() {
        let result = parse_queue_item_id("not-a-uuid");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid queue item ID"));
    }

    #[test]
    fn parse_empty_string() {
        let result = parse_queue_item_id("");
        assert!(result.is_err());
    }

    // --- StatusResponse ---

    #[test]
    fn status_response_serializes_as_object() {
        let resp = StatusResponse {
            message: "test".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.is_object());
        assert_eq!(json["message"], "test");
    }

    // --- Playback commands send correct PlayerCommand ---

    #[test]
    fn pause_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.pause();
        assert!(result.is_ok());
        assert!(matches!(rx.try_recv().unwrap(), PlayerCommand::Pause));
    }

    #[test]
    fn resume_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.resume();
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::Resume));
    }

    #[test]
    fn stop_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.stop();
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::Stop));
    }

    #[test]
    fn next_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.next();
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::NextTrack));
    }

    #[test]
    fn previous_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.previous();
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::PrevTrack));
    }

    #[test]
    fn seek_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.seek(Parameters(SeekParams { position_ms: 5000 }));
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::Seek(5000)));
    }

    #[test]
    fn play_sends_command_with_id() {
        let (server, rx, _tmp) = test_server();
        let uuid = Uuid::now_v7();
        let result = server.play(Parameters(PlayParams {
            queue_item_id: uuid.to_string(),
        }));
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        match cmd {
            PlayerCommand::Play(id) => assert_eq!(id.0, uuid),
            other => panic!("expected Play, got {:?}", other),
        }
    }

    #[test]
    fn play_rejects_invalid_uuid() {
        let (server, _rx, _tmp) = test_server();
        let result = server.play(Parameters(PlayParams {
            queue_item_id: "garbage".into(),
        }));
        assert!(result.is_err());
    }

    // --- Queue management ---

    #[test]
    fn clear_queue_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.clear_queue();
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::ClearPlaylist));
    }

    #[test]
    fn get_queue_returns_empty() {
        let (server, _rx, _tmp) = test_server();
        let Json(resp) = server.get_queue();
        assert_eq!(resp.count, 0);
        assert!(resp.items.is_empty());
    }

    #[test]
    fn add_to_queue_returns_status() {
        let (server, _rx, _tmp) = test_server();
        let Json(resp) = server.add_to_queue(Parameters(AddToQueueParams {
            track_ids: vec![1, 2, 3],
        }));
        assert!(resp.message.contains("3 tracks"));
    }

    #[test]
    fn remove_from_queue_sends_batch() {
        let (server, rx, _tmp) = test_server();
        let id1 = Uuid::now_v7();
        let id2 = Uuid::now_v7();
        let result = server.remove_from_queue(Parameters(RemoveFromQueueParams {
            queue_item_ids: vec![id1.to_string(), id2.to_string()],
        }));
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        match cmd {
            PlayerCommand::RemoveFromPlaylistBatch(ids) => {
                assert_eq!(ids.len(), 2);
                assert_eq!(ids[0].0, id1);
                assert_eq!(ids[1].0, id2);
            }
            other => panic!("expected RemoveFromPlaylistBatch, got {:?}", other),
        }
    }

    #[test]
    fn replace_queue_returns_status() {
        let (server, _rx, _tmp) = test_server();
        let Json(resp) =
            server.replace_queue(Parameters(ReplaceQueueParams { track_ids: vec![1] }));
        assert!(resp.message.contains("1 tracks"));
    }

    // --- Library discovery ---

    #[test]
    fn search_returns_matches() {
        let (server, _rx, _tmp) = test_server();
        insert_test_track(
            &server.db_path,
            "Windowlicker",
            "Aphex Twin",
            "Windowlicker EP",
        );
        insert_test_track(&server.db_path, "Avril 14th", "Aphex Twin", "Drukqs");

        let result = server.search(Parameters(SearchParams {
            query: "aphex".into(),
            limit: None,
            offset: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.count, 2);
    }

    #[test]
    fn search_empty_query() {
        let (server, _rx, _tmp) = test_server();
        let result = server.search(Parameters(SearchParams {
            query: "nonexistent_xyzzy".into(),
            limit: None,
            offset: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.count, 0);
    }

    #[test]
    fn list_artists_returns_all() {
        let (server, _rx, _tmp) = test_server();
        insert_test_track(&server.db_path, "Track A", "Artist One", "Album");
        insert_test_track(&server.db_path, "Track B", "Artist Two", "Album");

        let result = server.list_artists();
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.count >= 2);
    }

    #[test]
    fn list_albums_all() {
        let (server, _rx, _tmp) = test_server();
        insert_test_track(&server.db_path, "T1", "A", "Album One");
        insert_test_track(&server.db_path, "T2", "A", "Album Two");

        let result = server.list_albums(Parameters(ListAlbumsParams { artist_id: None }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.count >= 2);
    }

    #[test]
    fn list_tracks_for_album() {
        let (server, _rx, _tmp) = test_server();
        insert_test_track(&server.db_path, "T1", "A", "MyAlbum");
        insert_test_track(&server.db_path, "T2", "A", "MyAlbum");
        insert_test_track(&server.db_path, "T3", "A", "OtherAlbum");

        // Find the album ID for MyAlbum.
        let db = Database::open(&server.db_path).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();
        let my_album = albums.iter().find(|a| a.title == "MyAlbum").unwrap();

        let result = server.list_tracks(Parameters(ListTracksParams {
            album_id: Some(my_album.id),
            artist_id: None,
            artist_ids: None,
            limit: None,
            offset: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.count, 2);
    }

    #[test]
    fn get_track_found() {
        let (server, _rx, _tmp) = test_server();
        let tid = insert_test_track(&server.db_path, "Found Track", "Artist", "Album");

        let result = server.get_track(Parameters(GetTrackParams { track_id: tid }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.title, "Found Track");
        assert_eq!(resp.id, tid);
    }

    #[test]
    fn get_track_not_found() {
        let (server, _rx, _tmp) = test_server();
        let result = server.get_track(Parameters(GetTrackParams { track_id: 99999 }));
        let err = result.err().expect("expected error");
        assert!(err.contains("not found"));
    }

    #[test]
    fn library_stats_works() {
        let (server, _rx, _tmp) = test_server();
        insert_test_track(&server.db_path, "T", "A", "B");

        let result = server.library_stats();
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.total_tracks >= 1);
    }

    // --- State queries ---

    #[test]
    fn now_playing_stopped() {
        let (server, _rx, _tmp) = test_server();
        let Json(resp) = server.now_playing();
        assert_eq!(resp.state, "stopped");
        assert!(resp.track.is_none());
    }

    #[test]
    fn list_devices_returns_at_least_one() {
        let (server, _rx, _tmp) = test_server();
        let result = server.list_devices();
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        // macOS always has at least one audio device.
        assert!(resp.count >= 1);
    }

    #[test]
    fn set_device_sends_command() {
        let (server, rx, _tmp) = test_server();
        let result = server.set_device(Parameters(SetDeviceParams {
            device_name: "Test DAC".into(),
        }));
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        match cmd {
            PlayerCommand::SetOutputDevice(name) => assert_eq!(name, "Test DAC"),
            other => panic!("expected SetOutputDevice, got {:?}", other),
        }
    }

    // --- Reorder ---

    #[test]
    fn reorder_queue_sends_command() {
        let (server, rx, _tmp) = test_server();
        let id1 = Uuid::now_v7();
        let target = Uuid::now_v7();
        let result = server.reorder_queue(Parameters(ReorderQueueParams {
            queue_item_ids: vec![id1.to_string()],
            target_queue_item_id: target.to_string(),
            after: true,
        }));
        assert!(result.is_ok());
        let cmd = rx.try_recv().unwrap();
        match cmd {
            PlayerCommand::MoveItemsInPlaylist {
                ids,
                target: t,
                after,
            } => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0].0, id1);
                assert_eq!(t.0, target);
                assert!(after);
            }
            other => panic!("expected MoveItemsInPlaylist, got {:?}", other),
        }
    }
}

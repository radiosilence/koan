use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::audio::device;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState,
};
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
pub struct InsertInQueueParams {
    #[schemars(description = "Track IDs from the library database to insert")]
    pub track_ids: Vec<i64>,
    #[schemars(description = "Queue item ID (UUID string) to insert after")]
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
    #[schemars(description = "Optional artist ID to list tracks for")]
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

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct NowPlayingResponse {
    state: String,
    position_ms: u64,
    track: Option<NowPlayingTrack>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct NowPlayingTrack {
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
struct QueueEntryResponse {
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
struct TrackResponse {
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
struct ArtistResponse {
    id: i64,
    name: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct AlbumResponse {
    id: i64,
    title: String,
    artist_id: i64,
    artist_name: String,
    date: Option<String>,
    codec: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct LibraryStatsResponse {
    total_tracks: i64,
    local_tracks: i64,
    remote_tracks: i64,
    cached_tracks: i64,
    total_albums: i64,
    total_artists: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct DeviceResponse {
    name: String,
    sample_rates: Vec<f64>,
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

/// Resolve track IDs to PlaylistItems using the database.
fn resolve_tracks_to_items(db: &Database, track_ids: &[i64]) -> Result<Vec<PlaylistItem>, String> {
    let mut items = Vec::with_capacity(track_ids.len());
    for &tid in track_ids {
        let track = queries::get_track_row(&db.conn, tid)
            .map_err(|e| format!("db error: {}", e))?
            .ok_or_else(|| format!("track {} not found", tid))?;

        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .map(PathBuf::from)
            .ok_or_else(|| format!("track {} has no playable path", tid))?;

        items.push(PlaylistItem {
            id: QueueItemId::new(),
            path,
            title: track.title.clone(),
            artist: track.artist_name.clone(),
            album_artist: track.album_artist_name.clone(),
            album: track.album_title.clone(),
            year: None,
            codec: track.codec.clone(),
            track_number: track.track_number.map(|n| n as i64),
            disc: track.disc.map(|n| n as i64),
            duration_ms: track.duration_ms.map(|d| d as u64),
            load_state: LoadState::Ready,
        });
    }
    Ok(items)
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
}

impl KoanMcpServer {
    pub fn new(
        state: Arc<SharedPlayerState>,
        cmd_tx: Sender<PlayerCommand>,
        db_path: PathBuf,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            state,
            cmd_tx,
            db_path,
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
    fn play(&self, Parameters(params): Parameters<PlayParams>) -> Result<Json<String>, String> {
        let id = parse_queue_item_id(&params.queue_item_id)?;
        self.cmd_tx
            .send(PlayerCommand::Play(id))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("playing".into()))
    }

    #[tool(description = "Pause playback")]
    fn pause(&self) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::Pause)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("paused".into()))
    }

    #[tool(description = "Resume playback")]
    fn resume(&self) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::Resume)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("resumed".into()))
    }

    #[tool(description = "Stop playback")]
    fn stop(&self) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::Stop)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("stopped".into()))
    }

    #[tool(description = "Skip to the next track in the queue")]
    fn next(&self) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::NextTrack)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("skipped to next".into()))
    }

    #[tool(description = "Skip to the previous track in the queue")]
    fn previous(&self) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::PrevTrack)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("skipped to previous".into()))
    }

    #[tool(description = "Seek to a position in the current track (milliseconds)")]
    fn seek(&self, Parameters(params): Parameters<SeekParams>) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::Seek(params.position_ms))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json(format!("seeked to {}ms", params.position_ms)))
    }

    // -----------------------------------------------------------------------
    // Queue management
    // -----------------------------------------------------------------------

    #[tool(description = "Add tracks to the end of the queue by their library track IDs")]
    fn add_to_queue(
        &self,
        Parameters(params): Parameters<AddToQueueParams>,
    ) -> Result<Json<String>, String> {
        let db = self.open_db()?;
        let items = resolve_tracks_to_items(&db, &params.track_ids)?;
        let count = items.len();
        self.cmd_tx
            .send(PlayerCommand::AddToPlaylist(items))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json(format!("added {} tracks to queue", count)))
    }

    #[tool(
        description = "Insert tracks into the queue after a specific queue item, by their library track IDs"
    )]
    fn insert_in_queue(
        &self,
        Parameters(params): Parameters<InsertInQueueParams>,
    ) -> Result<Json<String>, String> {
        let after = parse_queue_item_id(&params.after_queue_item_id)?;
        let db = self.open_db()?;
        let items = resolve_tracks_to_items(&db, &params.track_ids)?;
        let count = items.len();
        self.cmd_tx
            .send(PlayerCommand::InsertInPlaylist { items, after })
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json(format!(
            "inserted {} tracks after {}",
            count, params.after_queue_item_id
        )))
    }

    #[tool(description = "Remove tracks from the queue by their queue item IDs")]
    fn remove_from_queue(
        &self,
        Parameters(params): Parameters<RemoveFromQueueParams>,
    ) -> Result<Json<String>, String> {
        let ids: Vec<QueueItemId> = params
            .queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let count = ids.len();
        self.cmd_tx
            .send(PlayerCommand::RemoveFromPlaylistBatch(ids))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json(format!("removed {} items from queue", count)))
    }

    #[tool(description = "Clear the entire queue and stop playback")]
    fn clear_queue(&self) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::ClearPlaylist)
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json("queue cleared".into()))
    }

    #[tool(
        description = "Replace the entire queue with new tracks and start playing. Takes library track IDs."
    )]
    fn replace_queue(
        &self,
        Parameters(params): Parameters<ReplaceQueueParams>,
    ) -> Result<Json<String>, String> {
        let db = self.open_db()?;
        let items = resolve_tracks_to_items(&db, &params.track_ids)?;
        if items.is_empty() {
            return Err("no tracks resolved".into());
        }
        let first_id = items[0].id;
        let count = items.len();

        // Clear, add, play.
        self.cmd_tx
            .send(PlayerCommand::ClearPlaylist)
            .map_err(|e| format!("send error: {}", e))?;
        self.cmd_tx
            .send(PlayerCommand::AddToPlaylist(items))
            .map_err(|e| format!("send error: {}", e))?;
        self.cmd_tx
            .send(PlayerCommand::Play(first_id))
            .map_err(|e| format!("send error: {}", e))?;

        Ok(Json(format!(
            "replaced queue with {} tracks, now playing",
            count
        )))
    }

    #[tool(description = "Get the current queue with track info and playback status")]
    fn get_queue(&self) -> Json<Vec<QueueEntryResponse>> {
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
        Json(entries)
    }

    #[tool(description = "Reorder items within the queue — move items to before/after a target")]
    fn reorder_queue(
        &self,
        Parameters(params): Parameters<ReorderQueueParams>,
    ) -> Result<Json<String>, String> {
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
        Ok(Json("queue reordered".into()))
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
    ) -> Result<Json<Vec<TrackResponse>>, String> {
        let db = self.open_db()?;
        let tracks = queries::search_tracks(&db.conn, &params.query)
            .map_err(|e| format!("search error: {}", e))?;
        Ok(Json(tracks.iter().map(track_row_to_response).collect()))
    }

    #[tool(description = "List all artists in the library (album artists only, sorted by name)")]
    fn list_artists(&self) -> Result<Json<Vec<ArtistResponse>>, String> {
        let db = self.open_db()?;
        let artists = queries::all_artists(&db.conn).map_err(|e| format!("db error: {}", e))?;
        Ok(Json(
            artists
                .iter()
                .map(|a| ArtistResponse {
                    id: a.id,
                    name: a.name.clone(),
                })
                .collect(),
        ))
    }

    #[tool(
        description = "List albums — all albums or filtered by artist ID. Sorted chronologically."
    )]
    fn list_albums(
        &self,
        Parameters(params): Parameters<ListAlbumsParams>,
    ) -> Result<Json<Vec<AlbumResponse>>, String> {
        let db = self.open_db()?;
        let albums = if let Some(artist_id) = params.artist_id {
            queries::albums_for_artist(&db.conn, artist_id)
                .map_err(|e| format!("db error: {}", e))?
        } else {
            queries::all_albums(&db.conn).map_err(|e| format!("db error: {}", e))?
        };
        Ok(Json(
            albums
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
                .collect(),
        ))
    }

    #[tool(
        description = "List tracks — by album ID, artist ID, or all. Ordered by disc/track number."
    )]
    fn list_tracks(
        &self,
        Parameters(params): Parameters<ListTracksParams>,
    ) -> Result<Json<Vec<TrackResponse>>, String> {
        let db = self.open_db()?;
        let tracks = if let Some(album_id) = params.album_id {
            queries::tracks_for_album(&db.conn, album_id).map_err(|e| format!("db error: {}", e))?
        } else if let Some(artist_id) = params.artist_id {
            queries::tracks_for_artist(&db.conn, artist_id)
                .map_err(|e| format!("db error: {}", e))?
        } else {
            queries::all_tracks(&db.conn).map_err(|e| format!("db error: {}", e))?
        };
        Ok(Json(tracks.iter().map(track_row_to_response).collect()))
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
            // Get title/artist/album from the playlist item, not TrackInfo (which only has codec data).
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
    fn list_devices(&self) -> Result<Json<Vec<DeviceResponse>>, String> {
        let devices = device::list_output_devices().map_err(|e| format!("device error: {}", e))?;
        Ok(Json(
            devices
                .iter()
                .map(|d| DeviceResponse {
                    name: d.name.clone(),
                    sample_rates: d.sample_rates.clone(),
                })
                .collect(),
        ))
    }

    #[tool(description = "Switch audio output to a different device by name")]
    fn set_device(
        &self,
        Parameters(params): Parameters<SetDeviceParams>,
    ) -> Result<Json<String>, String> {
        self.cmd_tx
            .send(PlayerCommand::SetOutputDevice(params.device_name.clone()))
            .map_err(|e| format!("send error: {}", e))?;
        Ok(Json(format!("switched to device '{}'", params.device_name)))
    }

    // -----------------------------------------------------------------------
    // Favourites
    // -----------------------------------------------------------------------

    #[tool(description = "Star/favourite a track by its library track ID")]
    fn favourite(
        &self,
        Parameters(params): Parameters<FavouriteParams>,
    ) -> Result<Json<String>, String> {
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
        Ok(Json(format!("favourited track {}", params.track_id)))
    }

    #[tool(description = "Unstar/unfavourite a track by its library track ID")]
    fn unfavourite(
        &self,
        Parameters(params): Parameters<FavouriteParams>,
    ) -> Result<Json<String>, String> {
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
        Ok(Json(format!("unfavourited track {}", params.track_id)))
    }

    #[tool(description = "List all favourited/starred tracks")]
    fn list_favourites(&self) -> Result<Json<Vec<TrackResponse>>, String> {
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
        Ok(Json(tracks))
    }
}

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

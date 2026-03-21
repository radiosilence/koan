use std::path::PathBuf;
use std::sync::Arc;

use async_graphql::connection::{Connection, Edge, EmptyFields};
use async_graphql::{Context, EmptySubscription, Enum, Object, Schema, SimpleObject};
use crossbeam_channel::Sender;
use koan_core::audio::device;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueItemId, SharedPlayerState};
use uuid::Uuid;

use super::open_db;

// ---------------------------------------------------------------------------
// Pagination helper — uses usize as cursor (async-graphql has built-in impl)
// ---------------------------------------------------------------------------

fn paginate<T: async_graphql::OutputType>(
    items: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> async_graphql::Result<Connection<usize, T, EmptyFields, EmptyFields>> {
    let total = items.len();

    let start = if let Some(ref cursor) = after {
        cursor.parse::<usize>().unwrap_or(0) + 1
    } else {
        0
    };

    let end = if let Some(f) = first {
        (start + f as usize).min(total)
    } else {
        total
    };

    let mut conn = Connection::new(start > 0, end < total);
    for (i, item) in items.into_iter().enumerate().skip(start).take(end - start) {
        conn.edges.push(Edge::new(i, item));
    }
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum PlaybackStateEnum {
    Stopped,
    Playing,
    Paused,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum TrackSource {
    Local,
    Remote,
    Cached,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum ArtistSortField {
    Name,
    TrackCount,
    AlbumCount,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum AlbumSortField {
    Title,
    Date,
    ArtistThenDate,
    TrackCount,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum TrackSortField {
    Title,
    Artist,
    Album,
    Duration,
    ArtistAlbumDiscTrack,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum SortDirection {
    Asc,
    Desc,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

struct GqlArtist {
    row: queries::ArtistRow,
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

struct GqlAlbum {
    row: queries::AlbumRow,
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

struct GqlTrack {
    row: queries::TrackRow,
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
}

#[derive(SimpleObject)]
struct GqlNowPlaying {
    state: PlaybackStateEnum,
    position_ms: u64,
    duration_ms: Option<u64>,
    track: Option<GqlNowPlayingTrack>,
    queue_item_id: Option<String>,
}

#[derive(SimpleObject)]
struct GqlNowPlayingTrack {
    title: String,
    artist: String,
    album: String,
    codec: String,
    sample_rate: u32,
    bit_depth: u16,
    channels: u16,
    duration_ms: u64,
}

struct GqlQueueEntry {
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
struct GqlLibraryStats {
    total_tracks: i64,
    local_tracks: i64,
    remote_tracks: i64,
    cached_tracks: i64,
    total_albums: i64,
    total_artists: i64,
}

#[derive(SimpleObject)]
struct GqlDevice {
    name: String,
    sample_rates: Vec<f64>,
}

/// Mutation/query result status.
struct GqlStatus {
    success: bool,
    message: String,
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
    fn success(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: msg.into(),
        }
    }
}

struct GqlQueueMutationResult {
    success: bool,
    message: String,
    added_count: i32,
    queue_item_ids: Vec<String>,
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

// ---------------------------------------------------------------------------
// DB handle wrapper (so we can put it in Context)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DbHandle {
    path: PathBuf,
}

impl DbHandle {
    fn open(&self) -> async_graphql::Result<Database> {
        Database::open(&self.path)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// Query root
// ---------------------------------------------------------------------------

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    #[allow(clippy::too_many_arguments)]
    async fn artists(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<i64>>,
        search: Option<String>,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "ArtistSortField::Name")] _sort_by: ArtistSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<Connection<usize, GqlArtist, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let mut artists = if let Some(ref query) = search {
            queries::find_artists(&db.conn, query)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else {
            queries::all_artists(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        };

        if let Some(ref id_list) = ids {
            artists.retain(|a| id_list.contains(&a.id));
        }

        paginate(
            artists.into_iter().map(|row| GqlArtist { row }).collect(),
            after,
            first,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn albums(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<i64>>,
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
        search: Option<String>,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "AlbumSortField::ArtistThenDate")] _sort_by: AlbumSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<Connection<usize, GqlAlbum, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;

        let mut albums = if let Some(aid) = artist_id {
            queries::albums_for_artist(&db.conn, aid)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else if let Some(ref aids) = artist_ids {
            let mut all = Vec::new();
            for &aid in aids {
                let mut a = queries::albums_for_artist(&db.conn, aid)
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                all.append(&mut a);
            }
            all
        } else {
            queries::all_albums(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        };

        if let Some(ref id_list) = ids {
            albums.retain(|a| id_list.contains(&a.id));
        }

        if let Some(ref query) = search {
            let q = query.to_lowercase();
            albums.retain(|a| {
                a.title.to_lowercase().contains(&q) || a.artist_name.to_lowercase().contains(&q)
            });
        }

        paginate(
            albums.into_iter().map(|row| GqlAlbum { row }).collect(),
            after,
            first,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn tracks(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<i64>>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
        search: Option<String>,
        source: Option<TrackSource>,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "TrackSortField::ArtistAlbumDiscTrack")] _sort_by: TrackSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;

        let mut tracks = if let Some(ref query) = search {
            queries::search_tracks_paged(&db.conn, query, 10000, 0)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else if let Some(album) = album_id {
            queries::tracks_for_album(&db.conn, album)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else if let Some(ref aids) = artist_ids {
            let mut all = Vec::new();
            for &aid in aids {
                let mut t = queries::tracks_for_artist(&db.conn, aid)
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                all.append(&mut t);
            }
            all
        } else if let Some(aid) = artist_id {
            queries::tracks_for_artist(&db.conn, aid)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else {
            queries::all_tracks(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        };

        if let Some(ref id_list) = ids {
            tracks.retain(|t| id_list.contains(&t.id));
        }

        if let Some(src) = source {
            let src_str = match src {
                TrackSource::Local => "local",
                TrackSource::Remote => "remote",
                TrackSource::Cached => "cached",
            };
            tracks.retain(|t| t.source == src_str);
        }

        paginate(
            tracks.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn track(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GqlTrack>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let row = queries::get_track_row(&db.conn, id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(row.map(|row| GqlTrack { row }))
    }

    async fn random_tracks(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 20)] count: i32,
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
        let db = ctx.data::<DbHandle>()?.open()?;

        if let Some(ref aids) = artist_ids {
            let mut all = Vec::new();
            let per = (count as u32 / aids.len() as u32).max(1);
            for &aid in aids {
                let mut t = queries::random_tracks(&db.conn, per, Some(aid))
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                all.append(&mut t);
            }
            all.truncate(count as usize);
            Ok(all.into_iter().map(|row| GqlTrack { row }).collect())
        } else {
            let tracks = queries::random_tracks(&db.conn, count as u32, artist_id)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            Ok(tracks.into_iter().map(|row| GqlTrack { row }).collect())
        }
    }

    async fn now_playing(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlNowPlaying> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let playback_state = match state.playback_state() {
            PlaybackState::Stopped => PlaybackStateEnum::Stopped,
            PlaybackState::Playing => PlaybackStateEnum::Playing,
            PlaybackState::Paused => PlaybackStateEnum::Paused,
        };
        let position_ms = state.position_ms();
        let (track, queue_item_id, duration_ms) = if let Some(info) = state.track_info() {
            let (items, _cursor) = state.snapshot_playlist();
            let playlist_item = items.iter().find(|i| i.id == info.id);
            let track = GqlNowPlayingTrack {
                title: playlist_item.map(|i| i.title.clone()).unwrap_or_default(),
                artist: playlist_item.map(|i| i.artist.clone()).unwrap_or_default(),
                album: playlist_item.map(|i| i.album.clone()).unwrap_or_default(),
                codec: info.codec.clone(),
                sample_rate: info.sample_rate,
                bit_depth: info.bit_depth,
                channels: info.channels,
                duration_ms: info.duration_ms,
            };
            (
                Some(track),
                Some(info.id.0.to_string()),
                Some(info.duration_ms),
            )
        } else {
            (None, None, None)
        };

        Ok(GqlNowPlaying {
            state: playback_state,
            position_ms,
            duration_ms,
            track,
            queue_item_id,
        })
    }

    async fn queue(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlQueueEntry>> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let (items, cursor) = state.snapshot_playlist();
        Ok(items
            .iter()
            .map(|item| GqlQueueEntry {
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
            .collect())
    }

    async fn library_stats(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlLibraryStats> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let stats = queries::library_stats(&db.conn)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(GqlLibraryStats {
            total_tracks: stats.total_tracks,
            local_tracks: stats.local_tracks,
            remote_tracks: stats.remote_tracks,
            cached_tracks: stats.cached_tracks,
            total_albums: stats.total_albums,
            total_artists: stats.total_artists,
        })
    }

    async fn devices(&self) -> async_graphql::Result<Vec<GqlDevice>> {
        let devices = device::list_output_devices()
            .map_err(|e| async_graphql::Error::new(format!("device error: {}", e)))?;
        Ok(devices
            .iter()
            .map(|d| GqlDevice {
                name: d.name.clone(),
                sample_rates: d.sample_rates.clone(),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Mutation root
// ---------------------------------------------------------------------------

pub struct MutationRoot;

fn parse_queue_item_id(s: &str) -> async_graphql::Result<QueueItemId> {
    Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|e| async_graphql::Error::new(format!("invalid queue item ID '{}': {}", s, e)))
}

fn send_cmd(ctx: &Context<'_>, cmd: PlayerCommand) -> async_graphql::Result<()> {
    let tx = ctx.data::<Sender<PlayerCommand>>()?;
    tx.send(cmd)
        .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))
}

#[Object]
impl MutationRoot {
    // -- Playback --

    async fn play(
        &self,
        ctx: &Context<'_>,
        queue_item_id: String,
    ) -> async_graphql::Result<GqlStatus> {
        let id = parse_queue_item_id(&queue_item_id)?;
        send_cmd(ctx, PlayerCommand::Play(id))?;
        Ok(GqlStatus::success("playing"))
    }

    async fn pause(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Pause)?;
        Ok(GqlStatus::success("paused"))
    }

    async fn resume(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Resume)?;
        Ok(GqlStatus::success("resumed"))
    }

    async fn stop(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Stop)?;
        Ok(GqlStatus::success("stopped"))
    }

    async fn next(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::NextTrack)?;
        Ok(GqlStatus::success("skipped to next"))
    }

    async fn previous(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::PrevTrack)?;
        Ok(GqlStatus::success("skipped to previous"))
    }

    async fn seek(&self, ctx: &Context<'_>, position_ms: i64) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Seek(position_ms as u64))?;
        Ok(GqlStatus::success(format!("seeked to {}ms", position_ms)))
    }

    // -- Queue --

    async fn add_to_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let mut items = Vec::new();
        let mut queue_item_ids = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, &db);
                queue_item_ids.push(item.id.0.to_string());
                items.push(item);
            }
        }

        let count = items.len() as i32;
        if !items.is_empty() {
            tx.send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

            // Auto-play if stopped
            if state.playback_state() == PlaybackState::Stopped
                && let Some(first_id) = queue_item_ids.first()
                && let Ok(id) = Uuid::parse_str(first_id).map(QueueItemId)
            {
                let _ = tx.send(PlayerCommand::Play(id));
            }
        }

        Ok(GqlQueueMutationResult {
            success: true,
            message: format!("queued {} tracks", count),
            added_count: count,
            queue_item_ids,
        })
    }

    async fn replace_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        tx.send(PlayerCommand::ClearPlaylist)
            .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

        let mut items = Vec::new();
        let mut queue_item_ids = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, &db);
                queue_item_ids.push(item.id.0.to_string());
                items.push(item);
            }
        }

        let count = items.len() as i32;
        let first_id = items.first().map(|i| i.id);
        if !items.is_empty() {
            tx.send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

            if let Some(id) = first_id {
                let _ = tx.send(PlayerCommand::Play(id));
            }
        }

        Ok(GqlQueueMutationResult {
            success: true,
            message: format!("replaced queue with {} tracks", count),
            added_count: count,
            queue_item_ids,
        })
    }

    async fn remove_from_queue(
        &self,
        ctx: &Context<'_>,
        queue_item_ids: Vec<String>,
    ) -> async_graphql::Result<GqlStatus> {
        let ids: Vec<QueueItemId> = queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let count = ids.len();
        send_cmd(ctx, PlayerCommand::RemoveFromPlaylistBatch(ids))?;
        Ok(GqlStatus::success(format!(
            "removed {} items from queue",
            count
        )))
    }

    async fn move_in_queue(
        &self,
        ctx: &Context<'_>,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> async_graphql::Result<GqlStatus> {
        let ids: Vec<QueueItemId> = queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let target = parse_queue_item_id(&target_queue_item_id)?;
        send_cmd(
            ctx,
            PlayerCommand::MoveItemsInPlaylist { ids, target, after },
        )?;
        Ok(GqlStatus::success("queue reordered"))
    }

    async fn clear_queue(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::ClearPlaylist)?;
        Ok(GqlStatus::success("queue cleared"))
    }

    async fn undo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Undo)?;
        Ok(GqlStatus::success("undone"))
    }

    async fn redo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Redo)?;
        Ok(GqlStatus::success("redone"))
    }

    // -- Device --

    async fn set_device(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::SetOutputDevice(name.clone()))?;
        Ok(GqlStatus::success(format!("switched to device '{}'", name)))
    }

    async fn clear_device(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::ClearOutputDevice)?;
        Ok(GqlStatus::success("device cleared, using system default"))
    }

    // -- Favourites --

    async fn favourite(&self, ctx: &Context<'_>, track_id: i64) -> async_graphql::Result<GqlTrack> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        queries::add_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(GqlTrack { row: track })
    }

    async fn unfavourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        queries::remove_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(GqlTrack { row: track })
    }

    async fn toggle_favourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        queries::toggle_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(GqlTrack { row: track })
    }
}

// ---------------------------------------------------------------------------
// Helper: TrackRow -> PlaylistItem (for queue mutations via GraphQL)
// ---------------------------------------------------------------------------

fn track_to_playlist_item(
    track: &queries::TrackRow,
    db: &Database,
) -> koan_core::player::state::PlaylistItem {
    use koan_core::player::state::{LoadState, PlaylistItem};

    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let (path, load_state) = if let Some(ref p) = track.path {
        (std::path::PathBuf::from(p), LoadState::Ready)
    } else if let Some(ref cp) = track.cached_path {
        (std::path::PathBuf::from(cp), LoadState::Ready)
    } else {
        (
            std::path::PathBuf::from(format!("/tmp/koan-pending-{}", track.id)),
            LoadState::Pending,
        )
    };

    let year = album_date.as_deref().and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });

    PlaylistItem {
        id: QueueItemId::new(),
        path,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

// ---------------------------------------------------------------------------
// Schema builder
// ---------------------------------------------------------------------------

pub type KoanSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

pub fn build_schema(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
) -> KoanSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(DbHandle { path: db_path })
        .data(state)
        .data(cmd_tx)
        .finish()
}

// ---------------------------------------------------------------------------
// `koan graphql` entry point
// ---------------------------------------------------------------------------

pub fn cmd_graphql(port: Option<u16>, playground: Option<bool>) {
    use axum::routing::{get, post};
    use koan_core::player::Player;

    let _db = open_db();
    let db_path = koan_core::config::db_path();

    let cfg = Config::load().unwrap_or_default();
    let port = port.unwrap_or(cfg.graphql.port);
    let playground_enabled = playground.unwrap_or(cfg.graphql.playground);

    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    let schema = build_schema(state, cmd_tx, db_path);

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let mut app = axum::Router::new().route("/graphql", post(graphql_handler));

        if playground_enabled {
            app = app.route("/graphql", get(graphql_playground));
        }

        let app = app.with_state(schema);

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("koan graphql server listening on http://0.0.0.0:{}", port);
        if playground_enabled {
            eprintln!("  GraphQL Playground: http://localhost:{}/graphql", port);
        }

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("failed to bind");
        axum::serve(listener, app).await.expect("server error");
    });
}

async fn graphql_handler(
    axum::extract::State(schema): axum::extract::State<KoanSchema>,
    req: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

async fn graphql_playground() -> axum::response::Html<String> {
    axum::response::Html(async_graphql::http::playground_source(
        async_graphql::http::GraphQLPlaygroundConfig::new("/graphql"),
    ))
}

// ---------------------------------------------------------------------------
// In-process execution (for MCP `graphql` tool)
// ---------------------------------------------------------------------------

/// Execute a GraphQL query in-process (no HTTP round-trip).
pub async fn execute_in_process(
    schema: &KoanSchema,
    query: &str,
    variables: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut request = async_graphql::Request::new(query);
    if let Some(serde_json::Value::Object(map)) = variables {
        let mut gql_vars = async_graphql::Variables::default();
        for (k, v) in map {
            gql_vars.insert(
                async_graphql::Name::new(&k),
                async_graphql::Value::from_json(v).unwrap_or(async_graphql::Value::Null),
            );
        }
        request = request.variables(gql_vars);
    }
    let response = schema.execute(request).await;
    serde_json::to_value(&response).unwrap_or(serde_json::Value::Null)
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

    fn test_schema() -> (
        KoanSchema,
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

        let schema = build_schema(state, tx, db_path);
        (schema, rx, tmp)
    }

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

    #[test]
    fn schema_builds() {
        let (_schema, _rx, _tmp) = test_schema();
    }

    #[tokio::test]
    async fn library_stats_query() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Track1", "Artist1", "Album1");

        let resp = schema
            .execute("{ libraryStats { totalTracks totalAlbums totalArtists } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["libraryStats"]["totalTracks"], 1);
        assert_eq!(data["libraryStats"]["totalAlbums"], 1);
        assert_eq!(data["libraryStats"]["totalArtists"], 1);
    }

    #[tokio::test]
    async fn artists_query() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "T1", "Aphex Twin", "Drukqs");
        insert_test_track(&db_path, "T2", "Boards of Canada", "MHTRTC");

        let resp = schema
            .execute("{ artists { edges { node { id name } } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let edges = data["artists"]["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 2);
    }

    #[tokio::test]
    async fn tracks_search() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Windowlicker", "Aphex Twin", "Windowlicker EP");
        insert_test_track(&db_path, "Roygbiv", "Boards of Canada", "MHTRTC");

        let resp = schema
            .execute(r#"{ tracks(search: "Aphex") { edges { node { id title artist } } } }"#)
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let edges = data["tracks"]["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["node"]["title"], "Windowlicker");
    }

    #[tokio::test]
    async fn now_playing_stopped() {
        let (schema, _rx, _tmp) = test_schema();
        let resp = schema
            .execute("{ nowPlaying { state positionMs track { title } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["nowPlaying"]["state"], "STOPPED");
    }

    #[tokio::test]
    async fn pause_mutation() {
        let (schema, rx, _tmp) = test_schema();
        let resp = schema.execute("mutation { pause { ok message } }").await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["pause"]["ok"], true);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::Pause));
    }

    #[tokio::test]
    async fn nested_artist_albums_tracks() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Vordhosbn", "Aphex Twin", "Drukqs");
        insert_test_track(&db_path, "Avril 14th", "Aphex Twin", "Drukqs");

        let resp = schema
            .execute(
                r#"{ artists(search: "Aphex") {
                    edges { node {
                        name
                        albums { edges { node {
                            title
                            tracks { edges { node { title } } }
                        } } }
                    } }
                } }"#,
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let artist = &data["artists"]["edges"][0]["node"];
        assert_eq!(artist["name"], "Aphex Twin");
        let album = &artist["albums"]["edges"][0]["node"];
        assert_eq!(album["title"], "Drukqs");
        let tracks = album["tracks"]["edges"].as_array().unwrap();
        assert_eq!(tracks.len(), 2);
    }

    #[tokio::test]
    async fn pagination_has_next() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        for i in 0..5 {
            insert_test_track(
                &db_path,
                &format!("Track{}", i),
                "Artist",
                &format!("Album{}", i),
            );
        }

        let resp = schema
            .execute(
                r#"{ artists(first: 1) {
                    edges { node { name } cursor }
                    pageInfo { hasNextPage endCursor }
                } }"#,
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        // Only 1 artist ("Artist"), so hasNextPage should be false
        // since all 5 tracks are by the same artist.
        assert_eq!(data["artists"]["edges"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn clear_queue_mutation() {
        let (schema, rx, _tmp) = test_schema();
        let resp = schema
            .execute("mutation { clearQueue { ok message } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::ClearPlaylist));
    }
}

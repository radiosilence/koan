use std::path::PathBuf;
use std::sync::Arc;

use async_graphql::{Context, EmptySubscription, Object, Schema};
use crossbeam_channel::Sender;
use koan_core::audio::device;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueItemId, SharedPlayerState};
use uuid::Uuid;

use super::types::*;
use crate::tui::app::PickerAction;

// ---------------------------------------------------------------------------
// Context data injected into every request
// ---------------------------------------------------------------------------

pub struct GqlContext {
    pub state: Arc<SharedPlayerState>,
    pub cmd_tx: Sender<PlayerCommand>,
    pub db_path: PathBuf,
}

impl GqlContext {
    fn open_db(&self) -> async_graphql::Result<Database> {
        Database::open(&self.db_path).map_err(|e| async_graphql::Error::new(format!("db: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Query root
// ---------------------------------------------------------------------------

pub struct QueryRoot;

// GraphQL resolvers take one parameter per query argument — inherent to the framework.
#[allow(clippy::too_many_arguments)]
#[Object]
impl QueryRoot {
    /// List artists with optional filtering, sorting, and cursor pagination.
    /// Returns ALL artists by default when no pagination args are provided.
    async fn artists(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        search: Option<String>,
        ids: Option<Vec<i32>>,
        #[graphql(default_with = "ArtistSortField::Name")] _sort_by: ArtistSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<GqlArtistConnection> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;

        let all = if let Some(ref q) = search {
            queries::find_artists(&db.conn, q)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        } else {
            queries::all_artists(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        };

        // Filter by IDs if provided.
        let all: Vec<_> = if let Some(ref ids) = ids {
            let id_set: std::collections::HashSet<i64> = ids.iter().map(|&i| i as i64).collect();
            all.into_iter().filter(|a| id_set.contains(&a.id)).collect()
        } else {
            all
        };

        let total = all.len() as i32;
        let (page, start, has_prev, has_next) =
            paginate(&all, first, after.as_deref(), last, before.as_deref());

        let edges: Vec<GqlArtistEdge> = page
            .iter()
            .enumerate()
            .map(|(i, a)| GqlArtistEdge {
                cursor: encode_cursor(start + i),
                node: GqlArtistFlat {
                    id: a.id,
                    name: a.name.clone(),
                    mbid: None,
                },
            })
            .collect();

        let page_info = make_page_info(start, edges.len(), has_prev, has_next);

        Ok(GqlArtistConnection {
            edges,
            page_info,
            total_count: total,
        })
    }

    /// List albums with optional filtering, sorting, and cursor pagination.
    /// Returns ALL albums by default.
    async fn albums(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        artist_id: Option<i32>,
        artist_ids: Option<Vec<i32>>,
        ids: Option<Vec<i32>>,
        _search: Option<String>,
        #[graphql(default_with = "AlbumSortField::ArtistThenDate")] _sort_by: AlbumSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<GqlAlbumConnection> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;

        let mut all = if let Some(aid) = artist_id {
            queries::albums_for_artist(&db.conn, aid as i64)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        } else if let Some(ref aids) = artist_ids {
            let mut result = Vec::new();
            for &aid in aids {
                let mut albums = queries::albums_for_artist(&db.conn, aid as i64)
                    .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
                result.append(&mut albums);
            }
            result
        } else {
            queries::all_albums(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        };

        if let Some(ref ids) = ids {
            let id_set: std::collections::HashSet<i64> = ids.iter().map(|&i| i as i64).collect();
            all.retain(|a| id_set.contains(&a.id));
        }

        let total = all.len() as i32;
        let (page, start, has_prev, has_next) =
            paginate(&all, first, after.as_deref(), last, before.as_deref());

        let edges: Vec<GqlAlbumEdge> = page
            .iter()
            .enumerate()
            .map(|(i, a)| GqlAlbumEdge {
                cursor: encode_cursor(start + i),
                node: GqlAlbumFlat {
                    id: a.id,
                    title: a.title.clone(),
                    artist_id: a.artist_id,
                    artist_name: a.artist_name.clone(),
                    date: a.date.clone(),
                    codec: a.codec.clone(),
                    label: a.label.clone(),
                },
            })
            .collect();

        let page_info = make_page_info(start, edges.len(), has_prev, has_next);

        Ok(GqlAlbumConnection {
            edges,
            page_info,
            total_count: total,
        })
    }

    /// List tracks with optional filtering, sorting, and cursor pagination.
    /// Returns ALL tracks by default.
    async fn tracks(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        album_id: Option<i32>,
        artist_id: Option<i32>,
        artist_ids: Option<Vec<i32>>,
        ids: Option<Vec<i32>>,
        search: Option<String>,
        source: Option<TrackSource>,
        #[graphql(default_with = "TrackSortField::ArtistAlbumDiscTrack")] _sort_by: TrackSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<GqlTrackConnection> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;

        let all = if let Some(ref q) = search {
            queries::search_tracks_paged(&db.conn, q, 10000, 0)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        } else if let Some(aid) = album_id {
            queries::tracks_for_album(&db.conn, aid as i64)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        } else if let Some(ref aids) = artist_ids {
            let mut result = Vec::new();
            for &aid in aids {
                let mut tracks = queries::tracks_for_artist(&db.conn, aid as i64)
                    .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
                result.append(&mut tracks);
            }
            result
        } else if let Some(aid) = artist_id {
            queries::tracks_for_artist(&db.conn, aid as i64)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        } else {
            queries::all_tracks(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        };

        // Filter by IDs.
        let all: Vec<_> = if let Some(ref ids) = ids {
            let id_set: std::collections::HashSet<i64> = ids.iter().map(|&i| i as i64).collect();
            all.into_iter().filter(|t| id_set.contains(&t.id)).collect()
        } else {
            all
        };

        // Filter by source.
        let all: Vec<_> = if let Some(src) = source {
            let src_str = match src {
                TrackSource::Local => "local",
                TrackSource::Remote => "remote",
                TrackSource::Cached => "cached",
            };
            all.into_iter().filter(|t| t.source == src_str).collect()
        } else {
            all
        };

        let total = all.len() as i32;
        let (page, start, has_prev, has_next) =
            paginate(&all, first, after.as_deref(), last, before.as_deref());

        let edges: Vec<GqlTrackEdge> = page
            .iter()
            .enumerate()
            .map(|(i, t)| GqlTrackEdge {
                cursor: encode_cursor(start + i),
                node: GqlTrack::from_row(t),
            })
            .collect();

        let page_info = make_page_info(start, edges.len(), has_prev, has_next);

        Ok(GqlTrackConnection {
            edges,
            page_info,
            total_count: total,
        })
    }

    /// Get a single track by ID.
    async fn track(&self, ctx: &Context<'_>, id: i32) -> async_graphql::Result<Option<GqlTrack>> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;
        let row = queries::get_track_row(&db.conn, id as i64)
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
        Ok(row.map(|r| GqlTrack::from_row(&r)))
    }

    /// Get random tracks from the library.
    async fn random_tracks(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 20)] count: i32,
        artist_id: Option<i32>,
        artist_ids: Option<Vec<i32>>,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;
        let count = count.clamp(1, 500) as u32;

        let tracks = if let Some(ref aids) = artist_ids {
            // Get random tracks from multiple artists.
            let mut all = Vec::new();
            for &aid in aids {
                let mut t = queries::random_tracks(&db.conn, count, Some(aid as i64))
                    .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
                all.append(&mut t);
            }
            all.truncate(count as usize);
            all
        } else {
            queries::random_tracks(&db.conn, count, artist_id.map(|a| a as i64))
                .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
        };

        Ok(tracks.iter().map(GqlTrack::from_row).collect())
    }

    /// Get the currently playing track, playback state, and position.
    async fn now_playing(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlNowPlaying> {
        let gql = ctx.data::<GqlContext>()?;
        let playback_state = gql.state.playback_state();
        let state = match playback_state {
            PlaybackState::Stopped => PlaybackStateEnum::Stopped,
            PlaybackState::Playing => PlaybackStateEnum::Playing,
            PlaybackState::Paused => PlaybackStateEnum::Paused,
        };
        let position_ms = gql.state.position_ms() as i64;

        let track = gql.state.track_info().map(|info| {
            let (items, _cursor) = gql.state.snapshot_playlist();
            let playlist_item = items.iter().find(|i| i.id == info.id);
            GqlNowPlayingTrack {
                queue_item_id: info.id.0.to_string(),
                title: playlist_item.map(|i| i.title.clone()).unwrap_or_default(),
                artist: playlist_item.map(|i| i.artist.clone()).unwrap_or_default(),
                album: playlist_item.map(|i| i.album.clone()).unwrap_or_default(),
                codec: Some(info.codec.clone()),
                sample_rate: Some(info.sample_rate as i32),
                bit_depth: Some(info.bit_depth as i32),
                channels: Some(info.channels as i32),
                duration_ms: Some(info.duration_ms as i64),
            }
        });

        let queue_item_id = track.as_ref().map(|t| t.queue_item_id.clone());
        let duration_ms = track.as_ref().and_then(|t| t.duration_ms);

        Ok(GqlNowPlaying {
            state,
            position_ms,
            duration_ms,
            track,
            queue_item_id,
        })
    }

    /// Get the current play queue.
    async fn queue(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlQueueConnection> {
        let gql = ctx.data::<GqlContext>()?;
        let (items, cursor) = gql.state.snapshot_playlist();

        let mut current_index = None;
        let edges: Vec<GqlQueueEdge> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_current = cursor == Some(item.id);
                if is_current {
                    current_index = Some(i as i32);
                }
                GqlQueueEdge {
                    cursor: encode_cursor(i),
                    node: GqlQueueEntry {
                        queue_item_id: item.id.0.to_string(),
                        title: item.title.clone(),
                        artist: item.artist.clone(),
                        album: item.album.clone(),
                        codec: item.codec.clone(),
                        track_number: item.track_number,
                        disc: item.disc,
                        duration_ms: item.duration_ms.map(|d| d as i64),
                        is_current,
                    },
                }
            })
            .collect();

        Ok(GqlQueueConnection {
            total_count: edges.len() as i32,
            edges,
            current_index,
        })
    }

    /// Get library statistics.
    async fn library_stats(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlLibraryStats> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;
        let stats = queries::library_stats(&db.conn)
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;

        Ok(GqlLibraryStats {
            total_tracks: stats.total_tracks,
            local_tracks: stats.local_tracks,
            remote_tracks: stats.remote_tracks,
            cached_tracks: stats.cached_tracks,
            total_albums: stats.total_albums,
            total_artists: stats.total_artists,
        })
    }

    /// List available audio output devices.
    async fn devices(&self) -> async_graphql::Result<Vec<GqlDevice>> {
        let devices = device::list_output_devices()
            .map_err(|e| async_graphql::Error::new(format!("device: {e}")))?;

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
        .map_err(|e| async_graphql::Error::new(format!("invalid queue item ID '{s}': {e}")))
}

#[Object]
impl MutationRoot {
    /// Play a specific queue item by its queue item ID.
    async fn play(
        &self,
        ctx: &Context<'_>,
        queue_item_id: String,
    ) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        let id = parse_queue_item_id(&queue_item_id)?;
        gql.cmd_tx
            .send(PlayerCommand::Play(id))
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("playing"))
    }

    /// Pause playback.
    async fn pause(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::Pause)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("paused"))
    }

    /// Resume playback.
    async fn resume(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::Resume)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("resumed"))
    }

    /// Stop playback.
    async fn stop(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::Stop)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("stopped"))
    }

    /// Skip to the next track.
    async fn next(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::NextTrack)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("skipped to next"))
    }

    /// Skip to the previous track.
    async fn previous(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::PrevTrack)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("skipped to previous"))
    }

    /// Seek to a position in the current track (milliseconds).
    async fn seek(&self, ctx: &Context<'_>, position_ms: i32) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::Seek(position_ms as u64))
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok(format!("seeked to {position_ms}ms")))
    }

    /// Add tracks to the end of the queue by their library track IDs.
    async fn add_to_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i32>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let gql = ctx.data::<GqlContext>()?;
        let count = track_ids.len() as i32;
        let ids: Vec<i64> = track_ids.iter().map(|&i| i as i64).collect();
        enqueue_tracks_bg(
            ids,
            PickerAction::Append,
            gql.cmd_tx.clone(),
            gql.state.clone(),
        );
        Ok(GqlQueueMutationResult {
            ok: true,
            message: format!("queueing {count} tracks"),
            added_count: count,
            queue_item_ids: vec![],
        })
    }

    /// Replace the entire queue with new tracks and start playing.
    async fn replace_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i32>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let gql = ctx.data::<GqlContext>()?;
        let count = track_ids.len() as i32;
        let ids: Vec<i64> = track_ids.iter().map(|&i| i as i64).collect();
        enqueue_tracks_bg(
            ids,
            PickerAction::ReplaceQueue,
            gql.cmd_tx.clone(),
            gql.state.clone(),
        );
        Ok(GqlQueueMutationResult {
            ok: true,
            message: format!("replacing queue with {count} tracks"),
            added_count: count,
            queue_item_ids: vec![],
        })
    }

    /// Remove tracks from the queue by their queue item IDs.
    async fn remove_from_queue(
        &self,
        ctx: &Context<'_>,
        queue_item_ids: Vec<String>,
    ) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        let ids: Vec<QueueItemId> = queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let count = ids.len();
        gql.cmd_tx
            .send(PlayerCommand::RemoveFromPlaylistBatch(ids))
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok(format!("removed {count} items")))
    }

    /// Move items within the queue to before/after a target.
    async fn move_in_queue(
        &self,
        ctx: &Context<'_>,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        let ids: Vec<QueueItemId> = queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let target = parse_queue_item_id(&target_queue_item_id)?;
        gql.cmd_tx
            .send(PlayerCommand::MoveItemsInPlaylist { ids, target, after })
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("queue reordered"))
    }

    /// Clear the entire queue and stop playback.
    async fn clear_queue(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::ClearPlaylist)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("queue cleared"))
    }

    /// Undo the last queue operation.
    async fn undo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::Undo)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("undone"))
    }

    /// Redo the last undone queue operation.
    async fn redo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::Redo)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("redone"))
    }

    /// Switch audio output to a different device by name.
    async fn set_device(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::SetOutputDevice(name.clone()))
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok(format!("switched to device '{name}'")))
    }

    /// Clear the configured output device, reverting to system default.
    async fn clear_device(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let gql = ctx.data::<GqlContext>()?;
        gql.cmd_tx
            .send(PlayerCommand::ClearOutputDevice)
            .map_err(|e| async_graphql::Error::new(format!("send: {e}")))?;
        Ok(GqlStatus::ok("reverted to default device"))
    }

    /// Star/favourite a track by its library track ID.
    async fn favourite(&self, ctx: &Context<'_>, track_id: i32) -> async_graphql::Result<GqlTrack> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;
        let track = queries::get_track_row(&db.conn, track_id as i64)
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {track_id} not found")))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {track_id} has no path")))?;
        queries::add_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
        Ok(GqlTrack::from_row(&track))
    }

    /// Unstar/unfavourite a track by its library track ID.
    async fn unfavourite(
        &self,
        ctx: &Context<'_>,
        track_id: i32,
    ) -> async_graphql::Result<GqlTrack> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;
        let track = queries::get_track_row(&db.conn, track_id as i64)
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {track_id} not found")))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {track_id} has no path")))?;
        queries::remove_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
        Ok(GqlTrack::from_row(&track))
    }

    /// Toggle favourite on a track — returns the track with updated state.
    async fn toggle_favourite(
        &self,
        ctx: &Context<'_>,
        track_id: i32,
    ) -> async_graphql::Result<GqlTrack> {
        let gql = ctx.data::<GqlContext>()?;
        let db = gql.open_db()?;
        let track = queries::get_track_row(&db.conn, track_id as i64)
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {track_id} not found")))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {track_id} has no path")))?;
        queries::toggle_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db: {e}")))?;
        Ok(GqlTrack::from_row(&track))
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
    let ctx = GqlContext {
        state,
        cmd_tx,
        db_path,
    };

    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(ctx)
        .finish()
}

// ---------------------------------------------------------------------------
// Background enqueue helper (same as MCP)
// ---------------------------------------------------------------------------

fn enqueue_tracks_bg(
    ids: Vec<i64>,
    action: PickerAction,
    tx: Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    let log_buf: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    std::thread::Builder::new()
        .name("koan-gql-enqueue".into())
        .spawn(move || {
            crate::commands::enqueue_playlist(ids, action, tx, log_buf, state);
        })
        .expect("failed to spawn enqueue thread");
}

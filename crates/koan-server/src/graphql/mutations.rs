use std::sync::Arc;

use async_graphql::{Context, Object};
use crossbeam_channel::Sender;
use koan_core::config::Config;
use koan_core::db::queries;
use koan_core::db::queries::playback_state::PersistedQueueItem;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueItemId, SharedPlayerState};
use uuid::Uuid;

use koan_core::auth::Role;

use super::helpers::{spawn_downloads, sync_favourite_to_remote, track_to_playlist_item};
use super::types::*;
use super::{DbHandle, parse_queue_item_id, require_role, send_cmd};

// ---------------------------------------------------------------------------
// Mutation root
// ---------------------------------------------------------------------------

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    // -- Playback --

    async fn play(
        &self,
        ctx: &Context<'_>,
        queue_item_id: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let id = parse_queue_item_id(&queue_item_id)?;
        send_cmd(ctx, PlayerCommand::Play(id))?;
        Ok(GqlStatus::success("playing"))
    }

    async fn pause(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Pause)?;
        Ok(GqlStatus::success("paused"))
    }

    async fn resume(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Resume)?;
        Ok(GqlStatus::success("resumed"))
    }

    async fn stop(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Stop)?;
        Ok(GqlStatus::success("stopped"))
    }

    async fn next(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::NextTrack)?;
        Ok(GqlStatus::success("skipped to next"))
    }

    async fn previous(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::PrevTrack)?;
        Ok(GqlStatus::success("skipped to previous"))
    }

    async fn seek(&self, ctx: &Context<'_>, position_ms: i64) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Seek(position_ms as u64))?;
        Ok(GqlStatus::success(format!("seeked to {}ms", position_ms)))
    }

    // -- Queue --

    async fn add_to_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let mut items = Vec::new();
        let mut queue_item_ids = Vec::new();
        let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, &db);
                queue_item_ids.push(item.id.0.to_string());
                if matches!(
                    item.load_state,
                    koan_core::player::state::LoadState::Pending
                ) {
                    pending_downloads.push((tid, item.id));
                }
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

            // Kick off downloads for remote tracks.
            if !pending_downloads.is_empty() {
                spawn_downloads(pending_downloads, tx.clone(), state.clone());
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
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        tx.send(PlayerCommand::ClearPlaylist)
            .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let mut items = Vec::new();
        let mut queue_item_ids = Vec::new();
        let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, &db);
                queue_item_ids.push(item.id.0.to_string());
                if matches!(
                    item.load_state,
                    koan_core::player::state::LoadState::Pending
                ) {
                    pending_downloads.push((tid, item.id));
                }
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

            if !pending_downloads.is_empty() {
                spawn_downloads(pending_downloads, tx.clone(), state.clone());
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
        require_role(ctx, Role::User)?;
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
        require_role(ctx, Role::User)?;
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
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::ClearPlaylist)?;
        Ok(GqlStatus::success("queue cleared"))
    }

    async fn undo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Undo)?;
        Ok(GqlStatus::success("undone"))
    }

    async fn redo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Redo)?;
        Ok(GqlStatus::success("redone"))
    }

    // -- Device --

    async fn set_device(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        send_cmd(ctx, PlayerCommand::SetOutputDevice(name.clone()))?;
        Ok(GqlStatus::success(format!("switched to device '{}'", name)))
    }

    async fn clear_device(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        send_cmd(ctx, PlayerCommand::ClearOutputDevice)?;
        Ok(GqlStatus::success("device cleared, using system default"))
    }

    // -- Favourites --

    async fn favourite(&self, ctx: &Context<'_>, track_id: i64) -> async_graphql::Result<GqlTrack> {
        require_role(ctx, Role::User)?;
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
        sync_favourite_to_remote(&db, path, true);
        Ok(GqlTrack { row: track })
    }

    async fn unfavourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        require_role(ctx, Role::User)?;
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
        sync_favourite_to_remote(&db, path, false);
        Ok(GqlTrack { row: track })
    }

    async fn toggle_favourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        let is_now_fav = queries::toggle_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        sync_favourite_to_remote(&db, path, is_now_fav);
        Ok(GqlTrack { row: track })
    }

    // -- Snapshots --

    async fn save_snapshot(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let (items, cursor) = state.snapshot_playlist();
        let position_ms = state.position_ms();

        let persisted: Vec<PersistedQueueItem> = items
            .iter()
            .map(PersistedQueueItem::from_playlist_item)
            .collect();
        let cursor_path = cursor.and_then(|cid| {
            items
                .iter()
                .find(|i| i.id == cid)
                .map(|i| i.path.to_string_lossy().into_owned())
        });

        queries::save_snapshot(
            &db.conn,
            &name,
            &persisted,
            cursor_path.as_deref(),
            position_ms,
        )
        .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;

        Ok(GqlStatus::success(format!("saved snapshot '{}'", name)))
    }

    async fn restore_snapshot(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let snap = queries::load_snapshot(&db.conn, &name)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("snapshot '{}' not found", name)))?;

        let state = ctx.data::<Arc<SharedPlayerState>>()?;

        // Resolve each snapshot item through the same path resolution as
        // addToQueue — ensures correct cache paths and triggers downloads.
        let mut items = Vec::new();
        let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();
        for snap_item in &snap.items {
            if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &snap_item.path)
                && let Ok(Some(track)) = queries::get_track_row(&db.conn, tid)
            {
                let item = track_to_playlist_item(&track, &db);
                if matches!(
                    item.load_state,
                    koan_core::player::state::LoadState::Pending
                ) {
                    pending_downloads.push((tid, item.id));
                }
                items.push(item);
            } else {
                // Track not in DB — use snapshot's stored data.
                items.push(snap_item.to_playlist_item());
            }
        }

        tx.send(PlayerCommand::ClearPlaylist)
            .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

        let cursor_item_id = snap.cursor_path.as_ref().and_then(|cp| {
            items
                .iter()
                .find(|i| i.path.to_string_lossy() == cp.as_str())
                .map(|i| i.id)
        });

        if !items.is_empty() {
            let first_id = cursor_item_id.unwrap_or(items[0].id);
            tx.send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;
            let _ = tx.send(PlayerCommand::Play(first_id));
            if snap.position_ms > 0 {
                let _ = tx.send(PlayerCommand::Seek(snap.position_ms));
            }

            if !pending_downloads.is_empty() {
                spawn_downloads(pending_downloads, tx.clone(), state.clone());
            }
        }

        Ok(GqlStatus::success(format!("restored snapshot '{}'", name)))
    }

    async fn delete_snapshot(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let deleted = queries::delete_snapshot(&db.conn, &name)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        if deleted {
            Ok(GqlStatus::success(format!("deleted snapshot '{}'", name)))
        } else {
            Err(async_graphql::Error::new(format!(
                "snapshot '{}' not found",
                name
            )))
        }
    }

    // -- Radio --

    async fn enable_radio(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        state.set_radio_mode(true);
        Ok(GqlStatus::success("radio mode enabled"))
    }

    async fn disable_radio(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        state.set_radio_mode(false);
        Ok(GqlStatus::success("radio mode disabled"))
    }

    // -- Organize --

    async fn organize_preview(
        &self,
        ctx: &Context<'_>,
        pattern: String,
        track_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<GqlOrganizePreview> {
        require_role(ctx, Role::Admin)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let result = if let Some(ids) = track_ids {
            koan_core::organize::preview_for_tracks(&db, &ids, &pattern, None)
        } else {
            koan_core::organize::preview(&db, &pattern, None)
        }
        .map_err(|e| async_graphql::Error::new(format!("organize error: {}", e)))?;

        Ok(GqlOrganizePreview {
            moves: result
                .moves
                .iter()
                .map(|m| GqlFileMove {
                    track_id: m.track_id,
                    from_path: m.from.to_string_lossy().into_owned(),
                    to_path: m.to.to_string_lossy().into_owned(),
                })
                .collect(),
            errors: result
                .errors
                .iter()
                .map(|(p, e)| format!("{}: {}", p.display(), e))
                .collect(),
            skipped: result.skipped as i32,
        })
    }

    async fn organize_execute(
        &self,
        ctx: &Context<'_>,
        pattern: String,
        track_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<GqlOrganizeResult> {
        require_role(ctx, Role::Admin)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let result = if let Some(ids) = track_ids {
            koan_core::organize::execute_for_tracks(&db, &ids, &pattern, None)
        } else {
            koan_core::organize::execute(&db, &pattern, None)
        }
        .map_err(|e| async_graphql::Error::new(format!("organize error: {}", e)))?;

        Ok(GqlOrganizeResult {
            moved_count: result.moves.len() as i32,
            errors: result
                .errors
                .iter()
                .map(|(p, e)| format!("{}: {}", p.display(), e))
                .collect(),
            skipped: result.skipped as i32,
        })
    }

    async fn organize_undo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let count = koan_core::organize::undo(&db)
            .map_err(|e| async_graphql::Error::new(format!("organize error: {}", e)))?;
        Ok(GqlStatus::success(format!("undone {} moves", count)))
    }

    // -- Config --

    /// Update configuration fields. Only provided fields are written to config.toml.
    async fn update_config(
        &self,
        ctx: &Context<'_>,
        input: GqlConfigInput,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        use koan_core::config::ReplayGainMode;

        Config::update_base(|cfg| {
            if let Some(ref folders) = input.library_folders {
                cfg.library.folders = folders.iter().map(std::path::PathBuf::from).collect();
            }
            if let Some(ref mode) = input.replaygain_mode {
                cfg.playback.replaygain = match mode.to_lowercase().as_str() {
                    "track" => ReplayGainMode::Track,
                    "album" => ReplayGainMode::Album,
                    _ => ReplayGainMode::Off,
                };
            }
            if let Some(pre_amp) = input.pre_amp_db {
                cfg.playback.pre_amp_db = pre_amp;
            }
            if let Some(ref device) = input.output_device {
                cfg.playback.output_device = if device.is_empty() {
                    None
                } else {
                    Some(device.clone())
                };
            }
            if let Some(fps) = input.target_fps {
                cfg.playback.target_fps = fps as u8;
            }
            if let Some(size) = input.art_size {
                cfg.playback.art_size = size as u16;
            }
            if let Some(enabled) = input.remote_enabled {
                cfg.remote.enabled = enabled;
            }
            if let Some(ref url) = input.remote_url {
                cfg.remote.url = url.clone();
            }
            if let Some(ref username) = input.remote_username {
                cfg.remote.username = username.clone();
            }
            if let Some(ref quality) = input.transcode_quality {
                cfg.remote.transcode_quality = quality.clone();
            }
            if let Some(ref limit) = input.cache_limit {
                cfg.remote.cache_limit = if limit.is_empty() {
                    None
                } else {
                    Some(limit.clone())
                };
            }
            if let Some(fps) = input.visualizer_fps {
                cfg.visualizer.fps = fps as u8;
            }
            if let Some(port) = input.graphql_port {
                cfg.graphql.port = port as u16;
            }
            if let Some(pg) = input.graphql_playground {
                cfg.graphql.playground = pg;
            }
        })
        .map_err(|e| async_graphql::Error::new(format!("config write error: {}", e)))?;

        Ok(GqlStatus::success("config updated"))
    }

    // -- Library management --

    async fn trigger_scan(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlScanResult> {
        require_role(ctx, Role::Admin)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let cfg = Config::load().unwrap_or_default();
        let result = koan_core::index::scanner::full_scan(&db, &cfg.library.folders, false, None);
        Ok(GqlScanResult {
            tracks_added: result.added as i64,
            tracks_updated: result.updated as i64,
            tracks_unchanged: result.skipped as i64,
        })
    }

    async fn trigger_remote_sync(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let cfg = Config::load().unwrap_or_default();
        let client = koan_core::helpers::subsonic_client(&cfg)
            .ok_or_else(|| async_graphql::Error::new("remote not configured"))?;
        koan_core::remote::sync::sync_library(
            &db,
            &client,
            false,
            &cfg.remote.url,
            &cfg.remote.username,
        )
        .map_err(|e| async_graphql::Error::new(format!("sync error: {}", e)))?;
        Ok(GqlStatus::success("remote sync complete"))
    }

    // -- Sharing --

    async fn create_share(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
        description: Option<String>,
    ) -> async_graphql::Result<GqlShare> {
        require_role(ctx, Role::User)?;
        let db = ctx.data::<DbHandle>()?.open()?;
        let cfg = Config::load().unwrap_or_default();
        let client = koan_core::helpers::subsonic_client(&cfg)
            .ok_or_else(|| async_graphql::Error::new("remote not configured"))?;

        // Resolve track IDs to remote IDs.
        let mut remote_ids = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid)
                && let Some(rid) = track.remote_id
            {
                remote_ids.push(rid);
            }
        }

        if remote_ids.is_empty() {
            return Err(async_graphql::Error::new(
                "none of the tracks have remote IDs (local-only tracks can't be shared)",
            ));
        }

        let id_refs: Vec<&str> = remote_ids.iter().map(|s| s.as_str()).collect();
        let share = client
            .create_share(&id_refs, description.as_deref())
            .map_err(|e| async_graphql::Error::new(format!("share error: {}", e)))?;

        Ok(GqlShare {
            url: share.url,
            id: share.id,
        })
    }
}

use std::sync::Arc;

use async_graphql::{Context, Object};
use crossbeam_channel::Sender;
use koan_core::config::Config;
use koan_core::db::queries;
use koan_core::db::queries::playback_state::PersistedQueueItem;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState};

use koan_core::auth::Role;

use super::helpers::{spawn_downloads, sync_favourite_to_remote};
use super::jobs::{JobRegistry, JobState};
use super::types::*;
use super::{DbHandle, parse_queue_item_id, require_role, send_cmd, send_cmd_via, with_db};
use koan_core::helpers::track_to_playlist_item;

/// The `organize*` mutations physically move files, so admin alone is not the
/// bar — the deployment has to have opted in.
fn require_organize() -> async_graphql::Result<()> {
    if Config::load().unwrap_or_default().graphql.allow_organize {
        Ok(())
    } else {
        Err(async_graphql::Error::new(
            "organize is disabled — set [graphql] allow_organize = true to enable it",
        ))
    }
}

/// Tracks resolved into queue items, plus the remote ones needing a download.
struct ResolvedQueue {
    items: Vec<PlaylistItem>,
    pending_downloads: Vec<(i64, QueueItemId)>,
}

/// Resolve track IDs into playlist items on the blocking pool.
async fn resolve_tracks(
    ctx: &Context<'_>,
    track_ids: Vec<i64>,
) -> async_graphql::Result<ResolvedQueue> {
    with_db(ctx, move |db| {
        let mut items = Vec::new();
        let mut pending_downloads = Vec::new();
        for tid in track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, db);
                if matches!(
                    item.load_state,
                    koan_core::player::state::LoadState::Pending
                ) {
                    pending_downloads.push((tid, item.id));
                }
                items.push(item);
            }
        }
        Ok(ResolvedQueue {
            items,
            pending_downloads,
        })
    })
    .await
}

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
        let resolved = resolve_tracks(ctx, track_ids).await?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let queue_item_ids: Vec<String> =
            resolved.items.iter().map(|i| i.id.0.to_string()).collect();
        let first_id = resolved.items.first().map(|i| i.id);
        let count = resolved.items.len() as i32;

        if !resolved.items.is_empty() {
            send_cmd_via(tx, PlayerCommand::AddToPlaylist(resolved.items))?;

            // Auto-play if stopped
            if state.playback_state() == PlaybackState::Stopped
                && let Some(id) = first_id
            {
                send_cmd_via(tx, PlayerCommand::Play(id))?;
            }

            // Kick off downloads for remote tracks.
            if !resolved.pending_downloads.is_empty() {
                spawn_downloads(resolved.pending_downloads, tx.clone(), state.clone());
            }
        }

        Ok(GqlQueueMutationResult {
            success: true,
            message: format!("queued {} tracks", count),
            added_count: count,
            queue_item_ids,
        })
    }

    /// Replace the queue, starting at `start_at` (default: the first track).
    ///
    /// One command rather than clear-then-add-then-play: three commands down a
    /// bounded channel are acted on as each arrives, so the first track starts
    /// before the cursor reaches the one that was asked for.
    async fn replace_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
        start_at: Option<i32>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        require_role(ctx, Role::User)?;
        let resolved = resolve_tracks(ctx, track_ids).await?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let queue_item_ids: Vec<String> =
            resolved.items.iter().map(|i| i.id.0.to_string()).collect();
        let count = resolved.items.len() as i32;

        if resolved.items.is_empty() {
            send_cmd_via(tx, PlayerCommand::ClearPlaylist)?;
        } else {
            send_cmd_via(
                tx,
                PlayerCommand::ReplacePlaylist {
                    items: resolved.items,
                    start: start_at.unwrap_or(0).max(0) as usize,
                },
            )?;

            if !resolved.pending_downloads.is_empty() {
                spawn_downloads(resolved.pending_downloads, tx.clone(), state.clone());
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
        set_favourite(ctx, track_id, Some(true)).await
    }

    async fn unfavourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        require_role(ctx, Role::User)?;
        set_favourite(ctx, track_id, Some(false)).await
    }

    async fn toggle_favourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        require_role(ctx, Role::User)?;
        set_favourite(ctx, track_id, None).await
    }

    // -- Playback state persistence --

    async fn save_playback_state(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;

        let (items, cursor) = state.snapshot_playlist();
        let position_ms = state.position_ms();
        let was_playing =
            state.playback_state() == koan_core::player::state::PlaybackState::Playing;
        let radio_enabled = state.radio_mode();
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

        with_db(ctx, move |db| {
            if persisted.is_empty() {
                queries::playback_state::clear_playback_state(&db.conn)
                    .map_err(|e| super::internal_error("db", e))?;
                return Ok(GqlStatus::success("playback state cleared (empty queue)"));
            }
            queries::playback_state::save_playback_state(
                &db.conn,
                &persisted,
                cursor_path.as_deref(),
                position_ms,
                was_playing,
                radio_enabled,
            )
            .map_err(|e| super::internal_error("db", e))?;
            Ok(GqlStatus::success("playback state saved"))
        })
        .await
    }

    async fn clear_playback_state(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        with_db(ctx, |db| {
            queries::playback_state::clear_playback_state(&db.conn)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(GqlStatus::success("playback state cleared"))
        })
        .await
    }

    // -- Playlists --
    //
    // The same objects the Subsonic endpoints serve and the app edits. Every
    // mutation writes locally and pushes to the upstream server in the
    // background; nothing here waits on the network.

    async fn create_playlist(
        &self,
        ctx: &Context<'_>,
        name: String,
        track_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<GqlPlaylist> {
        require_role(ctx, Role::User)?;
        with_db(ctx, move |db| {
            let id = queries::create_playlist(&db.conn, &name, None)
                .map_err(|e| super::internal_error("db", e))?;
            if let Some(track_ids) = &track_ids {
                queries::add_tracks(&db.conn, id, track_ids)
                    .map_err(|e| super::internal_error("db", e))?;
            }
            koan_core::playlists::push_to_remote(id);
            queries::get_playlist(&db.conn, id)
                .map_err(|e| super::internal_error("db", e))?
                .map(GqlPlaylist::from)
                .ok_or_else(|| async_graphql::Error::new("playlist vanished as it was created"))
        })
        .await
    }

    /// Keep the current queue under a name.
    async fn save_queue_as_playlist(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlPlaylist> {
        require_role(ctx, Role::User)?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        // A queue item with no library row behind it cannot come across: a
        // playlist points at rows, not at paths.
        let track_ids: Vec<i64> = state
            .snapshot_playlist()
            .0
            .iter()
            .filter_map(|item| item.db_id)
            .collect();
        self.create_playlist(ctx, name, Some(track_ids)).await
    }

    async fn rename_playlist(
        &self,
        ctx: &Context<'_>,
        id: i64,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        with_db(ctx, move |db| {
            if !queries::rename_playlist(&db.conn, id, &name)
                .map_err(|e| super::internal_error("db", e))?
            {
                return Err(async_graphql::Error::new(format!(
                    "playlist {id} not found"
                )));
            }
            koan_core::playlists::push_to_remote(id);
            Ok(GqlStatus::success(format!("renamed playlist to '{name}'")))
        })
        .await
    }

    async fn delete_playlist(
        &self,
        ctx: &Context<'_>,
        id: i64,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        with_db(ctx, move |db| {
            // Read before deleting: the delete has to reach the server too, or
            // the next sync brings the playlist back.
            let remote_id = queries::get_playlist(&db.conn, id)
                .ok()
                .flatten()
                .and_then(|p| p.remote_id);
            if !queries::delete_playlist(&db.conn, id)
                .map_err(|e| super::internal_error("db", e))?
            {
                return Err(async_graphql::Error::new(format!(
                    "playlist {id} not found"
                )));
            }
            if let Some(remote_id) = remote_id {
                koan_core::playlists::delete_on_remote(remote_id);
            }
            Ok(GqlStatus::success(format!("deleted playlist {id}")))
        })
        .await
    }

    async fn add_to_playlist(
        &self,
        ctx: &Context<'_>,
        id: i64,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        with_db(ctx, move |db| {
            let added = queries::add_tracks(&db.conn, id, &track_ids)
                .map_err(|e| super::internal_error("db", e))?;
            koan_core::playlists::push_to_remote(id);
            Ok(GqlStatus::success(format!(
                "added {} track(s)",
                added.len()
            )))
        })
        .await
    }

    /// Replace the contents wholesale — a reorder, a removal and a shuffle are
    /// all this once the caller has worked out the list it wants.
    async fn set_playlist_tracks(
        &self,
        ctx: &Context<'_>,
        id: i64,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        with_db(ctx, move |db| {
            queries::set_playlist_tracks(&db.conn, id, &track_ids)
                .map_err(|e| super::internal_error("db", e))?;
            koan_core::playlists::push_to_remote(id);
            Ok(GqlStatus::success(format!(
                "playlist {id} now holds {} track(s)",
                track_ids.len()
            )))
        })
        .await
    }

    /// Replace the queue with a playlist and play it.
    async fn play_playlist(
        &self,
        ctx: &Context<'_>,
        id: i64,
        #[graphql(default = false)] shuffled: bool,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let resolved = with_db(ctx, move |db| {
            let mut track_ids = queries::playlist_track_ids(&db.conn, id)
                .map_err(|e| super::internal_error("db", e))?;
            if shuffled {
                koan_core::helpers::shuffle(&mut track_ids);
            }
            let rows = queries::tracks_by_ids(&db.conn, &track_ids)
                .map_err(|e| super::internal_error("db", e))?;

            let mut items = Vec::new();
            let mut pending_downloads: Vec<(i64, QueueItemId)> = Vec::new();
            for track in &rows {
                let item = track_to_playlist_item(track, db);
                if matches!(
                    item.load_state,
                    koan_core::player::state::LoadState::Pending
                ) {
                    pending_downloads.push((track.id, item.id));
                }
                items.push(item);
            }
            Ok(ResolvedQueue {
                items,
                pending_downloads,
            })
        })
        .await?;

        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        send_cmd_via(tx, PlayerCommand::ClearPlaylist)?;
        let count = resolved.items.len();
        if !resolved.items.is_empty() {
            let first_id = resolved.items[0].id;
            send_cmd_via(tx, PlayerCommand::AddToPlaylist(resolved.items))?;
            send_cmd_via(tx, PlayerCommand::Play(first_id))?;
            if !resolved.pending_downloads.is_empty() {
                spawn_downloads(resolved.pending_downloads, tx.clone(), state.clone());
            }
        }
        Ok(GqlStatus::success(format!("playing {count} track(s)")))
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
    ) -> async_graphql::Result<GqlOrganizePlan> {
        require_role(ctx, Role::Admin)?;
        with_db(ctx, move |db| {
            require_organize()?;
            let result = if let Some(ids) = track_ids {
                koan_core::organize::preview_for_tracks(db, &ids, &pattern, None, true)
            } else {
                koan_core::organize::preview(db, &pattern, None, true)
            }
            .map_err(|e| super::internal_error("organize", e))?;

            Ok(result.into())
        })
        .await
    }

    async fn organize_execute(
        &self,
        ctx: &Context<'_>,
        pattern: String,
        track_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<GqlOrganizePlan> {
        require_role(ctx, Role::Admin)?;
        with_db(ctx, move |db| {
            require_organize()?;
            let result = if let Some(ids) = track_ids {
                koan_core::organize::execute_for_tracks(db, &ids, &pattern, None)
            } else {
                koan_core::organize::execute(db, &pattern, None)
            }
            .map_err(|e| super::internal_error("organize", e))?;

            Ok(result.into())
        })
        .await
    }

    async fn organize_undo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        with_db(ctx, |db| {
            require_organize()?;
            let result =
                koan_core::organize::undo(db).map_err(|e| super::internal_error("organize", e))?;
            let mut message = format!("undone {} moves", result.restored);
            if !result.errors.is_empty() {
                message.push_str(&format!(
                    "; {} left in place: {}",
                    result.errors.len(),
                    result
                        .errors
                        .iter()
                        .map(|(p, e)| format!("{}: {}", p.display(), e))
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            Ok(GqlStatus::success(message))
        })
        .await
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

        // `libraryFolders` plus `triggerScan` plus `organizeExecute` is a
        // remote move of arbitrary files into the music tree, and `remoteUrl`
        // repoints sync at whatever server the caller names. Neither belongs on
        // a network API — they stay CLI-only.
        if input.library_folders.is_some() {
            return Err(async_graphql::Error::new(
                "library folders can only be changed from the CLI",
            ));
        }
        if input.remote_url.is_some() {
            return Err(async_graphql::Error::new(
                "remote URL can only be changed from the CLI",
            ));
        }

        super::blocking(move || {
            Config::update_base(|cfg| {
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
                if let Some(ref username) = input.remote_username {
                    cfg.remote.username = username.clone();
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
            .map_err(|e| super::internal_error("config write", e))?;

            Ok(GqlStatus::success("config updated"))
        })
        .await
    }

    // -- Library management --

    /// Start a library scan and return immediately.
    ///
    /// A full scan walks the filesystem and writes for minutes. Run inline it
    /// held a runtime worker for the whole time, which stalled every in-flight
    /// audio stream on the same process.
    async fn trigger_scan(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlJob> {
        require_role(ctx, Role::Admin)?;
        spawn_job(ctx, "scan", |db| {
            let cfg = Config::load().unwrap_or_default();
            let result = koan_core::index::scanner::full_scan(
                &db,
                &cfg.library.folders,
                koan_core::index::scanner::ScanOptions::default(),
                None,
            );
            Ok(format!(
                "{} added, {} updated, {} unchanged",
                result.added, result.updated, result.skipped
            ))
        })
    }

    /// Start a remote library sync and return immediately.
    async fn trigger_remote_sync(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlJob> {
        require_role(ctx, Role::Admin)?;
        spawn_job(ctx, "remoteSync", |db| {
            let cfg = Config::load().unwrap_or_default();
            let client = koan_core::helpers::subsonic_client(&cfg)
                .ok_or_else(|| "remote not configured".to_string())?;
            let result = koan_core::remote::sync::sync_library(
                &db,
                &client,
                false,
                &cfg.remote.url,
                &cfg.remote.username,
            )
            .map_err(|e| e.to_string())?;
            if result.is_complete() {
                Ok("remote sync complete".to_string())
            } else {
                Ok(format!(
                    "remote sync incomplete: {} album(s) failed and will be retried next sync",
                    result.albums_failed
                ))
            }
        })
    }

    // -- Sharing --

    async fn create_share(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
        description: Option<String>,
    ) -> async_graphql::Result<GqlShare> {
        require_role(ctx, Role::User)?;
        with_db(ctx, move |db| {
            let cfg = Config::load().unwrap_or_default();
            // One query for the remote ids rather than one per track, a link
            // built from the share id when the server returns no URL, and a
            // distinct error for each way this can fail — all shared with the
            // FFI and the TUI so the three cannot drift.
            let outcome =
                koan_core::helpers::create_share(db, &cfg, &track_ids, description.as_deref())
                    .map_err(|e| async_graphql::Error::new(e.to_string()))?;

            Ok(GqlShare {
                url: Some(outcome.url),
                id: outcome.id,
                shared: outcome.shared as i32,
                skipped: outcome.skipped as i32,
            })
        })
        .await
    }
}

/// Star, unstar, or toggle — the three differ only in which write they run.
async fn set_favourite(
    ctx: &Context<'_>,
    track_id: i64,
    star: Option<bool>,
) -> async_graphql::Result<GqlTrack> {
    with_db(ctx, move |db| {
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| super::internal_error("db", e))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        let fs_path = std::path::Path::new(path);

        let now_starred = match star {
            Some(true) => {
                queries::add_favourite(&db.conn, fs_path)
                    .map_err(|e| super::internal_error("db", e))?;
                true
            }
            Some(false) => {
                queries::remove_favourite(&db.conn, fs_path)
                    .map_err(|e| super::internal_error("db", e))?;
                false
            }
            None => queries::toggle_favourite(&db.conn, fs_path)
                .map_err(|e| super::internal_error("db", e))?,
        };

        sync_favourite_to_remote(db, path, now_starred);
        Ok(GqlTrack { row: track })
    })
    .await
}

/// Run `work` on a detached thread with its own connection, returning a job
/// handle. A job of the same kind already running is returned as-is rather than
/// started twice.
fn spawn_job<F>(ctx: &Context<'_>, kind: &'static str, work: F) -> async_graphql::Result<GqlJob>
where
    F: FnOnce(koan_core::db::connection::Database) -> Result<String, String> + Send + 'static,
{
    let registry = ctx.data::<JobRegistry>()?.clone();
    let handle = ctx.data::<DbHandle>()?.clone();

    let job = match registry.start(kind) {
        Ok(job) => job,
        Err(running) => return Ok(running.into()),
    };

    let id = job.id.clone();
    let finisher = registry.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("koan-job-{}", kind))
        .spawn(move || {
            // Deliberately outside the pool: this connection is held for
            // minutes and must not deny one to request-path resolvers.
            let outcome = match handle.open_detached() {
                Ok(db) => work(db),
                Err(e) => Err(e.to_string()),
            };
            match outcome {
                Ok(message) => registry.finish(&id, JobState::Succeeded, message),
                Err(message) => {
                    log::error!("{} job failed: {}", kind, message);
                    registry.finish(&id, JobState::Failed, message)
                }
            }
        });

    if spawned.is_err() {
        finisher.finish(
            &job.id,
            JobState::Failed,
            "failed to spawn worker thread".into(),
        );
        return Err(async_graphql::Error::new("failed to start job"));
    }

    Ok(job.into())
}

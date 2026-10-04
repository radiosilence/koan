use std::sync::Arc;

use async_graphql::{Context, Object};
use crossbeam_channel::Sender;
use koan_core::config::Config;
use koan_core::db::queries;
use koan_core::db::queries::UidKind;
use koan_core::db::queries::playback_state::PersistedQueueItem;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaylistItem, QueueItemId, SharedPlayerState};

use koan_core::auth::Role;
use koan_core::remote::link::LinkCommand;

use super::helpers::sync_favourite_to_remote;
use super::jobs::{JobHandle, JobRegistry, JobState};
use super::types::*;
use super::{DbHandle, parse_queue_item_id, require_role, send_cmd, send_cmd_via, with_db};

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

/// Resolve track IDs into playlist items on the blocking pool.
async fn resolve_tracks(
    ctx: &Context<'_>,
    track_ids: Vec<i64>,
) -> async_graphql::Result<Vec<PlaylistItem>> {
    with_db(ctx, move |db| {
        let rows = queries::tracks_by_ids(&db.conn, &track_ids)
            .map_err(|e| super::internal_error("db", e))?;
        Ok(koan_core::helpers::playlist_items_for_tracks(db, &rows))
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

    /// This process's own player. On a server nobody hears it: for the
    /// music the user is listening to, use `controlClient` and the other
    /// `...OnClient` mutations.
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

    /// Play tracks on a linked koan app rather than on the server: replace
    /// its queue with `trackIds` and start at `startAt`, or append them with
    /// `enqueue`. `client` is a client's id or name from `clients`; without
    /// it, the one playing, else the one that played most recently. With
    /// several linked and none of them playing lately it is an error naming
    /// them: ask the person which.
    async fn play_on_client(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<async_graphql::ID>,
        client: Option<String>,
        start_at: Option<u32>,
        enqueue: Option<bool>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        if track_ids.is_empty() {
            return Err(async_graphql::Error::new("no tracks"));
        }
        let count = track_ids.len();
        let track_ids: Vec<String> = track_ids.into_iter().map(|id| id.0).collect();
        let cmd = if enqueue.unwrap_or(false) {
            LinkCommand::Enqueue { track_ids }
        } else {
            LinkCommand::Play {
                track_ids,
                start_at: start_at.unwrap_or(0),
                position_ms: 0,
                paused: false,
                handoff: false,
            }
        };
        let sent = send_to_client(ctx, client.as_deref(), cmd).await?;
        Ok(GqlStatus::success(format!(
            "sent {count} tracks to {}",
            reached(&sent)
        )))
    }

    /// Move what one linked koan app is playing to another: its queue and
    /// playhead are sent to `to` and it pauses. "Take the music with me":
    /// from the Mac to the phone. `from` and `to` as `client` for
    /// `playOnClient`; `from` defaults to the one playing.
    async fn hand_off_client(
        &self,
        ctx: &Context<'_>,
        to: String,
        from: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let scope = super::client_scope(ctx);
        let target = crate::clients::registry()
            .list(scope.as_deref())
            .into_iter()
            .find(|c| c.id == to || c.device == to || c.name.eq_ignore_ascii_case(&to))
            .map(|c| c.device)
            // Not linked: a phone iOS has suspended, reached by its id.
            .unwrap_or_else(|| to.clone());
        let sent =
            send_to_client(ctx, from.as_deref(), LinkCommand::HandOff { to: target }).await?;
        Ok(GqlStatus::success(format!(
            "{} is handing its queue to {to}",
            reached(&sent)
        )))
    }

    /// Play one track on a linked koan app: from where it is in that app's
    /// queue (see `clients { queue }`), or slotted in after the current track
    /// when the queue does not hold it. `client` as for `playOnClient`.
    async fn jump_on_client(
        &self,
        ctx: &Context<'_>,
        track_id: async_graphql::ID,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let sent = send_to_client(
            ctx,
            client.as_deref(),
            LinkCommand::JumpTo {
                track_id: track_id.0,
            },
        )
        .await?;
        Ok(GqlStatus::success(format!("sent to {}", reached(&sent))))
    }

    /// Insert tracks after the current one on a linked koan app.
    async fn play_next_on_client(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<async_graphql::ID>,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let track_ids: Vec<String> = track_ids.into_iter().map(|id| id.0).collect();
        let count = track_ids.len();
        let sent =
            send_to_client(ctx, client.as_deref(), LinkCommand::PlayNext { track_ids }).await?;
        Ok(GqlStatus::success(format!(
            "{count} tracks next on {}",
            reached(&sent)
        )))
    }

    /// Take tracks out of a linked koan app's queue (every entry for each).
    async fn remove_from_client(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<async_graphql::ID>,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let track_ids = track_ids.into_iter().map(|id| id.0).collect();
        let sent =
            send_to_client(ctx, client.as_deref(), LinkCommand::Remove { track_ids }).await?;
        Ok(GqlStatus::success(format!("sent to {}", reached(&sent))))
    }

    /// Have a linked koan app pull what this server has added since it last
    /// synced. Pushing tracks it has not seen does this on its own.
    async fn sync_client(
        &self,
        ctx: &Context<'_>,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let sent =
            send_to_client(ctx, client.as_deref(), LinkCommand::Sync { full: false }).await?;
        Ok(GqlStatus::success(format!("syncing {}", reached(&sent))))
    }

    /// Have every device of this account pull what the server has changed,
    /// now if linked, else when it next links. Playlist edits and library
    /// changes already do this on their own. Each device walks the library only
    /// if it moved since its last walk; `full` has it walk regardless.
    async fn sync_clients(
        &self,
        ctx: &Context<'_>,
        full: Option<bool>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let scope = super::client_scope(ctx);
        let cmd = LinkCommand::Sync {
            full: full.unwrap_or(false),
        };
        let (sent, queued) =
            super::blocking(move || Ok(crate::clients::registry().deliver(scope.as_deref(), cmd)))
                .await?;
        Ok(GqlStatus::success(reach(&sent, &queued)))
    }

    /// Have every linked koan app delete its downloaded copies of these
    /// tracks, so the next play fetches them from the server again: after a
    /// damaged file on the server has been replaced, since a device keeps the
    /// copy it downloaded. Apps that are not linked now are not reached.
    async fn evict_on_clients(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<async_graphql::ID>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let track_ids = track_ids.into_iter().map(|id| id.0).collect();
        let cmd = published(ctx, LinkCommand::Evict { track_ids }).await?;
        let scope = super::client_scope(ctx);
        let (reached, queued) =
            super::blocking(move || Ok(crate::clients::registry().deliver(scope.as_deref(), cmd)))
                .await?;
        if reached.is_empty() && queued.is_empty() {
            return Err(async_graphql::Error::new(
                "no koan app has ever linked to this server",
            ));
        }
        Ok(GqlStatus::success(reach(&reached, &queued)))
    }

    /// Empty a linked koan app's queue.
    async fn clear_client(
        &self,
        ctx: &Context<'_>,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let sent = send_to_client(ctx, client.as_deref(), LinkCommand::Clear).await?;
        Ok(GqlStatus::success(format!("cleared {}", reached(&sent))))
    }

    /// Queue an album on a linked koan app once it is in the library: for
    /// one being downloaded now (slsk's `grab`). Matched by artist and title
    /// substrings after each library scan; if it is already here it is sent
    /// at once. Unfulfilled orders lapse after a day. `client` as for
    /// `playOnClient`, resolved when the album arrives.
    async fn queue_on_client_when_added(
        &self,
        ctx: &Context<'_>,
        artist: String,
        album: String,
        client: Option<String>,
        play_next: Option<bool>,
    ) -> async_graphql::Result<GqlClientOrder> {
        require_role(ctx, Role::User)?;
        let order = crate::clients::Order {
            id: uuid::Uuid::now_v7().to_string(),
            username: super::client_scope(ctx),
            client,
            artist,
            album,
            play_next: play_next.unwrap_or(false),
            playlist: None,
            titles: Vec::new(),
            created_at: chrono::Utc::now().timestamp(),
        };
        crate::clients::registry().add_order(order.clone());
        let path = ctx.data::<super::DbHandle>()?.path();
        super::blocking(move || {
            crate::clients::fulfil_from(&path);
            Ok(())
        })
        .await?;
        Ok(order.into())
    }

    /// Add an album to a playlist once it is in the library: for one being
    /// downloaded now (slsk's `grab`). `titles` narrows it to those tracks
    /// (title substrings, in that order); empty or absent adds the whole
    /// album. Matched after each library scan, and at once if already here.
    /// The playlist then reaches every device like any edit. Orders are kept
    /// across restarts and lapse after a day.
    async fn add_to_playlist_when_added(
        &self,
        ctx: &Context<'_>,
        playlist_id: async_graphql::ID,
        artist: String,
        album: String,
        titles: Option<Vec<String>>,
    ) -> async_graphql::Result<GqlClientOrder> {
        let playlist_id = super::row_id(ctx, UidKind::Playlist, &playlist_id).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            super::editable_playlist(db, user, playlist_id).map(drop)
        })
        .await?;
        let order = crate::clients::Order {
            id: uuid::Uuid::now_v7().to_string(),
            username: super::client_scope(ctx),
            client: None,
            artist,
            album,
            play_next: false,
            playlist: Some(playlist_id),
            titles: titles.unwrap_or_default(),
            created_at: chrono::Utc::now().timestamp(),
        };
        crate::clients::registry().add_order(order.clone());
        let path = ctx.data::<super::DbHandle>()?.path();
        super::blocking(move || {
            crate::clients::fulfil_from(&path);
            Ok(())
        })
        .await?;
        Ok(order.into())
    }

    /// Withdraw an order made with `queueOnClientWhenAdded`.
    async fn cancel_client_order(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let scope = super::client_scope(ctx);
        if crate::clients::registry().cancel_order(scope.as_deref(), &id) {
            Ok(GqlStatus::success("cancelled"))
        } else {
            Err(async_graphql::Error::new("no such order"))
        }
    }

    /// Seek within the current track on a linked koan app.
    async fn seek_on_client(
        &self,
        ctx: &Context<'_>,
        position_ms: u64,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let sent =
            send_to_client(ctx, client.as_deref(), LinkCommand::Seek { position_ms }).await?;
        Ok(GqlStatus::success(format!("sent to {}", reached(&sent))))
    }

    /// Pause, resume or skip on a linked koan app; see `playOnClient`.
    async fn control_client(
        &self,
        ctx: &Context<'_>,
        action: GqlClientAction,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let cmd = match action {
            GqlClientAction::Pause => LinkCommand::Pause,
            GqlClientAction::Resume => LinkCommand::Resume,
            GqlClientAction::Next => LinkCommand::Next,
            GqlClientAction::Previous => LinkCommand::Previous,
        };
        let sent = send_to_client(ctx, client.as_deref(), cmd).await?;
        Ok(GqlStatus::success(format!("sent to {}", reached(&sent))))
    }

    /// Set shuffle, repeat or both on a linked koan app. Shuffle on reorders
    /// the rest of its queue at random; off puts it back as it was.
    async fn set_play_mode_on_client(
        &self,
        ctx: &Context<'_>,
        shuffle: Option<bool>,
        repeat: Option<GqlRepeat>,
        client: Option<String>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let mut sent = None;
        if let Some(on) = shuffle {
            sent = Some(send_to_client(ctx, client.as_deref(), LinkCommand::Shuffle { on }).await?);
        }
        if let Some(mode) = repeat {
            let cmd = LinkCommand::Repeat { mode: mode.into() };
            sent = Some(send_to_client(ctx, client.as_deref(), cmd).await?);
        }
        let sent = sent.ok_or_else(|| async_graphql::Error::new("give shuffle, repeat or both"))?;
        Ok(GqlStatus::success(format!("sent to {}", reached(&sent))))
    }

    /// This process's own player. On a server nobody hears it: for the
    /// music the user is listening to, use `controlClient` and the other
    /// `...OnClient` mutations.
    async fn pause(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Pause)?;
        Ok(GqlStatus::success("paused"))
    }

    /// This process's own player. On a server nobody hears it: for the
    /// music the user is listening to, use `controlClient` and the other
    /// `...OnClient` mutations.
    async fn resume(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        send_cmd(ctx, PlayerCommand::Resume)?;
        Ok(GqlStatus::success("resumed"))
    }

    /// This process's own player. On a server nobody hears it: for the
    /// music the user is listening to, use `controlClient` and the other
    /// `...OnClient` mutations.
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

    /// Set shuffle, repeat or both on this process's own player, as
    /// `nowPlaying` reports them.
    async fn set_play_mode(
        &self,
        ctx: &Context<'_>,
        shuffle: Option<bool>,
        repeat: Option<GqlRepeat>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        if let Some(on) = shuffle {
            send_cmd(ctx, PlayerCommand::SetShuffle(on))?;
        }
        if let Some(repeat) = repeat {
            send_cmd(ctx, PlayerCommand::SetRepeat(repeat.into()))?;
        }
        Ok(GqlStatus::success("play mode set"))
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
        track_ids: Vec<async_graphql::ID>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let track_ids = super::row_ids(ctx, UidKind::Track, &track_ids).await?;
        require_role(ctx, Role::User)?;
        let resolved = resolve_tracks(ctx, track_ids).await?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let queue_item_ids: Vec<String> = resolved.iter().map(|i| i.id.0.to_string()).collect();
        let first_id = resolved.first().map(|i| i.id);
        let count = resolved.len() as i32;

        if !resolved.is_empty() {
            send_cmd_via(tx, PlayerCommand::AddToPlaylist(resolved))?;

            if state.is_idle()
                && let Some(id) = first_id
            {
                send_cmd_via(tx, PlayerCommand::Play(id))?;
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
        track_ids: Vec<async_graphql::ID>,
        start_at: Option<i32>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let track_ids = super::row_ids(ctx, UidKind::Track, &track_ids).await?;
        require_role(ctx, Role::User)?;
        let resolved = resolve_tracks(ctx, track_ids).await?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let queue_item_ids: Vec<String> = resolved.iter().map(|i| i.id.0.to_string()).collect();
        let count = resolved.len() as i32;

        if resolved.is_empty() {
            send_cmd_via(tx, PlayerCommand::ClearPlaylist)?;
        } else {
            send_cmd_via(
                tx,
                PlayerCommand::ReplacePlaylist {
                    items: resolved,
                    start: start_at.unwrap_or(0).max(0) as usize,
                    position_ms: 0,
                    play: true,
                },
            )?;
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

    async fn favourite(
        &self,
        ctx: &Context<'_>,
        track_id: async_graphql::ID,
    ) -> async_graphql::Result<GqlTrack> {
        let track_id = super::row_id(ctx, UidKind::Track, &track_id).await?;
        require_role(ctx, Role::User)?;
        set_favourite(ctx, track_id, Some(true)).await
    }

    async fn unfavourite(
        &self,
        ctx: &Context<'_>,
        track_id: async_graphql::ID,
    ) -> async_graphql::Result<GqlTrack> {
        let track_id = super::row_id(ctx, UidKind::Track, &track_id).await?;
        require_role(ctx, Role::User)?;
        set_favourite(ctx, track_id, Some(false)).await
    }

    async fn toggle_favourite(
        &self,
        ctx: &Context<'_>,
        track_id: async_graphql::ID,
    ) -> async_graphql::Result<GqlTrack> {
        let track_id = super::row_id(ctx, UidKind::Track, &track_id).await?;
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
        let mode = state.play_mode();
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
                    .and_then(|()| {
                        queries::playback_state::save_playback_position(
                            &db.conn, mode, None, 0, false,
                        )
                    })
                    .map_err(|e| super::internal_error("db", e))?;
                return Ok(GqlStatus::success("playback state cleared (empty queue)"));
            }
            queries::playback_state::save_playback_state(
                &db.conn,
                &persisted,
                mode,
                cursor_path.as_deref(),
                position_ms,
                was_playing,
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
        track_ids: Option<Vec<async_graphql::ID>>,
    ) -> async_graphql::Result<GqlPlaylist> {
        let track_ids = super::opt_row_ids(ctx, UidKind::Track, track_ids.as_deref()).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            let id = queries::create_playlist(&db.conn, user, &name, None)
                .map_err(|e| super::internal_error("db", e))?;
            if let Some(track_ids) = &track_ids {
                queries::add_tracks(&db.conn, id, track_ids)
                    .map_err(|e| super::internal_error("db", e))?;
            }
            koan_core::playlists::push_to_remote(id);
            crate::clients::changed();
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
        let track_ids = state
            .snapshot_playlist()
            .0
            .iter()
            .filter_map(|item| item.db_id)
            .map(async_graphql::ID::from)
            .collect();
        self.create_playlist(ctx, name, Some(track_ids)).await
    }

    async fn rename_playlist(
        &self,
        ctx: &Context<'_>,
        id: async_graphql::ID,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        let id = super::row_id(ctx, UidKind::Playlist, &id).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            super::editable_playlist(db, user, id)?;
            if !queries::rename_playlist(&db.conn, id, &name)
                .map_err(|e| super::internal_error("db", e))?
            {
                return Err(async_graphql::Error::new(format!(
                    "playlist {id} not found"
                )));
            }
            koan_core::playlists::push_to_remote(id);
            crate::clients::changed();
            Ok(GqlStatus::success(format!("renamed playlist to '{name}'")))
        })
        .await
    }

    async fn delete_playlist(
        &self,
        ctx: &Context<'_>,
        id: async_graphql::ID,
    ) -> async_graphql::Result<GqlStatus> {
        let id = super::row_id(ctx, UidKind::Playlist, &id).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            // Read before deleting: the delete has to reach the server too, or
            // the next sync brings the playlist back.
            let remote_id = super::editable_playlist(db, user, id)?.remote_id;
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
            crate::clients::changed();
            Ok(GqlStatus::success(format!("deleted playlist {id}")))
        })
        .await
    }

    async fn add_to_playlist(
        &self,
        ctx: &Context<'_>,
        id: async_graphql::ID,
        track_ids: Vec<async_graphql::ID>,
    ) -> async_graphql::Result<GqlStatus> {
        let id = super::row_id(ctx, UidKind::Playlist, &id).await?;
        let track_ids = super::row_ids(ctx, UidKind::Track, &track_ids).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            super::editable_playlist(db, user, id)?;
            let added = queries::add_tracks(&db.conn, id, &track_ids)
                .map_err(|e| super::internal_error("db", e))?;
            koan_core::playlists::push_to_remote(id);
            crate::clients::changed();
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
        id: async_graphql::ID,
        track_ids: Vec<async_graphql::ID>,
    ) -> async_graphql::Result<GqlStatus> {
        let id = super::row_id(ctx, UidKind::Playlist, &id).await?;
        let track_ids = super::row_ids(ctx, UidKind::Track, &track_ids).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            super::editable_playlist(db, user, id)?;
            queries::set_playlist_tracks(&db.conn, id, &track_ids)
                .map_err(|e| super::internal_error("db", e))?;
            koan_core::playlists::push_to_remote(id);
            crate::clients::changed();
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
        id: async_graphql::ID,
        #[graphql(default = false)] shuffled: bool,
    ) -> async_graphql::Result<GqlStatus> {
        let id = super::row_id(ctx, UidKind::Playlist, &id).await?;
        require_role(ctx, Role::User)?;
        let user = super::user_id(ctx);
        let resolved = with_db(ctx, move |db| {
            super::readable_playlist(db, user, id)?;
            let mut entries = queries::playlist_entries(&db.conn, id)
                .map_err(|e| super::internal_error("db", e))?;
            if shuffled {
                koan_core::helpers::shuffle(&mut entries);
            }

            let tracks: Vec<_> = entries.iter().map(|e| e.track.clone()).collect();
            let mut items = koan_core::helpers::playlist_items_for_tracks(db, &tracks);
            // Which playlist row is playing, and which of two copies of a song.
            for (item, entry) in items.iter_mut().zip(&entries) {
                item.playlist_entry_id = Some(entry.id);
            }
            Ok(items)
        })
        .await?;

        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let count = resolved.len();
        if resolved.is_empty() {
            send_cmd_via(tx, PlayerCommand::ClearPlaylist)?;
        } else {
            send_cmd_via(
                tx,
                PlayerCommand::ReplacePlaylist {
                    items: resolved,
                    start: 0,
                    position_ms: 0,
                    play: true,
                },
            )?;
        }
        Ok(GqlStatus::success(format!("playing {count} track(s)")))
    }

    // -- Organize --

    async fn organize_preview(
        &self,
        ctx: &Context<'_>,
        pattern: String,
        track_ids: Option<Vec<async_graphql::ID>>,
    ) -> async_graphql::Result<GqlOrganizePlan> {
        let track_ids = super::opt_row_ids(ctx, UidKind::Track, track_ids.as_deref()).await?;
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
        track_ids: Option<Vec<async_graphql::ID>>,
    ) -> async_graphql::Result<GqlOrganizePlan> {
        let track_ids = super::opt_row_ids(ctx, UidKind::Track, track_ids.as_deref()).await?;
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

        fn in_range<T: TryFrom<i32>>(
            value: Option<i32>,
            name: &str,
        ) -> async_graphql::Result<Option<T>> {
            value
                .map(T::try_from)
                .transpose()
                .map_err(|_| async_graphql::Error::new(format!("{name} is out of range")))
        }
        let target_fps = in_range::<u8>(input.target_fps, "targetFps")?;
        let art_size = in_range::<u16>(input.art_size, "artSize")?;
        let visualizer_fps = in_range::<u8>(input.visualizer_fps, "visualizerFps")?;
        let graphql_port = in_range::<u16>(input.graphql_port, "graphqlPort")?;

        super::blocking(move || {
            Config::persist(|cfg| {
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
                if let Some(fps) = target_fps {
                    cfg.playback.target_fps = fps;
                }
                if let Some(size) = art_size {
                    cfg.playback.art_size = size;
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
                if let Some(fps) = visualizer_fps {
                    cfg.visualizer.fps = fps;
                }
                if let Some(port) = graphql_port {
                    cfg.graphql.port = port;
                }
                if let Some(pg) = input.graphql_playground {
                    cfg.graphql.playground = pg;
                }
            })
            .map_err(|e| super::internal_error("config write", e))?;
            if input.cache_limit.is_some() {
                koan_core::remote::queue::cache_limit_changed();
            }

            Ok(GqlStatus::success("config updated"))
        })
        .await
    }

    // -- Library management --

    /// Start a library scan and return immediately; poll `job(id:)`.
    ///
    /// A full scan walks the filesystem and writes for minutes, so it runs on a
    /// detached thread rather than holding a runtime worker that in-flight audio
    /// streams need.
    async fn trigger_scan(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlJob> {
        require_role(ctx, Role::Admin)?;
        spawn_job(ctx, "scan", |db, _| {
            let cfg = Config::load().unwrap_or_default();
            crate::clients::changed_if_library_moved(&db.conn);
            let result = koan_core::index::scanner::full_scan(
                &db,
                &cfg.library.folders,
                koan_core::index::scanner::ScanOptions::default(),
                None,
            );
            crate::clients::changed_if_library_moved(&db.conn);
            Ok(format!(
                "{} added, {} updated, {} unchanged",
                result.added, result.updated, result.skipped
            ))
        })
    }

    /// Start a remote library sync and return immediately.
    async fn trigger_remote_sync(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlJob> {
        require_role(ctx, Role::Admin)?;
        spawn_job(ctx, "remoteSync", |db, job| {
            let cfg = Config::load().unwrap_or_default();
            let client = koan_core::helpers::subsonic_client(&cfg)
                .ok_or_else(|| "remote not configured".to_string())?;
            crate::clients::changed_if_library_moved(&db.conn);
            let synced = koan_core::helpers::sync_remote(
                &db,
                &client,
                koan_core::helpers::Walk::Always,
                &cfg.remote.url,
                &cfg.remote.username,
                &|p| job.progress(p.done, p.total, describe_sync(p)),
            );
            crate::clients::changed_if_library_moved(&db.conn);
            let synced = synced.map_err(|e| e.to_string())?;
            if synced.library.is_complete() {
                Ok("remote sync complete".to_string())
            } else {
                Ok(format!(
                    "remote sync incomplete: {} album(s) failed and will be retried next sync",
                    synced.library.albums_failed
                ))
            }
        })
    }

    // -- Sharing --

    /// Revoke a share link served by this server. Its address then answers
    /// exactly as one that never existed.
    async fn delete_share(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let owner = super::share_owner(ctx);
        with_db(ctx, move |db| {
            let found = queries::shares::delete_share(&db.conn, owner, &id)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(GqlStatus {
                success: found,
                message: if found {
                    "revoked".into()
                } else {
                    "no such share".into()
                },
            })
        })
        .await
    }

    /// Change a share link's description and expiry (unix seconds; omit to
    /// never expire).
    async fn update_share(
        &self,
        ctx: &Context<'_>,
        id: String,
        description: Option<String>,
        expires_at: Option<i64>,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::User)?;
        let owner = super::share_owner(ctx);
        with_db(ctx, move |db| {
            let found = queries::shares::update_share(
                &db.conn,
                owner,
                &id,
                description.as_deref(),
                expires_at,
            )
            .map_err(|e| super::internal_error("db", e))?;
            Ok(GqlStatus {
                success: found,
                message: if found {
                    "updated".into()
                } else {
                    "no such share".into()
                },
            })
        })
        .await
    }

    /// Share a slice of the library. `artistId` shares the artist's albums,
    /// `albumId` an album (cued to `startTrackId` if given), and `trackIds`
    /// the tracks; a single track shares its album, cued to that track.
    async fn create_share(
        &self,
        ctx: &Context<'_>,
        track_ids: Option<Vec<async_graphql::ID>>,
        album_id: Option<async_graphql::ID>,
        artist_id: Option<async_graphql::ID>,
        start_track_id: Option<async_graphql::ID>,
        description: Option<String>,
    ) -> async_graphql::Result<GqlShare> {
        let track_ids = super::opt_row_ids(ctx, UidKind::Track, track_ids.as_deref()).await?;
        let album_id = super::opt_row_id(ctx, UidKind::Album, album_id.as_ref()).await?;
        let artist_id = super::opt_row_id(ctx, UidKind::Artist, artist_id.as_ref()).await?;
        let start_track_id =
            super::opt_row_id(ctx, UidKind::Track, start_track_id.as_ref()).await?;
        use koan_core::helpers::ShareTarget;
        require_role(ctx, Role::User)?;
        let target = match (artist_id, album_id, track_ids) {
            (Some(artist), _, _) => ShareTarget::Artist(artist),
            (None, Some(album_id), _) => ShareTarget::Album {
                album_id,
                start_track_id,
            },
            (None, None, Some(ids)) if !ids.is_empty() => ShareTarget::Tracks(ids),
            _ => {
                return Err(async_graphql::Error::new(
                    "give trackIds, albumId or artistId",
                ));
            }
        };
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            let cfg = Config::load().unwrap_or_default();
            // Native, as Subsonic createShare and the web UI make it: a link
            // made on an upstream would be neither listed nor revocable here.
            let outcome = koan_core::helpers::create_native_share(
                db,
                user,
                &cfg,
                &target,
                description.as_deref(),
            )
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

    // -- Accounts --

    /// Make an account with a generated password and return its invite, which
    /// carries the password this once.
    /// `server` is the address the invite points at; without it, the
    /// configured `sharing.public_url`, then the address this request came in
    /// on (which an in-process caller such as MCP does not have).
    async fn create_user(
        &self,
        ctx: &Context<'_>,
        username: String,
        role: GqlRole,
        server: Option<String>,
    ) -> async_graphql::Result<GqlInvite> {
        require_role(ctx, Role::Admin)?;
        let server = invite_server(ctx, server)?;
        with_db(ctx, move |db| {
            let made = koan_core::invite::create_account(&db.conn, &username, role.into())?;
            let token = invite_token(db, made.id)?;
            Ok(koan_core::invite::Invite::with_token(
                &server,
                username.trim(),
                &token,
                Some(&made.password),
            )
            .into())
        })
        .await
    }

    /// An invite for an existing account. Its devices keep working;
    /// `resetPassword` also gives it a new password, returned this once, which
    /// signs every device out.
    async fn invite_user(
        &self,
        ctx: &Context<'_>,
        username: String,
        #[graphql(default)] reset_password: bool,
        server: Option<String>,
    ) -> async_graphql::Result<GqlInvite> {
        require_role(ctx, Role::Admin)?;
        let server = invite_server(ctx, server)?;
        with_db(ctx, move |db| {
            let user = koan_core::invite::account(&db.conn, &username)?;
            let password = if reset_password {
                let p = koan_core::invite::set_password(&db.conn, &username, None)?;
                crate::clients::registry().disconnect(&username);
                Some(p)
            } else {
                None
            };
            let token = invite_token(db, user.id)?;
            Ok(koan_core::invite::Invite::with_token(
                &server,
                &username,
                &token,
                password.as_deref(),
            )
            .into())
        })
        .await
    }

    /// Give an account a password of the admin's choosing. Signs every device
    /// out, as any password change does.
    async fn set_user_password(
        &self,
        ctx: &Context<'_>,
        username: String,
        password: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        with_db(ctx, move |db| {
            koan_core::invite::set_password(&db.conn, &username, Some(&password))?;
            crate::clients::registry().disconnect(&username);
            Ok(GqlStatus::success(format!("{username}'s password changed")))
        })
        .await
    }

    async fn set_user_role(
        &self,
        ctx: &Context<'_>,
        username: String,
        role: GqlRole,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        with_db(ctx, move |db| {
            koan_core::invite::set_role(&db.conn, &username, role.into())?;
            Ok(GqlStatus::success(format!("{username} updated")))
        })
        .await
    }

    /// Delete an account, with its playlists, favourites and keys.
    async fn delete_user(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<GqlStatus> {
        require_role(ctx, Role::Admin)?;
        if super::get_auth_user(ctx).username == username {
            return Err("an account cannot delete itself".into());
        }
        with_db(ctx, move |db| {
            koan_core::invite::delete_account(&db.conn, &username)?;
            crate::clients::registry().disconnect(&username);
            Ok(GqlStatus::success(format!("{username} deleted")))
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
    let user = super::user_id(ctx);
    with_db(ctx, move |db| {
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| super::internal_error("db", e))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let now_starred = match star {
            Some(true) => {
                queries::add_favourite(&db.conn, user, track_id)
                    .map_err(|e| super::internal_error("db", e))?;
                true
            }
            Some(false) => {
                queries::remove_favourite(&db.conn, user, track_id)
                    .map_err(|e| super::internal_error("db", e))?;
                false
            }
            None => queries::toggle_favourite(&db.conn, user, track_id)
                .map_err(|e| super::internal_error("db", e))?,
        };

        // The upstream server has one account, and it is the local user's.
        if queries::auth::is_local_user(&db.conn, user)
            .map_err(|e| super::internal_error("db", e))?
        {
            sync_favourite_to_remote(db, track_id, now_starred);
        }
        Ok(GqlTrack { row: track })
    })
    .await
}

/// What a running sync job says it is doing.
fn describe_sync(p: koan_core::remote::sync::SyncProgress) -> String {
    use koan_core::remote::sync::SyncPhase;
    match (p.phase, p.total) {
        (SyncPhase::Albums, _) => format!("listing albums: {}", p.done),
        (SyncPhase::Tracks, Some(total)) => format!("tracks: {} of {}", p.done, total),
        (SyncPhase::Tracks, None) => format!("tracks: {}", p.done),
        (SyncPhase::Artists, _) => "recording artists".into(),
        (SyncPhase::Finishing, _) => "finishing".into(),
    }
}

/// Run `work` on a detached thread with its own connection, returning a job
/// handle. A job of the same kind already running is returned as-is rather than
/// started twice.
fn spawn_job<F>(ctx: &Context<'_>, kind: &'static str, work: F) -> async_graphql::Result<GqlJob>
where
    F: FnOnce(koan_core::db::connection::Database, JobHandle) -> Result<String, String>
        + Send
        + 'static,
{
    let registry = ctx.data::<JobRegistry>()?.clone();
    let handle = ctx.data::<DbHandle>()?.clone();

    let job = match registry.start(kind) {
        Ok(job) => job,
        Err(running) => return Ok(running.into()),
    };

    let id = job.id.clone();
    let finisher = registry.clone();
    let reporter = registry.handle(&id);
    let spawned = std::thread::Builder::new()
        .name(format!("koan-job-{}", kind))
        .spawn(move || {
            // Deliberately outside the pool: this connection is held for
            // minutes and must not deny one to request-path resolvers.
            let outcome = match handle.open_detached() {
                Ok(db) => work(db, reporter),
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

async fn send_to_client(
    ctx: &Context<'_>,
    client: Option<&str>,
    cmd: LinkCommand,
) -> async_graphql::Result<crate::clients::ClientInfo> {
    let cmd = published(ctx, cmd).await?;
    let scope = super::client_scope(ctx);
    let client = client.map(str::to_owned);
    // Reaching a device that is not linked reads the outbox and may push.
    super::blocking(move || {
        crate::clients::registry()
            .send(scope.as_deref(), client.as_deref(), cmd)
            .map_err(async_graphql::Error::new)
    })
    .await
}

/// `cmd` with each track named by its uid, however the caller named it: the
/// device holds the same uid for it.
async fn published(ctx: &Context<'_>, mut cmd: LinkCommand) -> async_graphql::Result<LinkCommand> {
    let ids: Vec<String> = cmd
        .track_ids_mut()
        .into_iter()
        .map(|id| id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(cmd);
    }
    let rows = super::row_ids(ctx, UidKind::Track, &ids).await?;
    let uids = super::uids(ctx, UidKind::Track, &rows).await?;
    for (id, uid) in cmd.track_ids_mut().into_iter().zip(uids) {
        *id = uid.0;
    }
    Ok(cmd)
}

/// Who a command reached, and how: a phone iOS has suspended is woken to take
/// it, and music iOS will not start there comes up as a notification to tap.
fn reached(c: &crate::clients::ClientInfo) -> String {
    if c.notified {
        format!(
            "{}, which was asleep: music comes up as a notification to tap, since iOS will not start it there; anything else is applied as it wakes",
            c.name
        )
    } else {
        c.name.clone()
    }
}

/// "sent to X; waiting for Y" for a delivery.
fn reach(sent: &[String], queued: &[String]) -> String {
    let mut parts = Vec::new();
    if !sent.is_empty() {
        parts.push(format!("sent to {}", sent.join(", ")));
    }
    if !queued.is_empty() {
        parts.push(format!("waiting for {} to open", queued.join(", ")));
    }
    if parts.is_empty() {
        "no devices".into()
    } else {
        parts.join("; ")
    }
}

/// Where an invite points: the caller's choice, `sharing.public_url`, or the
/// address the request came in on.
/// A token for an invite, signed with the server's key.
fn invite_token(
    db: &koan_core::db::connection::Database,
    user_id: i64,
) -> async_graphql::Result<String> {
    let keys = crate::auth::signing_keys()?;
    Ok(koan_core::invite::mint_token(&db.conn, &keys.0, user_id)?)
}

fn invite_server(ctx: &Context<'_>, server: Option<String>) -> async_graphql::Result<String> {
    let configured = Config::load().ok().and_then(|c| c.sharing.public_url);
    server
        .filter(|s| !s.trim().is_empty())
        .or_else(|| configured.filter(|s| !s.trim().is_empty()))
        .or_else(|| ctx.data::<super::RequestOrigin>().ok().map(|o| o.0.clone()))
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .ok_or_else(|| {
            "pass `server` or set sharing.public_url: this request has no address".into()
        })
}

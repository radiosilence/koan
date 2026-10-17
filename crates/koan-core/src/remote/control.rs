//! Playing for other devices, and on them, for a front end that holds the
//! player's command channel itself: the terminal UI. The apps do the same
//! through koan-ffi's engine, which has pages to tell about each step.
//!
//! `serve` makes this process a device: linked to the server, listening and
//! announced on the local network as `devices.*` says, its levels on offer to
//! a controller, and every command another device sends turned into player
//! commands (`apply`). `send` goes the other way: a player command for the
//! device being controlled, as the link carries it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crossbeam_channel::Sender;

use crate::audio::viz::VizSnapshot;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::commands::PlayerCommand;
use crate::player::state::{
    PlaybackState, PlaylistItem, QueueItemId, QueueMode, SharedPlayerState,
};
use crate::remote::acks::{AckOutcome, Pending};
use crate::remote::devices::{self, Missed};
use crate::remote::link::{
    CommandSource, LinkCommand, LinkIdentity, LinkQueueEntry, LinkState, Local,
};

/// Offer this player to the server and the local network as `identity`.
/// Commands are acted on one at a time, in the order they came, on a thread
/// of their own: resolving a track the library lacks can mean a sync.
pub fn serve(
    identity: LinkIdentity,
    state: Arc<SharedPlayerState>,
    viz: Arc<VizSnapshot>,
    tx: Sender<PlayerCommand>,
) {
    let playhead = state.clone();
    crate::remote::levels::feed().provide(viz, move || playhead.position_ms());
    crate::remote::dsp_sync::set_device_name(Some(identity.name.clone()));

    let (commands, inbox) =
        crossbeam_channel::unbounded::<(LinkCommand, CommandSource, Option<Pending>)>();
    {
        let (state, tx) = (state.clone(), tx);
        std::thread::Builder::new()
            .name("koan-link-commands".into())
            .spawn(move || {
                for (cmd, source, pending) in inbox {
                    let outcome = match apply(cmd, source, &tx, &state) {
                        Ok(()) => AckOutcome::Done,
                        Err(error) => {
                            log::warn!("link: {error}");
                            AckOutcome::Failed { error }
                        }
                    };
                    if let Some(pending) = pending {
                        pending.finish(outcome);
                    }
                }
            })
            .expect("failed to spawn the link command thread");
    }

    // The queue is read again only when it has changed: this runs on every
    // change to the engine, and naming its tracks is a query.
    let held = parking_lot::Mutex::new(None::<(u64, Vec<LinkQueueEntry>)>);
    devices::start(Local {
        identity,
        on_command: Arc::new(move |cmd, source, pending| {
            let _ = commands.send((cmd, source, pending));
        }),
        state: Arc::new(move || {
            let item = state.cursor().and_then(|c| state.get_item(c));
            let version = state.playlist_version();
            let mut held = held.lock();
            if held.as_ref().is_none_or(|(v, _)| *v != version) {
                *held = Some((version, link_queue(&state)));
            }
            LinkState {
                playing: state.playback_state() == PlaybackState::Playing,
                title: item.as_ref().map(|i| i.title.clone()),
                artist: item.as_ref().map(|i| i.artist.clone()),
                album: item.as_ref().map(|i| i.album.clone()),
                position_ms: state.position_ms(),
                duration_ms: state.duration_ms(),
                queue: held.as_ref().map(|(_, q)| q.clone()).unwrap_or_default(),
                outputs: Some(crate::remote::outputs::local(&state)),
                shuffle: state.play_mode().shuffle,
                repeat: state.play_mode().repeat,
                sleep: state.sleep(),
                sleep_fading: state.sleep_fading(),
            }
        }),
    });
}

/// Hang up on the server and the network: the process is ending, and the
/// devices that can see this one should stop listing it now rather than when
/// its sockets time out.
pub fn stop() {
    crate::remote::nearby::suspend();
    crate::remote::link::hang_up();
}

/// The queue as other devices are told it, with each track's id on the
/// server. At most `LINK_QUEUE_MAX` entries, from a few before the current one.
pub fn link_queue(state: &SharedPlayerState) -> Vec<LinkQueueEntry> {
    const LINK_QUEUE_MAX: usize = 300;
    let (window, cursor) = state.playlist_window(20, LINK_QUEUE_MAX);
    let remote = crate::db::pool::shared()
        .get()
        .map(|db| remote_ids(&db, &window))
        .unwrap_or_default();
    window
        .iter()
        .map(|i| LinkQueueEntry {
            id: Some(i.id.0.to_string()),
            track_id: i.db_id.and_then(|id| remote.get(&id).cloned()),
            title: i.title.clone(),
            artist: i.artist.clone(),
            album: i.album.clone(),
            duration_ms: i.duration_ms.unwrap_or(0),
            current: Some(i.id) == cursor,
        })
        .collect()
}

/// The server's ids for these items' tracks, where they have one.
fn remote_ids(db: &Database, items: &[PlaylistItem]) -> HashMap<i64, String> {
    let ids: Vec<i64> = items.iter().filter_map(|i| i.db_id).collect();
    queries::tracks_by_ids(&db.conn, &ids)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t.remote_id.map(|r| (t.id, r)))
        .collect()
}

fn parse_qid(s: &str) -> Result<QueueItemId, String> {
    uuid::Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|_| format!("not a queue item id: {s}"))
}

fn parse_qids(ids: &[String]) -> Result<Vec<QueueItemId>, String> {
    ids.iter().map(|s| parse_qid(s)).collect()
}

/// Act on what another device asked of this one, as player commands.
pub fn apply(
    cmd: LinkCommand,
    source: CommandSource,
    tx: &Sender<PlayerCommand>,
    state: &SharedPlayerState,
) -> Result<(), String> {
    let send = |cmd: PlayerCommand| {
        tx.send(cmd)
            .map_err(|_| "the player has stopped".to_string())
    };
    // Only the account may cost a sync: anyone on the network can send a
    // track id this library has never heard of.
    let may_sync = source == CommandSource::Account;
    let db = || {
        crate::db::pool::shared()
            .get()
            .map_err(|e| format!("library: {e}"))
    };
    let items = |track_ids: &[String]| -> Result<Vec<PlaylistItem>, String> {
        let db = db()?;
        let (ids, _) = crate::remote::link::resolve_tracks(&db, track_ids, may_sync);
        let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap_or_default();
        Ok(crate::helpers::playlist_items_for_tracks(&db, &rows))
    };
    match cmd {
        LinkCommand::Play {
            track_ids,
            start_at,
            position_ms,
            paused,
            handoff,
            mode,
        } => {
            // The music is coming back here: stop controlling whatever this
            // was controlling.
            if handoff && devices::target().is_some() {
                devices::set_target(None);
            }
            let items = items(&track_ids)?;
            if items.is_empty() {
                return Err(format!(
                    "none of {} tracks are in the library",
                    track_ids.len()
                ));
            }
            let start = (start_at as usize).min(items.len() - 1);
            send(PlayerCommand::ReplacePlaylist {
                items,
                start,
                position_ms,
                play: !paused,
                mode,
            })
        }
        LinkCommand::Enqueue { track_ids } => {
            let items = items(&track_ids)?;
            let Some(first) = items.first().map(|i| i.id) else {
                return Ok(());
            };
            let was_stopped = state.is_idle();
            send(PlayerCommand::AddToPlaylist(items))?;
            if was_stopped {
                send(PlayerCommand::Play(first))?;
            }
            Ok(())
        }
        LinkCommand::PlayNext { track_ids } => {
            let items = items(&track_ids)?;
            if items.is_empty() {
                return Ok(());
            }
            match state.cursor() {
                Some(after) => send(PlayerCommand::InsertInPlaylist { items, after }),
                None => send(PlayerCommand::AddToPlaylist(items)),
            }
        }
        LinkCommand::Insert { track_ids, after } => {
            let after = parse_qid(&after)?;
            let items = items(&track_ids)?;
            if items.is_empty() {
                return Ok(());
            }
            send(PlayerCommand::InsertInPlaylist { items, after })
        }
        LinkCommand::JumpTo { track_id } => {
            let db = db()?;
            let (found, _) =
                crate::remote::link::resolve_tracks(&db, std::slice::from_ref(&track_id), may_sync);
            let Some(&local) = found.first() else {
                return Err(format!("track {track_id} is not in the library"));
            };
            let (queued, cursor) = state.snapshot_playlist();
            if let Some(item) = queued.iter().find(|i| i.db_id == Some(local)) {
                return send(PlayerCommand::Play(item.id));
            }
            let rows = queries::tracks_by_ids(&db.conn, &[local]).unwrap_or_default();
            let Some(item) = crate::helpers::playlist_items_for_tracks(&db, &rows).pop() else {
                return Ok(());
            };
            let id = item.id;
            match cursor {
                Some(after) => send(PlayerCommand::InsertInPlaylist {
                    items: vec![item],
                    after,
                })?,
                None => send(PlayerCommand::AddToPlaylist(vec![item]))?,
            }
            send(PlayerCommand::Play(id))
        }
        LinkCommand::Remove { track_ids } => {
            let db = db()?;
            let (found, _) = crate::remote::link::resolve_tracks(&db, &track_ids, may_sync);
            let found: HashSet<i64> = found.into_iter().collect();
            let (queued, _) = state.snapshot_playlist();
            let gone: Vec<QueueItemId> = queued
                .iter()
                .filter(|i| i.db_id.is_some_and(|id| found.contains(&id)))
                .map(|i| i.id)
                .collect();
            if gone.is_empty() {
                return Ok(());
            }
            send(PlayerCommand::RemoveFromPlaylistBatch(gone))
        }
        LinkCommand::PlayItem { id } => {
            let id = parse_qid(&id)?;
            match state.get_item(id) {
                Some(_) => send(PlayerCommand::Play(id)),
                None => Ok(()),
            }
        }
        LinkCommand::RemoveItems { ids } => {
            send(PlayerCommand::RemoveFromPlaylistBatch(parse_qids(&ids)?))
        }
        LinkCommand::MoveItems { ids, target, after } => send(PlayerCommand::MoveItemsInPlaylist {
            ids: parse_qids(&ids)?,
            target: parse_qid(&target)?,
            after,
        }),
        LinkCommand::Clear => send(PlayerCommand::ClearPlaylist),
        LinkCommand::Undo => send(PlayerCommand::Undo),
        LinkCommand::Redo => send(PlayerCommand::Redo),
        LinkCommand::Seek { position_ms } => send(PlayerCommand::Seek(position_ms)),
        LinkCommand::Pause => send(PlayerCommand::Pause),
        LinkCommand::Resume => send(PlayerCommand::Resume),
        LinkCommand::Next => send(PlayerCommand::NextTrack),
        LinkCommand::Previous => send(PlayerCommand::PrevTrack),
        LinkCommand::Shuffle { on } => send(PlayerCommand::SetShuffle(on)),
        LinkCommand::Repeat { mode } => send(PlayerCommand::SetRepeat(mode)),
        LinkCommand::SleepTimer { timer } => send(PlayerCommand::SetSleepTimer(timer)),
        LinkCommand::HandOff { to } => hand_off(&to, source, tx, state),
        // From a notification tapped or an outbox: the link's own check,
        // made again.
        LinkCommand::Shared { command } => {
            if command.allowed_playback() {
                apply(*command, CommandSource::Shared, tx, state)?;
            }
            Ok(())
        }
        LinkCommand::SetOutput { output } => {
            crate::remote::outputs::set(output, crate::upnp::choose(), tx)
        }
        LinkCommand::RefreshOutputs => {
            crate::remote::outputs::refresh_for_controller();
            Ok(())
        }
        LinkCommand::SetRendererVolume { volume } => send(PlayerCommand::SetRendererVolume(volume)),
        LinkCommand::SetPreset { device, profile } => {
            crate::remote::outputs::set_preset(&device, profile.as_deref(), tx)
        }
        LinkCommand::Evict { track_ids } => {
            let db = db()?;
            let (ids, _) = crate::remote::link::resolve_tracks(&db, &track_ids, may_sync);
            let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap_or_default();
            for path in rows.iter().filter_map(|t| t.cached_path.as_deref()) {
                let _ = std::fs::remove_file(path);
            }
            queries::clear_cached_paths_for(&db.conn, &ids).map_err(|e| e.to_string())?;
            Ok(())
        }
        LinkCommand::HistoryChanged => {
            crate::remote::history::sync(&*db()?);
            Ok(())
        }
        LinkCommand::DspProfilesChanged => {
            if crate::remote::dsp_sync::sync(&*db()?).changed() {
                send(PlayerCommand::ReloadDsp)?;
            }
            Ok(())
        }
        LinkCommand::Sync { full } => {
            let walk = if full {
                crate::helpers::Walk::Always
            } else {
                crate::helpers::Walk::IfChanged
            };
            crate::remote::link::sync(&*db()?, walk);
            Ok(())
        }
        // Answered by the link session itself.
        LinkCommand::Devices { .. }
        | LinkCommand::DeviceKeys { .. }
        | LinkCommand::Shares { .. }
        | LinkCommand::Forgotten { .. }
        | LinkCommand::WatchLevels { .. }
        | LinkCommand::Levels { .. }
        | LinkCommand::Acked { .. } => Ok(()),
    }
}

/// Send this device's queue and playhead to `to`, asked by `source`, and
/// pause here. The tracks the server does not know are left out, since the
/// other device could not play them.
pub fn hand_off(
    to: &str,
    source: CommandSource,
    tx: &Sender<PlayerCommand>,
    state: &SharedPlayerState,
) -> Result<(), String> {
    let (items, cursor) = state.snapshot_playlist();
    let remote = crate::db::pool::shared()
        .get()
        .map(|db| remote_ids(&db, &items))
        .unwrap_or_default();
    let at = cursor
        .and_then(|c| items.iter().position(|i| i.id == c))
        .unwrap_or(0);
    let kept: Vec<(usize, String)> = items
        .iter()
        .enumerate()
        .filter_map(|(n, i)| {
            i.db_id
                .and_then(|id| remote.get(&id).cloned())
                .map(|r| (n, r))
        })
        .collect();
    if kept.is_empty() {
        return Err(
            "nothing in the queue is on the server, so the other device cannot play it".into(),
        );
    }
    let start_at = kept.iter().position(|(n, _)| *n >= at).unwrap_or(0);
    // Silent here first, and the playhead read where it went silent: anything
    // heard while the command travels would be heard twice.
    let paused = state.playback_state() == PlaybackState::Paused;
    let (reply, silent) = crossbeam_channel::bounded(1);
    tx.send(PlayerCommand::PauseAndReport(reply))
        .map_err(|_| "the player has stopped".to_string())?;
    let position_ms = silent
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap_or_else(|_| state.position_ms());
    let position_ms = if kept.get(start_at).is_some_and(|(n, _)| *n == at) {
        position_ms
    } else {
        0
    };
    let play = LinkCommand::Play {
        track_ids: kept.into_iter().map(|(_, r)| r).collect(),
        start_at: start_at as u32,
        position_ms,
        paused,
        handoff: true,
        mode: QueueMode::Keep,
    };
    if let Err(e) = devices::send_for(source, to, play) {
        if !paused {
            let _ = tx.send(PlayerCommand::Resume);
        }
        return Err(e);
    }
    Ok(())
}

/// Control the device `id`, or this one with `None`. This one pauses, and
/// what a front end shows and commands is that device's until another is
/// picked. Nothing moves; see `move_music`.
pub fn control(
    id: Option<String>,
    tx: &Sender<PlayerCommand>,
    state: &SharedPlayerState,
) -> Result<(), String> {
    if let Some(id) = &id {
        devices::choosable(id)?;
        if state.playback_state() == PlaybackState::Playing {
            let _ = tx.send(PlayerCommand::Pause);
        }
    }
    devices::set_target(id.clone());
    // Asleep: wake it, and let the list say how that is going.
    if let Some(id) = id {
        devices::wake(&id);
    }
    Ok(())
}

/// Move the music of the device being controlled (this one, when none is)
/// to `to` (this one with `None`), and control `to` from then on.
pub fn move_music(
    to: Option<String>,
    tx: &Sender<PlayerCommand>,
    state: &SharedPlayerState,
) -> Result<(), String> {
    let from = devices::target();
    if from == to {
        return Ok(());
    }
    match (&from, &to) {
        (None, Some(to)) => hand_off(to, CommandSource::Account, tx, state)?,
        (Some(from), to) => {
            let to = match to {
                Some(to) => to.clone(),
                None => devices::this_id()
                    .ok_or_else(|| "this device is not set up to be reached".to_string())?,
            };
            devices::send(from, LinkCommand::HandOff { to })?;
        }
        (None, None) => {}
    }
    devices::set_target(to);
    Ok(())
}

/// Send `cmd` to the device being controlled, as the link carries it, or to
/// this device's player when none is. A command for music that cannot reach
/// that device plays here instead, and control comes back here.
pub fn send(cmd: PlayerCommand, tx: &Sender<PlayerCommand>) -> Result<(), String> {
    let Some(to) = devices::target() else {
        return tx
            .send(cmd)
            .map_err(|_| "the player has stopped".to_string());
    };
    let here = cmd.instead_here();
    let link = match to_link(cmd)? {
        Routed::There(link) => link,
        Routed::Here(local) => {
            return tx
                .send(local)
                .map_err(|_| "the player has stopped".to_string());
        }
        Routed::Dropped => return Ok(()),
    };
    match (devices::send_reaching(&to, link), here) {
        (Ok(()), _) => Ok(()),
        (Err(Missed::OutOfReach(_)), Some(here)) => {
            devices::fall_back(&to, here.asks_to_play());
            tx.send(here)
                .map_err(|_| "the player has stopped".to_string())
        }
        (Err(missed), _) => Err(missed.into_message()),
    }
}

/// Where a player command goes while another device is controlled.
pub enum Routed {
    /// To that device, as this.
    There(LinkCommand),
    /// This device's own: its output, its DSP, the player's own events.
    Here(PlayerCommand),
    /// Bookkeeping of this device's own queue, which the other device keeps
    /// for itself.
    Dropped,
}

/// A player command as the device being controlled is sent it. Queue entries
/// are named by the ids that device reported them under; tracks by the
/// server's ids, which only a device playing from the same library can
/// resolve.
pub fn to_link(cmd: PlayerCommand) -> Result<Routed, String> {
    let entry = |id: QueueItemId| id.0.to_string();
    let tracks = |items: &[PlaylistItem]| -> Result<Vec<String>, String> {
        if !devices::target_device().is_some_and(|d| d.same_library) {
            return Err("that device plays from a different library".into());
        }
        let remote = crate::db::pool::shared()
            .get()
            .map(|db| remote_ids(&db, items))
            .unwrap_or_default();
        let out: Vec<String> = items
            .iter()
            .filter_map(|i| i.db_id.and_then(|id| remote.get(&id).cloned()))
            .collect();
        if out.is_empty() {
            return Err(
                "none of these are on the server, so the other device cannot play them".into(),
            );
        }
        Ok(out)
    };
    Ok(Routed::There(match cmd {
        PlayerCommand::Play(id) => LinkCommand::PlayItem { id: entry(id) },
        PlayerCommand::Pause | PlayerCommand::Stop => LinkCommand::Pause,
        PlayerCommand::Resume => LinkCommand::Resume,
        PlayerCommand::Seek(position_ms) => LinkCommand::Seek { position_ms },
        PlayerCommand::NextTrack => LinkCommand::Next,
        PlayerCommand::PrevTrack => LinkCommand::Previous,
        PlayerCommand::SetShuffle(on) => LinkCommand::Shuffle { on },
        PlayerCommand::SetRepeat(mode) => LinkCommand::Repeat { mode },
        PlayerCommand::SetSleepTimer(timer) => LinkCommand::SleepTimer { timer },
        PlayerCommand::AddToPlaylist(items) => LinkCommand::Enqueue {
            track_ids: tracks(&items)?,
        },
        PlayerCommand::InsertInPlaylist { items, after } => LinkCommand::Insert {
            track_ids: tracks(&items)?,
            after: entry(after),
        },
        PlayerCommand::ReplacePlaylist {
            items,
            start,
            position_ms,
            play,
            mode,
        } => {
            // Where `start` lands once the tracks the server lacks are left
            // out: on it, or on the next one that remains.
            let track_ids = tracks(&items)?;
            let remote = crate::db::pool::shared()
                .get()
                .map(|db| remote_ids(&db, &items))
                .unwrap_or_default();
            let start_at = items
                .iter()
                .take(start)
                .filter(|i| i.db_id.is_some_and(|id| remote.contains_key(&id)))
                .count();
            LinkCommand::Play {
                start_at: start_at.min(track_ids.len().saturating_sub(1)) as u32,
                track_ids,
                position_ms,
                paused: !play,
                handoff: false,
                mode,
            }
        }
        PlayerCommand::RemoveFromPlaylist(id) => LinkCommand::RemoveItems {
            ids: vec![entry(id)],
        },
        PlayerCommand::RemoveFromPlaylistBatch(ids) => LinkCommand::RemoveItems {
            ids: ids.into_iter().map(entry).collect(),
        },
        PlayerCommand::MoveInPlaylist { id, target, after } => LinkCommand::MoveItems {
            ids: vec![entry(id)],
            target: entry(target),
            after,
        },
        PlayerCommand::MoveItemsInPlaylist { ids, target, after } => LinkCommand::MoveItems {
            ids: ids.into_iter().map(entry).collect(),
            target: entry(target),
            after,
        },
        PlayerCommand::ClearPlaylist => LinkCommand::Clear,
        PlayerCommand::Undo => LinkCommand::Undo,
        PlayerCommand::Redo => LinkCommand::Redo,
        PlayerCommand::ReorderPlaylist(_)
        | PlayerCommand::BeginUndoBatch
        | PlayerCommand::EndUndoBatch => return Ok(Routed::Dropped),
        // Named rather than caught by a wildcard, so a command added later is
        // placed here or mapped above, never acted on locally by default
        // while another device is controlled.
        local @ (PlayerCommand::Cue { .. }
        | PlayerCommand::PauseAndReport(_)
        | PlayerCommand::Barrier(_)
        | PlayerCommand::UpdatePaths(_)
        | PlayerCommand::TrackReady(_)
        | PlayerCommand::TrackStreamReady(_)
        | PlayerCommand::StreamProbed { .. }
        | PlayerCommand::TrackFailed(_)
        | PlayerCommand::CacheTracks(_)
        | PlayerCommand::DecodeFinished(_)
        | PlayerCommand::TrackQueued
        | PlayerCommand::SetOutputDevice(_)
        | PlayerCommand::ClearOutputDevice
        | PlayerCommand::ReloadDsp
        | PlayerCommand::RouteChanged
        | PlayerCommand::RestartOutput
        | PlayerCommand::UseRenderer(_)
        | PlayerCommand::ResumeRenderer(_)
        | PlayerCommand::ResumeRendererMissed
        | PlayerCommand::ReleaseRenderer(_)
        | PlayerCommand::SetRendererVolume(_)
        | PlayerCommand::RestorePlayMode(_)
        | PlayerCommand::Renderer { .. }) => return Ok(Routed::Here(local)),
    }))
}

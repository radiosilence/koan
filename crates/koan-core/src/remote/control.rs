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
use crate::remote::devices;
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
                    let outcome = match apply(cmd, source, &tx, &state, &|| {}) {
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
/// `library_changed` is told when the command brought rows into the library
/// or changed what is on disk, for a front end whose pages have to read them
/// again.
pub fn apply(
    cmd: LinkCommand,
    source: CommandSource,
    tx: &Sender<PlayerCommand>,
    state: &SharedPlayerState,
    library_changed: &dyn Fn(),
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
    let resolve = |db: &Database, ids: &[String]| {
        let (found, synced) = crate::remote::link::resolve_each(db, ids, may_sync);
        if synced {
            library_changed();
        }
        found
    };
    let items_of = |db: &Database, ids: &[i64]| {
        let rows = queries::tracks_by_ids(&db.conn, ids).unwrap_or_default();
        crate::helpers::playlist_items_for_tracks(db, &rows)
    };
    let items = |track_ids: &[String]| -> Result<Vec<PlaylistItem>, String> {
        let db = db()?;
        let ids: Vec<i64> = resolve(&db, track_ids).into_iter().flatten().collect();
        Ok(items_of(&db, &ids))
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
                log::info!("link: handed the music; taking control back");
                devices::set_target(None);
            }
            let db = db()?;
            let found = resolve(&db, &track_ids);
            let (start, position_ms) = start_among(&found, start_at as usize, position_ms);
            let ids: Vec<i64> = found.into_iter().flatten().collect();
            let items = items_of(&db, &ids);
            if items.is_empty() {
                return Err(format!(
                    "none of {} tracks are in the library",
                    track_ids.len()
                ));
            }
            send(PlayerCommand::ReplacePlaylist {
                start: start.min(items.len() - 1),
                items,
                position_ms,
                play: !paused,
                mode,
            })
        }
        LinkCommand::Enqueue { track_ids, play } => {
            let items = items(&track_ids)?;
            if items.is_empty() {
                return Ok(());
            }
            if play || state.is_idle() {
                send(PlayerCommand::AddAndPlay(items))
            } else {
                send(PlayerCommand::AddToPlaylist(items))
            }
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
            let Some(local) = resolve(&db, std::slice::from_ref(&track_id))
                .first()
                .copied()
                .flatten()
            else {
                return Err(format!("track {track_id} is not in the library"));
            };
            let (queued, cursor) = state.snapshot_playlist();
            if let Some(item) = queued.iter().find(|i| i.db_id == Some(local)) {
                return send(PlayerCommand::Play(item.id));
            }
            let Some(item) = items_of(&db, &[local]).pop() else {
                return Err(format!("track {track_id} is not in the library"));
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
            let found: HashSet<i64> = resolve(&db, &track_ids).into_iter().flatten().collect();
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
        // Asked for by another device, which learns the outcome from what
        // the destination reports. Refused there, the music plays on here.
        LinkCommand::HandOff { to } => {
            let was_playing = state.playback_state() != PlaybackState::Paused;
            let player = tx.clone();
            let then: devices::Then = Box::new(move |outcome| {
                let refused = matches!(
                    outcome,
                    Some(AckOutcome::Refused { .. } | AckOutcome::Failed { .. })
                );
                if refused && was_playing {
                    let _ = player.send(PlayerCommand::Resume);
                }
            });
            hand_off(&to, source, tx, state, Some(then)).map(|_| ())
        }
        // From a notification tapped or an outbox: the link's own check,
        // made again.
        LinkCommand::Shared { command } => {
            if command.allowed_playback() {
                apply(*command, CommandSource::Shared, tx, state, library_changed)?;
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
            let ids: Vec<i64> = resolve(&db, &track_ids).into_iter().flatten().collect();
            let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap_or_default();
            for path in rows.iter().filter_map(|t| t.cached_path.as_deref()) {
                let _ = std::fs::remove_file(path);
            }
            queries::clear_cached_paths_for(&db.conn, &ids).map_err(|e| e.to_string())?;
            log::info!("link: evicted {} cached tracks", ids.len());
            library_changed();
            Ok(())
        }
        LinkCommand::HistoryChanged => {
            // History and Recently played follow `player::history::changed`,
            // which the sync rings; a track it had to sync for is the
            // library's news too.
            if crate::remote::history::sync(&*db()?).library_synced {
                library_changed();
            }
            Ok(())
        }
        LinkCommand::DspProfilesChanged => {
            if crate::remote::dsp_sync::sync(&*db()?).changed() {
                // Pages showing profiles follow the library.
                library_changed();
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
            library_changed();
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

/// Where a `Play` starts once the tracks this library lacks are left out: on
/// the track asked for, or the next one that remains, from the start when
/// none does. The playhead holds only on the track asked for itself.
fn start_among(found: &[Option<i64>], start_at: usize, position_ms: u64) -> (usize, u64) {
    let start = found.iter().take(start_at).filter(|f| f.is_some()).count();
    let kept = found.get(start_at).is_some_and(|f| f.is_some());
    let remaining = found.iter().skip(start_at).any(|f| f.is_some());
    match (kept, remaining) {
        (true, _) => (start, position_ms),
        (false, true) => (start, 0),
        (false, false) => (0, 0),
    }
}

/// Music sent to another device by `hand_off`.
pub struct HandOff {
    /// Tracks left out because the server does not have them.
    pub left_out: u32,
    /// The server's id for the track the destination should now be on.
    pub track: String,
    /// What the destination last reported before the music was sent.
    pub before: Option<LinkState>,
    /// This device was playing, and should play on if the music is refused.
    pub was_playing: bool,
}

/// Send this device's queue and playhead to `to`, asked by `source`, and
/// pause here; `then` is handed the destination's answer. The tracks the
/// server does not know are left out, since the other device could not play
/// them.
pub fn hand_off(
    to: &str,
    source: CommandSource,
    tx: &Sender<PlayerCommand>,
    state: &SharedPlayerState,
    then: Option<devices::Then>,
) -> Result<HandOff, String> {
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
    let left_out = (items.len() - kept.len()) as u32;
    let track = kept[start_at].1.clone();
    let before = devices::last_report(to);
    let play = LinkCommand::Play {
        track_ids: kept.into_iter().map(|(_, r)| r).collect(),
        start_at: start_at as u32,
        position_ms,
        paused,
        handoff: true,
        mode: QueueMode::Keep,
    };
    if let Err(e) = devices::send_for_then(source, to, play, then) {
        if !paused {
            let _ = tx.send(PlayerCommand::Resume);
        }
        return Err(e);
    }
    log::info!("devices: sent the queue to {to}, {left_out} left out");
    Ok(HandOff {
        left_out,
        track,
        before,
        was_playing: !paused,
    })
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
        (None, Some(to)) => {
            hand_off(to, CommandSource::Account, tx, state, None)?;
        }
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

/// Why a command for the device being controlled did not reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// It names music that device cannot play.
    Unplayable(String),
    /// The device could not be reached.
    Missed(String),
    /// This device's player has stopped.
    Stopped,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unplayable(m) | Self::Missed(m) => f.write_str(m),
            Self::Stopped => f.write_str("the player has stopped"),
        }
    }
}

/// Send `cmd` to the device being controlled, as the link carries it, or to
/// this device's player when none is. A device out of reach is an error, and
/// stays the one controlled: nothing meant for it plays here instead.
pub fn send(cmd: PlayerCommand, tx: &Sender<PlayerCommand>) -> Result<(), SendError> {
    let here_now = |cmd| tx.send(cmd).map_err(|_| SendError::Stopped);
    let Some(to) = devices::target() else {
        return here_now(cmd);
    };
    match to_link(cmd).map_err(SendError::Unplayable)? {
        Routed::There(link) => devices::send(&to, link).map_err(SendError::Missed),
        Routed::Here(local) => here_now(local),
        Routed::Dropped => Ok(()),
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

/// The server's id for each item's track, in order, `None` where the server
/// has none. Only a device playing from the same library can resolve them,
/// and a list the server has none of cannot be sent at all.
fn server_ids(items: &[PlaylistItem]) -> Result<Vec<Option<String>>, String> {
    if !devices::target_device().is_some_and(|d| d.same_library) {
        return Err("that device plays from a different library".into());
    }
    let remote = crate::db::pool::shared()
        .get()
        .map(|db| remote_ids(&db, items))
        .unwrap_or_default();
    let out: Vec<Option<String>> = items
        .iter()
        .map(|i| i.db_id.and_then(|id| remote.get(&id).cloned()))
        .collect();
    if out.iter().all(Option::is_none) {
        return Err("none of these are on the server, so the other device cannot play them".into());
    }
    Ok(out)
}

/// A player command as the device being controlled is sent it. Queue entries
/// are named by the ids that device reported them under; tracks by the
/// server's ids.
pub fn to_link(cmd: PlayerCommand) -> Result<Routed, String> {
    let entry = |id: QueueItemId| id.0.to_string();
    let tracks = |items: &[PlaylistItem]| -> Result<Vec<String>, String> {
        Ok(server_ids(items)?.into_iter().flatten().collect())
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
            play: false,
        },
        PlayerCommand::AddAndPlay(items) => LinkCommand::Enqueue {
            track_ids: tracks(&items)?,
            play: true,
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
            let ids = server_ids(&items)?;
            let found: Vec<Option<i64>> = ids.iter().map(|r| r.as_ref().map(|_| 0)).collect();
            let (start_at, position_ms) = start_among(&found, start, position_ms);
            LinkCommand::Play {
                track_ids: ids.into_iter().flatten().collect(),
                start_at: start_at as u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_starts_on_the_track_asked_for_among_those_found() {
        let found = [Some(1), None, Some(3), Some(4)];
        // On it, with its playhead.
        assert_eq!(start_among(&found, 2, 500), (1, 500));
        // Missing: the next one found, from its start.
        assert_eq!(start_among(&found, 1, 500), (1, 0));
        // Missing and nothing after: from the top.
        assert_eq!(start_among(&[Some(1), None], 1, 500), (0, 0));
        assert_eq!(start_among(&found, 0, 0), (0, 0));
    }

    #[test]
    fn an_enqueue_that_does_not_play_reads_as_it_always_did() {
        let plain = LinkCommand::Enqueue {
            track_ids: vec!["t".into()],
            play: false,
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("play").is_none(), "{json}");
        let old: LinkCommand =
            serde_json::from_value(serde_json::json!({"type": "enqueue", "trackIds": ["t"]}))
                .unwrap();
        assert_eq!(old, plain);
    }
}

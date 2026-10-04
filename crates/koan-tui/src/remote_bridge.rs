//! Remote bridge: drives a koan server from the TUI over GraphQL.
//!
//! The server plays the audio; this is a remote control. The TUI sees a normal
//! `SharedPlayerState` and `Sender<PlayerCommand>`: the state mirrors the
//! server's now-playing and queue, and commands go to the server.
//!
//! There is no local playback. Playing a server's library on this machine is
//! what signing in to it as a Subsonic server does, through the same engine,
//! download queue and cache as everything else.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use koan_core::graphql_client::GraphQLClient;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    ItemState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState, TrackInfo,
};

/// Spawn the remote bridge.
///
/// Returns the same types as `Player::spawn()` — the TUI works unchanged.
///
/// `client` is shared by every thread the bridge starts, so they share one
/// session with the server.
pub fn spawn_remote_bridge(
    client: GraphQLClient,
) -> (
    Arc<SharedPlayerState>,
    Arc<koan_core::audio::buffer::PlaybackTimeline>,
    Arc<koan_core::audio::viz::VizSnapshot>,
    Sender<PlayerCommand>,
) {
    let state = SharedPlayerState::new();
    let timeline = koan_core::audio::buffer::PlaybackTimeline::new();
    let viz = koan_core::audio::viz::VizSnapshot::new();

    // Channel for TUI → bridge commands.
    let (cmd_tx, cmd_rx) = bounded::<PlayerCommand>(16);

    // Poller thread: mirrors the server's state into the local one.
    {
        let state = state.clone();
        let client = client.clone();
        std::thread::Builder::new()
            .name("koan-remote-poll".into())
            .spawn(move || poll_loop(client, state))
            .expect("failed to spawn remote poller");
    }

    // Command translator: TUI commands → GQL mutations.
    std::thread::Builder::new()
        .name("koan-remote-cmd".into())
        .spawn(move || command_loop(client, cmd_rx))
        .expect("failed to spawn remote command handler");

    (state, timeline, viz, cmd_tx)
}

fn poll_loop(client: GraphQLClient, state: Arc<SharedPlayerState>) {
    let mut last_track_id: Option<String> = None;
    let mut connected = true;

    loop {
        match client.now_playing() {
            Ok(np) => {
                note_connection(&mut connected, true);
                state.set_playback_state(match np.state.as_str() {
                    "PLAYING" => PlaybackState::Playing,
                    "PAUSED" => PlaybackState::Paused,
                    _ => PlaybackState::Stopped,
                });

                let current_track_id = np.queue_item_id.clone();
                if current_track_id != last_track_id && current_track_id.is_some() {
                    last_track_id = current_track_id.clone();

                    if let Some(ref qid_str) = current_track_id
                        && let Ok(uuid) = uuid::Uuid::parse_str(qid_str)
                        && let Some(ref track) = np.track
                    {
                        state.set_track_info(Some(TrackInfo {
                            id: QueueItemId(uuid),
                            path: PathBuf::from(format!("/remote/{qid_str}")),
                            codec: track.codec.clone(),
                            sample_rate: track.sample_rate,
                            bit_depth: track.bit_depth,
                            bitrate_kbps: track.bitrate_kbps,
                            channels: track.channels,
                            duration_ms: track.duration_ms,
                        }));
                    }
                }

                state.set_position_ms(np.position_ms);
            }
            Err(e) => {
                // Without this the TUI silently freezes on the last known state
                // and keeps retrying at 10Hz with nothing shown to the user.
                if connected {
                    log::warn!("lost connection to {}: {}", client.server_url(), e);
                }
                note_connection(&mut connected, false);
            }
        }

        // Poll queue from server for TUI display.
        match client.queue() {
            Ok(entries) => {
                let items: Vec<PlaylistItem> = entries
                    .iter()
                    .map(|e| {
                        let qid = uuid::Uuid::parse_str(&e.queue_item_id)
                            .map(QueueItemId)
                            .unwrap_or_else(|_| QueueItemId::new());
                        PlaylistItem {
                            playlist_entry_id: None,
                            id: qid,
                            db_id: None,
                            path: PathBuf::from(format!("/remote/{}", e.queue_item_id)),
                            title: e.title.clone(),
                            artist: e.artist.clone(),
                            album_artist: e.artist.clone(),
                            album: e.album.clone(),
                            year: None,
                            codec: e.codec.clone(),
                            track_number: e.track_number,
                            disc: e.disc,
                            duration_ms: e.duration_ms,
                            state: ItemState::Ready,
                        }
                    })
                    .collect();

                let cursor = entries.iter().find(|e| e.is_current).and_then(|e| {
                    uuid::Uuid::parse_str(&e.queue_item_id)
                        .map(QueueItemId)
                        .ok()
                });

                state.restore_playlist(items, cursor);
            }
            Err(e) => log::debug!("queue poll failed: {}", e),
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Log connection transitions only, so a dead server does not spam the TUI at
/// the poll rate but the user still sees that it went away and came back.
fn note_connection(connected: &mut bool, now_up: bool) {
    if now_up && !*connected {
        log::info!("reconnected to server");
    }
    *connected = now_up;
}

fn command_loop(client: GraphQLClient, rx: Receiver<PlayerCommand>) {
    while let Ok(cmd) = rx.recv() {
        match &cmd {
            PlayerCommand::Pause => {
                client.pause().ok();
            }
            PlayerCommand::Resume => {
                client.resume().ok();
            }
            PlayerCommand::Stop => {
                client.stop().ok();
            }
            PlayerCommand::Seek(ms) => {
                client.seek(*ms).ok();
            }
            PlayerCommand::NextTrack => {
                client.next().ok();
            }
            PlayerCommand::PrevTrack => {
                client.previous().ok();
            }
            PlayerCommand::Play(id) => {
                client.play(&id.0.to_string()).ok();
            }
            PlayerCommand::ClearPlaylist => {
                client.clear_queue().ok();
            }
            // The server's queue is built from its own track ids, not from
            // items this process resolved against its library.
            PlayerCommand::ReplacePlaylist { .. } => {
                client.clear_queue().ok();
            }
            PlayerCommand::RemoveFromPlaylist(id) => {
                let _ = client.execute(
                    &format!(
                        r#"mutation {{ removeFromQueue(queueItemIds: ["{}"]) {{ ok }} }}"#,
                        id.0
                    ),
                    None,
                );
            }
            PlayerCommand::RemoveFromPlaylistBatch(ids) => {
                let id_strs: Vec<String> = ids.iter().map(|id| format!("\"{}\"", id.0)).collect();
                let _ = client.execute(
                    &format!(
                        "mutation {{ removeFromQueue(queueItemIds: [{}]) {{ ok }} }}",
                        id_strs.join(", ")
                    ),
                    None,
                );
            }
            PlayerCommand::Undo => {
                let _ = client.execute("mutation { undo { ok } }", None);
            }
            PlayerCommand::Redo => {
                let _ = client.execute("mutation { redo { ok } }", None);
            }
            // Not applicable in remote mode — listed explicitly so the compiler
            // catches new variants. The output device is the server's.
            PlayerCommand::SetOutputDevice(_)
            | PlayerCommand::RestartOutput
            | PlayerCommand::ClearOutputDevice
            | PlayerCommand::ReloadDsp
            | PlayerCommand::TrackReady(_)
            | PlayerCommand::TrackStreamReady(_)
            | PlayerCommand::StreamProbed { .. }
            | PlayerCommand::Cue { .. }
            | PlayerCommand::PauseAndReport(_)
            | PlayerCommand::TrackFailed(_)
            | PlayerCommand::CacheTracks(_)
            | PlayerCommand::BeginUndoBatch
            | PlayerCommand::EndUndoBatch
            | PlayerCommand::UpdatePaths(_)
            | PlayerCommand::MoveInPlaylist { .. }
            | PlayerCommand::MoveItemsInPlaylist { .. }
            | PlayerCommand::ReorderPlaylist(_)
            | PlayerCommand::InsertInPlaylist { .. }
            | PlayerCommand::AddToPlaylist(_)
            | PlayerCommand::DecodeFinished(_)
            | PlayerCommand::TrackQueued => {
                log::debug!("ignoring {:?} in remote mode", cmd);
            }
            // The output is the server's in remote mode, renderer or not.
            PlayerCommand::UseRenderer(_)
            | PlayerCommand::ResumeRenderer(_)
            | PlayerCommand::SetRendererVolume(_)
            | PlayerCommand::Renderer { .. } => {
                log::debug!("ignoring {:?} in remote mode", cmd);
            }
        }
    }
}

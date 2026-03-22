//! Remote bridge: makes a koan GQL server look like a local Player to the TUI.
//!
//! Creates a `SharedPlayerState` + `Sender<PlayerCommand>` pair that the TUI
//! can use unchanged. Under the hood:
//! - A poller thread fetches `nowPlaying` + `queue` from the server on tick
//!   and updates the SharedPlayerState.
//! - A command translator thread receives PlayerCommands and sends GQL mutations.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use koan_core::graphql_client::GraphQLClient;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState, TrackInfo,
};

/// Spawn the remote bridge — returns the same types as `Player::spawn()`.
///
/// The TUI can use these exactly like a local player.
pub fn spawn_remote_bridge(
    server_url: &str,
) -> (
    Arc<SharedPlayerState>,
    Arc<koan_core::audio::buffer::PlaybackTimeline>,
    Arc<koan_core::audio::viz::VizSnapshot>,
    Sender<PlayerCommand>,
) {
    let state = SharedPlayerState::new();
    let timeline = koan_core::audio::buffer::PlaybackTimeline::new();
    let viz = koan_core::audio::viz::VizSnapshot::new();

    let (cmd_tx, cmd_rx) = bounded::<PlayerCommand>(16);

    let client = GraphQLClient::new(server_url);
    let stop = Arc::new(AtomicBool::new(false));

    // Poller thread: updates SharedPlayerState from GQL.
    {
        let state = state.clone();
        let client = client.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("koan-remote-poll".into())
            .spawn(move || {
                poll_loop(client, state, stop);
            })
            .expect("failed to spawn remote poller");
    }

    // Command translator thread: PlayerCommand → GQL mutations.
    {
        let client = client.clone();
        std::thread::Builder::new()
            .name("koan-remote-cmd".into())
            .spawn(move || {
                command_loop(client, cmd_rx);
            })
            .expect("failed to spawn remote command handler");
    }

    (state, timeline, viz, cmd_tx)
}

fn poll_loop(client: GraphQLClient, state: Arc<SharedPlayerState>, stop: Arc<AtomicBool>) {
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        // Fetch now playing.
        if let Ok(np) = client.now_playing() {
            let playback_state = match np.state.as_str() {
                "PLAYING" => PlaybackState::Playing,
                "PAUSED" => PlaybackState::Paused,
                _ => PlaybackState::Stopped,
            };
            state.set_playback_state(playback_state);
            state.set_position_ms(np.position_ms);

            if let Some(ref track) = np.track {
                let qid = np
                    .queue_item_id
                    .as_deref()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .map(QueueItemId)
                    .unwrap_or_else(QueueItemId::new);

                state.set_track_info(Some(TrackInfo {
                    id: qid,
                    path: std::path::PathBuf::from("/remote"),
                    codec: track.codec.clone(),
                    sample_rate: track.sample_rate,
                    bit_depth: track.bit_depth,
                    channels: track.channels,
                    duration_ms: track.duration_ms,
                }));
            } else {
                state.set_track_info(None);
            }
        }

        // Fetch queue.
        if let Ok(entries) = client.queue() {
            let items: Vec<PlaylistItem> = entries
                .iter()
                .map(|e| {
                    let qid = uuid::Uuid::parse_str(&e.queue_item_id)
                        .map(QueueItemId)
                        .unwrap_or_else(|_| QueueItemId::new());
                    PlaylistItem {
                        id: qid,
                        path: std::path::PathBuf::from(format!("/remote/{}", e.queue_item_id)),
                        title: e.title.clone(),
                        artist: e.artist.clone(),
                        album_artist: e.artist.clone(),
                        album: e.album.clone(),
                        year: None,
                        codec: e.codec.clone(),
                        track_number: e.track_number,
                        disc: e.disc,
                        duration_ms: e.duration_ms,
                        load_state: LoadState::Ready,
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

        std::thread::sleep(Duration::from_millis(100));
    }
}

fn command_loop(client: GraphQLClient, rx: Receiver<PlayerCommand>) {
    while let Ok(cmd) = rx.recv() {
        let result = match cmd {
            PlayerCommand::Pause => client.pause(),
            PlayerCommand::Resume => client.resume(),
            PlayerCommand::Stop => client.stop(),
            PlayerCommand::NextTrack => client.next(),
            PlayerCommand::PrevTrack => client.previous(),
            PlayerCommand::Seek(ms) => client.seek(ms),
            PlayerCommand::Play(id) => client.play(&id.0.to_string()),
            PlayerCommand::ClearPlaylist => client.clear_queue(),
            PlayerCommand::AddToPlaylist(items) => {
                // We don't have track IDs on PlaylistItems — they have paths.
                // For remote mode, we'd need the track IDs. For now, log a warning.
                log::warn!(
                    "AddToPlaylist not directly supported in remote mode ({} items) — use GQL mutations",
                    items.len()
                );
                Ok(())
            }
            PlayerCommand::RemoveFromPlaylist(id) => {
                // Single remove via GQL
                let _ = client.execute(
                    &format!(
                        r#"mutation {{ removeFromQueue(queueItemIds: ["{}"]) {{ ok }} }}"#,
                        id.0
                    ),
                    None,
                );
                Ok(())
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
                Ok(())
            }
            PlayerCommand::Undo => {
                let _ = client.execute("mutation { undo { ok } }", None);
                Ok(())
            }
            PlayerCommand::Redo => {
                let _ = client.execute("mutation { redo { ok } }", None);
                Ok(())
            }
            PlayerCommand::SetOutputDevice(name) => {
                let _ = client.execute(
                    &format!(r#"mutation {{ setDevice(name: "{}") {{ ok }} }}"#, name),
                    None,
                );
                Ok(())
            }
            PlayerCommand::ClearOutputDevice => {
                let _ = client.execute("mutation { clearDevice { ok } }", None);
                Ok(())
            }
            // These don't map to remote operations.
            PlayerCommand::TrackReady(_)
            | PlayerCommand::TrackStreamReady(_)
            | PlayerCommand::BeginUndoBatch
            | PlayerCommand::EndUndoBatch
            | PlayerCommand::UpdatePaths(_)
            | PlayerCommand::MoveInPlaylist { .. }
            | PlayerCommand::MoveItemsInPlaylist { .. }
            | PlayerCommand::InsertInPlaylist { .. } => Ok(()),
        };

        if let Err(e) = result {
            log::warn!("remote command failed: {}", e);
        }
    }
}

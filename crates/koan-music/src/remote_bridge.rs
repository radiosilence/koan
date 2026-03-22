//! Remote bridge: connects TUI to a remote koan server via GQL.
//!
//! Spawns a local Player for audio output. The server owns the queue/library state.
//! When the server's now-playing changes, the bridge downloads the track from
//! the server's stream endpoint and plays it locally.
//!
//! The TUI sees a normal SharedPlayerState + Sender<PlayerCommand>.
//! Commands go to the server via GQL. Audio plays locally.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use koan_core::graphql_client::GraphQLClient;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState, TrackInfo,
};

/// Spawn the remote bridge with local audio playback.
///
/// Returns the same types as `Player::spawn()` — the TUI works unchanged.
pub fn spawn_remote_bridge(
    server_url: &str,
) -> (
    Arc<SharedPlayerState>,
    Arc<koan_core::audio::buffer::PlaybackTimeline>,
    Arc<koan_core::audio::viz::VizSnapshot>,
    Sender<PlayerCommand>,
) {
    // Spawn a real local Player for audio output.
    let (state, timeline, viz, local_tx) = koan_core::player::Player::spawn();

    // Channel for TUI → bridge commands.
    let (cmd_tx, cmd_rx) = bounded::<PlayerCommand>(16);

    let client = GraphQLClient::new(server_url);
    let stream_base = format!("{}/rest/stream", server_url.trim_end_matches('/'));

    // Poller thread: syncs remote state → local SharedPlayerState + triggers downloads.
    {
        let state = state.clone();
        let local_tx = local_tx.clone();
        let client = client.clone();
        let stream_base = stream_base.clone();
        std::thread::Builder::new()
            .name("koan-remote-poll".into())
            .spawn(move || {
                poll_and_stream_loop(client, state, local_tx, stream_base);
            })
            .expect("failed to spawn remote poller");
    }

    // Command translator: TUI commands → GQL mutations + local player forwarding.
    {
        let client = client.clone();
        let local_tx_fwd = local_tx.clone();
        std::thread::Builder::new()
            .name("koan-remote-cmd".into())
            .spawn(move || {
                command_loop(client, cmd_rx, local_tx_fwd);
            })
            .expect("failed to spawn remote command handler");
    }

    (state, timeline, viz, cmd_tx)
}

/// Downloads a track from the server and plays it via the local player.
fn download_and_play(
    stream_url: &str,
    queue_id: QueueItemId,
    state: &Arc<SharedPlayerState>,
    local_tx: &Sender<PlayerCommand>,
) {
    let cache_dir = koan_core::config::config_dir().join("cache/remote-stream");
    std::fs::create_dir_all(&cache_dir).ok();

    let dest = cache_dir.join(format!("{}.audio", queue_id.0));

    // If already cached from a previous play, just signal ready.
    if dest.exists()
        && std::fs::metadata(&dest)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        state.update_load_state(queue_id, LoadState::Ready);
        local_tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        return;
    }

    let http = reqwest::blocking::Client::new();
    let resp = match http.get(stream_url).send() {
        Ok(r) => r,
        Err(e) => {
            log::warn!("failed to stream from server: {}", e);
            state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
            return;
        }
    };

    let total = resp.content_length().unwrap_or(0);
    let bytes_written = Arc::new(AtomicU64::new(0));
    let stream_ready_sent = std::sync::atomic::AtomicBool::new(false);

    // Create parent dirs and open file.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = match std::fs::File::create(&dest) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("failed to create cache file: {}", e);
            state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
            return;
        }
    };

    // Stream bytes, update progress, signal when enough is buffered.
    let mut reader = resp;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match std::io::Read::read(&mut reader, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                log::warn!("stream read error: {}", e);
                break;
            }
        };

        if std::io::Write::write_all(&mut file, &buf[..n]).is_err() {
            break;
        }

        let downloaded = bytes_written.fetch_add(n as u64, Ordering::Release) + n as u64;
        state.update_load_state(
            queue_id,
            LoadState::Downloading {
                downloaded,
                total,
                bytes_written: bytes_written.clone(),
            },
        );

        // Signal streaming ready after threshold.
        if !stream_ready_sent.load(Ordering::Relaxed)
            && downloaded >= koan_core::player::state::STREAM_THRESHOLD
        {
            stream_ready_sent.store(true, Ordering::Relaxed);
            local_tx
                .send(PlayerCommand::TrackStreamReady(queue_id))
                .ok();
        }
    }

    std::io::Write::flush(&mut file).ok();
    state.update_load_state(queue_id, LoadState::Ready);
    local_tx.send(PlayerCommand::TrackReady(queue_id)).ok();
}

fn poll_and_stream_loop(
    client: GraphQLClient,
    state: Arc<SharedPlayerState>,
    local_tx: Sender<PlayerCommand>,
    stream_base: String,
) {
    let mut last_track_id: Option<String> = None;

    loop {
        // Poll now playing from server.
        if let Ok(np) = client.now_playing() {
            let server_state = match np.state.as_str() {
                "PLAYING" => PlaybackState::Playing,
                "PAUSED" => PlaybackState::Paused,
                _ => PlaybackState::Stopped,
            };

            // Detect track change — need to download new track.
            let current_track_id = np.queue_item_id.clone();
            if current_track_id != last_track_id && current_track_id.is_some() {
                last_track_id = current_track_id.clone();

                if let Some(ref qid_str) = current_track_id
                    && let Ok(uuid) = uuid::Uuid::parse_str(qid_str)
                    && let Some(ref track) = np.track
                {
                    let queue_id = QueueItemId(uuid);
                    let cache_dir = koan_core::config::config_dir().join("cache/remote-stream");
                    let dest = cache_dir.join(format!("{}.audio", uuid));

                    state.set_track_info(Some(TrackInfo {
                        id: queue_id,
                        path: dest.clone(),
                        codec: track.codec.clone(),
                        sample_rate: track.sample_rate,
                        bit_depth: track.bit_depth,
                        channels: track.channels,
                        duration_ms: track.duration_ms,
                    }));

                    let item = PlaylistItem {
                        id: queue_id,
                        path: dest,
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        album_artist: track.artist.clone(),
                        album: track.album.clone(),
                        year: None,
                        codec: Some(track.codec.clone()),
                        track_number: None,
                        disc: None,
                        duration_ms: Some(track.duration_ms),
                        load_state: LoadState::Pending,
                    };

                    local_tx.send(PlayerCommand::ClearPlaylist).ok();
                    local_tx.send(PlayerCommand::AddToPlaylist(vec![item])).ok();

                    let stream_url = format!("{}?id={}", stream_base, qid_str);
                    let state_dl = state.clone();
                    let tx_dl = local_tx.clone();
                    std::thread::Builder::new()
                        .name("koan-remote-dl".into())
                        .spawn(move || {
                            download_and_play(&stream_url, queue_id, &state_dl, &tx_dl);
                        })
                        .ok();
                }
            }

            // Sync playback state (pause/resume from server).
            let local_state = state.playback_state();
            if server_state == PlaybackState::Paused && local_state == PlaybackState::Playing {
                local_tx.send(PlayerCommand::Pause).ok();
            } else if server_state == PlaybackState::Playing && local_state == PlaybackState::Paused
            {
                local_tx.send(PlayerCommand::Resume).ok();
            }

            state.set_position_ms(np.position_ms);
        }

        // Poll queue from server for TUI display.
        if let Ok(entries) = client.queue() {
            let items: Vec<PlaylistItem> = entries
                .iter()
                .map(|e| {
                    let qid = uuid::Uuid::parse_str(&e.queue_item_id)
                        .map(QueueItemId)
                        .unwrap_or_else(|_| QueueItemId::new());
                    PlaylistItem {
                        id: qid,
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

fn command_loop(
    client: GraphQLClient,
    rx: Receiver<PlayerCommand>,
    local_tx: Sender<PlayerCommand>,
) {
    while let Ok(cmd) = rx.recv() {
        // Forward playback commands to both server (GQL) and local player.
        match &cmd {
            PlayerCommand::Pause => {
                client.pause().ok();
                local_tx.send(PlayerCommand::Pause).ok();
            }
            PlayerCommand::Resume => {
                client.resume().ok();
                local_tx.send(PlayerCommand::Resume).ok();
            }
            PlayerCommand::Stop => {
                client.stop().ok();
                local_tx.send(PlayerCommand::Stop).ok();
            }
            PlayerCommand::Seek(ms) => {
                client.seek(*ms).ok();
                local_tx.send(PlayerCommand::Seek(*ms)).ok();
            }
            // These go to server only — the poller handles local playback.
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
            PlayerCommand::SetOutputDevice(name) => {
                // Device switching is local — the client owns the audio output.
                local_tx
                    .send(PlayerCommand::SetOutputDevice(name.clone()))
                    .ok();
            }
            PlayerCommand::ClearOutputDevice => {
                local_tx.send(PlayerCommand::ClearOutputDevice).ok();
            }
            // Not applicable in remote mode.
            _ => {}
        }
    }
}

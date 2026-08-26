//! Remote bridge: connects TUI to a remote koan server via GQL.
//!
//! Spawns a local Player for audio output. The server owns the queue/library state.
//! When the server's now-playing changes, the bridge downloads the track from
//! the server's stream endpoint and plays it locally.
//!
//! The TUI sees a normal SharedPlayerState + Sender<PlayerCommand>.
//! Commands go to the server via GQL. Audio plays locally.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use koan_core::graphql_client::GraphQLClient;
use koan_core::helpers::sanitise_filename;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState, TrackInfo,
};
use koan_core::remote::client::SubsonicClient;
use koan_core::remote::download;

/// Disk budget for streamed-from-server tracks. These files belong to no local
/// track row, so DB-driven cache eviction cannot see them — the bridge prunes
/// its own directory instead.
const STREAM_CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Spawn the remote bridge with local audio playback.
///
/// Returns the same types as `Player::spawn()` — the TUI works unchanged.
///
/// `jukebox`: if true, the server plays audio. No local Player is spawned.
/// The client is purely a remote control.
pub fn spawn_remote_bridge(
    server_url: &str,
    jukebox: bool,
) -> (
    Arc<SharedPlayerState>,
    Arc<koan_core::audio::buffer::PlaybackTimeline>,
    Arc<koan_core::audio::viz::VizSnapshot>,
    Sender<PlayerCommand>,
) {
    // In jukebox mode: no local audio. In client mode: local Player for audio.
    let (state, timeline, viz, local_tx) = if jukebox {
        let state = SharedPlayerState::new();
        let timeline = koan_core::audio::buffer::PlaybackTimeline::new();
        let viz = koan_core::audio::viz::VizSnapshot::new();
        let (tx, _rx) = bounded::<PlayerCommand>(16); // dummy — no local player
        (state, timeline, viz, tx)
    } else {
        koan_core::player::Player::spawn()
    };

    // Channel for TUI → bridge commands.
    let (cmd_tx, cmd_rx) = bounded::<PlayerCommand>(16);

    let client = GraphQLClient::new(server_url);
    let streamer = stream_client(server_url).map(Arc::new);

    // Poller thread: syncs remote state → local SharedPlayerState.
    // In client mode also triggers downloads. In jukebox mode, display only.
    {
        let state = state.clone();
        let local_tx = local_tx.clone();
        let client = client.clone();
        let streamer = streamer.clone();
        std::thread::Builder::new()
            .name("koan-remote-poll".into())
            .spawn(move || {
                poll_and_stream_loop(client, state, local_tx, streamer, jukebox);
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

/// Client for the server's `/rest/*` endpoints.
///
/// `koan serve` guards those with the `[subsonic]` credentials, not the JWT the
/// GraphQL side uses, so the bridge signs its stream requests the Subsonic way
/// — `u` + `t=md5(secret + salt)` + `s`. The credentials come from *this*
/// machine's config: pointing at someone else's server means copying its
/// `[subsonic]` username and secret locally.
fn stream_client(server_url: &str) -> Option<SubsonicClient> {
    let cfg = koan_core::config::Config::load().unwrap_or_default();
    let secret = koan_core::helpers::get_subsonic_password(&cfg);
    match secret {
        Some(secret) if !cfg.subsonic.username.is_empty() => Some(SubsonicClient::new(
            server_url,
            &cfg.subsonic.username,
            &secret,
        )),
        _ => {
            log::warn!(
                "no [subsonic] credentials configured — cannot stream audio from the server. \
                 Run `koan subsonic setup` and copy the secret from the server's config."
            );
            None
        }
    }
}

/// Cache path for a track streamed from a koan server.
///
/// Keyed on the track's identity rather than its queue item id: a UUIDv7 per
/// queue entry meant nothing was ever reused and every play left a full-size
/// file behind. Mirrors the main cache layout so the tree is browsable.
fn stream_cache_path(
    cache_dir: &Path,
    track: &koan_core::graphql_client::NowPlayingTrack,
) -> PathBuf {
    let ext = if track.codec.is_empty() {
        "audio".to_string()
    } else {
        track.codec.to_lowercase()
    };
    cache_dir
        .join(sanitise_filename(&track.artist))
        .join(sanitise_filename(&track.album))
        .join(format!("{}.{}", sanitise_filename(&track.title), ext))
}

/// Trim the stream cache to `budget` bytes, dropping least-recently-modified
/// files first. Cheap enough to run before each download.
fn prune_stream_cache(cache_dir: &Path, budget: u64, keep: &Path) {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total = 0u64;
    for entry in jwalk::WalkDir::new(cache_dir).into_iter().flatten() {
        let path = entry.path();
        if !entry.file_type().is_file() || path == keep {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        total += meta.len();
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        files.push((mtime, meta.len(), path));
    }

    if total <= budget {
        return;
    }

    files.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, len, path) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            log::info!("pruned stream cache entry {}", path.display());
        }
    }
}

/// Downloads a track from the server and plays it via the local player.
fn download_and_play(
    streamer: &SubsonicClient,
    track_id: i64,
    dest: &Path,
    queue_id: QueueItemId,
    state: &Arc<SharedPlayerState>,
    local_tx: &Sender<PlayerCommand>,
) {
    // A file at `dest` is always complete — downloads land there by rename only.
    if dest.exists() {
        state.update_load_state(queue_id, LoadState::Ready);
        local_tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        return;
    }

    // Point the queue item at the in-progress file so the decoder reads bytes
    // as they land; it flips to `dest` once the rename has happened.
    state.update_paths(&[(queue_id, download::part_path(dest))]);

    let bytes_written = Arc::new(AtomicU64::new(0));
    let stream_ready_sent = std::sync::atomic::AtomicBool::new(false);

    // Once per attempt, not per chunk — see `helpers::download_track`. The
    // counter carries progress; the load state only carries the fact.
    let announced_total = AtomicU64::new(u64::MAX);
    let in_progress = download::part_path(dest);
    let result = streamer.stream_to_file(&track_id.to_string(), dest, |downloaded, total| {
        bytes_written.store(downloaded, Ordering::Release);
        if announced_total.swap(total, Ordering::Relaxed) != total {
            state.update_load_state(
                queue_id,
                LoadState::Downloading {
                    path: in_progress.clone(),
                    total,
                    bytes_written: bytes_written.clone(),
                },
            );
        }
        if !stream_ready_sent.load(Ordering::Relaxed)
            && downloaded >= koan_core::player::state::STREAM_THRESHOLD
        {
            stream_ready_sent.store(true, Ordering::Relaxed);
            local_tx
                .send(PlayerCommand::TrackStreamReady(queue_id))
                .ok();
        }
    });

    if let Err(e) = result {
        log::warn!("failed to stream {} from server: {}", dest.display(), e);
        state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
        return;
    }

    state.update_paths(&[(queue_id, dest.to_path_buf())]);
    state.update_load_state(queue_id, LoadState::Ready);
    local_tx.send(PlayerCommand::TrackReady(queue_id)).ok();
}

fn poll_and_stream_loop(
    client: GraphQLClient,
    state: Arc<SharedPlayerState>,
    local_tx: Sender<PlayerCommand>,
    streamer: Option<Arc<SubsonicClient>>,
    jukebox: bool,
) {
    let mut last_track_id: Option<String> = None;
    let mut connected = true;
    let cache_dir = koan_core::config::config_dir().join("cache/remote-stream");

    loop {
        // Poll now playing from server.
        match client.now_playing() {
            Ok(np) => {
                note_connection(&mut connected, true);
                let server_state = match np.state.as_str() {
                    "PLAYING" => PlaybackState::Playing,
                    "PAUSED" => PlaybackState::Paused,
                    _ => PlaybackState::Stopped,
                };

                // Detect track change.
                let current_track_id = np.queue_item_id.clone();
                if current_track_id != last_track_id && current_track_id.is_some() {
                    last_track_id = current_track_id.clone();

                    if let Some(ref qid_str) = current_track_id
                        && let Ok(uuid) = uuid::Uuid::parse_str(qid_str)
                        && let Some(ref track) = np.track
                    {
                        let queue_id = QueueItemId(uuid);
                        let dest = stream_cache_path(&cache_dir, track);

                        // Update track info for TUI display (both modes).
                        state.set_track_info(Some(TrackInfo {
                            id: queue_id,
                            path: dest.clone(),
                            codec: track.codec.clone(),
                            sample_rate: track.sample_rate,
                            bit_depth: track.bit_depth,
                            bitrate_kbps: track.bitrate_kbps,
                            channels: track.channels,
                            duration_ms: track.duration_ms,
                        }));

                        // Client mode: download and play locally.
                        if !jukebox {
                            let cached = dest.exists();
                            let item = PlaylistItem {
                                playlist_entry_id: None,
                                id: queue_id,
                                db_id: None,
                                path: if cached {
                                    dest.clone()
                                } else {
                                    download::part_path(&dest)
                                },
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

                            // `/rest/stream` takes the server's library row id.
                            // The queue item id was a UUIDv7 the endpoint could
                            // never resolve, so every request 400'd.
                            match (streamer.clone(), track.track_id) {
                                (Some(streamer), Some(track_id)) => {
                                    let state_dl = state.clone();
                                    let tx_dl = local_tx.clone();
                                    let cache_dir_dl = cache_dir.clone();
                                    if let Err(e) = std::thread::Builder::new()
                                        .name("koan-remote-dl".into())
                                        .spawn(move || {
                                            prune_stream_cache(
                                                &cache_dir_dl,
                                                STREAM_CACHE_BUDGET_BYTES,
                                                &dest,
                                            );
                                            download_and_play(
                                                &streamer, track_id, &dest, queue_id, &state_dl,
                                                &tx_dl,
                                            );
                                        })
                                    {
                                        log::error!("failed to spawn stream download: {}", e);
                                        state.update_load_state(
                                            queue_id,
                                            LoadState::Failed(e.to_string()),
                                        );
                                    }
                                }
                                (None, _) => state.update_load_state(
                                    queue_id,
                                    LoadState::Failed("no [subsonic] credentials".into()),
                                ),
                                (_, None) => state.update_load_state(
                                    queue_id,
                                    LoadState::Failed("server track has no library id".into()),
                                ),
                            }
                        }
                    }
                }

                // Sync playback state (pause/resume from server).
                let local_state = state.playback_state();
                if server_state == PlaybackState::Paused && local_state == PlaybackState::Playing {
                    local_tx.send(PlayerCommand::Pause).ok();
                } else if server_state == PlaybackState::Playing
                    && local_state == PlaybackState::Paused
                {
                    local_tx.send(PlayerCommand::Resume).ok();
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
            // The bridge drives a remote server's queue, which is rebuilt from
            // track ids rather than from items this process built.
            PlayerCommand::ReplacePlaylist { .. } => {
                client.clear_queue().ok();
                local_tx.send(cmd).ok();
                continue;
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
            // Not applicable in remote mode — listed explicitly so the compiler
            // catches new variants.
            PlayerCommand::TrackReady(_)
            | PlayerCommand::TrackStreamReady(_)
            | PlayerCommand::StreamProbed { .. }
            | PlayerCommand::TrackFailed(_)
            | PlayerCommand::BeginUndoBatch
            | PlayerCommand::EndUndoBatch
            | PlayerCommand::UpdatePaths(_)
            | PlayerCommand::MoveInPlaylist { .. }
            | PlayerCommand::MoveItemsInPlaylist { .. }
            | PlayerCommand::ReorderPlaylist(_)
            | PlayerCommand::InsertInPlaylist { .. }
            | PlayerCommand::AddToPlaylist(_)
            | PlayerCommand::DecodeFinished => {
                log::debug!("ignoring {:?} in remote mode", cmd);
            }
        }
    }
}

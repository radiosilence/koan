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
use koan_core::graphql_client::{GraphQLClient, NowPlaying, QueueEntry};
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

    // Mirror thread: follows the server's now-playing and queue.
    {
        let state = state.clone();
        let client = client.clone();
        std::thread::Builder::new()
            .name("koan-remote-mirror".into())
            .spawn(move || mirror_loop(client, state))
            .expect("failed to spawn remote mirror");
    }

    // Command translator: TUI commands → GQL mutations.
    std::thread::Builder::new()
        .name("koan-remote-cmd".into())
        .spawn(move || command_loop(client, cmd_rx))
        .expect("failed to spawn remote command handler");

    (state, timeline, viz, cmd_tx)
}

/// Waits between tries at a server that is not answering: doubled after each
/// failure, from the first to the last.
const RETRY: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(30)];

/// Follow the server's `nowPlaying` and `queueUpdated` subscriptions, which
/// push when something changes, and mirror each message into `state`. The
/// playhead is mirrored as an anchor that counts on by itself while playing.
fn mirror_loop(client: GraphQLClient, state: Arc<SharedPlayerState>) {
    let mut retry = RETRY[0];
    let mut connected = true;
    loop {
        let queries = [
            client.now_playing_subscription(),
            GraphQLClient::queue_subscription(),
        ];
        let mut last_track_id = None;
        let ended = client.subscribe(&queries, |query, data| {
            if !connected {
                crate::tasks::say_later("reconnected to the server");
                log::info!("reconnected to {}", client.server_url());
                connected = true;
            }
            retry = RETRY[0];
            match query {
                0 => mirror_now_playing(
                    &state,
                    NowPlaying::from_json(&data["nowPlaying"]),
                    &mut last_track_id,
                ),
                _ => mirror_queue(&state, &QueueEntry::list(&data["queueUpdated"]["entries"])),
            }
        });
        if client.predates(&ended) {
            continue;
        }
        if connected {
            log::warn!("lost connection to {}: {ended}", client.server_url());
            crate::tasks::say_later(format!("lost the server, retrying: {ended}"));
            connected = false;
        }
        std::thread::sleep(retry);
        retry = (retry * 2).min(RETRY[1]);
    }
}

fn mirror_now_playing(
    state: &SharedPlayerState,
    np: NowPlaying,
    last_track_id: &mut Option<String>,
) {
    let playback = match np.state.as_str() {
        "PLAYING" => PlaybackState::Playing,
        "PAUSED" => PlaybackState::Paused,
        _ => PlaybackState::Stopped,
    };
    state.set_playback_state(playback);
    state.set_play_mode(np.mode);
    state.set_sleep(np.sleep);

    if np.queue_item_id.is_some() && np.queue_item_id != *last_track_id {
        last_track_id.clone_from(&np.queue_item_id);
        if let Some(ref qid_str) = np.queue_item_id
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

    state.mirror_playhead(np.position_ms, playback == PlaybackState::Playing);
}

fn mirror_queue(state: &SharedPlayerState, entries: &[QueueEntry]) {
    let items: Vec<PlaylistItem> = entries
        .iter()
        .map(|e| {
            let qid = uuid::Uuid::parse_str(&e.queue_item_id)
                .map(QueueItemId)
                .unwrap_or_else(|_| QueueItemId::new());
            PlaylistItem {
                uid: None,
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
                played: false,
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

fn command_loop(client: GraphQLClient, rx: Receiver<PlayerCommand>) {
    while let Ok(cmd) = rx.recv() {
        let sent = match &cmd {
            PlayerCommand::Pause => client.pause(),
            PlayerCommand::Resume => client.resume(),
            PlayerCommand::Stop => client.stop(),
            PlayerCommand::Seek(ms) => client.seek(*ms),
            PlayerCommand::NextTrack => client.next(),
            PlayerCommand::PrevTrack => client.previous(),
            PlayerCommand::SetShuffle(on) => client.set_shuffle(*on),
            PlayerCommand::SetRepeat(repeat) => client.set_repeat(*repeat),
            PlayerCommand::SetSleepTimer(timer) => client.set_sleep_timer(*timer),
            PlayerCommand::Play(id) => client.play(&id.0.to_string()),
            PlayerCommand::ClearPlaylist => client.clear_queue(),
            // The server's queue is built from its own track ids, not from
            // items this process resolved against its library.
            PlayerCommand::ReplacePlaylist { .. } => client.clear_queue(),
            PlayerCommand::RemoveFromPlaylist(id) => client
                .execute(
                    &format!(
                        r#"mutation {{ removeFromQueue(queueItemIds: ["{}"]) {{ ok }} }}"#,
                        id.0
                    ),
                    None,
                )
                .map(drop),
            PlayerCommand::RemoveFromPlaylistBatch(ids) => {
                let id_strs: Vec<String> = ids.iter().map(|id| format!("\"{}\"", id.0)).collect();
                client
                    .execute(
                        &format!(
                            "mutation {{ removeFromQueue(queueItemIds: [{}]) {{ ok }} }}",
                            id_strs.join(", ")
                        ),
                        None,
                    )
                    .map(drop)
            }
            PlayerCommand::Undo => client.execute("mutation { undo { ok } }", None).map(drop),
            PlayerCommand::Redo => client.execute("mutation { redo { ok } }", None).map(drop),
            // Not applicable in remote mode — listed explicitly so the compiler
            // catches new variants. The output device is the server's.
            PlayerCommand::SetOutputDevice(_)
            | PlayerCommand::RestartOutput
            | PlayerCommand::ClearOutputDevice
            | PlayerCommand::ReloadDsp
            | PlayerCommand::RouteChanged
            | PlayerCommand::TrackReady(_)
            | PlayerCommand::TrackStreamReady(_)
            | PlayerCommand::StreamProbed { .. }
            | PlayerCommand::Cue { .. }
            | PlayerCommand::PauseAndReport(_)
            | PlayerCommand::Barrier(_)
            | PlayerCommand::TrackFailed(_)
            | PlayerCommand::CacheTracks(_)
            | PlayerCommand::SignedOut
            | PlayerCommand::BeginUndoBatch
            | PlayerCommand::EndUndoBatch
            | PlayerCommand::UpdatePaths(_)
            | PlayerCommand::MoveInPlaylist { .. }
            | PlayerCommand::MoveItemsInPlaylist { .. }
            | PlayerCommand::ReorderPlaylist(_)
            | PlayerCommand::InsertInPlaylist { .. }
            | PlayerCommand::AddToPlaylist(_)
            | PlayerCommand::AddAndPlay(_)
            | PlayerCommand::DecodeFinished(_)
            | PlayerCommand::RestorePlayMode(_)
            | PlayerCommand::TrackQueued => {
                log::debug!("ignoring {:?} in remote mode", cmd);
                Ok(())
            }
            // The output is the server's in remote mode, renderer or not.
            PlayerCommand::UseRenderer(_)
            | PlayerCommand::ResumeRenderer(_)
            | PlayerCommand::ResumeRendererMissed
            | PlayerCommand::ReleaseRenderer(_)
            | PlayerCommand::SetRendererVolume(_)
            | PlayerCommand::Renderer { .. } => {
                log::debug!("ignoring {:?} in remote mode", cmd);
                Ok(())
            }
        };
        if let Err(e) = sent {
            log::warn!("{cmd:?} failed on the server: {e}");
            crate::tasks::say_later(format!("server: {e}"));
        }
    }
}

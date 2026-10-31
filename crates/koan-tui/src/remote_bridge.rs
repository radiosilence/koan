//! Remote bridge: drives a koan server from the TUI over GraphQL.
//!
//! The server plays the audio; this is a remote control. The TUI sees a normal
//! `SharedPlayerState` and `Sender<PlayerCommand>`: the state mirrors the
//! server's now-playing and queue, and commands go to the server.
//!
//! There is no local playback. Playing a server's library on this machine is
//! what signing in to it as a Subsonic server does, through the same engine,
//! download queue and cache as everything else.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
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
            .spawn(move || mirror_loop(client, Arc::downgrade(&state)))
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
/// playhead is mirrored as an anchor that counts on by itself while it moves.
///
/// A socket that worked and then closed is opened again at once and said
/// nothing about: the server closes it on purpose whenever the access token
/// it was opened with lapses, and the reopening refreshes the token. Only an
/// attempt that fails to hear anything is an outage, shown and backed off.
///
/// Ends once nothing else holds `state`: at the next message, reconnection or
/// retry after the TUI lets it go.
fn mirror_loop(client: GraphQLClient, state: Weak<SharedPlayerState>) {
    let mut retry = RETRY[0];
    let mut connected = true;
    while state.strong_count() > 0 {
        let queries = [
            client.now_playing_subscription(),
            GraphQLClient::queue_subscription(),
        ];
        let mut last_track_id = None;
        let opened = std::time::Instant::now();
        let mut heard = false;
        let ended = client.subscribe(&queries, |query, data| {
            let Some(state) = state.upgrade() else {
                return ControlFlow::Break(());
            };
            heard = true;
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
            ControlFlow::Continue(())
        });
        if state.strong_count() == 0 {
            return;
        }
        if client.predates(&ended) {
            continue;
        }
        // Lasted a moment at least, so a server that answers and hangs up
        // straight away is still backed off.
        if heard && opened.elapsed() >= RETRY[0] {
            log::debug!("subscriptions closed, reopening: {ended}");
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

    state.mirror_playhead(np.position_ms, np.moving);
}

/// The server's id for a queue entry. One that is not a UUID gets an id
/// derived from it, so it keeps the same id from one message to the next.
fn queue_item_id(id: &str) -> QueueItemId {
    QueueItemId(uuid::Uuid::parse_str(id).unwrap_or_else(|_| {
        use std::hash::{Hash, Hasher};
        let half = |seed: u8| {
            let mut h = std::hash::DefaultHasher::new();
            (seed, id).hash(&mut h);
            h.finish()
        };
        uuid::Uuid::from_u64_pair(half(0), half(1))
    }))
}

/// The server's queue as this process's playlist: each entry played, failed,
/// downloading (with the server's progress) or ready as the server says.
fn mirror_queue(state: &SharedPlayerState, entries: &[QueueEntry]) {
    let transfers = entries
        .iter()
        .filter(|e| matches!(e.status.as_str(), "DOWNLOADING" | "PRIORITY_PENDING"))
        .map(|e| {
            (
                queue_item_id(&e.queue_item_id),
                e.download_progress.unwrap_or((0, 0)),
            )
        })
        .collect();
    state.mirror_transfers(transfers);
    let items: Vec<PlaylistItem> = entries
        .iter()
        .map(|e| {
            let qid = queue_item_id(&e.queue_item_id);
            let item_state = match e.status.as_str() {
                "FAILED" => ItemState::Failed(
                    e.failure_reason
                        .clone()
                        .unwrap_or_else(|| "failed on the server".into()),
                ),
                "DOWNLOADING" | "PRIORITY_PENDING" => ItemState::Pending,
                _ => ItemState::Ready,
            };
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
                state: item_state,
                played: e.status == "PLAYED",
            }
        })
        .collect();

    let cursor = entries
        .iter()
        .find(|e| e.is_current)
        .map(|e| queue_item_id(&e.queue_item_id));

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
            | PlayerCommand::PlayNext(_)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, status: &str, current: bool) -> QueueEntry {
        QueueEntry {
            queue_item_id: id.into(),
            track_id: None,
            title: id.into(),
            artist: "A".into(),
            album: "X".into(),
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: Some(1000),
            is_current: current,
            status: status.into(),
            failure_reason: None,
            download_progress: None,
        }
    }

    #[test]
    fn the_mirrored_queue_keeps_the_servers_statuses_and_ids() {
        let played = uuid::Uuid::now_v7().to_string();
        let entries = [
            entry(&played, "PLAYED", false),
            entry("not-a-uuid", "PLAYING", true),
            entry(&uuid::Uuid::now_v7().to_string(), "FAILED", false),
        ];
        let state = SharedPlayerState::new();
        mirror_queue(&state, &entries);
        let (first, cursor) = state.snapshot_playlist();
        assert!(first[0].played);
        assert!(!first[1].played);
        assert!(matches!(first[2].state, ItemState::Failed(_)));
        assert_eq!(first[0].id.0.to_string(), played);
        assert_eq!(cursor, Some(first[1].id));

        // The same entries again name the same items, the odd id included.
        mirror_queue(&state, &entries);
        let again: Vec<_> = state.snapshot_playlist().0.iter().map(|i| i.id).collect();
        assert_eq!(again, first.iter().map(|i| i.id).collect::<Vec<_>>());
    }

    #[test]
    fn a_track_the_server_is_fetching_shows_its_progress() {
        use koan_core::player::state::QueueEntryStatus;
        let mut fetching = entry(&uuid::Uuid::now_v7().to_string(), "DOWNLOADING", false);
        fetching.download_progress = Some((250, 1000));
        let entries = [
            entry(&uuid::Uuid::now_v7().to_string(), "PRIORITY_PENDING", true),
            fetching,
        ];
        let state = SharedPlayerState::new();
        mirror_queue(&state, &entries);
        let shown = state.derive_visible_queue().entries;
        assert_ne!(shown[0].status, QueueEntryStatus::Queued);
        assert_eq!(shown[1].status, QueueEntryStatus::Downloading);
        assert_eq!(shown[1].download_progress, Some((250, 1000)));
    }
}

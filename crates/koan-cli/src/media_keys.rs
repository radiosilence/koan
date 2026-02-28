use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueEntryStatus, SharedPlayerState};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};

pub struct MediaKeyHandler {
    controls: MediaControls,
}

impl MediaKeyHandler {
    pub fn new(tx: Sender<PlayerCommand>, state: Arc<SharedPlayerState>) -> Option<Self> {
        let config = PlatformConfig {
            dbus_name: "koan",
            display_name: "koan",
            hwnd: None,
        };

        let mut controls = MediaControls::new(config).ok()?;

        controls
            .attach(move |event: MediaControlEvent| match event {
                MediaControlEvent::Play => {
                    tx.send(PlayerCommand::Resume).ok();
                }
                MediaControlEvent::Pause => {
                    tx.send(PlayerCommand::Pause).ok();
                }
                MediaControlEvent::Toggle => {
                    let cmd = match state.playback_state() {
                        PlaybackState::Playing => PlayerCommand::Pause,
                        _ => PlayerCommand::Resume,
                    };
                    tx.send(cmd).ok();
                }
                MediaControlEvent::Next => {
                    tx.send(PlayerCommand::NextTrack).ok();
                }
                MediaControlEvent::Previous => {
                    tx.send(PlayerCommand::PrevTrack).ok();
                }
                MediaControlEvent::Stop => {
                    tx.send(PlayerCommand::Stop).ok();
                }
                _ => {}
            })
            .ok()?;

        Some(Self { controls })
    }

    pub fn update_metadata(&mut self, state: &SharedPlayerState) {
        let queue = state.full_queue();
        let playing = queue.iter().find(|e| e.status == QueueEntryStatus::Playing);

        if let Some(entry) = playing {
            let title = entry.title.clone();
            let artist = entry.artist.clone();
            let album = entry.album.clone();
            self.controls
                .set_metadata(MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artist),
                    album: Some(&album),
                    cover_url: None,
                    duration: entry.duration_ms.map(Duration::from_millis),
                })
                .ok();
        }
    }

    pub fn update_playback(&mut self, state: &SharedPlayerState) {
        let playback = match state.playback_state() {
            PlaybackState::Playing => MediaPlayback::Playing {
                progress: Some(MediaPosition(Duration::from_millis(state.position_ms()))),
            },
            PlaybackState::Paused => MediaPlayback::Paused {
                progress: Some(MediaPosition(Duration::from_millis(state.position_ms()))),
            },
            PlaybackState::Stopped => MediaPlayback::Stopped,
        };
        self.controls.set_playback(playback).ok();
    }
}

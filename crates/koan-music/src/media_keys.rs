use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, SharedPlayerState};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};

/// Pump the macOS run loop so MPRemoteCommandCenter handlers fire.
/// Terminal apps don't have a Cocoa event loop, so without this the
/// media key callbacks registered by souvlaki never get dispatched.
#[cfg(target_os = "macos")]
pub fn pump_run_loop() {
    use core_foundation::runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode};
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.001, 1);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn pump_run_loop() {}

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
        let Some(info) = state.track_info() else {
            return;
        };

        // Get metadata from playlist for the currently playing track.
        let vq = state.derive_visible_queue();
        let entry = vq.entries.iter().find(|e| e.id == info.id);

        let title = entry.map(|e| e.title.clone()).unwrap_or_default();
        let artist = entry.map(|e| e.artist.clone()).unwrap_or_default();
        let album = entry.map(|e| e.album.clone()).unwrap_or_default();
        let duration = entry.and_then(|e| e.duration_ms).or(Some(info.duration_ms));

        self.controls
            .set_metadata(MediaMetadata {
                title: Some(&title),
                artist: Some(&artist),
                album: Some(&album),
                cover_url: None,
                duration: duration.map(Duration::from_millis),
            })
            .ok();
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

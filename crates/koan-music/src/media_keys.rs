use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;
use koan_core::index::metadata::extract_cover_art;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, SharedPlayerState};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
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

/// Fixed seek amount for direction-only seek events (no duration provided).
const SEEK_STEP_MS: u64 = 10_000;

pub struct MediaKeyHandler {
    controls: MediaControls,
    /// Temp file for cover art — reused across tracks, cleaned up on drop.
    cover_art_path: Option<PathBuf>,
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
                MediaControlEvent::SetPosition(MediaPosition(pos)) => {
                    tx.send(PlayerCommand::Seek(pos.as_millis() as u64)).ok();
                }
                MediaControlEvent::SeekBy(direction, duration) => {
                    let current = state.position_ms();
                    let delta = duration.as_millis() as u64;
                    let target = match direction {
                        SeekDirection::Forward => current.saturating_add(delta),
                        SeekDirection::Backward => current.saturating_sub(delta),
                    };
                    tx.send(PlayerCommand::Seek(target)).ok();
                }
                MediaControlEvent::Seek(direction) => {
                    let current = state.position_ms();
                    let target = match direction {
                        SeekDirection::Forward => current.saturating_add(SEEK_STEP_MS),
                        SeekDirection::Backward => current.saturating_sub(SEEK_STEP_MS),
                    };
                    tx.send(PlayerCommand::Seek(target)).ok();
                }
                MediaControlEvent::Quit => {
                    state.request_quit();
                }
                _ => {}
            })
            .ok()?;

        Some(Self {
            controls,
            cover_art_path: None,
        })
    }

    pub fn update_metadata(&mut self, state: &SharedPlayerState, track_path: Option<&PathBuf>) {
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

        // Extract cover art to a temp file for macOS Now Playing.
        let cover_url = self.write_cover_art(track_path);
        let cover_url_str = cover_url.as_deref();

        self.controls
            .set_metadata(MediaMetadata {
                title: Some(&title),
                artist: Some(&artist),
                album: Some(&album),
                cover_url: cover_url_str,
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

    /// Write embedded cover art to a temp file, returning a file:// URL.
    /// Reuses a single temp path — overwritten each track change.
    fn write_cover_art(&mut self, track_path: Option<&PathBuf>) -> Option<String> {
        let path = track_path?;
        let bytes = extract_cover_art(path)?;

        let tmp = self.cover_art_path.get_or_insert_with(|| {
            std::env::temp_dir().join(format!("koan-cover-{}", std::process::id()))
        });

        std::fs::write(&*tmp, &bytes).ok()?;
        Some(format!("file://{}", tmp.display()))
    }
}

impl Drop for MediaKeyHandler {
    fn drop(&mut self) {
        if let Some(ref path) = self.cover_art_path {
            std::fs::remove_file(path).ok();
        }
    }
}

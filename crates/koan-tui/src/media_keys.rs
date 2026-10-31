use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use koan_core::index::folder_art::cover_art;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueEntry, SharedPlayerState};

use crate::queue::Facts;
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
    // SAFETY: CFRunLoopRunInMode is inherently safe — it pumps the current thread's
    // run loop with a tiny timeout. The unsafe is only required because it's a C FFI
    // call. No preconditions can be violated.
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
    /// What Now Playing was last told, to tell it again once the cover
    /// arrives.
    shown: Option<Shown>,
    /// Asks the cover thread for a track's art, tagged with a generation.
    covers: Sender<(u64, PathBuf)>,
    /// The cover thread's answers: a file:// URL, or none.
    cover_urls: CoverUrls,
    generation: u64,
    /// The file Now Playing's cover was asked for, and the cover once written.
    cover_path: Option<PathBuf>,
    cover_url: Option<String>,
}

/// The cover thread's answers, each tagged with the generation it was asked for.
type CoverUrls = Receiver<(u64, Option<String>)>;

/// A track's metadata as Now Playing was given it.
struct Shown {
    title: String,
    artist: String,
    album: String,
    duration: Option<u64>,
}

/// Reads covers and writes them to the one temp file Now Playing is pointed
/// at, off the frame thread, the latest ask of a burst only. Removes the file
/// once the handler is gone.
fn spawn_cover_thread() -> (Sender<(u64, PathBuf)>, CoverUrls) {
    let (tx, asks) = crossbeam_channel::unbounded::<(u64, PathBuf)>();
    let (done, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("koan-media-cover".into())
        .spawn(move || {
            let tmp = std::env::temp_dir().join(format!("koan-cover-{}", std::process::id()));
            while let Ok(mut next) = asks.recv() {
                while let Ok(newer) = asks.try_recv() {
                    next = newer;
                }
                let (generation, path) = next;
                let url = cover_art(&path)
                    .and_then(|bytes| std::fs::write(&tmp, bytes).ok())
                    .map(|()| format!("file://{}", tmp.display()));
                if done.send((generation, url)).is_err() {
                    break;
                }
            }
            std::fs::remove_file(&tmp).ok();
        })
        .ok();
    (tx, rx)
}

impl MediaKeyHandler {
    /// `shown` is the state the controls act on: this player's, or the
    /// controlled device's; `local` is this one's, which a quit is asked of.
    pub fn new(
        tx: Sender<PlayerCommand>,
        shown: Arc<parking_lot::RwLock<Arc<SharedPlayerState>>>,
        local: Arc<SharedPlayerState>,
    ) -> Option<Self> {
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
                    let cmd = if shown.read().wants_to_play() {
                        PlayerCommand::Pause
                    } else {
                        PlayerCommand::Resume
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
                    let current = shown.read().position_ms();
                    let delta = duration.as_millis() as u64;
                    let target = match direction {
                        SeekDirection::Forward => current.saturating_add(delta),
                        SeekDirection::Backward => current.saturating_sub(delta),
                    };
                    tx.send(PlayerCommand::Seek(target)).ok();
                }
                MediaControlEvent::Seek(direction) => {
                    let current = shown.read().position_ms();
                    let target = match direction {
                        SeekDirection::Forward => current.saturating_add(SEEK_STEP_MS),
                        SeekDirection::Backward => current.saturating_sub(SEEK_STEP_MS),
                    };
                    tx.send(PlayerCommand::Seek(target)).ok();
                }
                MediaControlEvent::Quit => {
                    local.request_quit();
                }
                _ => {}
            })
            .ok()?;

        let (covers, cover_urls) = spawn_cover_thread();
        Some(Self {
            controls,
            shown: None,
            covers,
            cover_urls,
            generation: 0,
            cover_path: None,
            cover_url: None,
        })
    }

    /// Tells Now Playing of the playing track, from the queue and credits
    /// the frame already holds. Its cover follows from the cover thread.
    pub(crate) fn update_metadata(
        &mut self,
        state: &SharedPlayerState,
        entries: &[QueueEntry],
        facts: &Facts,
        track_path: Option<&PathBuf>,
    ) {
        let Some(info) = state.track_info() else {
            return;
        };
        let entry = entries.iter().find(|e| e.id == info.id);
        let artist = entry.map(|e| match facts.credits.get(&e.id) {
            Some(c) => c.artist.clone(),
            None => crate::credits::of_names(&e.artist, &e.album_artist).artist,
        });
        self.shown = Some(Shown {
            title: entry.map(|e| e.title.clone()).unwrap_or_default(),
            artist: artist.unwrap_or_default(),
            album: entry.map(|e| e.album.clone()).unwrap_or_default(),
            duration: entry.and_then(|e| e.duration_ms).or(Some(info.duration_ms)),
        });
        // A refresh for the same file keeps its cover, rather than dropping
        // it and reading it again.
        if track_path.is_some() && track_path == self.cover_path.as_ref() {
            let url = self.cover_url.clone();
            self.set_metadata(url.as_deref());
            return;
        }
        self.generation += 1;
        self.cover_path = track_path.cloned();
        self.cover_url = None;
        self.set_metadata(None);
        if let Some(path) = track_path {
            self.covers.send((self.generation, path.clone())).ok();
        }
    }

    /// Takes in a cover the cover thread has written, for the track still
    /// shown.
    pub fn poll(&mut self) {
        while let Ok((generation, url)) = self.cover_urls.try_recv() {
            if generation == self.generation && url.is_some() {
                self.set_metadata(url.as_deref());
                self.cover_url = url;
            }
        }
    }

    fn set_metadata(&mut self, cover_url: Option<&str>) {
        let Some(shown) = self.shown.as_ref() else {
            return;
        };
        self.controls
            .set_metadata(MediaMetadata {
                title: Some(&shown.title),
                artist: Some(&shown.artist),
                album: Some(&shown.album),
                cover_url,
                duration: shown.duration.map(Duration::from_millis),
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

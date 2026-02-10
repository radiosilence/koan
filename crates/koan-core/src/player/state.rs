use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Playback state — maps to wire values for FFI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaybackState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

impl PlaybackState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Playing,
            2 => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// Audio format info for the currently playing track.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration_ms: u64,
}

/// Shared player state — atomics for lock-free reads from UI thread.
///
/// The engine writes these, the UI/FFI reads them. No mutexes in the hot path.
#[derive(Debug)]
pub struct SharedPlayerState {
    state: AtomicU8,
    position_ms: AtomicU64,
    track_info: parking_lot::RwLock<Option<TrackInfo>>,
}

impl SharedPlayerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(PlaybackState::Stopped as u8),
            position_ms: AtomicU64::new(0),
            track_info: parking_lot::RwLock::new(None),
        })
    }

    pub fn playback_state(&self) -> PlaybackState {
        PlaybackState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn set_playback_state(&self, state: PlaybackState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms.load(Ordering::Relaxed)
    }

    pub fn set_position_ms(&self, pos: u64) {
        self.position_ms.store(pos, Ordering::Relaxed);
    }

    pub fn track_info(&self) -> Option<TrackInfo> {
        self.track_info.read().clone()
    }

    pub fn set_track_info(&self, info: Option<TrackInfo>) {
        *self.track_info.write() = info;
    }
}

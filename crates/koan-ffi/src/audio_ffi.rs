use std::ffi::{CStr, c_char, c_int};
use std::sync::OnceLock;

use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};

struct PlayerHandle {
    state: std::sync::Arc<SharedPlayerState>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
}

static PLAYER: OnceLock<PlayerHandle> = OnceLock::new();

fn player() -> &'static PlayerHandle {
    PLAYER.get_or_init(|| {
        let (state, _timeline, tx) = Player::spawn();
        PlayerHandle { state, tx }
    })
}

/// Initialize the player. Safe to call multiple times (idempotent).
#[unsafe(no_mangle)]
pub extern "C" fn koan_init() {
    let _ = player();
}

/// Shut down the player.
#[unsafe(no_mangle)]
pub extern "C" fn koan_shutdown() {
    if let Some(handle) = PLAYER.get() {
        handle.tx.send(PlayerCommand::Stop).ok();
    }
}

/// Play a file. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn koan_play(path: *const c_char) -> c_int {
    if path.is_null() {
        return -1;
    }
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let handle = player();
    let id = QueueItemId::new();
    let path_buf = std::path::PathBuf::from(path_str);
    let title = path_buf
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let item = PlaylistItem {
        id,
        path: path_buf,
        title,
        artist: String::new(),
        album_artist: String::new(),
        album: String::new(),
        year: None,
        codec: None,
        track_number: None,
        disc: None,
        duration_ms: None,
        load_state: LoadState::Ready,
    };
    if handle
        .tx
        .send(PlayerCommand::AddToPlaylist(vec![item]))
        .is_err()
    {
        return -1;
    }
    match handle.tx.send(PlayerCommand::Play(id)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Pause playback.
#[unsafe(no_mangle)]
pub extern "C" fn koan_pause() {
    if let Some(handle) = PLAYER.get() {
        handle.tx.send(PlayerCommand::Pause).ok();
    }
}

/// Resume playback.
#[unsafe(no_mangle)]
pub extern "C" fn koan_resume() {
    if let Some(handle) = PLAYER.get() {
        handle.tx.send(PlayerCommand::Resume).ok();
    }
}

/// Stop playback.
#[unsafe(no_mangle)]
pub extern "C" fn koan_stop() {
    if let Some(handle) = PLAYER.get() {
        handle.tx.send(PlayerCommand::Stop).ok();
    }
}

/// Seek to position in milliseconds.
#[unsafe(no_mangle)]
pub extern "C" fn koan_seek(position_ms: u64) {
    if let Some(handle) = PLAYER.get() {
        handle.tx.send(PlayerCommand::Seek(position_ms)).ok();
    }
}

/// Get current playback position in milliseconds.
#[unsafe(no_mangle)]
pub extern "C" fn koan_get_position_ms() -> u64 {
    PLAYER.get().map(|h| h.state.position_ms()).unwrap_or(0)
}

/// Get playback state: 0=stopped, 1=playing, 2=paused.
#[unsafe(no_mangle)]
pub extern "C" fn koan_get_state() -> u8 {
    PLAYER
        .get()
        .map(|h| h.state.playback_state() as u8)
        .unwrap_or(0)
}

/// Now-playing info returned across FFI.
#[repr(C)]
pub struct KoanNowPlaying {
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration_ms: u64,
    pub position_ms: u64,
    pub state: u8,
    pub has_track: u8,
}

/// Get current now-playing info as a flat C struct.
#[unsafe(no_mangle)]
pub extern "C" fn koan_get_now_playing() -> KoanNowPlaying {
    let Some(handle) = PLAYER.get() else {
        return KoanNowPlaying {
            sample_rate: 0,
            bit_depth: 0,
            channels: 0,
            duration_ms: 0,
            position_ms: 0,
            state: 0,
            has_track: 0,
        };
    };

    let playback_state = handle.state.playback_state();
    let position_ms = handle.state.position_ms();

    match handle.state.track_info() {
        Some(info) => KoanNowPlaying {
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            channels: info.channels,
            duration_ms: info.duration_ms,
            position_ms,
            state: playback_state as u8,
            has_track: 1,
        },
        None => KoanNowPlaying {
            sample_rate: 0,
            bit_depth: 0,
            channels: 0,
            duration_ms: 0,
            position_ms,
            state: playback_state as u8,
            has_track: 0,
        },
    }
}

/// Ping function to verify C FFI is working.
#[unsafe(no_mangle)]
pub extern "C" fn koan_ping() -> c_int {
    42
}

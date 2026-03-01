mod cache;
mod config;
mod enqueue;
mod library;
mod organize;
mod pick;
mod picker_items;
mod play;
mod probe;
mod remote;
mod scan;
mod search;

pub use cache::{cmd_cache_clear, cmd_cache_status};
pub use config::{cmd_config, cmd_init};
pub use enqueue::enqueue_playlist;
pub use library::{cmd_albums, cmd_artists, cmd_library};
pub use organize::cmd_organize;
pub use pick::cmd_pick;
pub use picker_items::{
    load_picker_items, make_album_picker_items, make_artist_picker_items, make_track_picker_items,
};
pub use play::cmd_play;
pub use probe::{cmd_devices, cmd_probe};
pub use remote::{cmd_remote_login, cmd_remote_status, cmd_remote_sync};
pub use scan::cmd_scan;
pub use search::cmd_search;

use std::path::{Path, PathBuf};

use koan_core::config as core_config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::state::{LoadState, PlaylistItem, QueueItemId};
use owo_colors::OwoColorize;

/// Prompt the user to confirm a destructive action. Returns true if confirmed.
/// Accepts y/yes (case-insensitive), anything else is a no.
pub(crate) fn confirm(prompt: &str) -> bool {
    use std::io::{Write, stdin, stdout};
    print!("{} [y/N] ", prompt);
    stdout().flush().ok();
    let mut input = String::new();
    if stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

pub(crate) fn open_db() -> Database {
    Database::open_default().unwrap_or_else(|e| {
        eprintln!("{} {}", "db error:".red().bold(), e);
        std::process::exit(1);
    })
}

pub(crate) fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let secs = secs % 60;
    format!("{}:{:02}", mins, secs)
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{} B", b),
    }
}

/// Install a panic hook that restores the terminal before printing the panic message.
pub(crate) fn install_terminal_panic_hook() {
    use crossterm::{
        event::DisableMouseCapture,
        execute,
        terminal::{LeaveAlternateScreen, disable_raw_mode},
    };
    use std::io;
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic);
    }));
}

/// Get the remote password from config, falling back to Keychain for backwards compat.
pub(crate) fn get_remote_password(cfg: &core_config::Config) -> String {
    if !cfg.remote.password.is_empty() {
        return cfg.remote.password.clone();
    }
    // Fallback to Keychain for users who set up before the config change.
    match koan_core::credentials::get_password(&cfg.remote.url) {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "{} no password configured — run {} to set up",
                "error:".red().bold(),
                "koan remote login".bold()
            );
            std::process::exit(1);
        }
    }
}

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
pub(crate) fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    // Truncate on a char boundary to stay under 240 bytes.
    if cleaned.len() <= 240 {
        return cleaned;
    }
    let mut end = 240;
    while !cleaned.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    cleaned[..end].trim_end().to_string()
}

/// Build a structured cache path for a track:
///   cache_dir/Album Artist/(Year) Album [Codec]/01. Track Artist - Title.ext
pub(crate) fn cache_path_for_track(
    cache_dir: &Path,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> PathBuf {
    let artist_dir = sanitise_filename(&track.artist_name);

    let year = album_date
        .and_then(|d| if d.len() >= 4 { Some(&d[..4]) } else { None })
        .map(|y| format!("({}) ", y))
        .unwrap_or_default();
    let codec = track
        .codec
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default();
    let album_dir = sanitise_filename(&format!("{}{}{}", year, track.album_title, codec));

    let disc_prefix = match track.disc {
        Some(d) if d > 1 => format!("{}-", d),
        _ => String::new(),
    };
    let track_num = track
        .track_number
        .map(|n| format!("{:02}. ", n))
        .unwrap_or_default();

    let ext = track
        .codec
        .as_deref()
        .map(|c| c.to_lowercase())
        .unwrap_or_else(|| "flac".into());

    let filename = sanitise_filename(&format!(
        "{}{}{} - {}",
        disc_prefix, track_num, track.artist_name, track.title
    ));

    cache_dir
        .join(artist_dir)
        .join(album_dir)
        .join(format!("{}.{}", filename, ext))
}

/// Build a PlaylistItem from a TrackRow + album date + cache path.
pub(crate) fn playlist_item_from_track(
    track: &queries::TrackRow,
    album_date: Option<&str>,
    dest: PathBuf,
    load_state: LoadState,
) -> PlaylistItem {
    let year = album_date.and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });
    PlaylistItem {
        id: QueueItemId::new(),
        path: dest,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

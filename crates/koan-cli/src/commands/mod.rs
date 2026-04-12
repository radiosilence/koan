use rayon::prelude::*;

mod analyze;
mod auth;
mod cache;
mod config;
mod library;
mod play;
mod probe;
mod remote;
mod scan;
mod search;

pub use analyze::cmd_analyze;
pub use auth::{
    cmd_auth_create_user, cmd_auth_delete_user, cmd_auth_list_users, cmd_auth_login,
    cmd_auth_logout, cmd_auth_setup,
};
pub use cache::{cmd_cache_clear, cmd_cache_status, evict_cache};
pub use config::{cmd_config, cmd_init};
pub use library::cmd_library;
pub use play::{ApiOptions, cmd_play, cmd_play_remote};
pub use probe::{cmd_devices, cmd_probe};
pub use remote::{cmd_remote_login, cmd_remote_status, cmd_remote_sync};
pub use scan::cmd_scan;
pub use search::cmd_search;

use std::path::{Path, PathBuf};

use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::state::{LoadState, PlaylistItem, QueueItemId};
use owo_colors::OwoColorize;

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

/// Prompt for y/N confirmation on stdin.
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

/// Install a panic hook that logs the panic to the log file and restores the terminal.
///
/// Terminal restoration runs on ANY thread panic — raw mode + alternate screen
/// must always be disabled so the user gets a usable terminal back. The
/// restoration sequence is idempotent (safe to call multiple times or from a
/// thread that never entered raw mode).
pub(crate) fn install_terminal_panic_hook() {
    use crossterm::{
        cursor::Show,
        event::{DisableBracketedPaste, DisableMouseCapture},
        execute,
        terminal::{LeaveAlternateScreen, disable_raw_mode},
    };
    use std::io;
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let location = panic
            .location()
            .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let payload = if let Some(s) = panic.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = panic.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown".to_string()
        };
        log::error!("PANIC{}: {}", location, payload);
        let bt = std::backtrace::Backtrace::force_capture();
        log::error!("backtrace:\n{}", bt);
        log::logger().flush();

        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste,
            Show
        );
        original_hook(panic);
    }));
}

/// Parse text from a terminal paste/drop event into file paths.
/// Handles: shell-style escaping (`\ ` for spaces), `file://` URIs,
/// newline/space/tab separation, and quoted paths.
pub(crate) fn parse_dropped_paths(text: &str) -> Vec<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    if text.contains("file://") {
        let mut paths = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(uri) = line.strip_prefix("file://") {
                let raw = uri.strip_prefix("localhost").unwrap_or(uri);
                paths.push(PathBuf::from(percent_decode(raw)));
            } else {
                paths.push(PathBuf::from(line));
            }
        }
        return paths;
    }

    shell_split_paths(text)
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn shell_split_paths(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single => {
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                }
            }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ' ' | '\t' | '\n' | '\r' if !in_single && !in_double => {
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn percent_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let hex = [hi, lo];
            if let Ok(s) = std::str::from_utf8(&hex)
                && let Ok(val) = u8::from_str_radix(s, 16)
            {
                out.push(val as char);
                continue;
            }
            out.push('%');
            out.push(hi as char);
            out.push(lo as char);
        } else {
            out.push(b as char);
        }
    }
    out
}

/// Build PlaylistItems from file paths. Checks the DB first for already-scanned
/// tracks (instant), only falls back to lofty disk reads for unknown files.
pub(crate) fn playlist_items_from_paths(
    paths: &[PathBuf],
    progress: Option<&std::sync::atomic::AtomicUsize>,
) -> Vec<PlaylistItem> {
    const BULK_THRESHOLD: usize = 1000;
    let db_cache: std::collections::HashMap<String, queries::TrackRow> =
        if paths.len() >= BULK_THRESHOLD {
            open_db_optional()
                .and_then(|db| queries::all_tracks_by_path(&db.conn).ok())
                .unwrap_or_default()
        } else {
            let path_strings: Vec<String> = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            open_db_optional()
                .and_then(|db| queries::tracks_by_paths(&db.conn, &path_strings).ok())
                .unwrap_or_default()
        };

    let db_hits = std::sync::atomic::AtomicUsize::new(0);

    let items: Vec<PlaylistItem> = paths
        .par_iter()
        .map(|p| {
            let path_str = p.to_string_lossy();
            let item = if let Some(track) = db_cache.get(path_str.as_ref()) {
                db_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                playlist_item_from_track_row(track, p)
            } else {
                read_metadata_to_item(p)
            };
            if let Some(counter) = progress {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            item
        })
        .collect();

    let hits = db_hits.load(std::sync::atomic::Ordering::Relaxed);
    if hits > 0 {
        log::info!(
            "playlist: {}/{} tracks from DB cache, {} from disk",
            hits,
            paths.len(),
            paths.len() - hits
        );
    }

    items
}

fn open_db_optional() -> Option<Database> {
    Database::open_default().ok()
}

fn playlist_item_from_track_row(track: &queries::TrackRow, path: &Path) -> PlaylistItem {
    PlaylistItem {
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path: path.to_path_buf(),
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year: None,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state: LoadState::Ready,
    }
}

fn read_metadata_to_item(p: &Path) -> PlaylistItem {
    match koan_core::index::metadata::read_metadata(p) {
        Ok(meta) => PlaylistItem {
            id: QueueItemId::new(),
            db_id: None,
            path: p.to_path_buf(),
            title: meta.title,
            artist: meta.artist,
            album_artist: meta.album_artist.unwrap_or_default(),
            album: meta.album,
            year: meta.date.and_then(|d| {
                if d.len() >= 4 {
                    Some(d[..4].to_string())
                } else {
                    None
                }
            }),
            codec: meta.codec,
            track_number: meta.track_number.map(|n| n as i64),
            disc: meta.disc.map(|n| n as i64),
            duration_ms: meta.duration_ms.map(|d| d as u64),
            load_state: LoadState::Ready,
        },
        Err(_) => {
            let title = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            PlaylistItem {
                id: QueueItemId::new(),
                db_id: None,
                path: p.to_path_buf(),
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_split_backslash_spaces() {
        let input = "/path/to/My\\ Album /path/to/Other\\ File.flac";
        let result = shell_split_paths(input);
        assert_eq!(
            result,
            vec!["/path/to/My Album", "/path/to/Other File.flac"]
        );
    }

    #[test]
    fn test_shell_split_simple_paths() {
        let input = "/path/one.flac /path/two.flac";
        let result = shell_split_paths(input);
        assert_eq!(result, vec!["/path/one.flac", "/path/two.flac"]);
    }

    #[test]
    fn test_shell_split_newline_separated() {
        let input = "/path/one.flac\n/path/two.flac\n";
        let result = shell_split_paths(input);
        assert_eq!(result, vec!["/path/one.flac", "/path/two.flac"]);
    }

    #[test]
    fn test_shell_split_quoted_paths() {
        let input = "'/path/to/My Album' \"/path/to/Other Dir\"";
        let result = shell_split_paths(input);
        assert_eq!(result, vec!["/path/to/My Album", "/path/to/Other Dir"]);
    }

    #[test]
    fn test_parse_dropped_file_uris() {
        let input = "file:///Users/test/My%20Music/track.flac\nfile:///Users/test/other.flac";
        let paths = parse_dropped_paths(input);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("/Users/test/My Music/track.flac"));
        assert_eq!(paths[1], PathBuf::from("/Users/test/other.flac"));
    }

    #[test]
    fn test_parse_dropped_file_uri_localhost() {
        let input = "file://localhost/Users/test/track.flac";
        let paths = parse_dropped_paths(input);
        assert_eq!(paths, vec![PathBuf::from("/Users/test/track.flac")]);
    }

    #[test]
    fn test_parse_dropped_empty() {
        assert!(parse_dropped_paths("").is_empty());
        assert!(parse_dropped_paths("  \n  ").is_empty());
    }

    #[test]
    fn test_parse_dropped_finder_style() {
        let input = "/Users/test/Music/My\\ Album /Users/test/Music/Track\\ 01.flac";
        let paths = parse_dropped_paths(input);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("/Users/test/Music/My Album"));
        assert_eq!(paths[1], PathBuf::from("/Users/test/Music/Track 01.flac"));
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("/path/My%20Music"), "/path/My Music");
        assert_eq!(percent_decode("/no%2Fslash"), "/no/slash");
        assert_eq!(percent_decode("/plain/path"), "/plain/path");
    }
}

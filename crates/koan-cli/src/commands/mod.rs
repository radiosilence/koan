mod auth;
mod cache;
mod config;
mod library;
#[cfg(feature = "tui")]
mod play;
#[cfg(feature = "tui")]
mod player;
mod probe;
mod remote;
mod scan;
mod search;
mod subsonic;

pub use auth::{
    cmd_auth_api_key_create, cmd_auth_api_key_list, cmd_auth_api_key_revoke, cmd_auth_create_user,
    cmd_auth_delete_user, cmd_auth_invite, cmd_auth_list_users, cmd_auth_login, cmd_auth_logout,
    cmd_auth_regenerate_keys, cmd_auth_reset, cmd_auth_reset_password, cmd_auth_set_role,
    cmd_auth_setup,
};
pub use cache::{cmd_cache_clear, cmd_cache_status, evict_cache};
pub use config::{cmd_config, cmd_init};
pub use library::cmd_library;
#[cfg(feature = "tui")]
pub use play::{ApiOptions, cmd_play, cmd_play_remote};
pub use probe::{cmd_devices, cmd_probe};
pub use remote::{cmd_remote_login, cmd_remote_status, cmd_remote_sync};
pub use scan::cmd_scan;
pub use search::cmd_search;
pub use subsonic::{cmd_subsonic_disable, cmd_subsonic_setup, cmd_subsonic_status};

use koan_core::db::connection::Database;
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

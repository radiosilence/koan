//! The long jobs a page can start: reading the library's folders, forgetting
//! one taken out of them or the server signed out of, pulling the server's
//! library, clearing downloads and the index. Each runs to its end on the
//! thread it is given and comes back as a line to show.

use koan_core::config::Config;
use koan_core::db::pool;
use koan_core::helpers;
use koan_core::index::scanner::{self, ScanOptions};

use std::path::Path;

/// What a thread the TUI started sends back.
pub enum Report {
    /// A scan, a sync or a folder being forgotten has ended; another may start.
    Task(String),
    /// Other work has ended.
    Work(String),
}

/// Whether a track has to come from the server to play here.
pub fn needs_fetch(t: &koan_core::db::queries::TrackRow) -> bool {
    t.remote_id.is_some() && t.path.is_none() && t.cached_path.is_none()
}

/// Reads the library's folders; `force` re-reads every file's tags.
pub fn scan(force: bool) -> String {
    let cfg = Config::load().unwrap_or_default();
    if cfg.library.folders.is_empty() {
        return "no library folders to scan".into();
    }
    let Ok(db) = pool::shared().get() else {
        return "the library could not be opened".into();
    };
    // `force_remove` stays off, as in the apps: a failed mount must not look
    // like a library that was deleted.
    let opts = ScanOptions {
        force,
        force_remove: false,
        cancel: None,
    };
    let r = scanner::scan_folders(&db, &cfg.library.folders, opts, None, None);
    let mut line = format!(
        "scanned: {} added, {} updated, {} removed",
        r.added, r.updated, r.removed
    );
    if !r.errors.is_empty() {
        line.push_str(&format!(", {} errors", r.errors.len()));
    }
    line
}

pub fn sync() -> String {
    let cfg = Config::load().unwrap_or_default();
    let Some(client) = helpers::subsonic_client(&cfg) else {
        return "no server to sync with".into();
    };
    let Ok(db) = pool::shared().get() else {
        return "the library could not be opened".into();
    };
    match helpers::sync_remote(
        &db,
        &client,
        helpers::Walk::Always,
        &cfg.remote.url,
        &cfg.remote.username,
        &|_| {},
    ) {
        Ok(s) => format!(
            "synced: {} tracks, {} albums, {} artists",
            s.library.tracks_synced, s.library.albums_synced, s.library.artists_synced
        ),
        Err(e) => format!("sync failed: {e}"),
    }
}

pub fn forget(folder: &Path) -> String {
    let Ok(db) = pool::shared().get() else {
        return "the library could not be opened".into();
    };
    match helpers::forget_folder(&db, folder) {
        Ok(n) => format!("removed {}: {n} tracks forgotten", folder.display()),
        Err(e) => format!(
            "removed {} from the folders, but its tracks could not be forgotten: {e}",
            folder.display()
        ),
    }
}

pub fn forget_remote() -> String {
    let Ok(db) = pool::shared().get() else {
        return "the library could not be opened".into();
    };
    match helpers::forget_remote(&db) {
        Ok(n) => format!("signed out and forgot {n} tracks"),
        Err(e) => format!("signed out, but the server's tracks could not be forgotten: {e}"),
    }
}

pub fn rebuild_index() -> String {
    let Ok(db) = pool::shared().get() else {
        return "the library could not be opened".into();
    };
    match helpers::rebuild_index(&db) {
        Ok(s) => format!("removed {} tracks; scan or sync to rebuild", s.tracks),
        Err(e) => format!("the index could not be cleared: {e}"),
    }
}

/// Deletes every downloaded track; what the queue was waiting on is fetched
/// again.
pub fn clear_cache(state: &koan_core::player::state::SharedPlayerState) -> String {
    let Ok(db) = pool::shared().get() else {
        return "the library could not be opened".into();
    };
    let cleared = helpers::clear_download_cache(&db, &Config::load().unwrap_or_default());
    helpers::requeue_cleared_downloads(state);
    format!(
        "freed {} MB across {} files",
        cleared.bytes / 1_000_000,
        cleared.files
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::db::queries::TrackRow;

    fn track(path: Option<&str>, remote: Option<&str>, cached: Option<&str>) -> TrackRow {
        TrackRow {
            id: 1,
            album_id: None,
            artist_id: None,
            artist_name: String::new(),
            album_artist_name: String::new(),
            album_title: String::new(),
            disc: None,
            track_number: None,
            title: String::new(),
            duration_ms: None,
            path: path.map(Into::into),
            codec: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "remote".into(),
            remote_id: remote.map(Into::into),
            cached_path: cached.map(Into::into),
        }
    }

    #[test]
    fn only_server_tracks_not_yet_here_are_fetched() {
        assert!(needs_fetch(&track(None, Some("r"), None)));
        assert!(!needs_fetch(&track(None, Some("r"), Some("/cache/r.flac"))));
        assert!(!needs_fetch(&track(Some("/music/a.flac"), Some("r"), None)));
        assert!(!needs_fetch(&track(Some("/music/a.flac"), None, None)));
    }
}

//! The long jobs a page can start: reading the library's folders, and pulling
//! the server's library. Each runs to its end on the thread it is given and
//! comes back as a line to show.

use koan_core::config::Config;
use koan_core::db::pool;
use koan_core::helpers;
use koan_core::index::scanner::{self, ScanOptions};

use crate::settings_page::Task;

pub fn run(task: Task) -> String {
    match task {
        Task::Scan => scan(),
        Task::Sync => sync(),
        Task::AddFolder => String::new(),
    }
}

fn scan() -> String {
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
        force: false,
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

fn sync() -> String {
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

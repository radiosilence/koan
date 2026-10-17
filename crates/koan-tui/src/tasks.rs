//! The long jobs a page can start: reading the library's folders, forgetting
//! one taken out of them or the server signed out of, pulling the server's
//! library, clearing downloads and the index. Each runs to its end on the
//! thread it is given and comes back as a line to show.

use koan_core::config::Config;
use koan_core::db::pool;
use koan_core::helpers;
use koan_core::index::scanner::{self, ScanOptions};

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A job running, as the sidebar shows it: what it is doing and how far it
/// has got. Written by the job's thread, read by the frame.
pub struct Activity {
    label: parking_lot::Mutex<String>,
    pub done: AtomicU64,
    /// Zero until the job knows how much there is.
    pub total: AtomicU64,
    /// Set to stop it, for a job that can stop: what it did is kept.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Activity {
    pub fn new(label: impl Into<String>, cancellable: bool) -> Arc<Self> {
        Arc::new(Self {
            label: parking_lot::Mutex::new(label.into()),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            cancel: cancellable.then(|| Arc::new(AtomicBool::new(false))),
        })
    }

    pub fn label(&self) -> String {
        self.label.lock().clone()
    }

    fn relabel(&self, label: &str) {
        let mut l = self.label.lock();
        if *l != label {
            *l = label.into();
        }
    }

    /// "1,204 / 4,000", or the count alone before the total is known.
    pub fn counts(&self) -> Option<String> {
        let done = self.done.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        match (done, total) {
            (0, 0) => None,
            (d, 0) => Some(thousands(d)),
            (d, t) => Some(format!("{} / {}", thousands(d), thousands(t))),
        }
    }

    /// How far along, when there is a total.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0))
    }
}

/// `n` with a comma every three digits.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a thread the TUI started sends back.
pub enum Report {
    /// A scan, a sync or a folder being forgotten has ended; another may start.
    Task(String),
    /// Other work that changed the library has ended.
    Work(String),
    /// A line to show, the library untouched.
    Say(String),
}

static LIBRARY_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Rows arrived, or files went, by some route the TUI did not start: a
/// command from another device. The pages read the library again on the next
/// frame.
pub fn library_changed() {
    LIBRARY_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether `library_changed` was told since this was last asked.
pub fn take_library_changed() -> bool {
    LIBRARY_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Whether a track has to come from the server to play here.
pub fn needs_fetch(t: &koan_core::db::queries::TrackRow) -> bool {
    t.remote_id.is_some() && t.path.is_none() && t.cached_path.is_none()
}

/// Reads the library's folders; `force` re-reads every file's tags.
pub fn scan(force: bool, activity: &Activity) -> String {
    let cfg = Config::cached();
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
        cancel: activity.cancel.clone(),
    };
    let r = scanner::scan_folders(
        &db,
        &cfg.library.folders,
        opts,
        Some(&|total| activity.total.store(total, Ordering::Relaxed)),
        Some(&|_| {
            activity.done.fetch_add(1, Ordering::Relaxed);
        }),
    );
    let mut line = format!(
        "scanned: {} added, {} updated, {} removed",
        r.added, r.updated, r.removed
    );
    if !r.errors.is_empty() {
        line.push_str(&format!(", {} errors", r.errors.len()));
    }
    line
}

pub fn sync(activity: &Activity) -> String {
    let cfg = Config::cached();
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
        &|p| {
            use koan_core::remote::sync::SyncPhase;
            activity.relabel(match p.phase {
                SyncPhase::Albums => "syncing albums",
                SyncPhase::Tracks => "syncing tracks",
                SyncPhase::Artists => "syncing artists",
                SyncPhase::Finishing => "finishing the sync",
            });
            activity.done.store(p.done, Ordering::Relaxed);
            activity
                .total
                .store(p.total.unwrap_or(0), Ordering::Relaxed);
        },
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
    fn counts_read_as_numbers_do() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1204), "1,204");
        assert_eq!(thousands(1_000_000), "1,000,000");
        let a = Activity::new("scanning", true);
        assert_eq!(a.counts(), None);
        a.done.store(1204, Ordering::Relaxed);
        assert_eq!(a.counts().as_deref(), Some("1,204"));
        a.total.store(4000, Ordering::Relaxed);
        assert_eq!(a.counts().as_deref(), Some("1,204 / 4,000"));
    }

    #[test]
    fn only_server_tracks_not_yet_here_are_fetched() {
        assert!(needs_fetch(&track(None, Some("r"), None)));
        assert!(!needs_fetch(&track(None, Some("r"), Some("/cache/r.flac"))));
        assert!(!needs_fetch(&track(Some("/music/a.flac"), Some("r"), None)));
        assert!(!needs_fetch(&track(Some("/music/a.flac"), None, None)));
    }
}

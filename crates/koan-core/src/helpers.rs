//! Shared helpers used by downstream crates (koan-tui, koan-server, koan-cli).
//!
//! These functions provide common functionality for building playlist items,
//! resolving track paths, downloading remote tracks, and building Subsonic clients.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::commands::PlayerCommand;
use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};
use crate::remote::client::{SubsonicAuth, SubsonicClient};

// ---------------------------------------------------------------------------
// Subsonic client builder
// ---------------------------------------------------------------------------

/// Get the remote password. Keychain first, config second.
///
/// The config copy is only a migration path now: `set_remote_credentials` writes
/// to the keychain and clears it, so a plaintext password survives exactly until
/// the next sign-in.
pub fn get_remote_password(cfg: &Config) -> Option<String> {
    if let Ok(pw) = crate::credentials::get_password(&cfg.remote.url)
        && !pw.is_empty()
    {
        return Some(pw);
    }
    if !cfg.remote.password.is_empty() {
        return Some(cfg.remote.password.clone());
    }
    None
}

/// Index files that appear in the library folders while koan is running.
///
/// One incremental scan shortly after startup — the walk is a fraction of a
/// second even across fifty thousand files, and everything unchanged is skipped
/// on its mtime and size — then a rescan whenever the folders change.
///
/// Changes are debounced: copying an album in produces a burst of events, and
/// scanning once per file would be both slow and pointless. The scan is
/// incremental in every case, so the cost is proportional to what actually
/// changed rather than to the size of the library.
///
/// `on_state` reports whether a scan is running, so a UI can show it.
pub fn spawn_library_watch(
    db_path: std::path::PathBuf,
    on_state: impl Fn(bool) + Send + Sync + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    use notify::{RecursiveMode, Watcher};

    std::thread::Builder::new()
        .name("koan-library-watch".into())
        .spawn(move || {
            let scan_now = |reason: &str| {
                let cfg = Config::load().unwrap_or_default();
                if cfg.library.folders.is_empty() {
                    return;
                }
                let Ok(db) = Database::open(&db_path) else {
                    return;
                };
                on_state(true);
                let result = crate::index::scanner::full_scan(
                    &db,
                    &cfg.library.folders,
                    crate::index::scanner::ScanOptions::default(),
                    None,
                );
                on_state(false);
                log::info!(
                    "{reason} scan: {} added, {} updated, {} removed, {} unchanged",
                    result.added,
                    result.updated,
                    result.removed,
                    result.skipped
                );
            };

            // After the first frame and the first track, not competing with them.
            std::thread::sleep(std::time::Duration::from_secs(3));
            scan_now("startup");

            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut watcher) = notify::recommended_watcher(move |event| {
                let _ = tx.send(event);
            }) else {
                log::warn!("could not watch the library folders");
                return;
            };

            let cfg = Config::load().unwrap_or_default();
            for folder in &cfg.library.folders {
                if let Err(e) = watcher.watch(folder, RecursiveMode::Recursive) {
                    log::warn!("could not watch {}: {e}", folder.display());
                }
            }

            // Copying an album in is a burst of events. Wait for it to stop
            // before scanning, rather than scanning per file.
            const SETTLE: std::time::Duration = std::time::Duration::from_secs(5);
            while let Ok(first) = rx.recv() {
                if first.is_err() {
                    continue;
                }
                while rx.recv_timeout(SETTLE).is_ok() {}
                scan_now("watched change");
            }
        })
        .ok()
}

/// Keep the library in step with the server, without being asked.
///
/// One sync shortly after startup, then every `auto_sync_interval_mins`. Always
/// incremental: it asks the server what changed rather than walking the whole
/// library, which is what makes it cheap enough to run unattended. A full sync
/// stays a deliberate action.
///
/// The startup run is delayed a few seconds so it is not competing with the
/// first frame and the first track for the disk.
///
/// `on_state` reports whether a sync is running, so a UI can say so rather than
/// appearing to do nothing.
pub fn spawn_auto_sync(
    db_path: std::path::PathBuf,
    on_state: impl Fn(bool) + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("koan-auto-sync".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            loop {
                let cfg = Config::load().unwrap_or_default();
                if !cfg.remote.enabled || !cfg.remote.auto_sync {
                    // Re-read rather than exit: the setting can be turned on
                    // while the app is running.
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    continue;
                }

                if let Some(client) = subsonic_client(&cfg)
                    && let Ok(db) = Database::open(&db_path)
                {
                    on_state(true);
                    match crate::remote::sync::sync_library(
                        &db,
                        &client,
                        false,
                        &cfg.remote.url,
                        &cfg.remote.username,
                    ) {
                        Ok(r) => log::info!(
                            "auto sync: {} artists, {} albums, {} tracks ({} albums failed)",
                            r.artists_synced,
                            r.albums_synced,
                            r.tracks_synced,
                            r.albums_failed
                        ),
                        Err(e) => log::warn!("auto sync failed: {e}"),
                    }
                    on_state(false);
                }

                match cfg.remote.auto_sync_interval_mins {
                    // Once at startup and no more.
                    0 => return,
                    mins => std::thread::sleep(std::time::Duration::from_secs(mins * 60)),
                }
            }
        })
        .ok()
}

/// What a library rebuild removed.
#[derive(Debug, Clone, Copy, Default)]
pub struct RebuildSummary {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

/// Drop the index so the next scan rebuilds it from the files.
///
/// Favourites are keyed on the file path rather than a row id, so they survive
/// this and re-attach when the paths come back. Everything keyed on a track id
/// cannot: lyrics, play history and acoustic embeddings go, and the foreign keys
/// would refuse the delete otherwise. Lyrics and embeddings are re-derivable;
/// play counts are not, which is worth saying out loud wherever this is offered.
///
/// The remote half of the library comes back on the next sync, the local half on
/// the next scan.
pub fn rebuild_index(db: &Database) -> Result<RebuildSummary, crate::db::connection::DbError> {
    let count = |sql: &str| -> u64 {
        db.conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    };
    let summary = RebuildSummary {
        tracks: count("SELECT COUNT(*) FROM tracks"),
        albums: count("SELECT COUNT(*) FROM albums"),
        artists: count("SELECT COUNT(*) FROM artists"),
    };

    // Children before parents; the FTS index has no foreign keys but is derived
    // from tracks and would otherwise keep answering for rows that are gone.
    db.conn.execute_batch(
        "BEGIN;
         DELETE FROM track_vectors;
         DELETE FROM lyrics_cache;
         DELETE FROM play_history;
         DELETE FROM scan_cache;
         DELETE FROM tracks_fts;
         DELETE FROM tracks;
         DELETE FROM similar_artists;
         DELETE FROM albums;
         DELETE FROM artists;
         COMMIT;",
    )?;
    let _ = db.conn.execute_batch("VACUUM");
    Ok(summary)
}

/// Bytes currently held in the download cache.
pub fn cache_size_bytes(cfg: &Config) -> u64 {
    walkdir::WalkDir::new(cfg.cache_dir())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// How many tracks came from this folder.
///
/// The trailing separator matters: without it `/Volumes/Music` also counts
/// `/Volumes/Music Backup`.
pub fn tracks_under(db: &Database, folder: &Path) -> u64 {
    let prefix = format!(
        "{}{}%",
        folder
            .to_string_lossy()
            .trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE path LIKE ?1",
            [&prefix],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// How many tracks the server accounts for.
pub fn tracks_from_server(db: &Database) -> u64 {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE remote_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// Forget every track under a folder.
///
/// Removing a folder from the library should remove what it put there —
/// otherwise the library keeps showing records whose files it will never look
/// at again, and there is no way back to an empty library short of clearing the
/// whole index.
///
/// A track that also exists on the server keeps its row and loses only its local
/// path: it is still playable, just by download rather than from disk.
///
/// Albums and artists left holding nothing go too, or the browser fills with
/// empty shelves.
pub fn forget_folder(db: &Database, folder: &Path) -> Result<u64, crate::db::connection::DbError> {
    // Trailing separator, or `/Volumes/Music` also matches `/Volumes/Music Backup`.
    let prefix = format!(
        "{}{}%",
        folder
            .to_string_lossy()
            .trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );

    let tx = db.conn.unchecked_transaction()?;
    // Still on the server: keep the row, drop the local file.
    tx.execute(
        "UPDATE tracks SET path = NULL, source = 'remote'
          WHERE path LIKE ?1 AND remote_id IS NOT NULL",
        [&prefix],
    )?;

    let ids: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM tracks WHERE path LIKE ?1")?;
        let rows = stmt.query_map([&prefix], |r| r.get(0))?;
        rows.filter_map(Result::ok).collect()
    };
    for id in &ids {
        tx.execute("DELETE FROM track_vectors WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM lyrics_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM play_history WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM scan_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM tracks_fts WHERE rowid = ?1", [id])?;
        tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    }
    prune_empty_albums_and_artists(&tx)?;
    tx.commit()?;
    Ok(ids.len() as u64)
}

/// Forget everything that only existed on the server.
///
/// Signing out should leave the library with what is actually on this machine.
/// A track held both locally and remotely keeps its row and loses its remote id;
/// one that only ever came from the server goes.
pub fn forget_remote(db: &Database) -> Result<u64, crate::db::connection::DbError> {
    let tx = db.conn.unchecked_transaction()?;

    let ids: Vec<i64> = {
        let mut stmt =
            tx.prepare("SELECT id FROM tracks WHERE remote_id IS NOT NULL AND path IS NULL")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.filter_map(Result::ok).collect()
    };
    for id in &ids {
        tx.execute("DELETE FROM track_vectors WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM lyrics_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM play_history WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM scan_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM tracks_fts WHERE rowid = ?1", [id])?;
        tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    }
    // Local copies stay, minus the server they were also on.
    tx.execute(
        "UPDATE tracks SET remote_id = NULL, remote_url = NULL, source = 'local'
          WHERE remote_id IS NOT NULL",
        [],
    )?;
    tx.execute("DELETE FROM similar_artists", [])?;
    prune_empty_albums_and_artists(&tx)?;
    tx.commit()?;
    Ok(ids.len() as u64)
}

/// Albums and artists with nothing left in them.
fn prune_empty_albums_and_artists(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), crate::db::connection::DbError> {
    tx.execute(
        "DELETE FROM albums WHERE NOT EXISTS
           (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
        [],
    )?;
    tx.execute(
        "DELETE FROM similar_artists WHERE NOT EXISTS
           (SELECT 1 FROM albums WHERE albums.artist_id = similar_artists.artist_id)",
        [],
    )?;
    tx.execute(
        "DELETE FROM artists WHERE NOT EXISTS
             (SELECT 1 FROM albums WHERE albums.artist_id = artists.id)
           AND NOT EXISTS
             (SELECT 1 FROM tracks WHERE tracks.artist_id = artists.id)",
        [],
    )?;
    Ok(())
}

/// What clearing the download cache removed.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheCleared {
    pub files: u64,
    pub bytes: u64,
}

/// Delete every downloaded remote track and forget where they were.
///
/// The rows stay — a remote track is still in the library, it just has to be
/// fetched again to play.
pub fn clear_download_cache(db: &Database, cfg: &Config) -> CacheCleared {
    let dir = cfg.cache_dir();
    let mut cleared = CacheCleared::default();
    for entry in walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            cleared.bytes += meta.len();
            cleared.files += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let _ = queries::clear_cached_paths(&db.conn);
    cleared
}

/// Push a favourite to the remote server, if this track came from one.
///
/// Fire and forget on its own thread: starring is a courtesy to the server, and
/// a slow or unreachable one should not hold up the click that caused it. The
/// local favourite is already written by the time this runs.
///
/// Silently does nothing for a track with no `remote_id` — including a local
/// file whose copy on the server failed to merge with it (#221), which is the
/// one case where the silence is wrong.
///
/// Shared by the TUI, the server and the app, which each had their own copy.
pub fn sync_favourite_to_remote(db: &Database, path: &Path, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Ok(Some(remote_id)) = queries::remote_id_for_path(&db.conn, path) else {
        return;
    };
    let Some(client) = subsonic_client(&cfg) else {
        return;
    };
    std::thread::Builder::new()
        .name("koan-fav-sync".into())
        .spawn(move || {
            let result = if star {
                client.star(&remote_id)
            } else {
                client.unstar(&remote_id)
            };
            if let Err(e) = result {
                log::warn!("failed to sync favourite to remote: {e}");
            }
        })
        .ok();
}

/// Why signing in to a remote server failed.
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("the server did not accept those credentials: {0}")]
    Rejected(#[from] crate::remote::client::SubsonicError),
    #[error("could not save the password: {0}")]
    Credentials(#[from] crate::credentials::CredentialError),
    #[error("could not write the configuration: {0}")]
    Config(#[from] crate::config::ConfigError),
}

/// Sign in to a Subsonic/Navidrome server and remember it.
///
/// The password goes to the platform credential store — Keychain on macOS,
/// secret-service on Linux — and never to `config.local.toml`, which is a plain
/// file on disk. Any plaintext password already there is cleared, so signing in
/// again migrates an older setup.
///
/// The credentials are checked against the server before anything is written; a
/// stored password that does not work is worse than none.
///
/// Shared by the CLI and the app so the two cannot disagree about where
/// credentials live.
pub fn set_remote_credentials(
    url: &str,
    username: &str,
    password: &str,
) -> Result<(), SignInError> {
    let url = url.trim_end_matches('/');
    SubsonicClient::new(url, username, password).ping()?;
    crate::credentials::store_password(url, password)?;

    let mut values = toml::map::Map::new();
    values.insert("enabled".into(), toml::Value::Boolean(true));
    values.insert("url".into(), toml::Value::String(url.to_string()));
    values.insert("username".into(), toml::Value::String(username.to_string()));
    // Explicitly emptied: a password left here would keep being read by anything
    // still preferring the config copy.
    values.insert("password".into(), toml::Value::String(String::new()));
    Config::patch_local("remote", &values)?;
    Ok(())
}

/// Keychain account holding the Subsonic API shared secret.
pub const SUBSONIC_CREDENTIAL_ACCOUNT: &str = "koan-subsonic";

/// Shared secret for koan's own Subsonic API. Config first, then the keychain.
///
/// Deliberately not `get_remote_password` — see `SubsonicConfig`.
pub fn get_subsonic_password(cfg: &Config) -> Option<String> {
    if !cfg.subsonic.password.is_empty() {
        return Some(cfg.subsonic.password.clone());
    }
    crate::credentials::get_password(SUBSONIC_CREDENTIAL_ACCOUNT)
        .ok()
        .filter(|p| !p.is_empty())
}

/// Upstream Subsonic credentials from the merged config, returning `None` if
/// remote is disabled or has no URL configured.
///
/// Prefer this over `subsonic_client` when only a signed URL is needed:
/// building a client constructs blocking `reqwest` clients, which panics from
/// inside a tokio runtime.
pub fn subsonic_auth(cfg: &Config) -> Option<SubsonicAuth> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    let password = get_remote_password(cfg)?;
    Some(SubsonicAuth::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    ))
}

/// Build a `SubsonicClient` from the merged config. Never call from async code.
pub fn subsonic_client(cfg: &Config) -> Option<SubsonicClient> {
    subsonic_auth(cfg).map(SubsonicClient::from_auth)
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

/// Why a share link could not be made. Each variant is something the user can
/// act on, which is the point — every caller used to collapse these into
/// "local-only tracks can't be shared" and send people looking in the wrong
/// place.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("no remote server is configured")]
    NoRemote,
    #[error("none of these tracks are on the server, so a link has nothing to point at")]
    NothingRemote,
    #[error("the server refused to share these: {0}")]
    Server(#[from] crate::remote::client::SubsonicError),
    #[error(transparent)]
    Database(#[from] crate::db::connection::DbError),
}

/// A created share link, and how much of the request it covers.
#[derive(Debug, Clone)]
pub struct ShareOutcome {
    pub url: String,
    /// The server's own ID for the share, for callers that manage them.
    pub id: String,
    /// Tracks the server knows about, which went into the link.
    pub shared: usize,
    /// Tracks with no copy on the server, left out of it.
    pub skipped: usize,
}

/// Create a public share link on the remote server for these tracks.
///
/// A link points at the server, so only tracks the server knows about can go in
/// it. A mixed selection shares the part that can be shared and reports the
/// rest rather than failing whole — half a link beats none, as long as the
/// caller says which half.
///
/// Network-bound. Callers keep it off whatever thread draws.
pub fn create_share(
    db: &Database,
    cfg: &Config,
    track_ids: &[i64],
    description: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    let client = subsonic_client(cfg).ok_or(ShareError::NoRemote)?;

    // One query, not one per track: sharing an artist is thousands of tracks.
    let rows = queries::tracks_by_ids(&db.conn, track_ids)?;

    let shared = rows.iter().filter(|t| t.remote_id.is_some()).count();
    if shared == 0 {
        return Err(ShareError::NothingRemote);
    }

    // A whole record shares as one album rather than as N tracks — the server
    // renders it as the album it is, and the link survives the user adding to
    // it. Only when the selection is genuinely the whole thing.
    let one_album = rows
        .first()
        .and_then(|f| f.album_id)
        .filter(|first| rows.iter().all(|t| t.album_id == Some(*first)))
        .and_then(|album_id| album_remote_id(&db.conn, album_id, rows.len()));

    let remote_ids: Vec<String> = match one_album {
        Some(rid) => vec![rid],
        None => rows.into_iter().filter_map(|t| t.remote_id).collect(),
    };

    let refs: Vec<&str> = remote_ids.iter().map(String::as_str).collect();
    let share = client.create_share(&refs, description)?;

    // Navidrome does not always hand back a URL, and a share with no link is
    // useless to the caller — the ID is enough to build it.
    let url = share
        .url
        .clone()
        .unwrap_or_else(|| format!("{}/s/{}", client.base_url(), share.id));

    Ok(ShareOutcome {
        url,
        id: share.id,
        shared,
        skipped: track_ids.len().saturating_sub(shared),
    })
}

/// The album's own remote ID, but only when `selected` covers every track on
/// it. Sharing an album link for half an album would hand out more than the
/// user picked.
fn album_remote_id(conn: &rusqlite::Connection, album_id: i64, selected: usize) -> Option<String> {
    let (remote_id, total): (Option<String>, i64) = conn
        .query_row(
            "SELECT al.remote_id, (SELECT COUNT(*) FROM tracks WHERE album_id = al.id)
             FROM albums al WHERE al.id = ?1",
            [album_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    (total == selected as i64).then_some(remote_id).flatten()
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max` bytes, cutting on a char boundary.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
pub fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    truncate_bytes(&cleaned, 240).trim_end().to_string()
}

/// Build a structured cache path for a track:
///   cache_dir/Album Artist/(Year) Album [Codec]/01. Track Artist - Title.ext
pub fn cache_path_for_track(
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

// ---------------------------------------------------------------------------
// Track resolution
// ---------------------------------------------------------------------------

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, LoadState::Ready) for local/cached, (cache_path, LoadState::Pending) for remote.
pub fn resolve_item_path(
    db: &Database,
    cfg: &Config,
    id: i64,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> (PathBuf, LoadState) {
    match queries::resolve_playback_path(&db.conn, id) {
        Ok(Some(queries::PlaybackSource::Local(p))) => (p, LoadState::Ready),
        // A cache entry is only as good as its contents. Older builds could
        // store a Subsonic error body here, which reports Ready and then fails
        // to decode forever; treating it as Pending sends it back through the
        // download path, which discards it and re-fetches.
        Ok(Some(queries::PlaybackSource::Cached(p))) => {
            let state = if is_cached_audio(&p) {
                LoadState::Ready
            } else {
                LoadState::Pending
            };
            (p, state)
        }
        Ok(Some(queries::PlaybackSource::Remote(_))) => {
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            if dest.exists() && is_cached_audio(&dest) {
                (dest, LoadState::Ready)
            } else {
                (dest, LoadState::Pending)
            }
        }
        _ => {
            // Fallback: construct a cache path and mark pending.
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            (dest, LoadState::Pending)
        }
    }
}

/// Build a PlaylistItem from a TrackRow + album date + resolved path + load state.
pub fn playlist_item_from_track(
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
        db_id: Some(track.id),
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

/// Build playlist items for many tracks at once.
///
/// `track_to_playlist_item` loads the config on every call, which means
/// reading and parsing `config.toml` and `config.local.toml` once per track —
/// the reason a large add crawled. This loads it once and memoises album dates,
/// so a thousand-track add costs one config read instead of a thousand.
pub fn playlist_items_for_tracks(db: &Database, tracks: &[queries::TrackRow]) -> Vec<PlaylistItem> {
    use std::collections::HashMap;

    let cfg = Config::load().unwrap_or_default();
    let mut album_dates: HashMap<i64, Option<String>> = HashMap::new();

    tracks
        .iter()
        .map(|track| {
            let album_date = match track.album_id {
                Some(aid) => album_dates
                    .entry(aid)
                    .or_insert_with(|| queries::album_date(&db.conn, aid).ok().flatten())
                    .clone(),
                None => None,
            };
            let (path, load_state) =
                resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());
            playlist_item_from_track(track, album_date.as_deref(), path, load_state)
        })
        .collect()
}

/// Build a PlaylistItem from a TrackRow, resolving its path automatically.
pub fn track_to_playlist_item(track: &queries::TrackRow, db: &Database) -> PlaylistItem {
    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cfg = Config::load().unwrap_or_default();
    let (path, load_state) = resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());

    let year = album_date.as_deref().and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });

    PlaylistItem {
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path,
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

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Whether a cached file plausibly holds audio.
///
/// A stored Subsonic error is a few hundred bytes of JSON or XML; no real
/// encoded track comes close to that, so the size check alone settles almost
/// every case and the leading byte covers the rest.
fn is_cached_audio(path: &std::path::Path) -> bool {
    const MIN_PLAUSIBLE_BYTES: u64 = 4096;
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() >= MIN_PLAUSIBLE_BYTES => true,
        Ok(_) => {
            let mut first = [0u8; 1];
            match std::fs::File::open(path)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut first).map(|_| first[0]))
            {
                Ok(b) => b != b'{' && b != b'<',
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// Resolve a track to a playable file, downloading from remote if needed.
///
/// Resolution order:
/// 1. Local library path (DB `path` field) -- use directly if file exists
/// 2. Cache path -- use if already downloaded
/// 3. Download from remote to cache -- stream while downloading
pub fn download_track(
    db_id: i64,
    queue_id: QueueItemId,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    log_buf: &Arc<Mutex<Vec<String>>>,
    state: &Arc<SharedPlayerState>,
    cfg: &Config,
    client: &SubsonicClient,
) {
    let db = match Database::open_default() {
        Ok(db) => db,
        Err(e) => {
            state.update_load_state(queue_id, LoadState::Failed(format!("db error: {}", e)));
            return;
        }
    };
    let track = match queries::get_track_row(&db.conn, db_id) {
        Ok(Some(t)) => t,
        _ => {
            state.update_load_state(queue_id, LoadState::Failed("track not found".into()));
            return;
        }
    };

    let remote_id = match &track.remote_id {
        Some(rid) => rid.clone(),
        None => {
            // No remote_id -- check if the local file exists.
            if let Some(ref path) = track.path {
                let p = std::path::PathBuf::from(path);
                if p.exists() {
                    state.update_paths(&[(queue_id, p)]);
                    state.update_load_state(queue_id, LoadState::Ready);
                    if state.is_cursor(queue_id) {
                        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
                    }
                    return;
                }
            }
            state.update_load_state(queue_id, LoadState::Failed("no remote_id".into()));
            return;
        }
    };

    // 1. Check if the local library file exists.
    if let Some(ref local_path) = track.path {
        let p = std::path::PathBuf::from(local_path);
        if p.exists() {
            log::info!("download_track: local file exists, using {}", p.display());
            state.update_paths(&[(queue_id, p)]);
            state.update_load_state(queue_id, LoadState::Ready);
            if state.is_cursor(queue_id) {
                tx.send(PlayerCommand::TrackReady(queue_id)).ok();
            }
            return;
        }
    }

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());

    // 2. Already cached.
    //
    // Older builds could write a Subsonic error body here as if it were audio,
    // leaving a tiny JSON file that reports Ready and then fails to decode
    // forever. Treat those as absent so they get re-fetched.
    if dest.exists() && !is_cached_audio(&dest) {
        log::warn!(
            "discarding non-audio cache entry {} (likely a stored server error)",
            dest.display()
        );
        let _ = std::fs::remove_file(&dest);
    }
    if dest.exists() {
        state.update_paths(&[(queue_id, dest)]);
        state.update_load_state(queue_id, LoadState::Ready);
        if state.is_cursor(queue_id) {
            tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        }
        return;
    }

    // 3. Download from remote. The queue item points at the in-progress file so
    // the streaming pump reads bytes as they land; it flips to `dest` on success.
    state.update_paths(&[(queue_id, crate::remote::download::part_path(&dest))]);

    let bytes_written: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let progress_state = state.clone();
    let progress_qid = queue_id;
    let bytes_written_progress = bytes_written.clone();
    let progress_tx = tx.clone();
    let stream_ready_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stream_ready_flag = stream_ready_sent.clone();
    let result = client.download_with_progress(&remote_id, &dest, move |downloaded, total| {
        bytes_written_progress.store(downloaded, Ordering::Release);
        progress_state.update_load_state(
            progress_qid,
            LoadState::Downloading {
                downloaded,
                total,
                bytes_written: bytes_written_progress.clone(),
            },
        );
        if !stream_ready_flag.load(Ordering::Relaxed)
            && downloaded >= crate::player::state::STREAM_THRESHOLD
        {
            stream_ready_flag.store(true, Ordering::Relaxed);
            progress_tx
                .send(PlayerCommand::TrackStreamReady(progress_qid))
                .ok();
        }
    });

    if let Err(e) = result {
        state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
        push_log(log_buf, format!("x {} — {}", track.title, e));
        return;
    }

    // Download succeeded.
    state.update_paths(&[(queue_id, dest.clone())]);
    state.update_load_state(queue_id, LoadState::Ready);
    // Without this row the file is invisible to cache eviction and never reclaimed.
    if let Err(e) = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy()) {
        log::warn!(
            "cached {} but failed to record it ({}) — it will not be evicted",
            dest.display(),
            e
        );
    }

    push_log(
        log_buf,
        format!("+ {} — {}", track.title, track.artist_name),
    );

    if state.is_cursor(queue_id) {
        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
    }
}

/// Append to the TUI log pane, tolerating a poisoned lock — a download worker
/// must not die because some other thread panicked while holding it.
fn push_log(log_buf: &Arc<Mutex<Vec<String>>>, msg: String) {
    match log_buf.lock() {
        Ok(mut buf) => buf.push(msg),
        Err(_) => log::info!("{}", msg),
    }
}

/// Spawn background downloads for remote tracks with LoadState::Pending.
pub fn spawn_downloads(
    pending: Vec<(i64, QueueItemId)>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    let log_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    if let Err(e) = std::thread::Builder::new()
        .name("koan-download".into())
        .spawn(move || {
            let cfg = Config::load().unwrap_or_default();
            let Some(client) = subsonic_client(&cfg) else {
                log::warn!(
                    "remote not configured -- skipping {} downloads",
                    pending.len()
                );
                return;
            };
            for (db_id, queue_id) in pending {
                download_track(db_id, queue_id, &tx, &log_buf, &state, &cfg, &client);
            }
        })
    {
        log::error!("failed to spawn download thread: {}", e);
    }
}

#[cfg(test)]
mod rebuild_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn rebuild_drops_the_index_and_keeps_favourites() {
        let db = test_db();
        let mut meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        meta.path = Some("/music/windowlicker.flac".into());
        let track_id = queries::upsert_track(&db.conn, &meta).unwrap();

        // Favourites key on the path; lyrics key on the row id.
        queries::toggle_favourite(&db.conn, Path::new("/music/windowlicker.flac")).unwrap();
        db.conn
            .execute(
                "INSERT INTO lyrics_cache (track_id, source, content, fetched_at)
                 VALUES (?1, 'test', 'la la la', 0)",
                [track_id],
            )
            .unwrap();

        let summary = rebuild_index(&db).unwrap();
        assert_eq!(summary.tracks, 1);
        assert_eq!(summary.albums, 1);

        let tracks: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tracks, 0, "the index is gone");

        let favourites: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM favourites", [], |r| r.get(0))
            .unwrap();
        assert_eq!(favourites, 1, "favourites survive — they key on the path");

        let lyrics: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM lyrics_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(lyrics, 0, "anything keyed on a track id cannot survive");
    }

    #[test]
    fn rebuilding_an_empty_library_is_not_an_error() {
        let db = test_db();
        let summary = rebuild_index(&db).unwrap();
        assert_eq!(summary.tracks, 0);
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    /// Three tracks on one album; the album carries a remote ID.
    fn album_of_three(db: &Database) -> (i64, Vec<i64>) {
        let ids: Vec<i64> = ["One", "Two", "Three"]
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut meta = sample_meta(title, "Boards of Canada", "Geogaddi");
                meta.path = Some(format!("/music/geogaddi/{i}.flac"));
                meta.track_number = Some(i as i32 + 1);
                queries::upsert_track(&db.conn, &meta).unwrap()
            })
            .collect();
        let album_id: i64 = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [ids[0]], |r| {
                r.get(0)
            })
            .unwrap();
        db.conn
            .execute(
                "UPDATE albums SET remote_id = 'al-1' WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        (album_id, ids)
    }

    #[test]
    fn whole_album_collapses_to_the_album_link() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        assert_eq!(
            album_remote_id(&db.conn, album_id, ids.len()),
            Some("al-1".into())
        );
    }

    #[test]
    fn part_of_an_album_does_not() {
        let db = test_db();
        let (album_id, _) = album_of_three(&db);
        // Sharing an album link for two of three tracks would hand out a track
        // the user did not pick.
        assert_eq!(album_remote_id(&db.conn, album_id, 2), None);
    }

    #[test]
    fn a_local_only_album_has_no_link_to_collapse_to() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        db.conn
            .execute(
                "UPDATE albums SET remote_id = NULL WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        assert_eq!(album_remote_id(&db.conn, album_id, ids.len()), None);
    }
}

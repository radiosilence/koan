//! The download cache: its size, eviction, clearing, and where a track's download lives.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::state::SharedPlayerState;

use super::*;

/// Remove downloads until the cache is under the configured limit, a file at
/// a time: those fetched to play first, then those asked for, least recently
/// used first within each. Never a file whose album has a favourite in it,
/// nor a track in `keep` — the playback window, whose files the player may be
/// reading and which are what it wants next (`playback_window`). Returns the
/// bytes freed.
pub fn evict_cache(
    db: &Database,
    cfg: &Config,
    keep: &std::collections::HashSet<i64>,
    verbose: bool,
) -> u64 {
    let Some(limit) = cfg.cache_limit_bytes().map(|l| l as i64) else {
        return 0;
    };
    let mut current = match queries::total_cache_size(&db.conn) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("cache eviction: failed to query cache size: {e}");
            return 0;
        }
    };
    if current <= limit {
        if verbose {
            log::info!("cache within limit: {current} / {limit} bytes");
        }
        return 0;
    }
    let files = match queries::cached_files_lru(&db.conn) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("cache eviction: failed to query cached files: {e}");
            return 0;
        }
    };
    let mut gone = Vec::new();
    let mut freed: i64 = 0;
    for file in files.iter().filter(|f| !keep.contains(&f.track_id)) {
        if current <= limit {
            break;
        }
        match std::fs::remove_file(&file.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!("cache eviction: failed to delete {}: {e}", file.path);
                continue;
            }
        }
        log::info!(
            "evicted: {} ({} bytes{})",
            file.path,
            file.size,
            if file.pinned { ", pinned" } else { "" }
        );
        gone.push(file.track_id);
        current -= file.size;
        freed += file.size;
    }
    if let Err(e) = queries::clear_cached_paths_for(&db.conn, &gone) {
        log::warn!("cache eviction: failed to clear DB: {e}");
    }
    remove_empty_dirs(&cfg.cache_dir());
    if freed > 0 {
        cache_shrank(freed as u64);
        log::info!("cache eviction freed {freed} bytes");
    }
    freed as u64
}

/// How many of `upcoming` — the playing track, then the rest of the queue in
/// the order the player reaches it — the cache has room for. The playing
/// track and the next always: the decoder reads ahead into the next for
/// gapless. After them each track while the cache stays under `limit`,
/// counting favourites and pinned downloads first, since the queue does not
/// displace them. What is already downloaded costs its file, and what is not
/// its estimated size.
pub fn playback_window(
    db: &Database,
    limit: u64,
    upcoming: &[i64],
) -> Result<usize, crate::db::connection::DbError> {
    let total = queries::total_cache_size(&db.conn)?;
    let evictable: std::collections::HashMap<i64, i64> = queries::cached_files_lru(&db.conn)?
        .into_iter()
        .filter(|f| !f.pinned)
        .map(|f| (f.track_id, f.size))
        .collect();
    let estimates = queries::download_estimates(&db.conn, upcoming)?;

    let mut used = (total - evictable.values().sum::<i64>()).max(0) as u64;
    let mut counted = std::collections::HashSet::new();
    for (n, id) in upcoming.iter().enumerate() {
        if counted.insert(*id) {
            let cost = evictable.get(id).or_else(|| estimates.get(id));
            used += cost.copied().unwrap_or(0).max(0) as u64;
        }
        if n >= 2 && used > limit {
            return Ok(n);
        }
    }
    Ok(upcoming.len())
}

/// Remove empty directories under `dir`, leaving `dir` itself.
fn remove_empty_dirs(dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    for entry in walkdir::WalkDir::new(dir)
        .contents_first(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir() && e.path() != dir)
    {
        let _ = std::fs::remove_dir(entry.path());
    }
}

/// Bytes in the download cache, as front ends show it: measured whole by
/// `measure_cache` at startup and on a clear, and kept between them by what
/// lands and leaves, so a reading never walks the disk.
static CACHE_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn cache_bytes() -> u64 {
    CACHE_BYTES.load(Ordering::Relaxed)
}

/// Walk the cache and take what it holds as the count.
pub fn measure_cache(cfg: &Config) -> u64 {
    let bytes = cache_size_bytes(cfg);
    CACHE_BYTES.store(bytes, Ordering::Relaxed);
    crate::signal::engine_changed().bump();
    bytes
}

pub(super) fn cache_grew(bytes: u64) {
    CACHE_BYTES.fetch_add(bytes, Ordering::Relaxed);
    crate::signal::engine_changed().bump();
}

pub(crate) fn cache_shrank(bytes: u64) {
    let mut now = CACHE_BYTES.load(Ordering::Relaxed);
    while let Err(moved) = CACHE_BYTES.compare_exchange_weak(
        now,
        now.saturating_sub(bytes),
        Ordering::Relaxed,
        Ordering::Relaxed,
    ) {
        now = moved;
    }
    crate::signal::engine_changed().bump();
}

/// Bytes on disk in the download cache, walked. A `.part` is left out: it is
/// counted once it lands, and a walk racing the download would count it
/// twice.
pub fn cache_size_bytes(cfg: &Config) -> u64 {
    walkdir::WalkDir::new(cfg.cache_dir())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_none_or(|ext| ext != "part"))
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
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
    measure_cache(cfg);
    cleared
}

/// Delete the downloaded copies of just these tracks.
///
/// The per-track counterpart of `clear_download_cache`, for throwing away one
/// record rather than the lot. A track playing from a copy being removed keeps
/// playing — the decoder holds the file open, and unlinking it only takes the
/// name away — but the next play fetches it again.
pub fn clear_downloads_for(db: &Database, track_ids: &[i64]) -> CacheCleared {
    let mut cleared = CacheCleared::default();
    let paths = match queries::cached_paths_for(&db.conn, track_ids) {
        Ok(paths) => paths,
        Err(e) => {
            log::warn!("could not read cached paths: {e}");
            return cleared;
        }
    };
    for path in &paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(path) {
            Ok(()) => {
                cleared.files += 1;
                cleared.bytes += size;
            }
            // Already gone is the outcome asked for, so it is not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("could not remove {path}: {e}"),
        }
    }
    if let Err(e) = queries::clear_cached_paths_for(&db.conn, track_ids) {
        log::warn!("removed downloads but failed to forget them ({e})");
    }
    cache_shrank(cleared.bytes);
    cleared
}

/// Throw away half-finished downloads left behind by a previous run.
///
/// A `.part` file only means something to the transfer writing it. koan does
/// not resume — the file is written straight through and renamed at the end —
/// so one still on disk at startup is from a run that did not finish, and it
/// will be truncated and rewritten the next time that track is wanted anyway.
/// Until then it is bytes nothing knows about: cache eviction only tracks what
/// finished, so an interrupted download of a nine-hour recording is half a
/// gigabyte that never gets reclaimed.
///
/// At startup rather than at exit, because a run that ends without getting to
/// its own cleanup is exactly the run that leaves these behind.
pub fn sweep_partial_downloads(cfg: &Config) -> CacheCleared {
    let mut swept = CacheCleared::default();
    for entry in walkdir::WalkDir::new(cfg.cache_dir())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "part"))
    {
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                swept.files += 1;
                swept.bytes += size;
            }
            Err(e) => log::warn!("could not remove {}: {e}", entry.path().display()),
        }
    }
    if swept.files > 0 {
        log::info!(
            "swept {} unfinished download(s), {} bytes",
            swept.files,
            swept.bytes
        );
    }
    swept
}

/// Re-root cached paths that name a cache directory other than `cache_dir`.
///
/// iOS gives an app a new container path when it is updated. The cache moves
/// with it, but the absolute paths stored for its files do not, so every
/// download looks missing: tracks are fetched again beside the copies already
/// there, and eviction cannot find the old ones to delete. The cache lays
/// files out as `<cache>/<artist>/<album>/<file>` (`cache_path_for_track`), so
/// a stale path is re-rooted on its last three components when that file
/// exists under `cache_dir`. Paths whose file is not there are left alone; the
/// next play resolves them as it would any missing download.
///
/// Returns the number of paths rewritten.
pub fn relocate_cached_paths(db: &Database, cache_dir: &Path) -> rusqlite::Result<usize> {
    let prefix = format!("{}/", cache_dir.to_string_lossy().trim_end_matches('/'));
    let stale: Vec<(i64, String)> = db
        .conn
        .prepare(
            "SELECT id, cached_path FROM tracks
             WHERE cached_path IS NOT NULL AND substr(cached_path, 1, ?2) != ?1",
        )?
        .query_map(
            rusqlite::params![prefix, prefix.chars().count() as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    if stale.is_empty() {
        return Ok(0);
    }

    let tx = crate::db::queries::write_transaction(&db.conn)?;
    let mut moved = 0;
    for (id, old) in &stale {
        let tail: Vec<_> = Path::new(old).components().rev().take(3).collect();
        if tail.len() < 3 {
            continue;
        }
        let new = tail
            .iter()
            .rev()
            .fold(cache_dir.to_path_buf(), |p, c| p.join(c));
        if new.is_file() {
            tx.execute(
                "UPDATE tracks SET cached_path = ?1 WHERE id = ?2",
                rusqlite::params![new.to_string_lossy(), id],
            )?;
            moved += 1;
        }
    }
    tx.commit()?;
    if moved > 0 {
        log::info!(
            "re-rooted {moved} cached path(s) under {}",
            cache_dir.display()
        );
    }
    Ok(moved)
}

/// Fetch again anything in the queue whose downloaded copy has just been
/// removed.
///
/// Clearing downloads deletes files the queue is still pointing at, and an item
/// that goes on claiming to be ready plays nothing at all. Call this after
/// either clearing function, from anywhere with a player attached. Putting the
/// items back to `Pending` is all it takes: the download queue follows the
/// playlist and fetches them.
pub fn requeue_cleared_downloads(state: &SharedPlayerState) {
    let stale = state.reset_items_with_missing_files();
    if !stale.is_empty() {
        log::info!(
            "{} queued tracks lost their copy — fetching again",
            stale.len()
        );
    }
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
        .and_then(year_of)
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
        .and_then(sanitise_extension)
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

#[cfg(test)]
mod cache_path_tests {
    use super::*;

    fn track(artist: &str, album: &str, codec: &str) -> queries::TrackRow {
        queries::TrackRow {
            id: 1,
            album_id: None,
            artist_id: None,
            artist_name: artist.into(),
            album_artist_name: artist.into(),
            album_title: album.into(),
            disc: None,
            track_number: Some(1),
            title: "Song".into(),
            duration_ms: None,
            path: None,
            codec: Some(codec.into()),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "remote".into(),
            remote_id: Some("r1".into()),
            cached_path: None,
        }
    }

    #[test]
    fn a_server_suffix_cannot_leave_the_cache() {
        let cache = Path::new("/cache");
        for codec in [
            "flac/../../../../x",
            "..",
            "../..",
            "/etc/passwd",
            "\\..\\..",
        ] {
            let path = cache_path_for_track(cache, &track("A", "B", codec), None);
            assert!(path_within(cache, &path), "{codec}: {}", path.display());
        }
        let path = cache_path_for_track(cache, &track("A", "B", "flac/../../../../x"), None);
        assert_eq!(path.extension().unwrap(), "flacx");
    }

    #[test]
    fn dot_names_cannot_climb_out() {
        let cache = Path::new("/cache");
        let path = cache_path_for_track(cache, &track("..", ".", ".."), None);
        assert!(path_within(cache, &path), "{}", path.display());
        assert_eq!(sanitise_filename(".."), "_");
        assert_eq!(sanitise_filename(" . "), "_");
        assert_eq!(sanitise_filename("..."), "...");
    }

    #[test]
    fn an_empty_suffix_falls_back_to_flac() {
        assert_eq!(sanitise_extension("../"), None);
        assert_eq!(sanitise_extension("FLAC"), Some("flac".into()));
        let path = cache_path_for_track(Path::new("/c"), &track("A", "B", "./"), None);
        assert_eq!(path.extension().unwrap(), "flac");
    }

    #[test]
    fn path_within_rejects_parent_components() {
        let dir = Path::new("/cache");
        assert!(path_within(dir, Path::new("/cache/a/b.flac")));
        assert!(!path_within(dir, Path::new("/cache/a/../../x")));
        assert!(!path_within(dir, Path::new("/elsewhere/x")));
    }
}

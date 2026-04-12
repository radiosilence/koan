use koan_core::config;
use koan_core::db::queries;
use owo_colors::OwoColorize;

use super::{confirm, format_bytes, open_db};

pub fn cmd_cache_status() {
    let cfg = config::Config::load().unwrap_or_default();
    let cache_dir = cfg.cache_dir();

    println!("{} {}", "path:".cyan(), cache_dir.display());

    if !cache_dir.exists() {
        println!(
            "{} {}",
            "size:".cyan(),
            "empty (no cache directory)".dimmed()
        );
        if let Some(limit) = cfg.cache_limit_bytes() {
            println!("{} {}", "limit:".cyan(), format_bytes(limit));
        }
        return;
    }

    let mut total_bytes: u64 = 0;
    let mut file_count: u64 = 0;
    for entry in walkdir::WalkDir::new(&cache_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            total_bytes += meta.len();
            file_count += 1;
        }
    }

    let size = format_bytes(total_bytes);
    if let Some(limit) = cfg.cache_limit_bytes() {
        let pct = if limit > 0 {
            (total_bytes as f64 / limit as f64 * 100.0) as u64
        } else {
            0
        };
        println!(
            "{} {} / {} ({}%) {}",
            "size:".cyan(),
            size.bold(),
            format_bytes(limit),
            pct,
            format!("({} files)", file_count).dimmed(),
        );
    } else {
        println!(
            "{} {} {}",
            "size:".cyan(),
            size.bold(),
            format!("({} files)", file_count).dimmed(),
        );
    }
}

pub fn cmd_cache_clear(skip_confirm: bool) {
    let cfg = config::Config::load().unwrap_or_default();
    let cache_dir = cfg.cache_dir();

    if !cache_dir.exists() {
        println!("{}", "cache already empty".dimmed());
        return;
    }

    // Count what we're about to nuke.
    let mut total_bytes: u64 = 0;
    let mut file_count: u64 = 0;
    for entry in walkdir::WalkDir::new(&cache_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            total_bytes += meta.len();
            file_count += 1;
        }
    }

    if file_count == 0 {
        println!("{}", "cache already empty".dimmed());
        return;
    }

    // Show what will be deleted and confirm.
    println!(
        "{} {} ({} files) at {}",
        "will delete:".yellow().bold(),
        format_bytes(total_bytes).bold(),
        file_count,
        cache_dir.display().to_string().dimmed(),
    );

    if !skip_confirm && !confirm("proceed?") {
        println!("{}", "aborted".dimmed());
        return;
    }

    if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }

    // Clear cached_path in DB so tracks get re-downloaded next time.
    let db = open_db();
    let _ = queries::clear_cached_paths(&db.conn);

    println!(
        "{} {} {}",
        "cache cleared".green().bold(),
        format_bytes(total_bytes),
        format!("({} files removed)", file_count).dimmed(),
    );
}

/// Run LRU cache eviction: remove whole albums (oldest last-played first) until
/// total cache is under the configured limit. Never evicts favourites.
/// Returns the number of bytes freed.
pub fn evict_cache(cfg: &config::Config, verbose: bool) -> u64 {
    let limit = match cfg.cache_limit_bytes() {
        Some(l) => l as i64,
        None => return 0, // no limit configured
    };

    let db = open_db();

    // Get current total from DB tracking.
    let mut current_size = match queries::total_cache_size(&db.conn) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("cache eviction: failed to query cache size: {}", e);
            return 0;
        }
    };

    if current_size <= limit {
        if verbose {
            log::info!(
                "cache within limit: {} / {}",
                format_bytes(current_size as u64),
                format_bytes(limit as u64)
            );
        }
        return 0;
    }

    let albums = match queries::cached_albums_lru(&db.conn) {
        Ok(a) => a,
        Err(e) => {
            log::warn!("cache eviction: failed to query cached albums: {}", e);
            return 0;
        }
    };

    let mut freed: i64 = 0;
    for album in &albums {
        if current_size <= limit {
            break;
        }

        // Delete cached files for this album.
        for path in &album.cached_paths {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    log::warn!("cache eviction: failed to delete {}: {}", path, e);
                }
            }
        }

        // Clear DB tracking for evicted tracks.
        if let Err(e) = queries::clear_cache_for_tracks(&db.conn, &album.track_ids) {
            log::warn!("cache eviction: failed to clear DB for album: {}", e);
        }

        if verbose || log::log_enabled!(log::Level::Info) {
            log::info!(
                "evicted: {} — {} ({})",
                album.artist_name,
                album.album_title,
                format_bytes(album.total_size as u64),
            );
        }

        current_size -= album.total_size;
        freed += album.total_size;
    }

    // Clean up empty directories in cache.
    cleanup_empty_dirs(&cfg.cache_dir());

    let freed = freed as u64;
    if freed > 0 {
        log::info!("cache eviction freed {}", format_bytes(freed));
    }

    freed
}

/// Remove empty directories left after eviction.
fn cleanup_empty_dirs(dir: &std::path::Path) {
    if !dir.is_dir() {
        return;
    }
    // Walk bottom-up: try to remove leaf directories first.
    for entry in walkdir::WalkDir::new(dir)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        // Don't remove the cache root itself.
        if entry.path() == dir {
            continue;
        }
        // rmdir only succeeds if the directory is empty.
        let _ = std::fs::remove_dir(entry.path());
    }
}

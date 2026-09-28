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

/// Run LRU cache eviction with nothing held back: the CLI has no queue.
pub fn evict_cache(cfg: &config::Config, verbose: bool) -> u64 {
    let db = open_db();
    koan_core::helpers::evict_cache(&db, cfg, &Default::default(), verbose)
}

use std::io::Write;
use std::path::Path;

use koan_core::config;
use owo_colors::OwoColorize;

use super::open_db;

pub fn cmd_scan(path: Option<&Path>, force: bool) {
    let db = open_db();
    let cfg = config::Config::load().unwrap_or_default();

    let folders: Vec<std::path::PathBuf> = if let Some(p) = path {
        vec![p.to_path_buf()]
    } else {
        cfg.library.folders.clone()
    };

    if folders.is_empty() {
        eprintln!(
            "{} no folders to scan — pass a path or configure library.folders",
            "error:".red().bold()
        );
        std::process::exit(1);
    }

    for folder in &folders {
        eprintln!(
            "{} {}",
            "scanning".cyan().bold(),
            folder.display().to_string().dimmed()
        );
    }

    let start = std::time::Instant::now();
    let count = std::cell::Cell::new(0usize);
    let last_draw = std::cell::Cell::new(std::time::Instant::now());

    let on_track = |ev: koan_core::index::scanner::ScanEvent| {
        let c = count.get() + 1;
        count.set(c);
        // Throttle redraws to every 150ms.
        let now = std::time::Instant::now();
        if now.duration_since(last_draw.get()).as_millis() >= 150 {
            last_draw.set(now);
            let elapsed = start.elapsed().as_secs_f64();
            let rate = if elapsed > 0.1 {
                c as f64 / elapsed
            } else {
                0.0
            };
            eprint!(
                "\r  {} {} {} {} {}   ",
                "+".green(),
                c.to_string().cyan(),
                format!("({:.0}/s)", rate).dimmed(),
                format!("{} — {}", ev.artist, ev.title).white(),
                ev.album.dimmed(),
            );
            // Truncate to terminal width to avoid wrapping.
            eprint!("\x1b[K");
            std::io::stderr().flush().ok();
        }
    };

    let result = koan_core::index::scanner::full_scan(&db, &folders, force, Some(&on_track));
    let elapsed = start.elapsed();

    // Clear the progress line.
    eprint!("\r\x1b[K");

    // Summary.
    eprintln!(
        "{} {}",
        "scan complete".green().bold(),
        format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
    );
    eprintln!(
        "  {} added  {} updated  {} removed  {} skipped",
        result.added.to_string().green().bold(),
        result.updated.to_string().yellow(),
        result.removed.to_string().red(),
        result.skipped.to_string().dimmed(),
    );

    if !result.errors.is_empty() {
        eprintln!();
        eprintln!(
            "{} {}",
            format!("{} errors", result.errors.len()).red().bold(),
            "(will retry on next scan)".dimmed()
        );
        for (path, err) in result.errors.iter().take(20) {
            // Show filename prominently, full path dimmed, error in red.
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let parent = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            eprintln!(
                "  {} {} {}",
                "✗".red(),
                filename.white().bold(),
                format!("in {}", parent).dimmed(),
            );
            eprintln!("    {}", err.red());
        }
        if result.errors.len() > 20 {
            eprintln!(
                "  {} {}",
                "…".dimmed(),
                format!("and {} more", result.errors.len() - 20).dimmed()
            );
        }
    }

    // Run acoustic analysis if configured.
    if cfg.discovery.analysis_on_scan {
        let missing = koan_core::db::queries::tracks_missing_vectors(&db.conn).unwrap_or_default();
        if !missing.is_empty() {
            eprintln!();
            eprintln!(
                "{} analyzing {} tracks for acoustic similarity...",
                "♪".cyan(),
                missing.len().to_string().cyan().bold()
            );
            let analysis_start = std::time::Instant::now();
            let (ok, err) = koan_core::index::scanner::analyze_missing(&db, None);
            let analysis_elapsed = analysis_start.elapsed();
            eprintln!(
                "  {} analyzed  {} errors  {}",
                ok.to_string().green().bold(),
                err.to_string().red(),
                format!("({:.1}s)", analysis_elapsed.as_secs_f64()).dimmed(),
            );
        }
    }
}

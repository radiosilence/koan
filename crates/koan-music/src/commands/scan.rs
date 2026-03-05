use std::io::Write;
use std::path::Path;

use koan_core::config;
use owo_colors::OwoColorize;

use super::open_db;

pub fn cmd_scan(path: Option<&Path>, force: bool) {
    let db = open_db();

    let folders: Vec<std::path::PathBuf> = if let Some(p) = path {
        vec![p.to_path_buf()]
    } else {
        let cfg = config::Config::load().unwrap_or_default();
        cfg.library.folders
    };

    if folders.is_empty() {
        eprintln!(
            "{} no folders to scan — pass a path or configure library.folders",
            "error:".red().bold()
        );
        std::process::exit(1);
    }

    let start = std::time::Instant::now();
    let count = std::cell::Cell::new(0usize);
    let last_draw = std::cell::Cell::new(std::time::Instant::now());

    let on_track = |_ev: koan_core::index::scanner::ScanEvent| {
        let c = count.get() + 1;
        count.set(c);
        // Throttle redraws to every 100ms.
        let now = std::time::Instant::now();
        if now.duration_since(last_draw.get()).as_millis() >= 100 {
            last_draw.set(now);
            let elapsed = start.elapsed().as_secs_f64();
            let rate = if elapsed > 0.1 {
                c as f64 / elapsed
            } else {
                0.0
            };
            eprint!(
                "\r{} {} scanned ({:.0}/s)   ",
                "\u{2022}".green(),
                c.to_string().cyan(),
                rate
            );
            std::io::stderr().flush().ok();
        }
    };

    let result = koan_core::index::scanner::full_scan(&db, &folders, force, Some(&on_track));
    let elapsed = start.elapsed();

    // Clear the progress line.
    eprint!("\r{}\r", " ".repeat(60));

    println!(
        "{} {} {} added, {} updated, {} removed, {} skipped",
        "scan complete".green().bold(),
        format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
        result.added.to_string().green(),
        result.updated.to_string().yellow(),
        result.removed.to_string().red(),
        result.skipped.to_string().dimmed(),
    );

    if !result.errors.is_empty() {
        println!("{} {}:", "errors".red().bold(), result.errors.len());
        for (path, err) in result.errors.iter().take(10) {
            println!(
                "  {} {} {}",
                "\u{2502}".dimmed(),
                path.display().to_string().dimmed(),
                format!("\u{2014} {}", err).red()
            );
        }
        if result.errors.len() > 10 {
            println!(
                "  {} {}",
                "\u{2514}".dimmed(),
                format!("... and {} more", result.errors.len() - 10).dimmed()
            );
        }
    }
}

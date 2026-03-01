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
    let on_track = |ev: koan_core::index::scanner::ScanEvent| {
        println!(
            "  {} {} {} {}",
            "+".green(),
            ev.artist.cyan(),
            "\u{2014}".dimmed(),
            ev.title,
        );
    };
    let result = koan_core::index::scanner::full_scan(&db, &folders, force, Some(&on_track));
    let elapsed = start.elapsed();

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

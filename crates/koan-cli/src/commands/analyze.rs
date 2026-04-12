//! `koan analyze` — run acoustic analysis on the library.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use koan_core::db::queries;
use owo_colors::OwoColorize;

use super::open_db;

pub fn cmd_analyze() {
    let db = open_db();

    let total_tracks: i64 = queries::vector_count(&db.conn).unwrap_or(0);
    let missing = queries::tracks_missing_vectors(&db.conn).unwrap_or_default();

    if missing.is_empty() {
        println!(
            "{} all {} tracks already analyzed",
            "done".green().bold(),
            total_tracks
        );
        return;
    }

    println!(
        "{} analyzing {} tracks ({} already done)",
        "\u{2022}".green(),
        missing.len().to_string().cyan(),
        total_tracks.to_string().dimmed(),
    );

    let start = std::time::Instant::now();
    let analyzed = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);
    let last_draw = std::sync::Mutex::new(std::time::Instant::now());

    let on_track = |ev: koan_core::index::scanner::AnalysisEvent| {
        if ev.success {
            analyzed.fetch_add(1, Ordering::Relaxed);
        } else {
            errors.fetch_add(1, Ordering::Relaxed);
        }

        let now = std::time::Instant::now();
        let mut ld = last_draw.lock().unwrap();
        if now.duration_since(*ld).as_millis() >= 100 {
            *ld = now;
            let elapsed = start.elapsed().as_secs_f64();
            let done = analyzed.load(Ordering::Relaxed) + errors.load(Ordering::Relaxed);
            let rate = if elapsed > 0.1 {
                done as f64 / elapsed
            } else {
                0.0
            };
            eprint!(
                "\r{} {}/{} analyzed ({:.1}/s)   ",
                "\u{2022}".green(),
                done.to_string().cyan(),
                ev.total.to_string().dimmed(),
                rate
            );
            std::io::stderr().flush().ok();
        }
    };

    let (ok, err) = koan_core::index::scanner::analyze_missing(&db, Some(&on_track));
    let elapsed = start.elapsed();

    // Clear progress line.
    eprint!("\r{}\r", " ".repeat(60));

    println!(
        "{} {} {} analyzed, {} errors {}",
        "analysis complete".green().bold(),
        format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
        ok.to_string().green(),
        err.to_string().red(),
        format!(
            "({} total vectors)",
            queries::vector_count(&db.conn).unwrap_or(0)
        )
        .dimmed(),
    );
}

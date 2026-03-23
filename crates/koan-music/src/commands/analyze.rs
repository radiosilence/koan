//! `koan analyze` — run acoustic analysis on the library.
//!
//! `koan analyze` — bliss acoustic features (23-dim, default).
//! `koan analyze --neural` — DCLAP neural embeddings (512-dim, opt-in).

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use koan_core::config::Config;
use koan_core::db::queries;
use owo_colors::OwoColorize;

use super::open_db;

/// Shared progress state for analysis commands.
struct AnalysisProgress {
    start: std::time::Instant,
    analyzed: AtomicUsize,
    errors: AtomicUsize,
    last_draw: std::sync::Mutex<std::time::Instant>,
    label: String,
}

impl AnalysisProgress {
    fn new(label: &str) -> Self {
        let now = std::time::Instant::now();
        Self {
            start: now,
            analyzed: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
            last_draw: std::sync::Mutex::new(now),
            label: label.into(),
        }
    }

    fn callback(&self) -> impl Fn(koan_core::index::scanner::AnalysisEvent) + '_ {
        move |ev: koan_core::index::scanner::AnalysisEvent| {
            if ev.success {
                self.analyzed.fetch_add(1, Ordering::Relaxed);
            } else {
                self.errors.fetch_add(1, Ordering::Relaxed);
            }

            let now = std::time::Instant::now();
            let mut ld = self.last_draw.lock().unwrap();
            if now.duration_since(*ld).as_millis() >= 100 {
                *ld = now;
                let elapsed = self.start.elapsed().as_secs_f64();
                let done =
                    self.analyzed.load(Ordering::Relaxed) + self.errors.load(Ordering::Relaxed);
                let rate = if elapsed > 0.1 {
                    done as f64 / elapsed
                } else {
                    0.0
                };
                let remaining = if rate > 0.0 {
                    let left = (ev.total - done) as f64 / rate;
                    format!(" ~{:.0}s remaining", left)
                } else {
                    String::new()
                };
                eprint!(
                    "\r{} {}/{} {} ({:.1}/s){}   ",
                    "\u{2022}".green(),
                    done.to_string().cyan(),
                    ev.total.to_string().dimmed(),
                    self.label,
                    rate,
                    remaining.dimmed(),
                );
                std::io::stderr().flush().ok();
            }
        }
    }

    fn finish(&self, ok: usize, err: usize, total_label: &str, total_count: i64) {
        let elapsed = self.start.elapsed();
        // Clear progress line.
        eprint!("\r{}\r", " ".repeat(80));
        println!(
            "{} {} {} {}, {} errors {}",
            format!("{} complete", self.label).green().bold(),
            format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
            ok.to_string().green(),
            self.label,
            err.to_string().red(),
            format!("({} total {})", total_count, total_label).dimmed(),
        );
    }
}

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

    let progress = AnalysisProgress::new("analyzed");
    let cb = progress.callback();
    let (ok, err) = koan_core::index::scanner::analyze_missing(&db, Some(&cb));
    progress.finish(
        ok,
        err,
        "vectors",
        queries::vector_count(&db.conn).unwrap_or(0),
    );
}

pub fn cmd_analyze_neural() {
    let db = open_db();
    let cfg = Config::load_or_default();

    if !cfg.discovery.neural_enabled {
        eprintln!(
            "{} neural analysis is disabled in config (discovery.neural_enabled = false)",
            "skip".yellow().bold(),
        );
        return;
    }

    let model_dir = cfg.discovery.model_dir();
    if !koan_core::index::neural::is_audio_model_available(&model_dir) {
        eprintln!(
            "{} neural model not found at {}",
            "error".red().bold(),
            koan_core::index::neural::audio_model_path(&model_dir).display(),
        );
        eprintln!(
            "  download DCLAP ONNX models and place them at: {}/",
            model_dir.display(),
        );
        return;
    }

    let total_existing: i64 = queries::neural_vector_count(&db.conn).unwrap_or(0);
    let missing = queries::tracks_missing_neural_vectors(&db.conn).unwrap_or_default();

    if missing.is_empty() {
        println!(
            "{} all {} tracks already have neural embeddings",
            "done".green().bold(),
            total_existing
        );
        return;
    }

    println!(
        "{} neural analysis: {} tracks to process ({} already done)",
        "\u{2022}".green(),
        missing.len().to_string().cyan(),
        total_existing.to_string().dimmed(),
    );

    let progress = AnalysisProgress::new("neural analyzed");
    let cb = progress.callback();
    let (ok, err) = koan_core::index::scanner::analyze_missing_neural(&db, &model_dir, Some(&cb));
    progress.finish(
        ok,
        err,
        "neural vectors",
        queries::neural_vector_count(&db.conn).unwrap_or(0),
    );
}

use std::path::Path;

use koan_core::config;
use owo_colors::OwoColorize;

use super::{confirm, open_db};

pub fn cmd_organize(
    pattern: Option<&str>,
    base_dir: Option<&Path>,
    execute: bool,
    undo_mode: bool,
    skip_confirm: bool,
    list_patterns: bool,
) {
    use koan_core::organize;

    let cfg = config::Config::load().unwrap_or_default();

    // --list: show configured named patterns and exit.
    if list_patterns {
        if cfg.organize.patterns.is_empty() {
            println!("{}", "no named patterns configured".dimmed());
            println!(
                "\nadd them to {}:\n",
                config::config_file_path().display().to_string().dimmed()
            );
            println!("  {}", "[organize.patterns]".dimmed());
            println!(
                "  {}",
                "standard = \"%album artist%/(%date%) %album%/%tracknumber%. %title%\"".dimmed()
            );
        } else {
            let default_name = cfg.organize.default.as_deref();
            let mut names: Vec<_> = cfg.organize.patterns.keys().collect();
            names.sort();
            for name in names {
                let value = &cfg.organize.patterns[name];
                let marker = if default_name == Some(name.as_str()) {
                    " (default)".green().to_string()
                } else {
                    String::new()
                };
                println!("{}{}", name.bold(), marker);
                println!("  {}\n", value.dimmed());
            }
        }
        return;
    }

    let db = open_db();

    if undo_mode {
        // Preview what undo would do, then confirm.
        let batch_info = db.conn.query_row(
            "SELECT batch_id, COUNT(*) FROM organize_log WHERE batch_id = \
             (SELECT batch_id FROM organize_log ORDER BY created_at DESC LIMIT 1) \
             GROUP BY batch_id",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        );

        match batch_info {
            Ok((batch_id, count)) => {
                println!(
                    "{} will revert {} file moves from batch {}",
                    "undo:".yellow().bold(),
                    count,
                    batch_id.dimmed(),
                );

                if !skip_confirm && !confirm("proceed?") {
                    println!("{}", "aborted".dimmed());
                    return;
                }

                match organize::undo(&db) {
                    Ok(reverted) => {
                        println!("{} {} files reverted", "undo:".green().bold(), reverted);
                    }
                    Err(e) => {
                        eprintln!("{} {}", "error:".red().bold(), e);
                        std::process::exit(1);
                    }
                }
            }
            Err(_) => {
                println!("{}", "no organize batches to undo".dimmed());
            }
        }
        return;
    }

    // Resolve the pattern: --pattern flag, or default from config.
    let resolved = match pattern {
        Some(p) => cfg.organize.resolve_pattern(p).to_string(),
        None => match cfg.organize.default_pattern() {
            Some(p) => {
                println!(
                    "{} using default pattern '{}'\n",
                    "config:".cyan().bold(),
                    cfg.organize.default.as_deref().unwrap_or(""),
                );
                p.to_string()
            }
            None => {
                eprintln!(
                    "{} --pattern is required (or set [organize] default in config)",
                    "error:".red().bold()
                );
                std::process::exit(1);
            }
        },
    };
    let pattern = resolved.as_str();

    if execute {
        // Always preview first, then confirm before applying.
        match organize::preview(&db, pattern, base_dir) {
            Ok(result) => {
                if result.moves.is_empty() && result.errors.is_empty() {
                    println!("{}", "no tracks to organize".dimmed());
                    return;
                }

                println!(
                    "{} {} tracks will be moved\n",
                    "preview:".cyan().bold(),
                    result.moves.len()
                );

                let show_count = result.moves.len().min(20);
                for m in &result.moves[..show_count] {
                    println!("  {}", m.from.display().dimmed());
                    println!("    {} {}", "\u{2192}".cyan(), m.to.display());
                    println!();
                }

                let remaining = result.moves.len().saturating_sub(show_count);
                if remaining > 0 {
                    println!("  {} (and {} more)\n", "...".dimmed(), remaining);
                }

                if !result.errors.is_empty() {
                    println!(
                        "  {} {} errors",
                        "warning:".yellow().bold(),
                        result.errors.len()
                    );
                    for (path, err) in &result.errors {
                        eprintln!("    {} {}", path.display(), err.dimmed());
                    }
                    println!();
                }

                if !skip_confirm && !confirm("apply these moves?") {
                    println!("{}", "aborted".dimmed());
                    return;
                }
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }

        // Now actually execute.
        match organize::execute(&db, pattern, base_dir) {
            Ok(result) => {
                let moved = result.moves.len();
                for m in &result.moves {
                    println!("  {} {}", "\u{2713}".green(), m.to.display());
                }
                for (path, err) in &result.errors {
                    eprintln!("  {} {} {}", "\u{2717}".red(), path.display(), err.dimmed());
                }
                println!();
                println!(
                    "{} {} moved, {} errors{}",
                    "done:".green().bold(),
                    moved,
                    result.errors.len(),
                    if moved > 0 {
                        "\nrun 'koan organize --undo' to revert"
                    } else {
                        ""
                    }
                );
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
    } else {
        // Preview mode (default).
        match organize::preview(&db, pattern, base_dir) {
            Ok(result) => {
                if result.moves.is_empty() && result.errors.is_empty() {
                    println!("{}", "no tracks to organize".dimmed());
                    return;
                }

                println!(
                    "{} {} tracks would be moved\n",
                    "preview:".cyan().bold(),
                    result.moves.len()
                );

                let show_count = result.moves.len().min(20);
                for m in &result.moves[..show_count] {
                    println!("  {}", m.from.display().dimmed());
                    println!("    {} {}", "\u{2192}".cyan(), m.to.display());
                    println!();
                }

                let remaining = result.moves.len().saturating_sub(show_count);
                if remaining > 0 {
                    println!("  {} (and {} more)\n", "...".dimmed(), remaining);
                }

                if result.skipped > 0 {
                    println!(
                        "  {} {} already in place",
                        "skipped:".dimmed(),
                        result.skipped
                    );
                }

                if !result.errors.is_empty() {
                    println!(
                        "  {} {} errors",
                        "warning:".yellow().bold(),
                        result.errors.len()
                    );
                    for (path, err) in &result.errors {
                        eprintln!("    {} {}", path.display(), err.dimmed());
                    }
                }

                println!("\nrun with {} to apply", "--execute".bold());
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
    }
}

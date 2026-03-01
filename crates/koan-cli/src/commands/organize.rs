use std::path::Path;

use owo_colors::OwoColorize;

use super::open_db;

pub fn cmd_organize(
    pattern: Option<&str>,
    base_dir: Option<&Path>,
    execute: bool,
    undo_mode: bool,
) {
    use koan_core::organize;

    let db = open_db();

    if undo_mode {
        match organize::undo(&db) {
            Ok(count) => {
                println!("{} {} files reverted", "undo:".green().bold(), count);
            }
            Err(organize::OrganizeError::NothingToUndo) => {
                println!("{}", "no organize batches to undo".dimmed());
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
        return;
    }

    let Some(pattern) = pattern else {
        eprintln!(
            "{} --pattern is required (unless --undo)",
            "error:".red().bold()
        );
        std::process::exit(1);
    };

    if execute {
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

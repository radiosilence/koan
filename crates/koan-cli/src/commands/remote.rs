use std::io::Write as _;

use koan_core::config;
use koan_core::remote::sync::{SyncPhase, SyncProgress};
use owo_colors::OwoColorize;

use super::open_db;

pub fn cmd_remote_login(url: &str, username: &str) {
    if !url.starts_with("https://") && !url.contains("localhost") && !url.contains("127.0.0.1") {
        eprintln!("warning: server URL does not use HTTPS — credentials will be sent in plaintext");
    }

    let password = rpassword::prompt_password("password: ").unwrap_or_else(|e| {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    });

    // Pings, then writes the credentials to config.local.toml.
    if let Err(e) = koan_core::helpers::set_remote_credentials(url, username, &password) {
        eprintln!("{} {}", "sign-in failed:".red().bold(), e);
        std::process::exit(1);
    }
    println!("{} {}", "connected".green(), url);
    println!("{}", "password stored in the OS credential store".green());
}

pub fn cmd_remote_sync(full: bool) {
    let cfg = config::Config::load().unwrap_or_default();
    let client = match koan_core::helpers::subsonic_client(&cfg) {
        Some(c) => c,
        None => {
            eprintln!(
                "{} no remote server configured — run {} first",
                "error:".red().bold(),
                "koan remote login".bold()
            );
            std::process::exit(1);
        }
    };

    let db = open_db();
    let start = std::time::Instant::now();

    let synced = match koan_core::helpers::sync_remote(
        &db,
        &client,
        full,
        &cfg.remote.url,
        &cfg.remote.username,
        &draw_progress,
    ) {
        Ok(synced) => synced,
        Err(e) => {
            eprintln!("{} {}", "sync failed:".red().bold(), e);
            std::process::exit(1);
        }
    };

    eprint!("\r\x1b[K");
    let library = &synced.library;
    let elapsed = start.elapsed();
    let headline = if library.is_complete() {
        "sync complete".green().bold().to_string()
    } else {
        "sync incomplete".yellow().bold().to_string()
    };
    println!(
        "{} {} {} artists, {} albums, {} tracks",
        headline,
        format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
        library.artists_synced.to_string().bold(),
        library.albums_synced.to_string().bold(),
        library.tracks_synced.to_string().bold(),
    );
    if !library.is_complete() {
        eprintln!(
            "{} {} album(s) could not be fetched — the sync watermark was left \
             unchanged, so the next sync will retry them",
            "warning:".yellow().bold(),
            library.albums_failed.to_string().bold(),
        );
    }

    println!(
        "{} {} pushed, {} imported",
        "favourites synced:".green().bold(),
        synced.favourites.pushed.to_string().bold(),
        synced.favourites.imported.to_string().bold(),
    );
    println!(
        "{} {} pulled, {} pushed",
        "playlists synced:".green().bold(),
        synced.playlists.pulled.to_string().bold(),
        synced.playlists.pushed.to_string().bold(),
    );
}

/// One line on stderr, redrawn in place.
fn draw_progress(p: SyncProgress) {
    let phase = match p.phase {
        SyncPhase::Albums => "listing albums",
        SyncPhase::Tracks => "tracks",
        SyncPhase::Artists => "artists",
        SyncPhase::Finishing => "finishing",
    };
    let count = match (p.phase, p.total) {
        (SyncPhase::Finishing, _) => String::new(),
        (SyncPhase::Artists, Some(total)) => total.to_string(),
        (_, Some(total)) if total > 0 => {
            let pct = (p.done as f64 / total as f64 * 100.0).min(100.0);
            format!("{} of {} ({pct:.0}%)", p.done, total)
        }
        _ => p.done.to_string(),
    };
    eprint!("\r  {} {}\x1b[K", phase.cyan(), count.bold());
    std::io::stderr().flush().ok();
}

pub fn cmd_remote_status() {
    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        println!("no remote server configured");
        return;
    }

    println!("{} {}", "server:".cyan(), cfg.remote.url);
    println!("{} {}", "username:".cyan(), cfg.remote.username);

    // Asked the way everything else asks, rather than by reading the field
    // directly: a status that consults a different source from the code doing
    // the work will eventually disagree with it.
    let described = match koan_core::helpers::get_remote_password(&cfg) {
        Some(_) => "set".green().to_string(),
        None => "not set".red().to_string(),
    };
    println!("{} {}", "password:".cyan(), described);

    // Attempted whenever credentials resolve, rather than gated on a guess
    // about whether they would: only reaching the server proves anything.
    let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
        println!(
            "{} {}",
            "status:".cyan(),
            koan_core::helpers::remote_unavailable(&cfg).red()
        );
        return;
    };

    match client.ping() {
        Ok(()) => println!("{} {}", "status:".cyan(), "connected".green()),
        Err(e) => println!(
            "{} {} {}",
            "status:".cyan(),
            "error".red(),
            format!("\u{2014} {}", e).dimmed()
        ),
    }
}

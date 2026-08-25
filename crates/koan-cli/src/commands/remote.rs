use koan_core::config;
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

    // Pings, stores the password in the platform credential store, and clears
    // any plaintext copy an older koan left in config.local.toml.
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

    match koan_core::remote::sync::sync_library(
        &db,
        &client,
        full,
        &cfg.remote.url,
        &cfg.remote.username,
    ) {
        Ok(result) => {
            let elapsed = start.elapsed();
            let headline = if result.is_complete() {
                "sync complete".green().bold().to_string()
            } else {
                "sync incomplete".yellow().bold().to_string()
            };
            println!(
                "{} {} {} artists, {} albums, {} tracks",
                headline,
                format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
                result.artists_synced.to_string().bold(),
                result.albums_synced.to_string().bold(),
                result.tracks_synced.to_string().bold(),
            );
            if !result.is_complete() {
                eprintln!(
                    "{} {} album(s) could not be fetched — the sync watermark was left \
                     unchanged, so the next sync will retry them",
                    "warning:".yellow().bold(),
                    result.albums_failed.to_string().bold(),
                );
            }
        }
        Err(e) => {
            eprintln!("{} {}", "sync failed:".red().bold(), e);
            std::process::exit(1);
        }
    }

    // Sync favourites: push local → remote, pull remote → local.
    print!("{}", "syncing favourites...".dimmed());
    use std::io::Write;
    std::io::stdout().flush().ok();

    let synced = koan_core::helpers::reconcile_favourites(&db, &client);

    println!(
        "\r{} {} pushed, {} imported",
        "favourites synced:".green().bold(),
        synced.pushed.to_string().bold(),
        synced.imported.to_string().bold(),
    );

    print!("{}", "syncing playlists...".dimmed());
    std::io::stdout().flush().ok();

    let playlists = koan_core::playlists::reconcile_playlists(&db, &client, &cfg.remote.username);

    println!(
        "\r{} {} pulled, {} pushed",
        "playlists synced:".green().bold(),
        playlists.pulled.to_string().bold(),
        playlists.pushed.to_string().bold(),
    );
}

pub fn cmd_remote_status() {
    use koan_core::helpers::PasswordSource;

    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        println!("no remote server configured");
        return;
    }

    println!("{} {}", "server:".cyan(), cfg.remote.url);
    println!("{} {}", "username:".cyan(), cfg.remote.username);

    // Asked the way everything else asks. Reporting on a field koan itself does
    // not consult is how a keychain-backed sign-in — the arrangement `remote
    // login` creates — came to be reported as having no password at all.
    let (_, source) = koan_core::helpers::remote_password(&cfg);
    let described = match &source {
        PasswordSource::Keychain => "from the keychain".green().to_string(),
        PasswordSource::Config => "from config.local.toml".green().to_string(),
        PasswordSource::Missing => "not set".red().to_string(),
        PasswordSource::Unreadable(why) => format!(
            "{} {}",
            "in the keychain, but koan cannot read it".red(),
            format!("\u{2014} {why}").dimmed()
        ),
    };
    println!("{} {}", "password:".cyan(), described);

    // Attempted whenever credentials resolve, rather than gated on a guess
    // about whether they would. The reach is the only part of this that
    // actually proves anything.
    let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
        if matches!(source, PasswordSource::Unreadable(_)) {
            println!(
                "{} {}",
                "hint:".yellow(),
                "`koan remote login` rewrites the entry for this build".dimmed()
            );
        }
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

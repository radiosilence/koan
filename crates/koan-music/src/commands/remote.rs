use koan_core::config;
use owo_colors::OwoColorize;

use super::{get_remote_password, open_db};

pub fn cmd_remote_login(url: &str, username: &str) {
    let password = rpassword::prompt_password("password: ").unwrap_or_else(|e| {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    });

    let client = koan_core::remote::client::SubsonicClient::new(url, username, &password);
    match client.ping() {
        Ok(()) => println!("{} {}", "connected".green(), url),
        Err(e) => {
            eprintln!("{} {}", "connection failed:".red().bold(), e);
            std::process::exit(1);
        }
    }

    // Load existing local config (preserve folders etc), update remote fields.
    let local_path = config::config_local_file_path();
    let mut local_cfg = if local_path.exists() {
        config::Config::load_from(&local_path).unwrap_or_default()
    } else {
        config::Config::default()
    };
    local_cfg.remote.enabled = true;
    local_cfg.remote.url = url.to_string();
    local_cfg.remote.username = username.to_string();
    local_cfg.remote.password = password;
    if let Err(e) = local_cfg.save_local() {
        eprintln!("{} {}", "config error:".red().bold(), e);
        std::process::exit(1);
    }
    println!("{}", "credentials saved to config.local.toml".green());
}

pub fn cmd_remote_sync(full: bool) {
    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        eprintln!(
            "{} no remote server configured — run {} first",
            "error:".red().bold(),
            "koan remote login".bold()
        );
        std::process::exit(1);
    }

    let password = get_remote_password(&cfg);

    let client = koan_core::remote::client::SubsonicClient::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    );

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
            println!(
                "{} {} {} artists, {} albums, {} tracks",
                "sync complete".green().bold(),
                format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
                result.artists_synced.to_string().bold(),
                result.albums_synced.to_string().bold(),
                result.tracks_synced.to_string().bold(),
            );
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

    // Push: star any local favourites that have a remote_id.
    let local_favs =
        koan_core::db::queries::favourites_with_remote_id(&db.conn).unwrap_or_default();
    let mut starred = 0;
    for (_path, remote_id) in &local_favs {
        if client.star(remote_id).is_ok() {
            starred += 1;
        }
    }

    // Pull: import remote starred songs as local favourites.
    let imported = match client.get_starred() {
        Ok(songs) => {
            let remote_ids: Vec<String> = songs.into_iter().map(|s| s.id).collect();
            match koan_core::db::queries::import_remote_favourites(&db.conn, &remote_ids) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("\n{} importing favourites: {}", "error".red(), e);
                    0
                }
            }
        }
        Err(e) => {
            eprintln!("\n{} fetching starred: {}", "error".red(), e);
            0
        }
    };

    println!(
        "\r{} {} pushed, {} imported",
        "favourites synced:".green().bold(),
        starred.to_string().bold(),
        imported.to_string().bold(),
    );
}

pub fn cmd_remote_status() {
    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        println!("no remote server configured");
        return;
    }

    println!("{} {}", "server:".cyan(), cfg.remote.url);
    println!("{} {}", "username:".cyan(), cfg.remote.username);

    let has_password = !cfg.remote.password.is_empty();
    println!(
        "{} {}",
        "password:".cyan(),
        if has_password {
            "configured".green().to_string()
        } else {
            "not set".red().to_string()
        }
    );

    if has_password {
        let client = koan_core::remote::client::SubsonicClient::new(
            &cfg.remote.url,
            &cfg.remote.username,
            &cfg.remote.password,
        );
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
}

//! CLI commands for koan's own Subsonic REST API.

use koan_core::config::Config;
use koan_core::helpers::SUBSONIC_CREDENTIAL_ACCOUNT;
use koan_core::{auth, credentials};
use owo_colors::OwoColorize;

/// `koan subsonic setup [--username <u>]` — enable `/rest/*` with a fresh secret.
pub fn cmd_subsonic_setup(username: &str) {
    let secret = auth::random_token().unwrap_or_else(|e| {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    });

    let stored_in_keychain = credentials::store_password(SUBSONIC_CREDENTIAL_ACCOUNT, &secret)
        .inspect_err(|e| eprintln!("{} keychain unavailable: {}", "warning:".yellow(), e))
        .is_ok();

    // config.local.toml, like [remote]: which machine serves Subsonic is
    // machine-specific, and the secret must never reach a committed file.
    let mut values = toml::map::Map::new();
    values.insert("enabled".into(), toml::Value::Boolean(true));
    values.insert("username".into(), toml::Value::String(username.to_string()));
    if !stored_in_keychain {
        values.insert("password".into(), toml::Value::String(secret.clone()));
    }
    if let Err(e) = Config::patch_local("subsonic", &values) {
        eprintln!("{} {}", "config error:".red().bold(), e);
        std::process::exit(1);
    }

    println!("{} Subsonic API enabled.", "✓".green().bold());
    println!("  username: {}", username);
    println!("  password: {}", secret);
    println!(
        "  stored in: {}",
        if stored_in_keychain {
            "OS keychain"
        } else {
            "config.toml"
        }
    );
    println!(
        "\n{}",
        "This secret is shown once. It is not your Navidrome password, and it must not be —\n\
         Subsonic clients send md5(secret + salt) over whatever transport they pick, so anyone\n\
         who can watch the network gets a digest to crack offline. Do not expose /rest/ to the\n\
         internet."
            .dimmed()
    );
}

/// `koan subsonic status`
pub fn cmd_subsonic_status() {
    let cfg = Config::load().unwrap_or_default();
    let has_secret = koan_core::helpers::get_subsonic_password(&cfg).is_some();

    println!(
        "enabled:  {}",
        if cfg.subsonic.enabled {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        }
    );
    println!("username: {}", cfg.subsonic.username);
    println!(
        "secret:   {}",
        if has_secret {
            "configured".green().to_string()
        } else {
            "missing — run `koan subsonic setup`".yellow().to_string()
        }
    );
}

/// `koan subsonic disable` — stop serving `/rest/*` and drop the secret.
pub fn cmd_subsonic_disable() {
    let _ = credentials::delete_password(SUBSONIC_CREDENTIAL_ACCOUNT);
    let mut values = toml::map::Map::new();
    values.insert("enabled".into(), toml::Value::Boolean(false));
    values.insert("password".into(), toml::Value::String(String::new()));
    if let Err(e) = Config::patch_local("subsonic", &values) {
        eprintln!("{} {}", "config error:".red().bold(), e);
        std::process::exit(1);
    }
    println!("{} Subsonic API disabled.", "✓".green().bold());
}

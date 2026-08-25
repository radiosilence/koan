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

    let fallback_secret = (!stored_in_keychain).then(|| secret.clone());
    if let Err(e) = Config::persist(|cfg| {
        cfg.subsonic.enabled = true;
        cfg.subsonic.username = username.to_string();
        if let Some(secret) = fallback_secret {
            cfg.subsonic.password = secret;
        }
    }) {
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
            "config.local.toml"
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
    if let Err(e) = Config::persist(|cfg| {
        cfg.subsonic.enabled = false;
        cfg.subsonic.password = String::new();
    }) {
        eprintln!("{} {}", "config error:".red().bold(), e);
        std::process::exit(1);
    }
    println!("{} Subsonic API disabled.", "✓".green().bold());
}

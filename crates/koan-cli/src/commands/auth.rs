//! CLI auth commands: setup, create-user, delete-user, list-users, login, logout,
//! api-key.

use std::io::{IsTerminal, Write, stdin, stdout};

use koan_core::auth::{self, Role};
use koan_core::config::Config;
use koan_core::db::queries::api_keys;
use koan_core::db::queries::auth as auth_queries;
use owo_colors::OwoColorize;

use super::{confirm, open_db};

/// Whether the auth commands may ask questions. Not with `--non-interactive`,
/// and not when stdin is not a terminal: nobody is there to answer, and an
/// unanswered prompt must not be taken as yes.
#[derive(Clone, Copy)]
pub struct Tty(bool);

impl Tty {
    pub fn detect(non_interactive: bool) -> Self {
        Self(!non_interactive && stdin().is_terminal())
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{} {message}", "✗".red().bold());
    std::process::exit(1);
}

/// Go ahead with something that cannot be undone: on `--yes`, or when asked
/// at a terminal. Without either it refuses, rather than reading no answer as
/// one.
fn confirmed(tty: Tty, yes: bool, question: &str) -> bool {
    if yes {
        return true;
    }
    if !tty.0 {
        fail(&format!(
            "{question} Pass --yes to confirm without a prompt."
        ));
    }
    confirm(question)
}

/// `koan auth setup` — generate keypair and create first admin user.
pub fn cmd_auth_setup(tty: Tty, save_to_1password: bool) {
    let db = open_db();

    // Generate keypair if not present.
    match auth::load_keypair() {
        Ok(_) => {
            println!("{}", "Ed25519 keypair already exists.".dimmed());
        }
        Err(_) => match auth::generate_keypair() {
            Ok(_) => {
                println!("{} Ed25519 keypair generated.", "✓".green().bold());
            }
            Err(e) => {
                eprintln!("{} Failed to generate keypair: {}", "✗".red().bold(), e);
                std::process::exit(1);
            }
        },
    }

    // Check if any users exist already.
    if auth_queries::has_users(&db.conn).unwrap_or(false) {
        println!(
            "{}",
            "Users already exist. Use `koan auth create-user` to add more.".dimmed()
        );
        return;
    }

    println!("\nCreating admin user...");

    let username = std::env::var("KOAN_USERNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if !tty.0 {
                fail("No username: set KOAN_USERNAME.");
            }
            let u = prompt("Username: ");
            if u.is_empty() {
                fail("Username cannot be empty");
            }
            u
        });

    let password = new_password(tty);

    match auth_queries::create_user(&db.conn, &username, &password, Role::Admin) {
        Ok(id) => {
            println!(
                "{} Admin user '{}' created (id: {})",
                "✓".green().bold(),
                username,
                id
            );
            offer_save_to_1password(tty, save_to_1password, &username, &password);
            let cfg = koan_core::config::Config::load_or_default();
            if !cfg.graphql.auth_enabled {
                println!(
                    "\n{} Auth is currently disabled. Enable in config.toml:\n  [graphql]\n  auth_enabled = true",
                    "!".yellow().bold()
                );
            }
        }
        Err(e) => {
            eprintln!("{} Failed to create user: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth create-user --username <u> --role <r>`
pub fn cmd_auth_create_user(tty: Tty, username: &str, role_str: &str, save_to_1password: bool) {
    let db = open_db();

    let role: Role = role_str.parse().unwrap_or_else(|_| {
        eprintln!(
            "{} Invalid role '{}'. Must be: admin, user, readonly",
            "✗".red().bold(),
            role_str
        );
        std::process::exit(1);
    });

    let password = new_password(tty);

    match auth_queries::create_user(&db.conn, username, &password, role) {
        Ok(id) => {
            println!(
                "{} User '{}' created (id: {}, role: {})",
                "✓".green().bold(),
                username,
                id,
                role
            );
            offer_save_to_1password(tty, save_to_1password, username, &password);
        }
        Err(e) => {
            eprintln!("{} Failed to create user: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth reset-password <username>`
pub fn cmd_auth_reset_password(tty: Tty, username: &str, save_to_1password: bool) {
    let db = open_db();
    let password = new_password(tty);

    match auth_queries::update_password(&db.conn, username, &password) {
        Ok(true) => {
            println!(
                "{} Password updated for '{}'. All tokens revoked.",
                "✓".green().bold(),
                username
            );
            offer_save_to_1password(tty, save_to_1password, username, &password);
        }
        Ok(false) => {
            eprintln!("{} User '{}' not found", "✗".red().bold(), username);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} Failed to update password: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth set-role <username> <role>`
pub fn cmd_auth_set_role(username: &str, role_str: &str) {
    let db = open_db();

    let role: Role = role_str.parse().unwrap_or_else(|_| {
        eprintln!(
            "{} Invalid role '{}'. Must be: admin, user, readonly",
            "✗".red().bold(),
            role_str
        );
        std::process::exit(1);
    });

    match auth_queries::update_role(&db.conn, username, role) {
        Ok(true) => {
            println!(
                "{} Role updated: '{}' is now {}",
                "✓".green().bold(),
                username,
                role
            );
        }
        Ok(false) => {
            eprintln!("{} User '{}' not found", "✗".red().bold(), username);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} Failed to update role: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// Check if `op` is available.
fn op_available() -> bool {
    std::process::Command::new("op")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Generate a secure random password (alphanumeric + symbols, 32 chars).
fn generate_password() -> String {
    use ring::rand::SecureRandom;
    let chars = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%&*-_=+";
    // Bytes at or above the largest multiple of the alphabet length are thrown
    // away, so every character is equally likely.
    let limit = 256 - 256 % chars.len();
    let rng = ring::rand::SystemRandom::new();
    let mut password = String::with_capacity(32);
    let mut buf = [0u8; 64];
    while password.len() < 32 {
        rng.fill(&mut buf).expect("system RNG failure");
        for &b in &buf {
            if (b as usize) < limit && password.len() < 32 {
                password.push(chars[b as usize % chars.len()] as char);
            }
        }
    }
    password
}

/// `KOAN_PASSWORD`, which takes precedence over asking.
fn env_password() -> Option<String> {
    std::env::var("KOAN_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty())
}

/// A password for a new account: `KOAN_PASSWORD`, or at a terminal one
/// generated or typed. Without either it fails, rather than generating a
/// password nobody sees.
fn new_password(tty: Tty) -> String {
    if let Some(pw) = env_password() {
        return pw;
    }
    if !tty.0 {
        fail("No password: set KOAN_PASSWORD.");
    }

    let hint = if op_available() {
        " (can be saved to 1Password)"
    } else {
        ""
    };
    if ask(&format!("Generate a secure password{hint}?")) {
        let pw = generate_password();
        println!("{} Generated password: {}", "✓".green().bold(), pw.bold());
        return pw;
    }

    // Manual entry.
    let password = prompt_password("Password: ");
    if password.is_empty() {
        eprintln!("{} Password cannot be empty", "✗".red().bold());
        std::process::exit(1);
    }
    let confirm = prompt_password("Confirm password: ");
    if password != confirm {
        eprintln!("{} Passwords do not match", "✗".red().bold());
        std::process::exit(1);
    }
    password
}

/// Save credentials to 1Password: without asking on `--save-to-1password`,
/// otherwise only when asked at a terminal and `op` is installed.
fn offer_save_to_1password(tty: Tty, save: bool, username: &str, password: &str) {
    if !save && !tty.0 {
        return;
    }
    if !op_available() {
        if save {
            eprintln!(
                "{} `op` is not available; not saved to 1Password",
                "!".yellow().bold()
            );
        }
        return;
    }

    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "localhost".into());

    if !save && !ask(&format!("Save to 1Password as 'koan@{hostname}'?")) {
        return;
    }

    let title = format!("koan@{}", hostname);

    // Check if an item with this title already exists.
    let existing = std::process::Command::new("op")
        .args(["item", "get", &title, "--format=json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());

    if existing.is_some() {
        if !save
            && !ask(&format!(
                "'{title}' already exists in 1Password. Update it?"
            ))
        {
            return;
        }

        let user_field = format!("username={}", username);
        let pass_field = format!("password={}", password);
        let status = std::process::Command::new("op")
            .args(["item", "edit", &title, &user_field, &pass_field])
            .stdout(std::process::Stdio::null())
            .status();

        match status {
            Ok(s) if s.success() => {
                println!("{} Updated '{}' in 1Password", "✓".green().bold(), title);
            }
            _ => {
                eprintln!("{} Failed to update in 1Password", "!".yellow().bold());
            }
        }
    } else {
        // Create new item — pipe template via stdin.
        let template = serde_json::json!({
            "title": title,
            "category": "LOGIN",
            "fields": [
                {"id": "username", "type": "STRING", "value": username, "purpose": "USERNAME"},
                {"id": "password", "type": "CONCEALED", "value": password, "purpose": "PASSWORD"}
            ],
            "urls": [{"primary": true, "href": "http://localhost:4000"}]
        });

        let mut child = match std::process::Command::new("op")
            .args(["item", "create", "--format=json"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(template.to_string().as_bytes());
        }

        match child.wait() {
            Ok(s) if s.success() => {
                println!("{} Saved to 1Password as '{}'", "✓".green().bold(), title);
            }
            _ => {
                eprintln!(
                    "{} Failed to save to 1Password (is `op` signed in?)",
                    "!".yellow().bold()
                );
            }
        }
    }
}

/// `koan auth delete-user <username>`
pub fn cmd_auth_delete_user(tty: Tty, username: &str, yes: bool) {
    let db = open_db();

    let user = match auth_queries::get_user_by_username(&db.conn, username) {
        Ok(Some(u)) => u,
        Ok(None) => {
            eprintln!("{} User '{}' not found", "✗".red().bold(), username);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} DB error: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };

    // Prevent deleting the last admin.
    if user.role == Role::Admin {
        let admin_count = auth_queries::admin_count(&db.conn).unwrap_or(0);
        if admin_count <= 1 {
            eprintln!("{} Cannot delete the last admin user", "✗".red().bold());
            std::process::exit(1);
        }
    }

    if !confirmed(tty, yes, &format!("Delete user '{username}'?")) {
        println!("Cancelled.");
        return;
    }

    // Revoke all tokens first.
    let _ = auth_queries::revoke_all_user_tokens(&db.conn, user.id);

    match auth_queries::delete_user(&db.conn, user.id) {
        Ok(true) => println!("{} User '{}' deleted", "✓".green().bold(), username),
        Ok(false) => eprintln!("{} User '{}' not found", "✗".red().bold(), username),
        Err(e) => {
            eprintln!("{} Failed to delete user: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth list-users`
pub fn cmd_auth_list_users() {
    let db = open_db();

    let users = auth_queries::list_users(&db.conn).unwrap_or_else(|e| {
        eprintln!("{} DB error: {}", "✗".red().bold(), e);
        std::process::exit(1);
    });

    if users.is_empty() {
        println!("No users. Run `koan auth setup` to create the first admin.");
        return;
    }

    println!(
        "{:<5} {:<20} {:<10} {}",
        "ID".bold(),
        "Username".bold(),
        "Role".bold(),
        "Created".bold()
    );
    for user in &users {
        println!(
            "{:<5} {:<20} {:<10} {}",
            user.id,
            user.username,
            user.role,
            user.created_at.as_deref().unwrap_or("-")
        );
    }
    println!("\n{} user(s)", users.len());
}

/// `koan auth api-key create --username <u> --name <n>`
pub fn cmd_auth_api_key_create(username: &str, name: &str) {
    let db = open_db();
    let name = name.trim();
    if name.is_empty() {
        eprintln!("{} Name cannot be empty", "✗".red().bold());
        std::process::exit(1);
    }
    let user = match auth_queries::get_user_by_username(&db.conn, username) {
        Ok(Some(user)) => user,
        Ok(None) => {
            eprintln!("{} No user '{}'", "✗".red().bold(), username);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} DB error: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };
    match api_keys::create_api_key(&db.conn, user.id, name) {
        Ok((id, key)) => {
            println!(
                "{} API key {} '{}' created for '{}' ({})",
                "✓".green().bold(),
                id,
                name,
                username,
                user.role
            );
            println!("\n  {}\n", key.bold());
            println!(
                "{}",
                "Shown once. Subsonic clients send it as apiKey=, with no username.".dimmed()
            );
        }
        Err(e) => {
            eprintln!("{} Failed to create key: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth api-key list [--username <u>]`
pub fn cmd_auth_api_key_list(username: Option<&str>) {
    let db = open_db();
    let user_id = username.map(|u| match auth_queries::get_user_by_username(&db.conn, u) {
        Ok(Some(user)) => user.id,
        Ok(None) => {
            eprintln!("{} No user '{}'", "✗".red().bold(), u);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} DB error: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    });
    let keys = api_keys::list_api_keys(&db.conn, user_id).unwrap_or_else(|e| {
        eprintln!("{} DB error: {}", "✗".red().bold(), e);
        std::process::exit(1);
    });
    if keys.is_empty() {
        println!("No API keys.");
        return;
    }
    let when = |secs: i64| {
        chrono::DateTime::from_timestamp(secs, 0)
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default()
    };
    println!(
        "{:<5} {:<20} {:<24} {:<17} {}",
        "ID".bold(),
        "Username".bold(),
        "Name".bold(),
        "Created".bold(),
        "Last used".bold()
    );
    for key in &keys {
        println!(
            "{:<5} {:<20} {:<24} {:<17} {}",
            key.id,
            key.username,
            key.name,
            when(key.created_at),
            key.last_used_at.map_or_else(|| "never".into(), when)
        );
    }
}

/// `koan auth api-key revoke <id>`
pub fn cmd_auth_api_key_revoke(id: i64) {
    let db = open_db();
    match api_keys::revoke_api_key(&db.conn, id, None) {
        Ok(true) => println!("{} API key {} revoked", "✓".green().bold(), id),
        Ok(false) => {
            eprintln!("{} No API key {}", "✗".red().bold(), id);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} DB error: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth login --server <url> --username <u>`
pub fn cmd_auth_login(tty: Tty, server_url: &str, username: &str) {
    // Said aloud at a terminal: a KOAN_PASSWORD left exported from an earlier
    // setup would otherwise sign in as nobody, with no hint why.
    let password = match env_password() {
        Some(pw) => {
            if tty.0 {
                eprintln!("{}", "Using KOAN_PASSWORD.".dimmed());
            }
            pw
        }
        None if tty.0 => prompt_password("Password: "),
        None => fail("No password: set KOAN_PASSWORD."),
    };
    if password.is_empty() {
        eprintln!("{} Password cannot be empty", "✗".red().bold());
        std::process::exit(1);
    }

    let url = format!("{}/auth/login", server_url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::new();
    let resp = match client
        .post(&url)
        .json(&serde_json::json!({
            "username": username,
            "password": password
        }))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} Connection failed: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        eprintln!("{} Login failed ({}): {}", "✗".red().bold(), status, body);
        std::process::exit(1);
    }

    let body: serde_json::Value = resp.json().unwrap_or_default();
    let refresh_token = body["refresh_token"].as_str().unwrap_or("");
    let access_token = body["access_token"].as_str().unwrap_or("");

    if refresh_token.is_empty() || access_token.is_empty() {
        eprintln!("{} Invalid response from server", "✗".red().bold());
        std::process::exit(1);
    }

    if let Err(e) = Config::persist(|cfg| {
        cfg.auth.server = server_url.to_string();
        cfg.auth.refresh_token = refresh_token.to_string();
    }) {
        eprintln!("{} Failed to store token: {}", "✗".red().bold(), e);
        eprintln!("Refresh token (save manually): {}", refresh_token);
    } else {
        println!(
            "{} Logged in. Token stored in config.local.toml.",
            "✓".green().bold()
        );
    }

    let role = body["user"]["role"].as_str().unwrap_or("unknown");
    println!(
        "  User: {} ({})",
        body["user"]["username"].as_str().unwrap_or(username),
        role
    );
}

/// `koan auth regenerate-keys` — delete and regenerate Ed25519 keypair.
/// All existing tokens are invalidated (signed by old key).
pub fn cmd_auth_regenerate_keys(tty: Tty, yes: bool) {
    if !confirmed(
        tty,
        yes,
        "This will invalidate ALL existing tokens. Continue?",
    ) {
        println!("Aborted.");
        return;
    }

    let dir = koan_core::auth::keypair_dir();
    let _ = std::fs::remove_file(dir.join("ed25519.pem"));
    let _ = std::fs::remove_file(dir.join("ed25519_pub.pem"));

    match koan_core::auth::generate_keypair() {
        Ok(_) => {
            println!(
                "{} New Ed25519 keypair generated. All users must re-login.",
                "✓".green().bold()
            );
        }
        Err(e) => {
            eprintln!("{} Failed to generate keypair: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// `koan auth reset` — delete all auth state (keys, users, tokens). Nuclear option.
pub fn cmd_auth_reset(tty: Tty, yes: bool) {
    if !confirmed(
        tty,
        yes,
        "This will delete ALL keys, users, and tokens. Continue?",
    ) {
        println!("Aborted.");
        return;
    }

    // Delete keypair.
    let dir = koan_core::auth::keypair_dir();
    let _ = std::fs::remove_dir_all(&dir);

    // Delete users and tokens from DB.
    let db = open_db();
    let _ = db.conn.execute("DELETE FROM refresh_tokens", []);
    let _ = db.conn.execute("DELETE FROM users", []);

    println!(
        "{} Auth state wiped. Run `koan auth setup` to start fresh.",
        "✓".green().bold()
    );
}

pub fn cmd_auth_logout(server_url: &str) {
    let cfg = Config::load().unwrap_or_default();
    let token_is_for_this_server =
        cfg.auth.server == server_url && !cfg.auth.refresh_token.is_empty();

    // Revoking at the server is what actually ends the session — dropping our
    // copy only stops this machine from using it.
    if token_is_for_this_server {
        let url = format!("{}/auth/logout", server_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::new();
        let _ = client
            .post(&url)
            .json(&serde_json::json!({ "refresh_token": cfg.auth.refresh_token }))
            .send();
    }

    if !token_is_for_this_server {
        println!("{} No stored token found.", "✓".green().bold());
        return;
    }

    match Config::persist(|cfg| {
        cfg.auth.server = String::new();
        cfg.auth.refresh_token = String::new();
    }) {
        Ok(()) => println!("{} Logged out. Token cleared.", "✓".green().bold()),
        Err(e) => eprintln!("{} Could not clear the token: {}", "✗".red().bold(), e),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn prompt(message: &str) -> String {
    print!("{}", message);
    stdout().flush().ok();
    let mut input = String::new();
    stdin().read_line(&mut input).ok();
    input.trim().to_string()
}

/// A yes-by-default question, for terminals only. Enter is yes; end of input
/// is no, so Ctrl-D cannot write to a vault.
fn ask(question: &str) -> bool {
    eprint!("{} {question} [Y/n] ", "?".cyan().bold());
    let mut input = String::new();
    matches!(stdin().read_line(&mut input), Ok(n) if n > 0)
        && !input.trim().eq_ignore_ascii_case("n")
}

fn prompt_password(message: &str) -> String {
    rpassword::prompt_password(message).unwrap_or_default()
}

/// `koan auth invite` — the invite link and email for an account.
pub fn cmd_auth_invite(username: &str, server: Option<&str>, reset: bool) {
    let configured = Config::load().ok().and_then(|c| c.sharing.public_url);
    let Some(server) = server
        .map(str::to_owned)
        .or(configured)
        .filter(|s| !s.trim().is_empty())
    else {
        eprintln!(
            "{} Pass --server, or set sharing.public_url, to the address clients reach this server at.",
            "✗".red().bold()
        );
        std::process::exit(1);
    };
    let db = open_db();
    let made = (|| -> Result<_, Box<dyn std::error::Error>> {
        let user = koan_core::invite::account(&db.conn, username)?;
        let password = if reset {
            Some(koan_core::invite::set_password(&db.conn, username, None)?)
        } else {
            None
        };
        let (private, _) = auth::load_or_generate_keypair()?;
        let token = koan_core::invite::mint_token(&db.conn, &private, user.id)?;
        Ok(koan_core::invite::Invite::with_token(
            &server,
            username,
            &token,
            password.as_deref(),
        ))
    })();
    let invite = made.unwrap_or_else(|e| {
        eprintln!("{} {e}", "✗".red().bold());
        std::process::exit(1);
    });
    println!("{}\n", invite.link());
    println!("{} {}\n", "Subject:".dimmed(), invite.email_subject());
    print!("{}", invite.email_text());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_password() {
        let a = generate_password();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_graphic()));
        assert_ne!(a, generate_password());
    }
}

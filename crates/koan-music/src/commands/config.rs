use koan_core::config;
use koan_core::db::connection::Database;
use owo_colors::OwoColorize;

pub fn cmd_config() {
    let base_path = config::config_file_path();
    let local_path = config::config_local_file_path();

    println!("{}", "sources".bold());
    if base_path.exists() {
        println!("  {} {}", "config:".cyan(), base_path.display());
    } else {
        println!(
            "  {} {} {}",
            "config:".cyan(),
            base_path.display(),
            "(not found)".red().dimmed()
        );
    }
    if local_path.exists() {
        println!("  {} {}", "config.local:".cyan(), local_path.display());
    } else {
        println!(
            "  {} {} {}",
            "config.local:".cyan(),
            local_path.display(),
            "(not found)".dimmed()
        );
    }
    println!("  {} {}", "db:".cyan(), config::db_path().display());
    println!();

    println!("{}", "resolved".bold());
    let cfg = config::Config::load().unwrap_or_default();
    match toml::to_string_pretty(&cfg) {
        Ok(s) => print!("{}", s),
        Err(e) => eprintln!("{} {}", "error:".red().bold(), e),
    }
}

pub fn cmd_init() {
    let dir = config::config_dir();
    let config_path = config::config_file_path();
    let local_path = config::config_local_file_path();
    let cache_dir = config::Config::default().cache_dir();

    // Create directories.
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }

    println!("{} {}", "dir".cyan(), dir.display());

    // Write default config.toml if it doesn't exist.
    if config_path.exists() {
        println!(
            "  {} {} {}",
            "config:".cyan(),
            config_path.display(),
            "(exists)".dimmed()
        );
    } else {
        let base_content = r#"# koan — shareable defaults (safe to commit to dotfiles)

[library]
watch = true

[playback]
exclusive_mode = false
software_volume = false
replaygain = "album"  # off | track | album

[organize]
default = "standard"

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
"#;
        if let Err(e) = std::fs::write(&config_path, base_content) {
            eprintln!("{} {}", "error:".red().bold(), e);
        } else {
            println!(
                "  {} {} {}",
                "config:".cyan(),
                config_path.display(),
                "created".green()
            );
        }
    }

    // Write config.local.toml if it doesn't exist.
    if local_path.exists() {
        println!(
            "  {} {} {}",
            "config.local:".cyan(),
            local_path.display(),
            "(exists)".dimmed()
        );
    } else {
        let local_content = r#"# koan — machine-specific overrides (gitignored)
# Edit the paths below, then run: koan scan

[library]
folders = ["/path/to/music"]

# Uncomment to connect a Navidrome/Subsonic server:
# (run `koan remote login URL username` instead for interactive setup)
#
# [remote]
# enabled = true
# url = "https://music.example.com"
# username = "admin"
# password = ""
"#;
        if let Err(e) = std::fs::write(&local_path, local_content) {
            eprintln!("{} {}", "error:".red().bold(), e);
        } else {
            println!(
                "  {} {} {}",
                "config.local:".cyan(),
                local_path.display(),
                "created".green()
            );
        }
    }

    // Ensure DB exists.
    let db_path = config::db_path();
    if db_path.exists() {
        println!(
            "  {} {} {}",
            "db:".cyan(),
            db_path.display(),
            "(exists)".dimmed()
        );
    } else {
        match Database::open_default() {
            Ok(_) => println!(
                "  {} {} {}",
                "db:".cyan(),
                db_path.display(),
                "created".green()
            ),
            Err(e) => eprintln!("{} {}", "error:".red().bold(), e),
        }
    }

    println!(
        "  {} {} {}",
        "cache:".cyan(),
        cache_dir.display(),
        "ready".green()
    );
    println!("  {} {}", "log:".cyan(), dir.join("koan.log").display());
}

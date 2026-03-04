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

    // Write or augment config.toml — merge defaults under existing keys so new
    // config options appear without overwriting user customizations.
    {
        let defaults = config::Config::default();
        let default_val: toml::Value =
            toml::Value::try_from(&defaults).expect("default config serializes");

        let already_exists = config_path.exists();
        let mut base_val: toml::Value = if already_exists {
            let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
            toml::from_str(&contents).unwrap_or_else(|_| default_val.clone())
        } else {
            toml::Value::Table(toml::map::Map::new())
        };

        // deep_merge(base, overlay) puts overlay keys INTO base.
        // We want existing keys to win, so merge defaults first then overlay existing.
        // Easier: swap — merge existing into defaults so defaults fill gaps.
        fn deep_merge_defaults(defaults: &mut toml::Value, existing: toml::Value) {
            match (defaults, existing) {
                (toml::Value::Table(def_map), toml::Value::Table(exist_map)) => {
                    for (key, exist_val) in exist_map {
                        if let Some(def_entry) = def_map.get_mut(&key) {
                            deep_merge_defaults(def_entry, exist_val);
                        } else {
                            def_map.insert(key, exist_val);
                        }
                    }
                }
                (slot, existing) => {
                    // Existing value wins over default.
                    *slot = existing;
                }
            }
        }

        let existing = base_val.clone();
        base_val = default_val;
        deep_merge_defaults(&mut base_val, existing);

        match toml::to_string_pretty(&base_val) {
            Ok(s) => {
                let header = "# koan — shareable defaults (safe to commit to dotfiles)\n\n";
                if let Err(e) = std::fs::write(&config_path, format!("{header}{s}")) {
                    eprintln!("{} {}", "error:".red().bold(), e);
                } else {
                    let action = if already_exists { "updated" } else { "created" };
                    println!(
                        "  {} {} {}",
                        "config:".cyan(),
                        config_path.display(),
                        action.green()
                    );
                }
            }
            Err(e) => eprintln!("{} {}", "error:".red().bold(), e),
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

    // Write .gitignore if it doesn't exist (keeps logs, db, and local config out of dotfile repos).
    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = "*.log\n*.db\nconfig.local.toml\ncache/\n";
        if let Err(e) = std::fs::write(&gitignore_path, gitignore_content) {
            eprintln!("{} {}", "error:".red().bold(), e);
        } else {
            println!(
                "  {} {} {}",
                ".gitignore:".cyan(),
                gitignore_path.display(),
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

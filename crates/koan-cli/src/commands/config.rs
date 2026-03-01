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
        let default_cfg = config::Config::default();
        match toml::to_string_pretty(&default_cfg) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&config_path, &content) {
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
        let local_content = "\
# Machine-specific overrides (gitignored)
# Uncomment and edit as needed.

# [library]
# folders = [\"/path/to/music\"]

# [remote]
# enabled = true
# url = \"https://music.example.com\"
# username = \"admin\"
# password = \"\"
";
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

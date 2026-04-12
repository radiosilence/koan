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

    // Show any active KOAN_* env var overrides.
    let env_overrides: Vec<_> = std::env::vars()
        .filter(|(k, _)| k.starts_with("KOAN_"))
        .collect();
    if !env_overrides.is_empty() {
        println!(
            "  {} {} active",
            "env:".cyan(),
            format!("{} KOAN_* vars", env_overrides.len()).green()
        );
        for (k, _) in &env_overrides {
            println!("    {}", k.dimmed());
        }
    }
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

    // Generate config.toml as a commented reference with user overrides uncommented.
    {
        let already_exists = config_path.exists();

        let existing_base: toml::map::Map<String, toml::Value> = if already_exists {
            let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
            toml::from_str::<toml::Value>(&contents)
                .ok()
                .and_then(|v| v.as_table().cloned())
                .unwrap_or_default()
        } else {
            toml::map::Map::new()
        };

        let output = generate_config_template(&existing_base);
        if let Err(e) = std::fs::write(&config_path, output) {
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

    // Write config.local.toml if it doesn't exist.
    if local_path.exists() {
        println!(
            "  {} {} {}",
            "config.local:".cyan(),
            local_path.display(),
            "(exists)".dimmed()
        );
    } else {
        let default_folders = config::Config::default().library.folders;
        let folders_str = default_folders
            .iter()
            .map(|p| format!("\"{}\"", p.display()))
            .collect::<Vec<_>>()
            .join(", ");
        let local_content = format!(
            r#"# koan — machine-specific overrides (gitignored)
# Edit the paths below, then run: koan scan

[library]
folders = [{folders_str}]

# Uncomment to connect a Navidrome/Subsonic server:
# (run `koan remote login URL username` instead for interactive setup)
#
# [remote]
# enabled = true
# url = "https://music.example.com"
# username = "admin"
# password = ""
"#
        );
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
        let gitignore_content = "*.log\n*.db\n*.db-wal\n*.db-shm\nconfig.local.toml\ncache/\n";
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

/// Generate config.toml content with all defaults commented out.
/// User's existing values stay uncommented. Keys already in config.local.toml are skipped.
/// Sections that belong in config.local.toml (library, remote) are excluded entirely.
fn generate_config_template(existing_base: &toml::map::Map<String, toml::Value>) -> String {
    let defaults = config::Config::default();
    let default_toml = toml::to_string_pretty(&defaults).expect("default config serializes");

    // Sections that should never appear in config.toml (machine-specific / sensitive).
    let skip_sections = ["library", "remote"];

    let mut output = String::from(
        "# koan — shareable defaults (safe to commit to dotfiles)\n\
         # Uncomment to customise. Run `koan config` to see resolved values.\n\n",
    );

    let mut current_section = String::new();
    let mut skip = false;
    let mut section_buf = String::new();
    let mut section_has_content = false;

    for line in default_toml.lines() {
        let trimmed = line.trim();

        // Section header: [section] or [section.sub]
        if trimmed.starts_with('[') {
            // Flush previous section if it had content.
            if section_has_content {
                output.push_str(&section_buf);
                output.push('\n');
            }
            section_buf.clear();
            section_has_content = false;

            let section = trimmed.trim_start_matches('[').trim_end_matches(']');
            let top_level = section.split('.').next().unwrap_or(section);
            skip = skip_sections.contains(&top_level);

            if !skip {
                current_section = section.to_string();
                section_buf.push_str(line);
                section_buf.push('\n');
            }
            continue;
        }

        if skip {
            continue;
        }

        // Empty line — skip (we control spacing ourselves).
        if trimmed.is_empty() {
            continue;
        }

        // key = value line.
        if let Some((key, default_val_str)) = trimmed.split_once(" = ") {
            let key = key.trim();

            let base_section = existing_base
                .get(&current_section)
                .and_then(|v| v.as_table());

            if let Some(user_val) = base_section.and_then(|t| t.get(key)) {
                // User has explicitly set this in config.toml — keep uncommented.
                section_buf.push_str(&format!("{} = {}\n", key, format_toml_value(user_val)));
            } else {
                // Default — commented out as reference.
                section_buf.push_str(&format!("# {} = {}\n", key, default_val_str));
            }
            section_has_content = true;
        }
    }

    // Flush last section.
    if section_has_content {
        output.push_str(&section_buf);
        output.push('\n');
    }

    // Preserve any user sections in config.toml that aren't in defaults.
    let default_val = toml::Value::try_from(&defaults).expect("default config serializes");
    let default_table = default_val.as_table().unwrap();
    for (section_name, section_val) in existing_base {
        if default_table.contains_key(section_name)
            || skip_sections.contains(&section_name.as_str())
        {
            continue;
        }
        if let Some(table) = section_val.as_table() {
            output.push_str(&format!("[{}]\n", section_name));
            for (key, value) in table {
                output.push_str(&format!("{} = {}\n", key, format_toml_value(value)));
            }
            output.push('\n');
        }
    }

    output
}

/// Format a TOML value for inline display in a config template.
fn format_toml_value(val: &toml::Value) -> String {
    match val {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => {
            if f.fract() == 0.0 {
                format!("{:.1}", f)
            } else {
                f.to_string()
            }
        }
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(a) => {
            let items: Vec<String> = a.iter().map(format_toml_value).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(_) => {
            // Inline tables — fallback to toml serialization.
            toml::to_string(val).unwrap_or_else(|_| "{}".into())
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}

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

/// Sections koan used to have. Preserved user sections are how a config
/// survives a koan that does not know about them yet, but that same rule would
/// keep a retired section alive forever once koan stopped reading it.
const RETIRED_SECTIONS: &[&str] = &["discovery"];

/// Generate config.toml content with all defaults commented out.
///
/// A value the user has changed stays uncommented; one that matches the default
/// goes back to being a comment, which is how a file an older koan filled with
/// its own serialisation shrinks back to a template.
///
/// Only shared settings are listed: `config::layer_of` decides, so the template
/// and koan's own writes can never disagree about what belongs here.
fn generate_config_template(existing_base: &toml::map::Map<String, toml::Value>) -> String {
    let defaults = config::Config::default();
    let default_toml = toml::to_string_pretty(&defaults).expect("default config serializes");

    let mut output = String::from(
        "# koan — shareable defaults (safe to commit to dotfiles)\n\
         # Uncomment to customise. Run `koan config` to see resolved values.\n\n",
    );

    // `skip_serializing_if` keys never appear in the default TOML the template
    // is generated from, so they have to be carried across by hand or the
    // patterns people wrote would be deleted by a command billed as safe.
    let held_back = |section: &str| -> String {
        let mut out = String::new();
        if section == "organize"
            && let Some(v) = existing_base
                .get("organize")
                .and_then(|s| s.as_table())
                .and_then(|t| t.get("default"))
        {
            out.push_str(&format!("default = {}\n", format_toml_value(v)));
        }
        out
    };

    let mut current_section = String::new();
    let mut section_buf = String::new();
    let mut section_has_content = false;

    for line in default_toml.lines() {
        let trimmed = line.trim();

        // Section header: [section] or [section.sub]
        if trimmed.starts_with('[') {
            // Flush previous section if it had content.
            let extra = held_back(&current_section);
            if section_has_content || !extra.is_empty() {
                section_buf.push_str(&extra);
                output.push_str(&section_buf);
                output.push('\n');
            }
            section_buf.clear();
            section_has_content = false;

            current_section = trimmed
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            section_buf.push_str(line);
            section_buf.push('\n');
            continue;
        }

        // Empty line — skip (we control spacing ourselves).
        if trimmed.is_empty() {
            continue;
        }

        // key = value line.
        if let Some((key, default_val_str)) = trimmed.split_once(" = ") {
            let key = key.trim();

            // Machine-scoped settings live in config.local.toml; listing them
            // here, even commented, invites them into a dotfiles repo.
            if config::layer_of(&format!("{}.{}", current_section, key)) == config::Layer::Machine {
                continue;
            }

            let base_section = existing_base
                .get(&current_section)
                .and_then(|v| v.as_table());

            let user_val = base_section
                .and_then(|t| t.get(key))
                .filter(|v| format_toml_value(v) != default_val_str);

            if let Some(user_val) = user_val {
                // Differs from the default — the user meant it. Keep it.
                section_buf.push_str(&format!("{} = {}\n", key, format_toml_value(user_val)));
            } else {
                // Default — commented out as reference.
                section_buf.push_str(&format!("# {} = {}\n", key, default_val_str));
            }
            section_has_content = true;
        }
    }

    // Flush last section.
    let extra = held_back(&current_section);
    if section_has_content || !extra.is_empty() {
        section_buf.push_str(&extra);
        output.push_str(&section_buf);
        output.push('\n');
    }

    // Named patterns are a sub-table, held back when empty, and the reason
    // people hand-edit this file at all.
    if let Some(patterns) = existing_base
        .get("organize")
        .and_then(|s| s.as_table())
        .and_then(|t| t.get("patterns"))
        .and_then(|v| v.as_table())
        .filter(|t| !t.is_empty())
    {
        output.push_str("[organize.patterns]\n");
        for (name, pattern) in patterns {
            output.push_str(&format!("{} = {}\n", name, format_toml_value(pattern)));
        }
        output.push('\n');
    }

    // Whole sections this koan does not know about are kept, so a config from a
    // newer build survives an older one. Unknown *keys* inside a section koan
    // does know are dropped: those are settings it used to have and no longer
    // reads, and carrying them would keep them alive forever.
    let default_val = toml::Value::try_from(&defaults).expect("default config serializes");
    let default_table = default_val.as_table().unwrap();
    for (section_name, section_val) in existing_base {
        if default_table.contains_key(section_name)
            || RETIRED_SECTIONS.contains(&section_name.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn template_from(existing: &str) -> String {
        let table = toml::from_str::<toml::Value>(existing)
            .unwrap()
            .as_table()
            .cloned()
            .unwrap();
        generate_config_template(&table)
    }

    /// The template is regenerated over the user's own file, so anything it
    /// drops is gone. It has to parse, too — emitting `[organize]` twice does
    /// not.
    #[test]
    fn the_generated_template_is_valid_toml() {
        let out = template_from(
            r#"
[organize]
default = "mine"

[organize.patterns]
mine = "%artist%/%title%"
"#,
        );
        let parsed: config::Config = toml::from_str(&out).expect("template must parse");
        assert_eq!(parsed.organize.default.as_deref(), Some("mine"));
        assert_eq!(parsed.organize.patterns["mine"], "%artist%/%title%");
    }

    /// Both are held back by `skip_serializing_if`, so they never appear in the
    /// default TOML the template is built from — and a hand-written pattern is
    /// the main reason anyone edits this file.
    #[test]
    fn hand_written_patterns_survive_regeneration() {
        let out = template_from(
            r#"
[organize.patterns]
va = "%album artist%/%album%/%title%"
"#,
        );
        assert!(out.contains("[organize.patterns]"), "{out}");
        assert!(
            out.contains(r#"va = "%album artist%/%album%/%title%""#),
            "{out}"
        );
    }

    /// koan wrote these itself when it serialised the whole struct. Left
    /// uncommented they make the file look hand-tuned when none of it is.
    #[test]
    fn values_matching_the_default_go_back_to_being_comments() {
        let out = template_from("[playback]\ntarget_fps = 60\nshow_fps = true\n");
        assert!(out.contains("# target_fps = 60"), "{out}");
        assert!(out.contains("\nshow_fps = true"), "{out}");
    }

    /// A setting koan no longer reads must not be carried forward, or it sits
    /// in the file looking load-bearing forever.
    #[test]
    fn retired_settings_are_not_resurrected() {
        let out = template_from(
            "[playback]\nticker_fps = 8\n\n[radio]\nuse_subsonic = true\n\n\
             [discovery]\nacoustic_weight = 0.5\n",
        );
        for gone in [
            "ticker_fps",
            "use_subsonic",
            "[discovery]",
            "acoustic_weight",
        ] {
            assert!(!out.contains(gone), "{gone} should be gone:\n{out}");
        }
    }

    /// Forward compatibility: an older koan regenerating a newer koan's config
    /// should not delete the section it does not understand.
    #[test]
    fn sections_this_build_does_not_know_are_kept() {
        let out = template_from("[dsp]\ncrossfeed = true\n");
        assert!(out.contains("[dsp]"), "{out}");
        assert!(out.contains("crossfeed = true"), "{out}");
    }

    /// Machine-scoped settings belong in config.local.toml; listing them here,
    /// even commented out, invites them into a dotfiles repo.
    #[test]
    fn machine_settings_never_reach_the_shared_template() {
        let out = template_from("[playback]\nart_size = 56\n\n[library]\nfolders = [\"/music\"]\n");
        for machine in ["art_size", "folders", "output_device", "[subsonic]"] {
            assert!(!out.contains(machine), "{machine} should be gone:\n{out}");
        }
    }
}

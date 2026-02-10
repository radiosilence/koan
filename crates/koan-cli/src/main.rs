use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use indicatif::{ProgressBar, ProgressStyle};
use koan_core::audio::{buffer, device};
use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, SharedPlayerState};

#[derive(Parser)]
#[command(name = "koan", about = "bit-perfect music player", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Play audio file(s)
    Play {
        /// Paths to audio files
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Probe a file and show format info
    Probe {
        /// Path to audio file
        path: PathBuf,
    },
    /// List available audio output devices
    Devices,
    /// Scan a folder for audio files and index them
    Scan {
        /// Path to scan (defaults to configured library folders)
        path: Option<PathBuf>,
        /// Force re-scan of all files
        #[arg(long)]
        force: bool,
    },
    /// Search the library
    Search {
        /// Search query
        query: String,
    },
    /// Show library statistics
    Library,
    /// Show or manage configuration
    Config,
    /// Manage remote Subsonic/Navidrome server
    #[command(subcommand)]
    Remote(RemoteCommands),
    /// Generate shell completions
    Completions {
        /// Shell to generate for
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum RemoteCommands {
    /// Log in to a Subsonic/Navidrome server
    Login {
        /// Server URL (e.g. https://navidrome.example.com)
        url: String,
        /// Username
        username: String,
    },
    /// Sync remote library to local database
    Sync,
    /// Show remote server status
    Status,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Play { paths } => cmd_play(&paths),
        Commands::Probe { path } => cmd_probe(&path),
        Commands::Devices => cmd_devices(),
        Commands::Scan { path, force } => cmd_scan(path.as_deref(), force),
        Commands::Search { query } => cmd_search(&query),
        Commands::Library => cmd_library(),
        Commands::Config => cmd_config(),
        Commands::Remote(sub) => match sub {
            RemoteCommands::Login { url, username } => cmd_remote_login(&url, &username),
            RemoteCommands::Sync => cmd_remote_sync(),
            RemoteCommands::Status => cmd_remote_status(),
        },
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "koan", &mut io::stdout());
        }
    }
}

// --- Playback ---

enum Event {
    Key(u8),
    Tick,
}

fn cmd_play(paths: &[PathBuf]) {
    for path in paths {
        if !path.exists() {
            eprintln!("file not found: {}", path.display());
            std::process::exit(1);
        }
    }

    let (state, tx) = Player::spawn();

    tx.send(PlayerCommand::PlayQueue(paths.to_vec()))
        .expect("player thread died");

    wait_for_playing(&state);

    println!("controls: [space] pause/resume  [</>] seek 10s  [n] next  [q] quit\n");

    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{prefix} {bar:40.cyan/dim} {msg}")
            .unwrap()
            .progress_chars("━╸─"),
    );

    let quit = Arc::new(AtomicBool::new(false));
    let (ev_tx, ev_rx) = crossbeam_channel::unbounded::<Event>();

    let ev_tx_keys = ev_tx.clone();
    let quit_input = quit.clone();
    std::thread::Builder::new()
        .name("koan-input".into())
        .spawn(move || {
            let _raw = RawModeGuard::enter();
            let stdin = io::stdin();
            let mut handle = stdin.lock();
            let mut buf = [0u8; 1];
            while !quit_input.load(Ordering::Relaxed) {
                match handle.read(&mut buf) {
                    Ok(1) => {
                        if ev_tx_keys.send(Event::Key(buf[0])).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        })
        .expect("failed to spawn input thread");

    let ev_tx_tick = ev_tx;
    let quit_tick = quit.clone();
    std::thread::Builder::new()
        .name("koan-tick".into())
        .spawn(move || {
            while !quit_tick.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                if ev_tx_tick.send(Event::Tick).is_err() {
                    break;
                }
            }
        })
        .expect("failed to spawn tick thread");

    let mut current_track: Option<PathBuf> = None;
    update_progress_bar(&pb, &state, &mut current_track);

    while let Ok(event) = ev_rx.recv() {
        match event {
            Event::Tick => {
                update_progress_bar(&pb, &state, &mut current_track);
                if state.playback_state() == PlaybackState::Stopped
                    && state.track_info().is_none()
                    && current_track.is_some()
                {
                    pb.finish_and_clear();
                    println!("done.");
                    quit.store(true, Ordering::Relaxed);
                    break;
                }
            }
            Event::Key(byte) => match byte {
                b'q' | 3 => {
                    tx.send(PlayerCommand::Stop).ok();
                    pb.finish_and_clear();
                    println!("stopped.");
                    quit.store(true, Ordering::Relaxed);
                    break;
                }
                b'n' => {
                    tx.send(PlayerCommand::NextTrack).ok();
                }
                b' ' => {
                    if state.playback_state() == PlaybackState::Playing {
                        tx.send(PlayerCommand::Pause).ok();
                    } else {
                        tx.send(PlayerCommand::Resume).ok();
                    }
                }
                b',' | b'.' => {
                    let pos = state.position_ms();
                    let new_pos = if byte == b'.' {
                        pos.saturating_add(10_000)
                    } else {
                        pos.saturating_sub(10_000)
                    };
                    tx.send(PlayerCommand::Seek(new_pos)).ok();
                }
                0x1b => {
                    if let (Ok(Event::Key(b'[')), Ok(Event::Key(arrow))) =
                        (ev_rx.recv(), ev_rx.recv())
                    {
                        let pos = state.position_ms();
                        match arrow {
                            b'C' => {
                                tx.send(PlayerCommand::Seek(pos.saturating_add(10_000)))
                                    .ok();
                            }
                            b'D' => {
                                tx.send(PlayerCommand::Seek(pos.saturating_sub(10_000)))
                                    .ok();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
        }
    }

    std::thread::sleep(Duration::from_millis(100));
}

fn update_progress_bar(
    pb: &ProgressBar,
    state: &Arc<SharedPlayerState>,
    current_track: &mut Option<PathBuf>,
) {
    let Some(info) = state.track_info() else {
        return;
    };

    if current_track.as_ref() != Some(&info.path) {
        pb.println(format!(
            "\n{}",
            info.path.file_name().unwrap_or_default().to_string_lossy()
        ));
        pb.println(format!(
            "  {} | {}Hz | {}bit | {}ch",
            info.codec, info.sample_rate, info.bit_depth, info.channels,
        ));
        pb.set_length(info.duration_ms);
        *current_track = Some(info.path.clone());
    }

    let pos = state.position_ms();
    let status = match state.playback_state() {
        PlaybackState::Playing => "▶",
        PlaybackState::Paused => "⏸",
        PlaybackState::Stopped => "■",
    };

    pb.set_prefix(status.to_string());
    pb.set_position(pos);
    pb.set_message(format!(
        "{}/{}",
        format_time(pos),
        format_time(info.duration_ms)
    ));
}

fn wait_for_playing(state: &Arc<SharedPlayerState>) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(10));
        if state.playback_state() == PlaybackState::Playing {
            return;
        }
    }
    eprintln!("playback failed to start");
}

// --- Library ---

fn cmd_scan(path: Option<&Path>, force: bool) {
    let db = open_db();

    let folders: Vec<PathBuf> = if let Some(p) = path {
        vec![p.to_path_buf()]
    } else {
        let cfg = config::Config::load().unwrap_or_default();
        cfg.library.folders
    };

    if folders.is_empty() {
        eprintln!("no folders to scan — pass a path or configure library.folders");
        std::process::exit(1);
    }

    let start = std::time::Instant::now();
    let result = koan_core::index::scanner::full_scan(&db, &folders, force);
    let elapsed = start.elapsed();

    println!(
        "scan complete in {:.1}s: {} added, {} updated, {} removed, {} skipped",
        elapsed.as_secs_f64(),
        result.added,
        result.updated,
        result.removed,
        result.skipped
    );

    if !result.errors.is_empty() {
        println!("{} errors:", result.errors.len());
        for (path, err) in result.errors.iter().take(10) {
            println!("  {} — {}", path.display(), err);
        }
        if result.errors.len() > 10 {
            println!("  ... and {} more", result.errors.len() - 10);
        }
    }
}

fn cmd_search(query: &str) {
    let db = open_db();
    match queries::search_tracks(&db.conn, query) {
        Ok(tracks) => {
            if tracks.is_empty() {
                println!("no results for \"{}\"", query);
                return;
            }
            println!("{} results for \"{}\":\n", tracks.len(), query);
            for t in &tracks {
                let source_tag = match t.source.as_str() {
                    "remote" => " [remote]",
                    "cached" => " [cached]",
                    _ => "",
                };
                println!(
                    "  {} - {} - {}{}",
                    t.artist_name, t.album_title, t.title, source_tag
                );
                if let Some(ref codec) = t.codec {
                    let rate = t
                        .sample_rate
                        .map(|r| format!(" {}Hz", r))
                        .unwrap_or_default();
                    let depth = t
                        .bit_depth
                        .map(|b| format!("/{}bit", b))
                        .unwrap_or_default();
                    let dur = t
                        .duration_ms
                        .map(|d| format!(" {}", format_time(d as u64)))
                        .unwrap_or_default();
                    println!("    {}{}{}{}", codec, rate, depth, dur);
                }
            }
        }
        Err(e) => {
            eprintln!("search failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_library() {
    let db = open_db();
    match queries::library_stats(&db.conn) {
        Ok(stats) => {
            println!("library:");
            println!("  tracks:  {} total", stats.total_tracks);
            println!("    local:  {}", stats.local_tracks);
            println!("    remote: {}", stats.remote_tracks);
            println!("    cached: {}", stats.cached_tracks);
            println!("  albums:  {}", stats.total_albums);
            println!("  artists: {}", stats.total_artists);
            println!("\ndb: {}", config::db_path().display());
        }
        Err(e) => {
            eprintln!("failed to get library stats: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_config() {
    let cfg = config::Config::load().unwrap_or_default();
    println!("config: {}", config::config_file_path().display());
    println!("data:   {}", config::data_dir().display());
    println!("db:     {}", config::db_path().display());
    println!();
    match toml::to_string_pretty(&cfg) {
        Ok(s) => print!("{}", s),
        Err(e) => eprintln!("failed to serialize config: {}", e),
    }
}

// --- Remote ---

fn cmd_remote_login(url: &str, username: &str) {
    let password = rpassword::prompt_password("password: ").unwrap_or_else(|e| {
        eprintln!("failed to read password: {}", e);
        std::process::exit(1);
    });

    // Test connection.
    let client = koan_core::remote::client::SubsonicClient::new(url, username, &password);
    match client.ping() {
        Ok(()) => println!("connected to {}", url),
        Err(e) => {
            eprintln!("connection failed: {}", e);
            std::process::exit(1);
        }
    }

    // Store credentials.
    if let Err(e) = koan_core::credentials::store_password(url, &password) {
        eprintln!("failed to store password in Keychain: {}", e);
        std::process::exit(1);
    }
    println!("password stored in Keychain");

    // Update config.
    let mut cfg = config::Config::load().unwrap_or_default();
    cfg.remote.enabled = true;
    cfg.remote.url = url.to_string();
    cfg.remote.username = username.to_string();
    if let Err(e) = cfg.save() {
        eprintln!("failed to save config: {}", e);
        std::process::exit(1);
    }
    println!("config updated");
}

fn cmd_remote_sync() {
    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        eprintln!("no remote server configured — run `koan remote login` first");
        std::process::exit(1);
    }

    let password = match koan_core::credentials::get_password(&cfg.remote.url) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to get password from Keychain: {}", e);
            eprintln!("run `koan remote login` to re-authenticate");
            std::process::exit(1);
        }
    };

    let client = koan_core::remote::client::SubsonicClient::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    );

    let db = open_db();
    let start = std::time::Instant::now();

    match koan_core::remote::sync::sync_library(&db, &client) {
        Ok(result) => {
            let elapsed = start.elapsed();
            println!(
                "sync complete in {:.1}s: {} artists, {} albums, {} tracks, {} matched to local",
                elapsed.as_secs_f64(),
                result.artists_synced,
                result.albums_synced,
                result.tracks_synced,
                result.matched_local
            );
        }
        Err(e) => {
            eprintln!("sync failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_remote_status() {
    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        println!("no remote server configured");
        return;
    }

    println!("server:   {}", cfg.remote.url);
    println!("username: {}", cfg.remote.username);

    let has_password = koan_core::credentials::get_password(&cfg.remote.url).is_ok();
    println!(
        "password: {}",
        if has_password {
            "stored in Keychain"
        } else {
            "not found"
        }
    );

    if has_password {
        let password = koan_core::credentials::get_password(&cfg.remote.url).unwrap();
        let client = koan_core::remote::client::SubsonicClient::new(
            &cfg.remote.url,
            &cfg.remote.username,
            &password,
        );
        match client.ping() {
            Ok(()) => println!("status:   connected"),
            Err(e) => println!("status:   error — {}", e),
        }
    }
}

// --- Info ---

fn cmd_probe(path: &Path) {
    if !path.exists() {
        eprintln!("file not found: {}", path.display());
        std::process::exit(1);
    }

    match buffer::probe_file(path) {
        Ok(info) => {
            println!("file:        {}", path.display());
            println!("codec:       {}", info.codec);
            println!("sample rate: {} Hz", info.sample_rate);
            println!("bit depth:   {}", info.bit_depth);
            println!("channels:    {}", info.channels);
            println!(
                "duration:    {} ({}ms)",
                format_time(info.duration_ms),
                info.duration_ms
            );
        }
        Err(e) => {
            eprintln!("probe failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_devices() {
    match device::list_output_devices() {
        Ok(devices) => {
            let default_id = device::default_output_device().ok();
            for dev in &devices {
                let marker = if Some(dev.id) == default_id { " *" } else { "" };
                println!("[{}]{} {}", dev.id, marker, dev.name);
                if !dev.sample_rates.is_empty() {
                    let rates: Vec<String> = dev
                        .sample_rates
                        .iter()
                        .map(|r| format!("{}Hz", *r as u32))
                        .collect();
                    println!("  rates: {}", rates.join(", "));
                }
            }
        }
        Err(e) => {
            eprintln!("failed to list devices: {}", e);
            std::process::exit(1);
        }
    }
}

// --- Helpers ---

fn open_db() -> Database {
    Database::open_default().unwrap_or_else(|e| {
        eprintln!("failed to open database: {}", e);
        std::process::exit(1);
    })
}

fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let secs = secs % 60;
    format!("{}:{:02}", mins, secs)
}

// --- Raw mode RAII guard ---

struct RawModeGuard {
    original: libc::termios,
}

impl RawModeGuard {
    fn enter() -> Self {
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            libc::tcgetattr(libc::STDIN_FILENO, &mut original);

            let mut raw = original;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw);

            Self { original }
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
        }
    }
}

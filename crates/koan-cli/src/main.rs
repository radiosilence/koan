use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::env::CompleteEnv;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use koan_core::audio::{buffer, device};
use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::Player;

// --- MultiProgress-aware logger ---
// Routes log output through mp.println() during playback so it doesn't stomp the progress bars.
// Falls back to stderr when no MultiProgress is registered.

static LOGGER: OnceLock<MpLogger> = OnceLock::new();

struct MpLogger {
    mp: Mutex<Option<Arc<MultiProgress>>>,
}

impl MpLogger {
    fn init() {
        let logger = LOGGER.get_or_init(|| MpLogger {
            mp: Mutex::new(None),
        });
        log::set_logger(logger).expect("failed to set logger");
        log::set_max_level(log::LevelFilter::Info);
    }

    fn set_mp(mp: Arc<MultiProgress>) {
        if let Some(logger) = LOGGER.get() {
            *logger.mp.lock().unwrap() = Some(mp);
        }
    }

    fn clear_mp() {
        if let Some(logger) = LOGGER.get() {
            *logger.mp.lock().unwrap() = None;
        }
    }
}

impl log::Log for MpLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let msg = format!(
            "{}: {}",
            record.level().as_str().to_lowercase(),
            record.args()
        );
        if let Some(mp) = self.mp.lock().unwrap().as_ref() {
            mp.println(&msg).ok();
        } else {
            eprintln!("{}", msg);
        }
    }

    fn flush(&self) {}
}
mod picker;

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, SharedPlayerState};
use owo_colors::OwoColorize;
use picker::{Picker, PickerItem, PickerKey, PickerResult};

#[derive(Parser)]
#[command(name = "koan", about = "bit-perfect music player", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Play audio file(s), tracks by ID, or an album/artist
    Play {
        /// Paths to audio files
        paths: Vec<PathBuf>,
        /// Track IDs from the library database
        #[arg(long = "id", num_args = 1..)]
        ids: Vec<i64>,
        /// Play an album by ID (from `koan albums`)
        #[arg(long, add = ArgValueCandidates::new(complete_albums))]
        album: Option<i64>,
        /// Play all tracks by an artist ID (from `koan artists`)
        #[arg(long, add = ArgValueCandidates::new(complete_artists))]
        artist: Option<i64>,
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
    /// List artists (optionally filter by name)
    Artists {
        /// Filter by name
        query: Option<String>,
    },
    /// List albums (optionally for an artist)
    Albums {
        /// Artist name to filter by
        query: Option<String>,
    },
    /// Show library statistics
    Library,
    /// Show or manage configuration
    Config,
    /// Manage remote Subsonic/Navidrome server
    #[command(subcommand)]
    Remote(RemoteCommands),
    /// Interactive fuzzy picker (requires fzf)
    Pick {
        /// Optional search query to pre-filter
        query: Option<String>,
        /// Pick an album to play
        #[arg(long)]
        album: bool,
        /// Pick an artist to play
        #[arg(long)]
        artist: bool,
    },
    /// Manage the download cache
    #[command(subcommand)]
    Cache(CacheCommands),
    /// Generate shell completions (legacy static)
    Completions {
        /// Shell to generate for
        shell: clap_complete::Shell,
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

#[derive(Subcommand)]
enum CacheCommands {
    /// Show cache size and location
    Status,
    /// Clear all cached downloads
    Clear,
}

fn main() {
    // Ensure Ctrl+C kills the process immediately, even during blocking I/O.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }

    // Dynamic shell completions — handles COMPLETE=zsh/bash/fish env var.
    CompleteEnv::with_factory(Cli::command).complete();

    MpLogger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Play {
            paths,
            ids,
            album,
            artist,
        } => cmd_play(&paths, &ids, album, artist),
        Commands::Probe { path } => cmd_probe(&path),
        Commands::Devices => cmd_devices(),
        Commands::Scan { path, force } => cmd_scan(path.as_deref(), force),
        Commands::Search { query } => cmd_search(&query),
        Commands::Artists { query } => cmd_artists(query.as_deref()),
        Commands::Albums { query } => cmd_albums(query.as_deref()),
        Commands::Library => cmd_library(),
        Commands::Config => cmd_config(),
        Commands::Remote(sub) => match sub {
            RemoteCommands::Login { url, username } => cmd_remote_login(&url, &username),
            RemoteCommands::Sync => cmd_remote_sync(),
            RemoteCommands::Status => cmd_remote_status(),
        },
        Commands::Pick {
            query,
            album,
            artist,
        } => cmd_pick(query.as_deref(), album, artist),
        Commands::Cache(sub) => match sub {
            CacheCommands::Status => cmd_cache_status(),
            CacheCommands::Clear => cmd_cache_clear(),
        },
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "koan", &mut io::stdout());
        }
    }
}

// --- Dynamic completions ---

fn complete_artists() -> Vec<CompletionCandidate> {
    let Ok(db) = Database::open_default() else {
        return vec![];
    };
    let Ok(artists) = queries::all_artists(&db.conn) else {
        return vec![];
    };
    artists
        .into_iter()
        .map(|a| CompletionCandidate::new(a.id.to_string()).help(Some(a.name.into())))
        .collect()
}

fn complete_albums() -> Vec<CompletionCandidate> {
    let Ok(db) = Database::open_default() else {
        return vec![];
    };
    let Ok(albums) = queries::all_albums(&db.conn) else {
        return vec![];
    };
    albums
        .into_iter()
        .map(|a| {
            let label = format!("{} — {}", a.artist_name, a.title);
            CompletionCandidate::new(a.id.to_string()).help(Some(label.into()))
        })
        .collect()
}

// --- Playback ---

enum Event {
    Key(u8),
    Tick,
}

fn cmd_play(paths: &[PathBuf], ids: &[i64], album: Option<i64>, artist: Option<i64>) {
    // Gather track IDs to resolve, or raw file paths.
    let track_ids: Option<Vec<i64>> = if let Some(album_id) = album {
        let db = open_db();
        let tracks = queries::tracks_for_album(&db.conn, album_id).unwrap_or_else(|e| {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        });
        if tracks.is_empty() {
            eprintln!("no tracks found for album {}", album_id);
            std::process::exit(1);
        }
        Some(tracks.iter().map(|t| t.id).collect())
    } else if let Some(artist_id) = artist {
        let db = open_db();
        let tracks = queries::tracks_for_artist(&db.conn, artist_id).unwrap_or_else(|e| {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        });
        if tracks.is_empty() {
            eprintln!("no tracks found for artist {}", artist_id);
            std::process::exit(1);
        }
        Some(tracks.iter().map(|t| t.id).collect())
    } else if !ids.is_empty() {
        Some(ids.to_vec())
    } else {
        None
    };

    // MultiProgress owns all bars — downloads + playback render cleanly together.
    let mp = Arc::new(MultiProgress::new());
    MpLogger::set_mp(mp.clone());
    let (state, tx) = Player::spawn();

    if let Some(ids) = track_ids {
        // Resolve first track immediately, start playback, download rest in background.
        let first_path = resolve_single_track(ids[0], Some(&mp));
        tx.send(PlayerCommand::Play(first_path))
            .expect("player thread died");

        if ids.len() > 1 {
            let remaining = ids[1..].to_vec();
            let tx_bg = tx.clone();
            let mp_bg = mp.clone();
            std::thread::Builder::new()
                .name("koan-resolve".into())
                .spawn(move || {
                    use rayon::prelude::*;
                    let resolved: Vec<PathBuf> = remaining
                        .par_iter()
                        .map(|&id| resolve_single_track(id, Some(&mp_bg)))
                        .collect();
                    for path in resolved {
                        if tx_bg.send(PlayerCommand::Enqueue(path)).is_err() {
                            break;
                        }
                    }
                })
                .expect("failed to spawn resolve thread");
        }
    } else {
        // Raw file paths — no resolution needed.
        if paths.is_empty() {
            eprintln!("provide file paths, --id, --album, or --artist");
            std::process::exit(1);
        }
        for path in paths {
            if !path.exists() {
                eprintln!("{} {}", "not found:".red().bold(), path.display());
                std::process::exit(1);
            }
        }
        tx.send(PlayerCommand::PlayQueue(paths.to_vec()))
            .expect("player thread died");
    }

    wait_for_playing(&state);

    let mut pb = make_playback_bar(&mp);
    let mut controls = make_controls_bar(&mp);

    let quit = Arc::new(AtomicBool::new(false));
    let picking = Arc::new(AtomicBool::new(false));
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
    let picking_tick = picking.clone();
    std::thread::Builder::new()
        .name("koan-tick".into())
        .spawn(move || {
            while !quit_tick.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                if !picking_tick.load(Ordering::Relaxed) && ev_tx_tick.send(Event::Tick).is_err() {
                    break;
                }
            }
        })
        .expect("failed to spawn tick thread");

    let mut current_track: Option<PathBuf> = None;
    update_progress_bar(&mp, &pb, &state, &mut current_track);

    while let Ok(event) = ev_rx.recv() {
        match event {
            Event::Tick => {
                update_progress_bar(&mp, &pb, &state, &mut current_track);
                if state.playback_state() == PlaybackState::Stopped
                    && state.track_info().is_none()
                    && current_track.is_some()
                {
                    pb.finish_and_clear();
                    controls.finish_and_clear();
                    mp.println(format!("{}", "done.".dimmed())).ok();
                    quit.store(true, Ordering::Relaxed);
                    break;
                }
            }
            Event::Key(byte) => match byte {
                b'q' | 3 => {
                    tx.send(PlayerCommand::Stop).ok();
                    pb.finish_and_clear();
                    controls.finish_and_clear();
                    mp.println(format!("{}", "stopped.".dimmed())).ok();
                    quit.store(true, Ordering::Relaxed);
                    break;
                }
                b'n' | b'>' => {
                    tx.send(PlayerCommand::NextTrack).ok();
                }
                b'<' => {
                    tx.send(PlayerCommand::PrevTrack).ok();
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
                b'p' | b'a' | b'r' => {
                    // Hide bars, run in-process picker, restore bars.
                    // Playback continues uninterrupted.
                    picking.store(true, Ordering::Relaxed);
                    pb.finish_and_clear();
                    controls.finish_and_clear();

                    let ids = match byte {
                        b'p' => inline_pick_tracks(&ev_rx),
                        b'a' => inline_pick_album(&ev_rx),
                        b'r' => inline_pick_artist(&ev_rx),
                        _ => unreachable!(),
                    };

                    picking.store(false, Ordering::Relaxed);
                    current_track = None;
                    pb = make_playback_bar(&mp);
                    controls = make_controls_bar(&mp);
                    update_progress_bar(&mp, &pb, &state, &mut current_track);

                    // Resolve and enqueue selected tracks in background.
                    if !ids.is_empty() {
                        let count = ids.len();
                        let tx_bg = tx.clone();
                        let mp_bg = mp.clone();
                        std::thread::Builder::new()
                            .name("koan-enqueue".into())
                            .spawn(move || {
                                for id in ids {
                                    let path = resolve_single_track(id, Some(&mp_bg));
                                    if tx_bg.send(PlayerCommand::Enqueue(path)).is_err() {
                                        break;
                                    }
                                }
                                mp_bg
                                    .println(format!(
                                        "{} {} track{} queued",
                                        "✓".green(),
                                        count,
                                        if count == 1 { "" } else { "s" },
                                    ))
                                    .ok();
                            })
                            .ok();
                    }
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

    MpLogger::clear_mp();
    std::thread::sleep(Duration::from_millis(100));
}

fn make_playback_bar(mp: &MultiProgress) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(0));
    pb.set_style(
        ProgressStyle::with_template("{prefix} {bar:40.cyan/dim} {msg}")
            .unwrap()
            .progress_chars("━╸─"),
    );
    pb
}

fn make_controls_bar(mp: &MultiProgress) -> ProgressBar {
    let bar = mp.add(ProgressBar::new(0));
    bar.set_style(ProgressStyle::with_template("{msg}").unwrap());
    bar.set_message(format!(
        "{}  {}  {}  {}  {}",
        "[space]".dimmed(),
        "[< >] skip".dimmed(),
        "[,/.] seek".dimmed(),
        "[p]track [a]lbum [r]artist".dimmed(),
        "[q] quit".dimmed(),
    ));
    bar
}

// --- Inline pickers (during playback) ---
// These read key events from the playback event channel, so playback continues uninterrupted.

fn make_event_key_reader<'a>(
    ev_rx: &'a crossbeam_channel::Receiver<Event>,
) -> impl FnMut() -> Option<PickerKey> + 'a {
    move || loop {
        match ev_rx.recv() {
            Ok(Event::Key(byte)) => {
                let key = picker::parse_key(byte, &mut || {
                    // Read more bytes for escape sequences.
                    match ev_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Event::Key(b)) => Some(b),
                        _ => None,
                    }
                });
                if let Some(k) = key {
                    return Some(k);
                }
            }
            Ok(Event::Tick) => continue, // ignore ticks during picking
            Err(_) => return None,
        }
    }
}

fn inline_pick_tracks(ev_rx: &crossbeam_channel::Receiver<Event>) -> Vec<i64> {
    let db = open_db();
    let tracks = queries::all_tracks(&db.conn).unwrap_or_default();
    if tracks.is_empty() {
        return vec![];
    }

    let items = make_track_picker_items(&tracks);
    let picker = Picker::new(items, "enqueue>", true);
    let mut reader = make_event_key_reader(ev_rx);
    match picker.run(&mut reader) {
        PickerResult::Selected(ids) => ids,
        PickerResult::Cancelled => vec![],
    }
}

fn inline_pick_album(ev_rx: &crossbeam_channel::Receiver<Event>) -> Vec<i64> {
    let db = open_db();
    let albums = queries::all_albums(&db.conn).unwrap_or_default();
    if albums.is_empty() {
        return vec![];
    }

    let items = make_album_picker_items(&albums);
    let picker = Picker::new(items, "album>", false);
    let mut reader = make_event_key_reader(ev_rx);
    match picker.run(&mut reader) {
        PickerResult::Selected(ids) => {
            let album_id = ids[0];
            queries::tracks_for_album(&db.conn, album_id)
                .unwrap_or_default()
                .iter()
                .map(|t| t.id)
                .collect()
        }
        PickerResult::Cancelled => vec![],
    }
}

fn inline_pick_artist(ev_rx: &crossbeam_channel::Receiver<Event>) -> Vec<i64> {
    let db = open_db();
    let artists = queries::all_artists(&db.conn).unwrap_or_default();
    if artists.is_empty() {
        return vec![];
    }

    let items = make_artist_picker_items(&artists);
    let picker = Picker::new(items, "artist>", false);
    let mut reader = make_event_key_reader(ev_rx);
    let artist_id = match picker.run(&mut reader) {
        PickerResult::Selected(ids) => ids[0],
        PickerResult::Cancelled => return vec![],
    };

    // Drill down: pick album for this artist.
    let albums = queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();
    if albums.is_empty() {
        return queries::tracks_for_artist(&db.conn, artist_id)
            .unwrap_or_default()
            .iter()
            .map(|t| t.id)
            .collect();
    }

    // Build album picker with "all tracks" option.
    let mut items = vec![PickerItem {
        id: -1,
        display: format!("{}", "all tracks".bold()),
        match_text: "all tracks".into(),
    }];
    items.extend(make_album_picker_items(&albums));

    let picker = Picker::new(items, "album>", false);
    let mut reader = make_event_key_reader(ev_rx);
    match picker.run(&mut reader) {
        PickerResult::Selected(ids) if ids[0] == -1 => {
            queries::tracks_for_artist(&db.conn, artist_id)
                .unwrap_or_default()
                .iter()
                .map(|t| t.id)
                .collect()
        }
        PickerResult::Selected(ids) => queries::tracks_for_album(&db.conn, ids[0])
            .unwrap_or_default()
            .iter()
            .map(|t| t.id)
            .collect(),
        PickerResult::Cancelled => vec![],
    }
}

// --- PickerItem builders ---

fn make_track_picker_items(tracks: &[queries::TrackRow]) -> Vec<PickerItem> {
    tracks
        .iter()
        .map(|t| {
            let dur = t
                .duration_ms
                .map(|d| format_time(d as u64))
                .unwrap_or_default();
            let track_num = match (t.disc, t.track_number) {
                (Some(d), Some(n)) if d > 1 => format!("{}.{:02}", d, n),
                (_, Some(n)) => format!("{:02}", n),
                _ => "  ".into(),
            };
            PickerItem {
                id: t.id,
                display: format!(
                    "{} {} {} {}  {}",
                    track_num.dimmed(),
                    t.artist_name.cyan(),
                    "—".dimmed(),
                    t.title,
                    dur.dimmed(),
                ),
                match_text: format!("{} {} {}", t.artist_name, t.album_title, t.title),
            }
        })
        .collect()
}

fn make_album_picker_items(albums: &[queries::AlbumRow]) -> Vec<PickerItem> {
    albums
        .iter()
        .map(|a| {
            let year = a
                .date
                .as_deref()
                .and_then(|d| if d.len() >= 4 { Some(&d[..4]) } else { None })
                .map(|y| format!("({}) ", y).dimmed().to_string())
                .unwrap_or_default();
            let codec = a
                .codec
                .as_deref()
                .map(|c| format!(" [{}]", c).yellow().dimmed().to_string())
                .unwrap_or_default();
            PickerItem {
                id: a.id,
                display: format!(
                    "{} {} {}{}{}",
                    a.artist_name.cyan(),
                    "—".dimmed(),
                    year,
                    a.title.green(),
                    codec,
                ),
                match_text: format!("{} {}", a.artist_name, a.title),
            }
        })
        .collect()
}

fn make_artist_picker_items(artists: &[queries::ArtistRow]) -> Vec<PickerItem> {
    artists
        .iter()
        .map(|a| PickerItem {
            id: a.id,
            display: a.name.bold().cyan().to_string(),
            match_text: a.name.clone(),
        })
        .collect()
}

fn update_progress_bar(
    mp: &MultiProgress,
    pb: &ProgressBar,
    state: &Arc<SharedPlayerState>,
    current_track: &mut Option<PathBuf>,
) {
    let Some(info) = state.track_info() else {
        return;
    };

    if current_track.as_ref() != Some(&info.path) {
        let display_name = info.path.file_stem().unwrap_or_default().to_string_lossy();

        // Use mp.println so output goes above all managed bars — no redraw stomping.
        mp.println(format!("\n{}", display_name.bold())).ok();
        mp.println(format!(
            "  {} {} {} {} {}",
            info.codec.yellow().dimmed(),
            "|".dimmed(),
            format!("{}Hz", info.sample_rate).dimmed(),
            format!("{}bit", info.bit_depth).dimmed(),
            format!("{}ch", info.channels).dimmed(),
        ))
        .ok();
        pb.set_length(info.duration_ms);
        *current_track = Some(info.path.clone());
    }

    let pos = state.position_ms();
    let status = match state.playback_state() {
        PlaybackState::Playing => "▶".cyan().to_string(),
        PlaybackState::Paused => "⏸".yellow().to_string(),
        PlaybackState::Stopped => "■".dimmed().to_string(),
    };

    pb.set_prefix(status);
    pb.set_position(pos);
    pb.set_message(format!(
        "{}/{}",
        format_time(pos),
        format_time(info.duration_ms).dimmed()
    ));
}

fn wait_for_playing(state: &Arc<SharedPlayerState>) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(10));
        if state.playback_state() == PlaybackState::Playing {
            return;
        }
    }
    eprintln!("{}", "playback failed to start".red());
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
        eprintln!(
            "{} no folders to scan — pass a path or configure library.folders",
            "error:".red().bold()
        );
        std::process::exit(1);
    }

    let start = std::time::Instant::now();
    let result = koan_core::index::scanner::full_scan(&db, &folders, force);
    let elapsed = start.elapsed();

    println!(
        "{} {} {} added, {} updated, {} removed, {} skipped",
        "scan complete".green().bold(),
        format!("({:.1}s)", elapsed.as_secs_f64()).dimmed(),
        result.added.to_string().green(),
        result.updated.to_string().yellow(),
        result.removed.to_string().red(),
        result.skipped.to_string().dimmed(),
    );

    if !result.errors.is_empty() {
        println!("{} {}:", "errors".red().bold(), result.errors.len());
        for (path, err) in result.errors.iter().take(10) {
            println!(
                "  {} {} {}",
                "│".dimmed(),
                path.display().to_string().dimmed(),
                format!("— {}", err).red()
            );
        }
        if result.errors.len() > 10 {
            println!(
                "  {} {}",
                "└".dimmed(),
                format!("... and {} more", result.errors.len() - 10).dimmed()
            );
        }
    }
}

fn cmd_search(query: &str) {
    let db = open_db();
    match queries::search_tracks(&db.conn, query) {
        Ok(tracks) => {
            if tracks.is_empty() {
                println!("no results for {}", format!("\"{}\"", query).dimmed());
                return;
            }
            println!(
                "{} results for {}\n",
                tracks.len().to_string().bold(),
                format!("\"{}\"", query).dimmed()
            );

            // Group tracks by artist → album for tree display.
            struct AlbumGroup {
                title: String,
                album_id: Option<i64>,
                codec: Option<String>,
                has_local: bool,
                has_remote: bool,
                tracks: Vec<queries::TrackRow>,
            }
            struct ArtistGroup {
                name: String,
                albums: Vec<AlbumGroup>,
            }

            let mut artists: Vec<ArtistGroup> = Vec::new();

            for t in tracks {
                let artist = artists.iter_mut().find(|a| a.name == t.artist_name);
                let artist = match artist {
                    Some(a) => a,
                    None => {
                        artists.push(ArtistGroup {
                            name: t.artist_name.clone(),
                            albums: Vec::new(),
                        });
                        artists.last_mut().unwrap()
                    }
                };

                let album = artist
                    .albums
                    .iter_mut()
                    .find(|a| a.title == t.album_title && a.album_id == t.album_id);
                let album = match album {
                    Some(a) => a,
                    None => {
                        artist.albums.push(AlbumGroup {
                            title: t.album_title.clone(),
                            album_id: t.album_id,
                            codec: t.codec.clone(),
                            has_local: false,
                            has_remote: false,
                            tracks: Vec::new(),
                        });
                        artist.albums.last_mut().unwrap()
                    }
                };

                if t.path.is_some() {
                    album.has_local = true;
                }
                if t.remote_id.is_some() {
                    album.has_remote = true;
                }
                album.tracks.push(t);
            }

            // Render tree.
            for (ai, artist) in artists.iter().enumerate() {
                let is_last_artist = ai == artists.len() - 1;
                println!("{}", artist.name.bold().cyan());

                for (ali, album) in artist.albums.iter().enumerate() {
                    let is_last_album = ali == artist.albums.len() - 1;
                    let branch = if is_last_album {
                        "└── "
                    } else {
                        "├── "
                    };

                    let codec_tag = album
                        .codec
                        .as_deref()
                        .map(|c| format!(" [{}]", c).yellow().dimmed().to_string())
                        .unwrap_or_default();
                    let source_tag = match (album.has_local, album.has_remote) {
                        (true, true) => format!(" {}", "[local+remote]".magenta().dimmed()),
                        (false, true) => format!(" {}", "[remote]".magenta().dimmed()),
                        _ => String::new(),
                    };
                    let album_id = album
                        .album_id
                        .map(|id| format!(" {}", format!("[album:{}]", id).dimmed()))
                        .unwrap_or_default();

                    println!(
                        "{}{}{}{}{}",
                        branch.dimmed(),
                        album.title.green(),
                        codec_tag,
                        source_tag,
                        album_id,
                    );

                    let pipe = if is_last_album { "    " } else { "│   " };
                    for (ti, t) in album.tracks.iter().enumerate() {
                        let is_last_track = ti == album.tracks.len() - 1;
                        let track_branch = if is_last_track {
                            "└── "
                        } else {
                            "├── "
                        };

                        let disc_track = match (t.disc, t.track_number) {
                            (Some(d), Some(n)) if d > 1 => format!("{}.{:02}", d, n),
                            (_, Some(n)) => format!("{:02}", n),
                            _ => "  ".into(),
                        };

                        let dur = t
                            .duration_ms
                            .map(|d| format_time(d as u64))
                            .unwrap_or_default();

                        println!(
                            "{}{}{} {} {}  {}",
                            pipe.dimmed(),
                            track_branch.dimmed(),
                            disc_track.dimmed(),
                            format!("[{}]", t.id).dimmed(),
                            t.title,
                            dur.dimmed(),
                        );
                    }
                }
                if !is_last_artist {
                    println!();
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "search failed:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn cmd_artists(query: Option<&str>) {
    let db = open_db();
    let artists = if let Some(q) = query {
        queries::find_artists(&db.conn, q)
    } else {
        queries::all_artists(&db.conn)
    };

    match artists {
        Ok(artists) => {
            if artists.is_empty() {
                println!("no artists found");
                return;
            }
            for a in &artists {
                let remote_tag = if a.remote_id.is_some() {
                    format!(" {}", "[remote]".magenta().dimmed())
                } else {
                    String::new()
                };
                println!(
                    "  {} {}{}",
                    format!("[{}]", a.id).dimmed(),
                    a.name.bold().cyan(),
                    remote_tag,
                );
            }
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn cmd_albums(query: Option<&str>) {
    let db = open_db();

    let albums = if let Some(q) = query {
        let artists = queries::find_artists(&db.conn, q).unwrap_or_default();
        if artists.is_empty() {
            println!("no artist matching {}", format!("\"{}\"", q).dimmed());
            return;
        }
        let mut all_albums = Vec::new();
        for a in &artists {
            if let Ok(mut albums) = queries::albums_for_artist(&db.conn, a.id) {
                all_albums.append(&mut albums);
            }
        }
        Ok(all_albums)
    } else {
        queries::all_albums(&db.conn)
    };

    match albums {
        Ok(albums) => {
            if albums.is_empty() {
                println!("no albums found");
                return;
            }
            let mut current_artist = String::new();
            let mut artist_albums: Vec<&queries::AlbumRow> = Vec::new();

            // Collect albums per artist for tree rendering.
            let mut grouped: Vec<(String, Vec<&queries::AlbumRow>)> = Vec::new();
            for al in &albums {
                if al.artist_name != current_artist {
                    if !artist_albums.is_empty() {
                        grouped.push((current_artist.clone(), artist_albums.clone()));
                    }
                    current_artist = al.artist_name.clone();
                    artist_albums = Vec::new();
                }
                artist_albums.push(al);
            }
            if !artist_albums.is_empty() {
                grouped.push((current_artist, artist_albums));
            }

            for (gi, (artist_name, als)) in grouped.iter().enumerate() {
                if gi > 0 {
                    println!();
                }
                println!("{}", artist_name.bold().cyan());
                for (i, al) in als.iter().enumerate() {
                    let is_last = i == als.len() - 1;
                    let branch = if is_last { "└── " } else { "├── " };

                    let year = al
                        .date
                        .as_deref()
                        .map(|d| {
                            let y = if d.len() >= 4 { &d[..4] } else { d };
                            format!("({}) ", y).dimmed().to_string()
                        })
                        .unwrap_or_default();
                    let codec = al
                        .codec
                        .as_deref()
                        .map(|c| format!(" [{}]", c).yellow().dimmed().to_string())
                        .unwrap_or_default();

                    println!(
                        "{}{}{}{} {}",
                        branch.dimmed(),
                        year,
                        al.title.green(),
                        codec,
                        format!("[{}]", al.id).dimmed(),
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn cmd_library() {
    let db = open_db();
    match queries::library_stats(&db.conn) {
        Ok(stats) => {
            println!("{}", "library".bold());
            println!(
                "  {} {}",
                "tracks:".cyan(),
                format!("{} total", stats.total_tracks).bold(),
            );
            println!("    {} {}", "local:".dimmed(), stats.local_tracks,);
            println!("    {} {}", "remote:".dimmed(), stats.remote_tracks,);
            println!("    {} {}", "cached:".dimmed(), stats.cached_tracks,);
            println!("  {} {}", "albums:".cyan(), stats.total_albums);
            println!("  {} {}", "artists:".cyan(), stats.total_artists);
            println!(
                "\n{} {}",
                "db:".cyan(),
                config::db_path().display().to_string().dimmed()
            );
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn cmd_config() {
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

// --- Remote ---

fn cmd_remote_login(url: &str, username: &str) {
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

fn cmd_remote_sync() {
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

    match koan_core::remote::sync::sync_library(&db, &client) {
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
}

fn cmd_remote_status() {
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
                format!("— {}", e).dimmed()
            ),
        }
    }
}

// --- Cache ---

fn cmd_cache_status() {
    let cfg = config::Config::load().unwrap_or_default();
    let cache_dir = cfg.cache_dir();

    println!("{} {}", "path:".cyan(), cache_dir.display());

    if !cache_dir.exists() {
        println!(
            "{} {}",
            "size:".cyan(),
            "empty (no cache directory)".dimmed()
        );
        return;
    }

    let mut total_bytes: u64 = 0;
    let mut file_count: u64 = 0;
    for entry in walkdir::WalkDir::new(&cache_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            total_bytes += meta.len();
            file_count += 1;
        }
    }

    let size = format_bytes(total_bytes);
    println!(
        "{} {} {}",
        "size:".cyan(),
        size.bold(),
        format!("({} files)", file_count).dimmed(),
    );
}

fn cmd_cache_clear() {
    let cfg = config::Config::load().unwrap_or_default();
    let cache_dir = cfg.cache_dir();

    if !cache_dir.exists() {
        println!("{}", "cache already empty".dimmed());
        return;
    }

    // Count what we're about to nuke.
    let mut total_bytes: u64 = 0;
    let mut file_count: u64 = 0;
    for entry in walkdir::WalkDir::new(&cache_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            total_bytes += meta.len();
            file_count += 1;
        }
    }

    if file_count == 0 {
        println!("{}", "cache already empty".dimmed());
        return;
    }

    if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }

    // Clear cached_path in DB so tracks get re-downloaded next time.
    let db = open_db();
    let _ = db
        .conn
        .execute("UPDATE tracks SET cached_path = NULL", rusqlite::params![]);

    println!(
        "{} {} {}",
        "cache cleared".green().bold(),
        format_bytes(total_bytes),
        format!("({} files removed)", file_count).dimmed(),
    );
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{} B", b),
    }
}

// --- Pick (standalone) ---

fn cmd_pick(query: Option<&str>, album_mode: bool, artist_mode: bool) {
    let db = open_db();

    if album_mode {
        pick_album_standalone(&db, query);
    } else if artist_mode {
        pick_artist_standalone(&db, query);
    } else {
        pick_tracks_standalone(&db, query);
    }
}

/// Read keys from stdin in raw mode for standalone picker (no playback event loop).
fn stdin_key_reader() -> impl FnMut() -> Option<PickerKey> {
    let stdin = io::stdin();
    move || {
        let mut handle = stdin.lock();
        let mut buf = [0u8; 1];
        match handle.read(&mut buf) {
            Ok(1) => picker::parse_key(buf[0], &mut || {
                let mut b = [0u8; 1];
                match handle.read(&mut b) {
                    Ok(1) => Some(b[0]),
                    _ => None,
                }
            }),
            _ => None,
        }
    }
}

fn pick_tracks_standalone(db: &Database, query: Option<&str>) {
    let tracks = if let Some(q) = query {
        queries::search_tracks(&db.conn, q).unwrap_or_default()
    } else {
        queries::all_tracks(&db.conn).unwrap_or_default()
    };

    if tracks.is_empty() {
        eprintln!("no tracks found");
        std::process::exit(1);
    }

    let items = make_track_picker_items(&tracks);
    let picker = Picker::new(items, "track>", true);

    let _raw = RawModeGuard::enter();
    let mut reader = stdin_key_reader();
    if let PickerResult::Selected(ids) = picker.run(&mut reader) {
        drop(_raw); // restore terminal before playback
        cmd_play(&[], &ids, None, None);
    }
}

fn pick_album_standalone(db: &Database, query: Option<&str>) {
    let albums = if let Some(q) = query {
        let artists = queries::find_artists(&db.conn, q).unwrap_or_default();
        let mut all = Vec::new();
        for a in &artists {
            if let Ok(mut als) = queries::albums_for_artist(&db.conn, a.id) {
                all.append(&mut als);
            }
        }
        if all.is_empty() {
            queries::all_albums(&db.conn)
                .unwrap_or_default()
                .into_iter()
                .filter(|a| a.title.to_lowercase().contains(&q.to_lowercase()))
                .collect()
        } else {
            all
        }
    } else {
        queries::all_albums(&db.conn).unwrap_or_default()
    };

    if albums.is_empty() {
        eprintln!("no albums found");
        std::process::exit(1);
    }

    let items = make_album_picker_items(&albums);
    let picker = Picker::new(items, "album>", false);

    let _raw = RawModeGuard::enter();
    let mut reader = stdin_key_reader();
    if let PickerResult::Selected(ids) = picker.run(&mut reader) {
        drop(_raw);
        cmd_play(&[], &[], Some(ids[0]), None);
    }
}

fn pick_artist_standalone(db: &Database, query: Option<&str>) {
    let artists = if let Some(q) = query {
        queries::find_artists(&db.conn, q).unwrap_or_default()
    } else {
        queries::all_artists(&db.conn).unwrap_or_default()
    };

    if artists.is_empty() {
        eprintln!("no artists found");
        std::process::exit(1);
    }

    let items = make_artist_picker_items(&artists);
    let picker = Picker::new(items, "artist>", false);

    let _raw = RawModeGuard::enter();
    let mut reader = stdin_key_reader();
    let artist_id = match picker.run(&mut reader) {
        PickerResult::Selected(ids) => ids[0],
        PickerResult::Cancelled => return,
    };

    // Drill down: pick album.
    let albums = queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();
    if albums.is_empty() {
        drop(_raw);
        cmd_play(&[], &[], None, Some(artist_id));
        return;
    }

    let mut items = vec![PickerItem {
        id: -1,
        display: format!("{}", "all tracks".bold()),
        match_text: "all tracks".into(),
    }];
    items.extend(make_album_picker_items(&albums));

    let picker = Picker::new(items, "album>", false);
    let mut reader = stdin_key_reader();
    match picker.run(&mut reader) {
        PickerResult::Selected(ids) if ids[0] == -1 => {
            drop(_raw);
            cmd_play(&[], &[], None, Some(artist_id));
        }
        PickerResult::Selected(ids) => {
            drop(_raw);
            cmd_play(&[], &[], Some(ids[0]), None);
        }
        PickerResult::Cancelled => {}
    }
}

// --- Info ---

fn cmd_probe(path: &Path) {
    if !path.exists() {
        eprintln!("{} {}", "not found:".red().bold(), path.display());
        std::process::exit(1);
    }

    match buffer::probe_file(path) {
        Ok(info) => {
            println!("{} {}", "file:".cyan(), path.display());
            println!("{} {}", "codec:".cyan(), info.codec.yellow());
            println!("{} {} Hz", "sample rate:".cyan(), info.sample_rate);
            println!("{} {}", "bit depth:".cyan(), info.bit_depth);
            println!("{} {}", "channels:".cyan(), info.channels);
            println!(
                "{} {} {}",
                "duration:".cyan(),
                format_time(info.duration_ms),
                format!("({}ms)", info.duration_ms).dimmed()
            );
        }
        Err(e) => {
            eprintln!("{} {}", "probe failed:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn cmd_devices() {
    match device::list_output_devices() {
        Ok(devices) => {
            let default_id = device::default_output_device().ok();
            for dev in &devices {
                let is_default = Some(dev.id) == default_id;
                let marker = if is_default {
                    " *".yellow().bold().to_string()
                } else {
                    String::new()
                };
                println!(
                    "{} {}{}",
                    format!("[{}]", dev.id).dimmed(),
                    if is_default {
                        dev.name.bold().to_string()
                    } else {
                        dev.name.to_string()
                    },
                    marker,
                );
                if !dev.sample_rates.is_empty() {
                    let rates: Vec<String> = dev
                        .sample_rates
                        .iter()
                        .map(|r| format!("{}Hz", *r as u32))
                        .collect();
                    println!("  {} {}", "rates:".dimmed(), rates.join(", ").dimmed());
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

// --- Helpers ---

/// Get the remote password from config, falling back to Keychain for backwards compat.
fn get_remote_password(cfg: &config::Config) -> String {
    if !cfg.remote.password.is_empty() {
        return cfg.remote.password.clone();
    }
    // Fallback to Keychain for users who set up before the config change.
    match koan_core::credentials::get_password(&cfg.remote.url) {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "{} no password configured — run {} to set up",
                "error:".red().bold(),
                "koan remote login".bold()
            );
            std::process::exit(1);
        }
    }
}

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    // Truncate on a char boundary to stay under 240 bytes.
    if cleaned.len() <= 240 {
        return cleaned;
    }
    let mut end = 240;
    while !cleaned.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    cleaned[..end].trim_end().to_string()
}

/// Build a structured cache path for a track:
///   cache_dir/Album Artist/(Year) Album [Codec]/01. Track Artist - Title.ext
fn cache_path_for_track(
    cache_dir: &Path,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> PathBuf {
    let artist_dir = sanitise_filename(&track.artist_name);

    let year = album_date
        .and_then(|d| if d.len() >= 4 { Some(&d[..4]) } else { None })
        .map(|y| format!("({}) ", y))
        .unwrap_or_default();
    let codec = track
        .codec
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default();
    let album_dir = sanitise_filename(&format!("{}{}{}", year, track.album_title, codec));

    let disc_prefix = match track.disc {
        Some(d) if d > 1 => format!("{}-", d),
        _ => String::new(),
    };
    let track_num = track
        .track_number
        .map(|n| format!("{:02}. ", n))
        .unwrap_or_default();

    let ext = track
        .codec
        .as_deref()
        .map(|c| c.to_lowercase())
        .unwrap_or_else(|| "flac".into());

    let filename = sanitise_filename(&format!(
        "{}{}{} - {}",
        disc_prefix, track_num, track.artist_name, track.title
    ));

    cache_dir
        .join(artist_dir)
        .join(album_dir)
        .join(format!("{}.{}", filename, ext))
}

/// Resolve a single track ID to a playback path.
/// Downloads from remote if needed, using structured cache paths.
/// When `mp` is provided, creates a spinner bar for download progress.
fn resolve_single_track(id: i64, mp: Option<&MultiProgress>) -> PathBuf {
    let db = open_db();
    let cfg = config::Config::load().unwrap_or_default();

    match queries::resolve_playback_path(&db.conn, id) {
        Ok(Some(queries::PlaybackSource::Local(p))) => p,
        Ok(Some(queries::PlaybackSource::Cached(p))) => p,
        Ok(Some(queries::PlaybackSource::Remote(_url))) => {
            let track = match queries::get_track_row(&db.conn, id) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    eprintln!("{} track {} not found", "error:".red().bold(), id);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("{} {}", "error:".red().bold(), e);
                    std::process::exit(1);
                }
            };

            let remote_id = match &track.remote_id {
                Some(rid) => rid.clone(),
                None => {
                    eprintln!("{} track {} has no remote_id", "error:".red().bold(), id);
                    std::process::exit(1);
                }
            };

            let album_date: Option<String> = track.album_id.and_then(|aid| {
                db.conn
                    .query_row(
                        "SELECT date FROM albums WHERE id = ?1",
                        rusqlite::params![aid],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten()
            });

            let cache_dir = cfg.cache_dir();
            let dest = cache_path_for_track(&cache_dir, &track, album_date.as_deref());

            if dest.exists() {
                return dest;
            }

            // Create a spinner bar for this download, managed by MultiProgress.
            let label = format!(
                "{} {} — {}",
                "↓".cyan(),
                track.title.bold(),
                track.artist_name.dimmed(),
            );
            let spinner = if let Some(mp) = mp {
                let sp = mp.add(ProgressBar::new_spinner());
                sp.set_style(
                    ProgressStyle::with_template("{spinner:.cyan} {msg}")
                        .unwrap()
                        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
                );
                sp.set_message(label.clone());
                sp.enable_steady_tick(Duration::from_millis(80));
                Some(sp)
            } else {
                eprintln!("{}", label);
                None
            };

            let password = get_remote_password(&cfg);
            let client = koan_core::remote::client::SubsonicClient::new(
                &cfg.remote.url,
                &cfg.remote.username,
                &password,
            );

            let result = client.download(&remote_id, &dest);

            if let Some(sp) = &spinner {
                sp.finish_and_clear();
            }

            if let Err(e) = result {
                if let Some(mp) = mp {
                    mp.println(format!("{} {} — {}", "✗".red().bold(), track.title, e))
                        .ok();
                } else {
                    eprintln!("{} {}", "download failed:".red().bold(), e);
                }
                std::process::exit(1);
            }

            // Log completion above the bars.
            if let Some(mp) = mp {
                mp.println(format!(
                    "{} {} — {}",
                    "✓".green(),
                    track.title,
                    track.artist_name.dimmed(),
                ))
                .ok();
            }

            let _ = queries::set_cached_path(&db.conn, id, &dest.to_string_lossy());

            dest
        }
        Ok(None) => {
            eprintln!("{} track {} not found", "error:".red().bold(), id);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn open_db() -> Database {
    Database::open_default().unwrap_or_else(|e| {
        eprintln!("{} {}", "db error:".red().bold(), e);
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

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::env::CompleteEnv;
use koan_core::audio::{buffer, device};
use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::player::Player;

// --- Logger ---
// All log messages go to ~/.config/koan/koan.log.
// During playback, they're also buffered for the queue display.
// Outside playback, they also go to stderr.

static LOGGER: OnceLock<BufferedLogger> = OnceLock::new();

struct BufferedLogger {
    buffer: Mutex<Option<Arc<Mutex<Vec<String>>>>>,
    log_file: Mutex<Option<std::fs::File>>,
}

impl BufferedLogger {
    fn init() {
        let log_path = config::config_dir().join("koan.log");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();

        let logger = LOGGER.get_or_init(|| BufferedLogger {
            buffer: Mutex::new(None),
            log_file: Mutex::new(log_file),
        });
        log::set_logger(logger).expect("failed to set logger");
        log::set_max_level(log::LevelFilter::Info);
    }

    fn set_buffer(buf: Arc<Mutex<Vec<String>>>) {
        if let Some(logger) = LOGGER.get() {
            *logger.buffer.lock().unwrap() = Some(buf);
        }
    }

    fn clear_buffer() {
        if let Some(logger) = LOGGER.get() {
            *logger.buffer.lock().unwrap() = None;
        }
    }
}

impl log::Log for BufferedLogger {
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

        // Always write to log file.
        if let Some(file) = self.log_file.lock().unwrap().as_mut() {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let _ = writeln!(file, "[{}] {}", now, msg);
        }

        if let Some(buf) = self.buffer.lock().unwrap().as_ref() {
            buf.lock().unwrap().push(msg);
        } else {
            eprintln!("{}", msg);
        }
    }

    fn flush(&self) {}
}
mod media_keys;
mod tui;

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, SharedPlayerState};
use owo_colors::OwoColorize;

use tui::picker::{PickerItem, PickerKind, PickerState};

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
    /// Initialise config directory with default config
    Init,
    /// Manage the download cache
    #[command(subcommand)]
    Cache(CacheCommands),
    /// Organize/rename library files using format strings
    Organize {
        /// Format string pattern for the new path (e.g. '%album artist%/(%date%) %album%/%tracknumber%. %title%')
        #[arg(long)]
        pattern: Option<String>,
        /// Base directory (defaults to first library folder)
        #[arg(long)]
        base_dir: Option<PathBuf>,
        /// Actually move files (default is dry-run/preview)
        #[arg(long)]
        execute: bool,
        /// Undo the most recent organize operation
        #[arg(long)]
        undo: bool,
    },
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

    BufferedLogger::init();

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
        Commands::Init => cmd_init(),
        Commands::Cache(sub) => match sub {
            CacheCommands::Status => cmd_cache_status(),
            CacheCommands::Clear => cmd_cache_clear(),
        },
        Commands::Organize {
            pattern,
            base_dir,
            execute,
            undo,
        } => cmd_organize(pattern.as_deref(), base_dir.as_deref(), execute, undo),
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

    // Shared log buffer — background threads push, render loop drains.
    let log_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    BufferedLogger::set_buffer(log_buffer.clone());

    let (state, tx) = Player::spawn();

    if let Some(ids) = track_ids {
        // Resolve first track immediately so playback starts ASAP.
        let first_path = resolve_single_track(ids[0], Some(&log_buffer), Some(&state));
        tx.send(PlayerCommand::Play(first_path))
            .expect("player thread died");

        // Background: build pending queue metadata, then download remaining
        // tracks in parallel batches, prioritizing queue order.
        if ids.len() > 1 {
            let remaining = ids[1..].to_vec();
            let tx_bg = tx.clone();
            let log_bg = log_buffer.clone();
            let state_bg = state.clone();
            std::thread::Builder::new()
                .name("koan-resolve".into())
                .spawn(move || {
                    resolve_and_enqueue_batch(remaining, tx_bg, log_bg, state_bg);
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

    // Run the Ratatui TUI.
    if let Err(e) = run_tui(state, tx, log_buffer) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    BufferedLogger::clear_buffer();
    std::thread::sleep(Duration::from_millis(100));
}

fn run_tui(
    state: Arc<SharedPlayerState>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buffer: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    use crossterm::{
        event::{DisableMouseCapture, EnableMouseCapture},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    // Set panic hook to restore terminal.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic);
    }));

    // Setup terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let db_path = config::db_path();
    let mut app = tui::app::App::new(state, tx.clone(), log_buffer, db_path);

    // Media keys (macOS Control Center integration).
    let mut media = media_keys::MediaKeyHandler::new(tx.clone(), app.state.clone());
    let mut last_track_path: Option<PathBuf> = None;

    loop {
        terminal.draw(|f| tui::ui::render(f, &mut app))?;

        let event = tui::event::poll(Duration::from_millis(50))?;

        match event {
            tui::event::Event::Key(key) => app.handle_key(key),
            tui::event::Event::Mouse(mouse) => app.handle_mouse(mouse),
            tui::event::Event::Tick => {
                app.handle_tick();

                // Update media keys.
                if let Some(ref mut mk) = media {
                    mk.update_playback(&app.state);
                    let current = app.state.track_info().map(|t| t.path.clone());
                    if current != last_track_path {
                        last_track_path = current;
                        mk.update_metadata(&app.state);
                    }
                }
            }
        }

        // Handle picker opening — load items from DB.
        if let tui::app::Mode::Picker(kind) = &app.mode
            && app.picker.is_none()
        {
            let items = load_picker_items(*kind);
            let multi = matches!(kind, PickerKind::Track);
            app.picker = Some(PickerState::new(*kind, items, multi));
        }

        // Handle artist drill-down.
        if let Some(artist_id) = app.artist_drill_down.take() {
            let db = open_db();
            let albums = queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();
            if albums.is_empty() {
                // No albums — get all tracks for this artist.
                let track_ids: Vec<i64> = queries::tracks_for_artist(&db.conn, artist_id)
                    .unwrap_or_default()
                    .iter()
                    .map(|t| t.id)
                    .collect();
                if !track_ids.is_empty() {
                    app.picker_result = Some(track_ids);
                }
            } else {
                // Open album picker for this artist.
                let mut items = vec![PickerItem {
                    id: -1,
                    display: "all tracks".to_string(),
                    match_text: "all tracks".into(),
                }];
                items.extend(make_album_picker_items(&albums));
                app.mode = tui::app::Mode::Picker(PickerKind::Album);
                let picker = PickerState::new(PickerKind::Album, items, false);
                app.picker = Some(picker);
                // Store artist_id for if they pick "all tracks".
                // We'll handle this by checking if result ID is -1.
                // Actually, the confirm() method returns the PickerItem.id,
                // so -1 means "all tracks" for this artist.
                // We need to stash the artist_id somewhere.
                // Let's just process the result below.
            }
        }

        // Handle picker result — enqueue in background.
        if let Some(ids) = app.picker_result.take() {
            // Check for "all tracks" sentinel from artist drill-down.
            // (Not needed — the drill-down already resolved to track IDs above.)
            let tx_bg = tx.clone();
            let log_bg = app.log_buffer.clone();
            let state_bg = app.state.clone();

            // If it's album picker results from artist drill-down, resolve album tracks.
            let track_ids = if ids.len() == 1 && ids[0] < 0 {
                // "All tracks" sentinel — should have been handled above.
                vec![]
            } else {
                // Check if these are album IDs (from album picker) or track IDs.
                // Album picker returns album IDs — need to expand to tracks.
                // Track picker returns track IDs directly.
                // We can tell by looking at the mode that was active... but it's
                // already changed back to Normal. Let's just check the DB.
                // Actually, the picker kind determines this:
                // Track picker → ids are track IDs
                // Album picker → ids are album IDs → expand
                // For simplicity: try album expansion first, fall back to track IDs.
                // Actually no — let's just tag the result properly.
                ids
            };

            if !track_ids.is_empty() {
                std::thread::Builder::new()
                    .name("koan-enqueue".into())
                    .spawn(move || {
                        resolve_and_enqueue_batch(track_ids, tx_bg, log_bg, state_bg);
                    })
                    .ok();
            }
        }

        if app.quit {
            break;
        }
    }

    // Restore terminal.
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn load_picker_items(kind: PickerKind) -> Vec<PickerItem> {
    let db = open_db();
    match kind {
        PickerKind::Track => {
            let tracks = queries::all_tracks(&db.conn).unwrap_or_default();
            make_track_picker_items(&tracks)
        }
        PickerKind::Album => {
            let albums = queries::all_albums(&db.conn).unwrap_or_default();
            make_album_picker_items(&albums)
        }
        PickerKind::Artist => {
            let artists = queries::all_artists(&db.conn).unwrap_or_default();
            make_artist_picker_items(&artists)
        }
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
                display: format!("{} {} - {} {}", track_num, t.artist_name, t.title, dur,),
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
                .map(|y| format!("({}) ", y))
                .unwrap_or_default();
            let codec = a
                .codec
                .as_deref()
                .map(|c| format!(" [{}]", c))
                .unwrap_or_default();
            PickerItem {
                id: a.id,
                display: format!("{} - {}{}{}", a.artist_name, year, a.title, codec,),
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
            display: a.name.clone(),
            match_text: a.name.clone(),
        })
        .collect()
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
    let on_track = |ev: koan_core::index::scanner::ScanEvent| {
        println!(
            "  {} {} {} {}",
            "+".green(),
            ev.artist.cyan(),
            "—".dimmed(),
            ev.title,
        );
    };
    let result = koan_core::index::scanner::full_scan(&db, &folders, force, Some(&on_track));
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

// --- Init ---

fn cmd_init() {
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

fn cmd_organize(pattern: Option<&str>, base_dir: Option<&Path>, execute: bool, undo_mode: bool) {
    use koan_core::organize;

    let db = open_db();

    if undo_mode {
        match organize::undo(&db) {
            Ok(count) => {
                println!("{} {} files reverted", "undo:".green().bold(), count);
            }
            Err(organize::OrganizeError::NothingToUndo) => {
                println!("{}", "no organize batches to undo".dimmed());
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
        return;
    }

    let Some(pattern) = pattern else {
        eprintln!(
            "{} --pattern is required (unless --undo)",
            "error:".red().bold()
        );
        std::process::exit(1);
    };

    if execute {
        match organize::execute(&db, pattern, base_dir) {
            Ok(result) => {
                let moved = result.moves.len();
                for m in &result.moves {
                    println!("  {} {}", "\u{2713}".green(), m.to.display());
                }
                for (path, err) in &result.errors {
                    eprintln!("  {} {} {}", "\u{2717}".red(), path.display(), err.dimmed());
                }
                println!();
                println!(
                    "{} {} moved, {} errors{}",
                    "done:".green().bold(),
                    moved,
                    result.errors.len(),
                    if moved > 0 {
                        "\nrun 'koan organize --undo' to revert"
                    } else {
                        ""
                    }
                );
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
    } else {
        // Preview mode (default).
        match organize::preview(&db, pattern, base_dir) {
            Ok(result) => {
                if result.moves.is_empty() && result.errors.is_empty() {
                    println!("{}", "no tracks to organize".dimmed());
                    return;
                }

                println!(
                    "{} {} tracks would be moved\n",
                    "preview:".cyan().bold(),
                    result.moves.len()
                );

                let show_count = result.moves.len().min(20);
                for m in &result.moves[..show_count] {
                    println!("  {}", m.from.display().dimmed());
                    println!("    {} {}", "\u{2192}".cyan(), m.to.display());
                    println!();
                }

                let remaining = result.moves.len().saturating_sub(show_count);
                if remaining > 0 {
                    println!("  {} (and {} more)\n", "...".dimmed(), remaining);
                }

                if result.skipped > 0 {
                    println!(
                        "  {} {} already in place",
                        "skipped:".dimmed(),
                        result.skipped
                    );
                }

                if !result.errors.is_empty() {
                    println!(
                        "  {} {} errors",
                        "warning:".yellow().bold(),
                        result.errors.len()
                    );
                    for (path, err) in &result.errors {
                        eprintln!("    {} {}", path.display(), err.dimmed());
                    }
                }

                println!("\nrun with {} to apply", "--execute".bold());
            }
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
    }
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

fn cmd_pick(_query: Option<&str>, album_mode: bool, artist_mode: bool) {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::execute;
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    let db = open_db();
    let theme = tui::theme::Theme::default();

    let (items, kind) = if album_mode {
        let albums = queries::all_albums(&db.conn).unwrap_or_default();
        if albums.is_empty() {
            eprintln!("no albums found");
            std::process::exit(1);
        }
        (make_album_picker_items(&albums), PickerKind::Album)
    } else if artist_mode {
        let artists = queries::all_artists(&db.conn).unwrap_or_default();
        if artists.is_empty() {
            eprintln!("no artists found");
            std::process::exit(1);
        }
        (make_artist_picker_items(&artists), PickerKind::Artist)
    } else {
        let tracks = queries::all_tracks(&db.conn).unwrap_or_default();
        if tracks.is_empty() {
            eprintln!("no tracks found");
            std::process::exit(1);
        }
        (make_track_picker_items(&tracks), PickerKind::Track)
    };

    let multi = matches!(kind, PickerKind::Track);
    let mut picker = PickerState::new(kind, items, multi);

    // Setup terminal for picker.
    enable_raw_mode().expect("failed to enable raw mode");
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).expect("failed to enter alt screen");
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");

    let result = loop {
        terminal
            .draw(|f| {
                let overlay = tui::picker::PickerOverlay::new(&picker, &theme);
                f.render_widget(overlay, f.area());
            })
            .ok();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(Event::Key(key)) = event::read()
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                break None;
            }
            match key.code {
                KeyCode::Esc => break None,
                KeyCode::Enter => {
                    let ids = picker.confirm();
                    break if ids.is_empty() { None } else { Some(ids) };
                }
                KeyCode::Up => picker.move_up(),
                KeyCode::Down => picker.move_down(),
                KeyCode::Tab => picker.toggle_select(),
                KeyCode::Backspace => picker.backspace(),
                KeyCode::Char(c) => picker.type_char(c),
                _ => {}
            }
        }
        picker.tick();
    };

    // Restore terminal.
    disable_raw_mode().expect("failed to disable raw mode");
    execute!(terminal.backend_mut(), LeaveAlternateScreen).expect("failed to leave alt screen");
    terminal.show_cursor().ok();

    // Process result.
    if let Some(ids) = result {
        match kind {
            PickerKind::Track => {
                cmd_play(&[], &ids, None, None);
            }
            PickerKind::Album => {
                if let Some(&album_id) = ids.first() {
                    cmd_play(&[], &[], Some(album_id), None);
                }
            }
            PickerKind::Artist => {
                if let Some(&artist_id) = ids.first() {
                    // Drill down: pick album for this artist.
                    let albums =
                        queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();
                    if albums.is_empty() {
                        cmd_play(&[], &[], None, Some(artist_id));
                    } else {
                        // Show album picker for this artist.
                        let mut items = vec![PickerItem {
                            id: -1,
                            display: "all tracks".to_string(),
                            match_text: "all tracks".into(),
                        }];
                        items.extend(make_album_picker_items(&albums));

                        let mut picker2 = PickerState::new(PickerKind::Album, items, false);

                        enable_raw_mode().expect("failed to enable raw mode");
                        let mut stdout2 = io::stdout();
                        execute!(stdout2, EnterAlternateScreen)
                            .expect("failed to enter alt screen");
                        let backend2 = CrosstermBackend::new(stdout2);
                        let mut terminal2 =
                            Terminal::new(backend2).expect("failed to create terminal");

                        let album_result = loop {
                            terminal2
                                .draw(|f| {
                                    let overlay = tui::picker::PickerOverlay::new(&picker2, &theme);
                                    f.render_widget(overlay, f.area());
                                })
                                .ok();

                            if event::poll(Duration::from_millis(50)).unwrap_or(false)
                                && let Ok(Event::Key(key)) = event::read()
                            {
                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.code == KeyCode::Char('c')
                                {
                                    break None;
                                }
                                match key.code {
                                    KeyCode::Esc => break None,
                                    KeyCode::Enter => {
                                        let ids = picker2.confirm();
                                        break if ids.is_empty() { None } else { Some(ids) };
                                    }
                                    KeyCode::Up => picker2.move_up(),
                                    KeyCode::Down => picker2.move_down(),
                                    KeyCode::Backspace => picker2.backspace(),
                                    KeyCode::Char(c) => picker2.type_char(c),
                                    _ => {}
                                }
                            }
                            picker2.tick();
                        };

                        disable_raw_mode().expect("failed to disable raw mode");
                        execute!(terminal2.backend_mut(), LeaveAlternateScreen)
                            .expect("failed to leave alt screen");
                        terminal2.show_cursor().ok();

                        if let Some(album_ids) = album_result {
                            if album_ids[0] == -1 {
                                cmd_play(&[], &[], None, Some(artist_id));
                            } else {
                                cmd_play(&[], &[], Some(album_ids[0]), None);
                            }
                        }
                    }
                }
            }
        }
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

/// Build a QueueEntryMeta from a TrackRow + album date.
fn meta_from_track(
    track: &queries::TrackRow,
    album_date: Option<&str>,
    status: koan_core::player::state::QueueEntryStatus,
) -> koan_core::player::state::QueueEntryMeta {
    use koan_core::player::state::QueueEntryMeta;
    let year = album_date.and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });
    QueueEntryMeta {
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        status,
    }
}

/// Resolve a batch of track IDs: populate pending queue immediately,
/// then download in parallel batches (4 concurrent), enqueuing each as it finishes.
/// Tracks are processed in queue order so the top of the queue downloads first.
fn resolve_and_enqueue_batch(
    ids: Vec<i64>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buf: Arc<Mutex<Vec<String>>>,
    state: Arc<SharedPlayerState>,
) {
    use koan_core::player::state::{QueueEntry, QueueEntryStatus};

    let db = open_db();
    let cfg = config::Config::load().unwrap_or_default();

    // Phase 1: Build pending queue metadata from DB (fast, no downloads).
    // This populates the UI immediately so the user sees what's coming.
    let mut track_info: Vec<(i64, queries::TrackRow, Option<String>)> = Vec::new();
    let mut pending: Vec<QueueEntry> = Vec::new();

    for &id in &ids {
        let Some(track) = queries::get_track_row(&db.conn, id).ok().flatten() else {
            continue;
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
        let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());
        let status = if dest.exists() {
            QueueEntryStatus::Queued
        } else {
            QueueEntryStatus::Downloading
        };
        let meta = meta_from_track(&track, album_date.as_deref(), status);
        pending.push(QueueEntry {
            path: dest,
            title: meta.title,
            artist: meta.artist,
            album_artist: meta.album_artist,
            album: meta.album,
            year: meta.year,
            codec: meta.codec,
            track_number: meta.track_number,
            disc: meta.disc,
            duration_ms: meta.duration_ms,
            status: meta.status,
        });
        track_info.push((id, track, album_date));
    }

    state.set_pending_queue(pending);

    // Phase 2: Download/resolve in parallel batches, queue order.
    // 4 concurrent downloads, each enqueued as it completes.
    const BATCH_SIZE: usize = 4;

    for chunk in track_info.chunks(BATCH_SIZE) {
        let results: Vec<(i64, PathBuf)> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(id, _track, _date)| {
                    let log_ref = &log_buf;
                    let state_ref = &state;
                    let track_id = *id;
                    s.spawn(move || {
                        let path = resolve_single_track(track_id, Some(log_ref), Some(state_ref));
                        (track_id, path)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });

        // Enqueue in original order within the batch.
        for (_id, path) in results {
            state.remove_pending(&path);
            if tx.send(PlayerCommand::Enqueue(path)).is_err() {
                return;
            }
        }
    }
}

/// Resolve a single track ID to a playback path.
/// Downloads from remote if needed, using structured cache paths.
/// Updates track metadata in shared state for queue display.
fn resolve_single_track(
    id: i64,
    log_buf: Option<&Arc<Mutex<Vec<String>>>>,
    shared_state: Option<&Arc<SharedPlayerState>>,
) -> PathBuf {
    use koan_core::player::state::QueueEntryStatus;

    let db = open_db();
    let cfg = config::Config::load().unwrap_or_default();

    // Helper: register track metadata in shared state for queue display.
    let register_meta = |path: &PathBuf, state: Option<&Arc<SharedPlayerState>>| {
        let Some(state) = state else { return };
        if let Ok(Some(track)) = queries::get_track_row(&db.conn, id) {
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
            state.set_track_meta(
                path.clone(),
                meta_from_track(
                    &track,
                    album_date.as_deref(),
                    koan_core::player::state::QueueEntryStatus::Queued,
                ),
            );
        }
    };

    match queries::resolve_playback_path(&db.conn, id) {
        Ok(Some(queries::PlaybackSource::Local(p))) => {
            register_meta(&p, shared_state);
            p
        }
        Ok(Some(queries::PlaybackSource::Cached(p))) => {
            register_meta(&p, shared_state);
            p
        }
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
                // Register metadata even for cached tracks.
                if let Some(state) = shared_state {
                    state.set_track_meta(
                        dest.clone(),
                        meta_from_track(&track, album_date.as_deref(), QueueEntryStatus::Queued),
                    );
                }
                return dest;
            }

            // Register as downloading in shared state.
            if let Some(state) = shared_state {
                state.set_track_meta(
                    dest.clone(),
                    meta_from_track(&track, album_date.as_deref(), QueueEntryStatus::Downloading),
                );
            }

            let password = get_remote_password(&cfg);
            let client = koan_core::remote::client::SubsonicClient::new(
                &cfg.remote.url,
                &cfg.remote.username,
                &password,
            );

            let result = client.download(&remote_id, &dest);

            if let Err(e) = result {
                if let Some(state) = shared_state {
                    state.update_track_status(&dest, QueueEntryStatus::Failed);
                }
                let msg = format!("{} {} — {}", "x".red().bold(), track.title, e);
                if let Some(buf) = log_buf {
                    buf.lock().unwrap().push(msg);
                } else {
                    eprintln!("{}", msg);
                }
                std::process::exit(1);
            }

            // Mark as queued (downloaded successfully).
            if let Some(state) = shared_state {
                state.update_track_status(&dest, QueueEntryStatus::Queued);
            }

            let msg = format!(
                "{} {} — {}",
                "+".green(),
                track.title,
                track.artist_name.dimmed(),
            );
            if let Some(buf) = log_buf {
                buf.lock().unwrap().push(msg);
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

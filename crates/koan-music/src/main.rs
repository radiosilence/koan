use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::env::CompleteEnv;
use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;

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

        // Always write to log file (including noisy library warnings).
        if let Some(file) = self.log_file.lock().unwrap().as_mut() {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let _ = writeln!(file, "[{}] {}", now, msg);
        }

        // Suppress warn-level noise from lofty/symphonia internals on stderr/buffer.
        // Our own fallback warnings (from koan_core) still come through.
        let module = record.module_path().unwrap_or("");
        if record.level() == log::Level::Warn
            && (module.starts_with("lofty") || module.starts_with("symphonia"))
        {
            return;
        }

        if let Some(buf) = self.buffer.lock().unwrap().as_ref() {
            buf.lock().unwrap().push(msg);
        } else {
            eprintln!("{}", msg);
        }
    }

    fn flush(&self) {
        if let Some(file) = self.log_file.lock().unwrap().as_mut() {
            let _ = file.flush();
        }
    }
}

mod commands;
mod media_keys;
mod remote_bridge;
mod tui;

#[derive(Parser)]
#[command(name = "koan", about = "bit-perfect music player", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // --- Playback args (top-level, previously under `koan play`) ---
    /// Paths to audio files
    #[arg(global = false)]
    paths: Vec<PathBuf>,

    /// Track IDs from the library database
    #[arg(long = "id", num_args = 1..)]
    ids: Vec<i64>,

    /// Play an album by ID
    #[arg(long, add = ArgValueCandidates::new(complete_albums))]
    album: Option<i64>,

    /// Play all tracks by an artist ID
    #[arg(long, add = ArgValueCandidates::new(complete_artists))]
    artist: Option<i64>,

    /// Open the TUI in library browse mode
    #[arg(long, short = 'l')]
    library: bool,

    /// Clear persisted queue instead of restoring it
    #[arg(long)]
    clear: bool,

    /// Connect to a remote koan server (e.g. http://host:4000)
    #[arg(long)]
    server: Option<String>,

    /// Jukebox mode: server plays audio, client is remote control only
    #[arg(long, requires = "server")]
    jukebox: bool,

    // --- Server flags (unified process) ---
    /// Run headless (no TUI) — GraphQL API only
    #[arg(long)]
    headless: bool,

    /// Run as a background daemon (fork and detach, implies --headless)
    #[arg(short, long)]
    daemonize: bool,

    /// GraphQL API port (default: from config or 4000)
    #[arg(long)]
    port: Option<u16>,

    /// Enable Subsonic REST API on this port (e.g. --subsonic 4040)
    #[arg(long)]
    subsonic: Option<u16>,

    /// Disable the GraphQL API server (TUI-only mode)
    #[arg(long)]
    no_api: bool,

    /// Enable GraphiQL web IDE at GET /graphql
    #[arg(long)]
    playground: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run as MCP server on stdio (for Claude Desktop / MCP clients)
    Mcp,
    /// Scan a folder for audio files and index them
    Scan {
        /// Path to scan (defaults to configured library folders)
        path: Option<PathBuf>,
        /// Force re-scan of all files
        #[arg(long)]
        force: bool,
        /// Also run acoustic analysis after scanning
        #[arg(long)]
        analyze: bool,
    },
    /// Run acoustic analysis on the library for similarity features
    Analyze,
    /// Search the library
    Search {
        /// Search query
        query: String,
    },
    /// Show library statistics
    Library,
    /// Probe a file and show format info
    Probe {
        /// Path to audio file
        path: PathBuf,
    },
    /// List available audio output devices
    Devices,
    /// Show or manage configuration
    Config,
    /// Manage remote Subsonic/Navidrome server
    #[command(subcommand)]
    Remote(RemoteCommands),
    /// Initialise config directory with default config
    Init,
    /// Manage the download cache
    #[command(subcommand)]
    Cache(CacheCommands),
    /// Generate shell completions
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
    Sync {
        /// Force a full sync instead of incremental
        #[arg(long)]
        full: bool,
    },
    /// Show remote server status
    Status,
}

#[derive(Subcommand)]
enum CacheCommands {
    /// Show cache size and location
    Status,
    /// Clear all cached downloads
    Clear {
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Global flag set by SIGINT handler for graceful Ctrl+C shutdown.
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Returns true if Ctrl+C has been pressed.
pub fn sigint_received() -> bool {
    SIGINT_RECEIVED.load(Ordering::Relaxed)
}

fn main() {
    // Graceful SIGINT: set a flag instead of killing immediately so we can
    // persist queue state. In raw mode crossterm delivers Ctrl+C as a key
    // event, but outside raw mode (e.g. during scan) we need this handler.
    ctrlc::set_handler(|| {
        if SIGINT_RECEIVED.load(Ordering::Relaxed) {
            // Second Ctrl+C — force restore terminal and exit.
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::LeaveAlternateScreen,
                crossterm::event::DisableMouseCapture,
                crossterm::cursor::Show
            );
            std::process::exit(130);
        }
        SIGINT_RECEIVED.store(true, Ordering::Relaxed);
    })
    .ok();

    // Dynamic shell completions — handles COMPLETE=zsh/bash/fish env var.
    CompleteEnv::with_factory(Cli::command).complete();

    BufferedLogger::init();

    let cli = Cli::parse();

    // Subcommands take priority — they're standalone operations.
    if let Some(command) = cli.command {
        match command {
            Commands::Scan {
                path,
                force,
                analyze,
            } => {
                commands::cmd_scan(path.as_deref(), force);
                if analyze {
                    commands::cmd_analyze();
                }
            }
            Commands::Mcp => {
                commands::cmd_mcp();
                return;
            }
            Commands::Analyze => commands::cmd_analyze(),
            Commands::Search { query } => commands::cmd_search(&query),
            Commands::Library => commands::cmd_library(),
            Commands::Probe { path } => commands::cmd_probe(&path),
            Commands::Devices => commands::cmd_devices(),
            Commands::Config => commands::cmd_config(),
            Commands::Remote(sub) => match sub {
                RemoteCommands::Login { url, username } => {
                    commands::cmd_remote_login(&url, &username)
                }
                RemoteCommands::Sync { full } => commands::cmd_remote_sync(full),
                RemoteCommands::Status => commands::cmd_remote_status(),
            },
            Commands::Init => commands::cmd_init(),
            Commands::Cache(sub) => match sub {
                CacheCommands::Status => commands::cmd_cache_status(),
                CacheCommands::Clear { yes } => commands::cmd_cache_clear(yes),
            },
            Commands::Completions { shell } => {
                clap_complete::generate(shell, &mut Cli::command(), "koan", &mut io::stdout());
            }
        }
        return;
    }

    // No subcommand — unified player process.

    // MCP mode: headless MCP server on stdio.
    // Daemon mode: fork a headless child and exit.
    if cli.daemonize {
        commands::cmd_serve_daemon(cli.port, cli.subsonic, cli.playground);
        return;
    }

    // Headless mode: GraphQL API server, no TUI.
    if cli.headless {
        commands::cmd_serve(cli.port, cli.subsonic, cli.playground);
        return;
    }

    // Default: TUI mode (with optional API server alongside).
    // CLI flags override config values.
    let cfg = koan_core::config::Config::load_or_default();

    if let Some(ref url) = cli.server {
        commands::cmd_play_remote(url, cli.jukebox);
    } else {
        let api_enabled = !cli.no_api && cfg.graphql.enabled;
        let api_opts = if api_enabled {
            Some(commands::ApiOptions {
                port: cli.port.or(Some(cfg.graphql.port)),
                subsonic: cli.subsonic.or(cfg.graphql.subsonic_port),
                playground: cli.playground || cfg.graphql.playground,
            })
        } else {
            None
        };
        commands::cmd_play(
            &cli.paths,
            &cli.ids,
            cli.album,
            cli.artist,
            cli.library,
            cli.clear,
            api_opts,
        );
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
            let label = format!("{} \u{2014} {}", a.artist_name, a.title);
            CompletionCandidate::new(a.id.to_string()).help(Some(label.into()))
        })
        .collect()
}

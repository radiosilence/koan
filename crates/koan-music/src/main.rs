use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::PathBuf;
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
mod tui;

#[derive(Parser)]
#[command(name = "koan", about = "bit-perfect music player", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
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
        /// Open the TUI in library browse mode
        #[arg(long, short = 'l')]
        library: bool,
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
    /// Interactive fuzzy picker
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
        // No subcommand — open TUI (use `l` to browse library).
        None => commands::cmd_play(&[], &[], None, None, false),
        Some(Commands::Play {
            paths,
            ids,
            album,
            artist,
            library,
        }) => commands::cmd_play(&paths, &ids, album, artist, library),
        Some(Commands::Probe { path }) => commands::cmd_probe(&path),
        Some(Commands::Devices) => commands::cmd_devices(),
        Some(Commands::Scan { path, force }) => commands::cmd_scan(path.as_deref(), force),
        Some(Commands::Search { query }) => commands::cmd_search(&query),
        Some(Commands::Artists { query }) => commands::cmd_artists(query.as_deref()),
        Some(Commands::Albums { query }) => commands::cmd_albums(query.as_deref()),
        Some(Commands::Library) => commands::cmd_library(),
        Some(Commands::Config) => commands::cmd_config(),
        Some(Commands::Remote(sub)) => match sub {
            RemoteCommands::Login { url, username } => commands::cmd_remote_login(&url, &username),
            RemoteCommands::Sync { full } => commands::cmd_remote_sync(full),
            RemoteCommands::Status => commands::cmd_remote_status(),
        },
        Some(Commands::Pick {
            query,
            album,
            artist,
        }) => commands::cmd_pick(query.as_deref(), album, artist),
        Some(Commands::Init) => commands::cmd_init(),
        Some(Commands::Cache(sub)) => match sub {
            CacheCommands::Status => commands::cmd_cache_status(),
            CacheCommands::Clear { yes } => commands::cmd_cache_clear(yes),
        },
        Some(Commands::Completions { shell }) => {
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
            let label = format!("{} \u{2014} {}", a.artist_name, a.title);
            CompletionCandidate::new(a.id.to_string()).help(Some(label.into()))
        })
        .collect()
}

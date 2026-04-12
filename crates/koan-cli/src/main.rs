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

#[derive(Parser)]
#[command(name = "koan", about = "bit-perfect music player", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

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

    /// Bind address for the API server (default: 127.0.0.1)
    #[arg(long)]
    bind: Option<std::net::IpAddr>,

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
    /// Play audio files or open the TUI player
    Play {
        /// Paths to audio files
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
    },
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
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
    /// Manage remote Subsonic/Navidrome server
    #[command(subcommand)]
    Remote(RemoteCommands),
    /// Manage the download cache
    #[command(subcommand)]
    Cache(CacheCommands),
    /// Manage authentication (users, tokens)
    #[command(subcommand)]
    Auth(AuthCommands),
    /// Generate shell completions
    Completions {
        /// Shell to generate for
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Initialise or sync config directory with default config
    Init,
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
    /// Evict least-recently-played albums until cache is within limit
    Evict,
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Initial setup — generate keypair and create first admin user
    Setup,
    /// Create a new user
    CreateUser {
        /// Username
        #[arg(long)]
        username: String,
        /// Role (admin, user, readonly)
        #[arg(long, default_value = "user")]
        role: String,
    },
    /// Delete a user
    DeleteUser {
        /// Username to delete
        username: String,
    },
    /// List all users
    ListUsers,
    /// Log in to a koan server and store refresh token
    Login {
        /// Server URL (e.g. http://localhost:4000)
        #[arg(long, default_value = "http://127.0.0.1:4000")]
        server: String,
        /// Username
        #[arg(long)]
        username: String,
    },
    /// Log out (revoke token and clear keychain)
    Logout {
        /// Server URL
        #[arg(long, default_value = "http://127.0.0.1:4000")]
        server: String,
    },
    /// Reset a user's password
    ResetPassword {
        /// Username
        username: String,
    },
    /// Change a user's role
    SetRole {
        /// Username
        username: String,
        /// New role (admin, user, readonly)
        role: String,
    },
    /// Regenerate Ed25519 keypair (invalidates all existing tokens)
    RegenerateKeys,
    /// Delete all auth state (keys, users, tokens) — nuclear option
    Reset,
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

    let mut cli = Cli::parse();
    let command = cli.command.take();

    // Daemon/headless are root-level server modes — handle before subcommands.
    if cli.daemonize {
        koan_server::graphql::cmd_serve_daemon(cli.port, cli.bind, cli.subsonic, cli.playground);
        return;
    }
    if cli.headless {
        koan_server::graphql::cmd_serve(cli.port, cli.bind, cli.subsonic, cli.playground);
        return;
    }

    match command {
        Some(Commands::Play {
            paths,
            ids,
            album,
            artist,
            library,
            clear,
            server,
            jukebox,
        }) => {
            start_player(
                &cli, &paths, &ids, album, artist, library, clear, server, jukebox,
            );
        }
        Some(Commands::Scan {
            path,
            force,
            analyze,
        }) => {
            commands::cmd_scan(path.as_deref(), force);
            if analyze {
                commands::cmd_analyze();
            }
        }
        Some(Commands::Mcp) => koan_server::mcp::cmd_mcp(),
        Some(Commands::Analyze) => commands::cmd_analyze(),
        Some(Commands::Search { query }) => commands::cmd_search(&query),
        Some(Commands::Library) => commands::cmd_library(),
        Some(Commands::Probe { path }) => commands::cmd_probe(&path),
        Some(Commands::Devices) => commands::cmd_devices(),
        Some(Commands::Config { command }) => match command {
            Some(ConfigCommands::Init) => commands::cmd_init(),
            None => commands::cmd_config(),
        },
        Some(Commands::Remote(sub)) => match sub {
            RemoteCommands::Login { url, username } => commands::cmd_remote_login(&url, &username),
            RemoteCommands::Sync { full } => commands::cmd_remote_sync(full),
            RemoteCommands::Status => commands::cmd_remote_status(),
        },
        Some(Commands::Cache(sub)) => match sub {
            CacheCommands::Status => commands::cmd_cache_status(),
            CacheCommands::Clear { yes } => commands::cmd_cache_clear(yes),
            CacheCommands::Evict => {
                let cfg = config::Config::load().unwrap_or_default();
                let freed = commands::evict_cache(&cfg, true);
                if freed == 0 {
                    println!("cache within limit, nothing to evict");
                }
            }
        },
        Some(Commands::Auth(sub)) => match sub {
            AuthCommands::Setup => commands::cmd_auth_setup(),
            AuthCommands::CreateUser { username, role } => {
                commands::cmd_auth_create_user(&username, &role);
            }
            AuthCommands::DeleteUser { username } => commands::cmd_auth_delete_user(&username),
            AuthCommands::ListUsers => commands::cmd_auth_list_users(),
            AuthCommands::Login { server, username } => {
                commands::cmd_auth_login(&server, &username);
            }
            AuthCommands::Logout { server } => commands::cmd_auth_logout(&server),
            AuthCommands::ResetPassword { username } => {
                commands::cmd_auth_reset_password(&username);
            }
            AuthCommands::SetRole { username, role } => {
                commands::cmd_auth_set_role(&username, &role);
            }
            AuthCommands::RegenerateKeys => commands::cmd_auth_regenerate_keys(),
            AuthCommands::Reset => commands::cmd_auth_reset(),
        },
        Some(Commands::Completions { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "koan", &mut io::stdout());
        }
        // No subcommand — default to TUI player (equivalent to `koan play`).
        None => {
            start_player(&cli, &[], &[], None, None, false, false, None, false);
        }
    }
}

/// Launch the player/TUI. Shared by `koan play` and bare `koan` (no subcommand).
#[allow(clippy::too_many_arguments)]
fn start_player(
    cli: &Cli,
    paths: &[PathBuf],
    ids: &[i64],
    album: Option<i64>,
    artist: Option<i64>,
    start_in_library: bool,
    clear: bool,
    server: Option<String>,
    jukebox: bool,
) {
    let cfg = koan_core::config::Config::load_or_default();
    commands::evict_cache(&cfg, false);

    if let Some(ref url) = server {
        commands::cmd_play_remote(url, jukebox);
    } else {
        let api_enabled = !cli.no_api && cfg.graphql.enabled;
        let api_opts = if api_enabled {
            Some(commands::ApiOptions {
                port: cli.port.or(Some(cfg.graphql.port)),
                bind: cli.bind.or(Some(cfg.graphql.bind)),
                subsonic: cli.subsonic.or(cfg.graphql.subsonic_port),
                playground: cli.playground || cfg.graphql.playground,
            })
        } else {
            None
        };
        commands::cmd_play(paths, ids, album, artist, start_in_library, clear, api_opts);
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_no_args_parses() {
        let cli = Cli::try_parse_from(["koan"]).unwrap();
        assert!(cli.command.is_none());
        assert!(!cli.headless);
    }

    #[test]
    fn cli_play_subcommand_parses() {
        let cli = Cli::try_parse_from(["koan", "play", "/tmp/test.flac"]).unwrap();
        match cli.command {
            Some(Commands::Play { ref paths, .. }) => {
                assert_eq!(paths.len(), 1);
                assert_eq!(paths[0].to_str().unwrap(), "/tmp/test.flac");
            }
            _ => panic!("expected Play subcommand"),
        }
    }

    #[test]
    fn cli_play_with_flags_parses() {
        let cli =
            Cli::try_parse_from(["koan", "play", "--library", "--clear", "--id", "42"]).unwrap();
        match cli.command {
            Some(Commands::Play {
                library,
                clear,
                ref ids,
                ..
            }) => {
                assert!(library);
                assert!(clear);
                assert_eq!(ids, &[42]);
            }
            _ => panic!("expected Play subcommand"),
        }
    }

    #[test]
    fn cli_headless_flag_on_root() {
        let cli = Cli::try_parse_from(["koan", "--headless"]).unwrap();
        assert!(cli.headless);
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_paths_on_root_rejected() {
        // Positional paths should NOT parse on the root command — they live under `play`.
        let result = Cli::try_parse_from(["koan", "/tmp/test.flac"]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_scan_still_works() {
        let cli = Cli::try_parse_from(["koan", "scan", "--force"]).unwrap();
        match cli.command {
            Some(Commands::Scan { force, .. }) => assert!(force),
            _ => panic!("expected Scan subcommand"),
        }
    }

    #[test]
    fn cli_play_server_requires_url() {
        let cli =
            Cli::try_parse_from(["koan", "play", "--server", "http://localhost:4000"]).unwrap();
        match cli.command {
            Some(Commands::Play { ref server, .. }) => {
                assert_eq!(server.as_deref(), Some("http://localhost:4000"));
            }
            _ => panic!("expected Play subcommand"),
        }
    }

    #[test]
    fn cli_play_jukebox_requires_server() {
        let result = Cli::try_parse_from(["koan", "play", "--jukebox"]);
        assert!(result.is_err());
    }
}

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::env::CompleteEnv;
use koan_core::config;
use koan_core::db::connection::Database;
use koan_core::db::queries;

/// glibc's allocator gives each thread that allocates heavily an arena of its
/// own and rarely returns what is freed in one, so a server that does bursts
/// of work across many threads only ever grows. mimalloc gives freed memory
/// back to the system.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// --- Logger ---
// All log messages go to ~/.config/koan/koan.log.
// During playback, they're also buffered for the queue display.
// Outside playback, they also go to stderr.

static LOGGER: OnceLock<BufferedLogger> = OnceLock::new();

struct BufferedLogger {
    buffer: Mutex<Option<Arc<Mutex<Vec<String>>>>>,
    log_file: Mutex<config::LogFile>,
}

impl BufferedLogger {
    fn init() {
        let logger = LOGGER.get_or_init(|| BufferedLogger {
            buffer: Mutex::new(None),
            log_file: Mutex::new(config::LogFile::default()),
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
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        self.log_file
            .lock()
            .unwrap()
            .write(format_args!("[{}] {}", now, msg));

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
        self.log_file.lock().unwrap().flush();
    }
}

mod commands;

#[derive(Parser)]
#[command(name = "koan", about = "bit-perfect music player", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // --- Server flags (unified process) ---
    /// Run the server without the TUI
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

    /// Also serve the Subsonic API on a dedicated port (e.g. --subsonic 4040), for
    /// clients that expect one. Once enabled (`koan subsonic setup`) it is always
    /// on the API port as well.
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

        /// Control a kōan server's playback (e.g. http://host:4000). The
        /// server plays the audio.
        #[arg(long)]
        server: Option<String>,

        /// What `--server` always does now. Accepted so scripts that pass it
        /// keep working.
        #[arg(long, requires = "server", hide = true)]
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
        /// Delete stale tracks even when so many are missing that it looks like an
        /// unmounted volume. Takes their play history and lyrics too.
        #[arg(long)]
        force_remove: bool,
    },
    /// Search the library
    Search {
        /// Search query
        query: String,
    },
    /// Show library statistics
    Library,
    /// Check that this build can open the database: its migrations are run
    /// on a snapshot, leaving the original untouched. Exits non-zero if not.
    CheckDb {
        /// Database to check (defaults to the configured one)
        path: Option<PathBuf>,
    },
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
    /// EQ per output device: a correction that makes it neutral, a tuning
    /// of EQs on top, and presets that save the two
    #[command(before_help = DSP_EXAMPLES)]
    Dsp {
        #[command(subcommand)]
        command: Option<DspCommands>,
    },
    /// Manage authentication (users, tokens)
    Auth(AuthArgs),
    /// Manage kōan's own Subsonic REST API
    #[command(subcommand)]
    Subsonic(SubsonicCommands),
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
        /// Accepted and ignored: every sync walks the whole library.
        #[arg(long, hide = true)]
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
enum DspCommands {
    /// What a device plays (the current output by default): the chain in a
    /// sentence, then its preset, correction and tuning
    Show {
        device: Option<String>,
        /// As JSON: device, correction, target, tuning, preset, edited,
        /// left_out, notes, flat, sentence
        #[arg(long)]
        json: bool,
    },
    /// Set a device's whole chain: its correction and the EQs of its tuning,
    /// in order. Repeating it changes nothing
    #[command(group(clap::ArgGroup::new("chain").required(true).multiple(true).args(["correction", "tuning"])))]
    Set {
        device: String,
        /// A correction's name, or `none`
        #[arg(long)]
        correction: Option<String>,
        /// EQs in the order they play, separated by commas, or `none`
        #[arg(long, value_delimiter = ',')]
        tuning: Option<Vec<String>>,
    },
    /// Make a device (the current output by default) flat: no correction
    /// and no tuning, so it plays untouched
    Flat { device: Option<String> },
    /// Every correction, EQ and preset, and the devices each is used on
    List {
        /// As JSON: name, kind, used_on, edited, members, problem
        #[arg(long)]
        json: bool,
    },
    /// Make or update an EQ or correction from files: AutoEQ or Equalizer
    /// APO text, impulse WAVs, Roon zips and Convolver .cfg, CamillaDSP YAML,
    /// raw or text coefficients. Files, folders or zips; an existing one of
    /// the same name keeps what this does not replace
    Import {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Its name (defaults to the first file's name)
        #[arg(long)]
        name: Option<String>,
        /// Sample rate of coefficients that do not say
        #[arg(long)]
        rate: Option<u32>,
        /// Also play this output device through it
        #[arg(long)]
        device: Option<String>,
    },
    /// The same as `set DEVICE --correction NAME`, or `preset use` for a
    /// preset, under its old name
    #[command(hide = true)]
    Use {
        name: String,
        #[arg(long)]
        device: Option<String>,
    },
    /// The same as `flat`, under its old name
    #[command(hide = true)]
    Clear {
        #[arg(long)]
        device: Option<String>,
    },
    /// Delete a correction, EQ or preset
    Remove { name: String },
    /// Move an AutoEQ correction to another target, or back to its own
    Target {
        /// The correction
        name: String,
        /// The target to use: an id from the list this prints without it
        #[arg(long = "use")]
        target: Option<String>,
        /// Play it as it was made, for the target it was made for
        #[arg(long, conflicts_with = "target")]
        reset: bool,
    },
    /// Add a target to choose from: a CSV of frequency and level, or a
    /// squig.link export
    AddTarget { path: PathBuf },
    /// Correct a headphone from its measurement (a CSV of frequency and
    /// level, or a squig.link export) to a target. The file is taken as it
    /// is: a squig.link export already has both channels averaged and the
    /// site's calibration applied, but a site's raw `L.txt` or `R.txt` has
    /// neither. `koan dsp squig` fetches both and does both
    Measure {
        path: PathBuf,
        /// The headphone, as the correction's name
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = ["in", "over"])]
        ear: String,
        /// A target id, as `koan dsp target` lists them
        #[arg(long)]
        target: String,
        /// `graphic`, a smoothed curve, or `squig`, the parametric bands
        /// squig.link's auto-EQ fits
        #[arg(long, value_parser = ["graphic", "squig"], default_value = "graphic")]
        fit: String,
    },
    /// Split a correction that already includes a tuning into a correction
    /// from the headphones' measurement and an EQ holding the rest; the
    /// outputs that played it play both
    Split {
        name: String,
        /// The headphones' measurement, a squig.link or REW CSV
        path: PathBuf,
        #[arg(long, value_parser = ["in", "over"])]
        ear: String,
        /// The target that counts as neutral, as `koan dsp target` lists them
        #[arg(long)]
        target: String,
    },
    /// What a device's chain plays, as CSV of frequency and dB on AutoEQ's
    /// grid, preamp aside. `--correction` and `--tuning` stand in for the
    /// device's own for this one reading; nothing is saved
    Response {
        device: Option<String>,
        /// A correction's name, in place of the device's
        #[arg(long)]
        correction: Option<String>,
        /// EQs in the order they play, separated by commas, in place of the
        /// device's tuning
        #[arg(long, value_delimiter = ',')]
        tuning: Option<Vec<String>>,
        #[arg(long, default_value_t = 48_000)]
        rate: u32,
    },
    /// Say what an EQ is for: a neutral correction, a tuning on top of
    /// one, or `mixed`, a correction that already includes a tuning. A
    /// device has one correction
    Role {
        name: String,
        #[arg(value_parser = clap::builder::PossibleValuesParser::new([
            clap::builder::PossibleValue::new("correction"),
            clap::builder::PossibleValue::new("tuning"),
            clap::builder::PossibleValue::new("mixed").alias("baked"),
        ]))]
        role: String,
    },
    /// Search squig.link sites for a headphone's measurement, or with
    /// `--name`, correct it from the result numbered NUMBER
    Squig {
        query: String,
        #[arg(long)]
        limit: Option<usize>,
        /// A result's number, to make a correction from
        #[arg(long)]
        use_result: Option<usize>,
        /// The correction's name, with `--use-result`
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = ["in", "over"])]
        ear: Option<String>,
        /// A target id, as `koan dsp target` lists them
        #[arg(long)]
        target: Option<String>,
        /// With `--use-result`: `graphic`, a smoothed curve, or `squig`, the
        /// parametric bands squig.link's auto-EQ fits
        #[arg(long, value_parser = ["graphic", "squig"], default_value = "graphic")]
        fit: String,
    },
    /// The target a ready-made EQ was made for, or `unknown`, which leaves
    /// target switching off
    MadeFor { name: String, target: String },
    /// The same as `set DEVICE --tuning`, with EQs kept but off
    #[command(hide = true)]
    Tuning {
        #[arg(required = true)]
        names: Vec<String>,
        /// EQs kept in the tuning but switched off
        #[arg(long = "off")]
        off: Vec<String>,
        #[arg(long)]
        device: Option<String>,
    },
    /// Presets: an output's correction and tuning saved together
    Preset {
        #[command(subcommand)]
        command: PresetCommands,
    },
    /// Put an imported EQ back as it was imported
    Revert { name: String },
    /// Copy a correction, EQ or preset as it is now, used by no output
    Copy { name: String, new: Option<String> },
    /// The target a tuning was made against, or `unknown`. On headphones
    /// corrected to another, the difference plays first
    TunedFor { name: String, target: String },
    /// One EQ built from others
    Eq {
        #[command(subcommand)]
        command: EqCommands,
    },
    /// The same as `eq plays`, under its old name
    #[command(hide = true)]
    Stack { name: String, layers: Vec<String> },
    /// The same as `eq switch`, under its old name
    #[command(hide = true)]
    Layer {
        stack: String,
        layer: String,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Find a headphone's correction in AutoEQ's results and install it
    Autoeq {
        #[command(subcommand)]
        command: AutoeqCommands,
    },
}

#[derive(Subcommand)]
enum PresetCommands {
    /// Save an output's correction and tuning (the current output by
    /// default) as a preset
    Save {
        name: String,
        #[arg(long)]
        device: Option<String>,
    },
    /// Set an output from a preset, or `flat`: no correction and no tuning
    Use {
        name: String,
        #[arg(long)]
        device: Option<String>,
    },
    /// The presets, and the devices set from each
    List {
        /// As JSON: name, used_on, edited
        #[arg(long)]
        json: bool,
    },
}

/// Opens `koan dsp --help`: what a person or an assistant usually wants.
const DSP_EXAMPLES: &str = "Examples:
  koan dsp set \"Scarlett 4i4 USB\" --correction \"Wharfedale EVO 4.1\" --tuning Lush
      Correct the Scarlett with the Wharfedale correction, Lush on top.
  koan dsp preset save \"Desk\" --device \"Scarlett 4i4 USB\"
      Save that as the preset Desk, then `koan dsp preset use Desk` brings it back.
  koan dsp show \"Scarlett 4i4 USB\" --json
      What it plays, for a script.";

#[derive(Subcommand)]
enum EqCommands {
    /// Make an EQ that plays others in the order given. A device's
    /// correction and tuning are set with `set` and `preset`; this
    /// builds one EQ from several. Creates it if need be
    Plays { name: String, eqs: Vec<String> },
    /// Switch one of the EQs it plays on or off
    Switch {
        name: String,
        eq: String,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
}

#[derive(Subcommand)]
enum AutoeqCommands {
    /// Search AutoEQ's index by headphone name; numbers are what install takes
    Search {
        query: String,
        /// How many matches to show
        #[arg(long, default_value_t = 15)]
        limit: usize,
        /// Fetch the index again even if the copy kept is recent
        #[arg(long)]
        refresh: bool,
    },
    /// Install a result as a correction: its number from search, or its name
    Install {
        entry: String,
        /// Who measured it, where several sources have the same headphone
        /// (AutoEQ's preferred one by default)
        #[arg(long)]
        source: Option<String>,
        /// Also play this output device through it
        #[arg(long, alias = "output")]
        device: Option<String>,
    },
}

#[derive(Subcommand)]
enum SubsonicCommands {
    /// Enable the Subsonic API and generate its own secret
    Setup {
        /// Username Subsonic clients authenticate as
        #[arg(long, default_value = "koan")]
        username: String,
    },
    /// Show whether the Subsonic API is enabled and configured
    Status,
    /// Disable the Subsonic API and delete its secret
    Disable,
}

#[derive(clap::Args)]
struct AuthArgs {
    /// Never prompt: credentials come from KOAN_USERNAME and KOAN_PASSWORD,
    /// confirmations from --yes. Implied when stdin is not a terminal
    #[arg(long, global = true)]
    non_interactive: bool,
    #[command(subcommand)]
    command: AuthCommands,
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Initial setup — generate keypair and create first admin user
    Setup {
        /// Save the new credentials to 1Password without asking
        #[arg(long)]
        save_to_1password: bool,
    },
    /// Create a new user
    CreateUser {
        /// Username
        #[arg(long)]
        username: String,
        /// Role (admin, user, readonly)
        #[arg(long, default_value = "user")]
        role: String,
        /// Save the new credentials to 1Password without asking
        #[arg(long)]
        save_to_1password: bool,
    },
    /// Delete a user
    DeleteUser {
        /// Username to delete
        username: String,
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List all users
    ListUsers,
    /// Log in to a kōan server and store refresh token
    Login {
        /// Server URL (e.g. http://localhost:4000)
        #[arg(long, default_value = "http://127.0.0.1:4000")]
        server: String,
        /// Username
        #[arg(long)]
        username: String,
    },
    /// Log out (revoke the token and forget it)
    Logout {
        /// Server URL
        #[arg(long, default_value = "http://127.0.0.1:4000")]
        server: String,
    },
    /// Reset a user's password
    ResetPassword {
        /// Username
        username: String,
        /// Save the new credentials to 1Password without asking
        #[arg(long)]
        save_to_1password: bool,
    },
    /// Change a user's role
    SetRole {
        /// Username
        username: String,
        /// New role (admin, user, readonly)
        role: String,
    },
    /// Print an invite for a user: the link that sets koan up with the
    /// account, and the email to send it in
    Invite {
        /// Username
        username: String,
        /// Address clients reach this server at (default: sharing.public_url)
        #[arg(long)]
        server: Option<String>,
        /// Also give the account a new password, put in the email, signing its
        /// existing devices out
        #[arg(long)]
        reset_password: bool,
    },
    /// Manage Subsonic API keys
    #[command(subcommand)]
    ApiKey(ApiKeyCommands),
    /// Regenerate Ed25519 keypair (invalidates all existing tokens)
    RegenerateKeys {
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Delete all auth state: keys, users and tokens
    Reset {
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ApiKeyCommands {
    /// Make a key for a user; it is printed once
    Create {
        /// The user the key signs in as
        #[arg(long)]
        username: String,
        /// What the key is for, to tell keys apart when revoking
        #[arg(long)]
        name: String,
    },
    /// List keys, for every user or one
    List {
        #[arg(long)]
        username: Option<String>,
    },
    /// Revoke a key by id
    Revoke {
        /// Key id, from `koan auth api-key list`
        id: i64,
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
            jukebox: _,
        }) => {
            start_player(&cli, &paths, &ids, album, artist, library, clear, server);
        }
        Some(Commands::Scan {
            path,
            force,
            force_remove,
        }) => commands::cmd_scan(path.as_deref(), force, force_remove),
        Some(Commands::Mcp) => koan_server::mcp::cmd_mcp(),
        Some(Commands::Search { query }) => commands::cmd_search(&query),
        Some(Commands::Library) => commands::cmd_library(),
        Some(Commands::CheckDb { path }) => {
            let path = path.unwrap_or_else(koan_core::config::db_path);
            if !path.exists() {
                println!("{}: no database yet, nothing to migrate", path.display());
                return;
            }
            match koan_core::db::connection::Database::check_upgrade(&path) {
                Ok(()) => println!("{}: opens with this build", path.display()),
                Err(e) => {
                    eprintln!("{}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Probe { path }) => commands::cmd_probe(&path),
        Some(Commands::Devices) => commands::cmd_devices(),
        Some(Commands::Config { command }) => match command {
            Some(ConfigCommands::Init) => commands::cmd_init(),
            None => commands::cmd_config(),
        },
        Some(Commands::Remote(sub)) => match sub {
            RemoteCommands::Login { url, username } => commands::cmd_remote_login(&url, &username),
            RemoteCommands::Sync { .. } => commands::cmd_remote_sync(),
            RemoteCommands::Status => commands::cmd_remote_status(),
        },
        Some(Commands::Dsp { command }) => {
            // The background sync an edit starts would die with the process.
            koan_core::remote::dsp_sync::defer();
            dsp(command);
            commands::cmd_dsp_flush();
        }
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
        Some(Commands::Auth(AuthArgs {
            non_interactive,
            command,
        })) => {
            let tty = commands::Tty::detect(non_interactive);
            match command {
                AuthCommands::Setup { save_to_1password } => {
                    commands::cmd_auth_setup(tty, save_to_1password);
                }
                AuthCommands::CreateUser {
                    username,
                    role,
                    save_to_1password,
                } => commands::cmd_auth_create_user(tty, &username, &role, save_to_1password),
                AuthCommands::DeleteUser { username, yes } => {
                    commands::cmd_auth_delete_user(tty, &username, yes);
                }
                AuthCommands::ListUsers => commands::cmd_auth_list_users(),
                AuthCommands::Login { server, username } => {
                    commands::cmd_auth_login(tty, &server, &username);
                }
                AuthCommands::Logout { server } => commands::cmd_auth_logout(&server),
                AuthCommands::ResetPassword {
                    username,
                    save_to_1password,
                } => commands::cmd_auth_reset_password(tty, &username, save_to_1password),
                AuthCommands::SetRole { username, role } => {
                    commands::cmd_auth_set_role(&username, &role);
                }
                AuthCommands::Invite {
                    username,
                    server,
                    reset_password,
                } => commands::cmd_auth_invite(&username, server.as_deref(), reset_password),
                AuthCommands::ApiKey(sub) => match sub {
                    ApiKeyCommands::Create { username, name } => {
                        commands::cmd_auth_api_key_create(&username, &name);
                    }
                    ApiKeyCommands::List { username } => {
                        commands::cmd_auth_api_key_list(username.as_deref());
                    }
                    ApiKeyCommands::Revoke { id } => commands::cmd_auth_api_key_revoke(id),
                },
                AuthCommands::RegenerateKeys { yes } => {
                    commands::cmd_auth_regenerate_keys(tty, yes)
                }
                AuthCommands::Reset { yes } => commands::cmd_auth_reset(tty, yes),
            }
        }
        Some(Commands::Subsonic(sub)) => match sub {
            SubsonicCommands::Setup { username } => commands::cmd_subsonic_setup(&username),
            SubsonicCommands::Status => commands::cmd_subsonic_status(),
            SubsonicCommands::Disable => commands::cmd_subsonic_disable(),
        },
        Some(Commands::Completions { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "koan", &mut io::stdout());
        }
        // No subcommand — default to TUI player (equivalent to `koan play`).
        None => {
            start_player(&cli, &[], &[], None, None, false, false, None);
        }
    }
}

fn dsp(command: Option<DspCommands>) {
    match command.unwrap_or(DspCommands::Show {
        device: None,
        json: false,
    }) {
        DspCommands::Show { device, json } => commands::cmd_dsp_show(device, json),
        DspCommands::Set {
            device,
            correction,
            tuning,
        } => commands::cmd_dsp_set(&device, correction.as_deref(), tuning.as_deref()),
        DspCommands::Flat { device } => commands::cmd_dsp_flat(device),
        DspCommands::List { json } => commands::cmd_dsp_list(json),
        DspCommands::Import {
            paths,
            name,
            rate,
            device,
        } => commands::cmd_dsp_import(&paths, name, rate, device),
        DspCommands::Use { name, device } => commands::cmd_dsp_use(&name, device),
        DspCommands::Clear { device } => commands::cmd_dsp_clear(device),
        DspCommands::Remove { name } => commands::cmd_dsp_remove(&name),
        DspCommands::Target {
            name,
            target,
            reset,
        } => commands::cmd_dsp_target(&name, target.as_deref(), reset),
        DspCommands::AddTarget { path } => commands::cmd_dsp_add_target(&path),
        DspCommands::Measure {
            path,
            name,
            ear,
            target,
            fit,
        } => commands::cmd_dsp_measure(&path, &name, ear == "in", &target, fit == "squig"),
        DspCommands::Split {
            name,
            path,
            ear,
            target,
        } => commands::cmd_dsp_split(&name, &path, ear == "in", &target),
        DspCommands::Role { name, role } => commands::cmd_dsp_role(&name, &role),
        DspCommands::Response {
            device,
            correction,
            tuning,
            rate,
        } => commands::cmd_dsp_response(device, correction.as_deref(), tuning.as_deref(), rate),
        DspCommands::Squig {
            query,
            limit,
            use_result,
            name,
            ear,
            target,
            fit,
        } => commands::cmd_dsp_squig(
            &query,
            limit.unwrap_or(20),
            use_result,
            name.as_deref(),
            ear.as_deref().map(|e| e == "in"),
            target.as_deref(),
            fit == "squig",
        ),
        DspCommands::MadeFor { name, target } => {
            commands::cmd_dsp_made_for(&name, Some(target.as_str()).filter(|t| *t != "unknown"))
        }
        DspCommands::Tuning { names, off, device } => {
            commands::cmd_dsp_tuning(&names, &off, device)
        }
        DspCommands::Preset { command } => match command {
            PresetCommands::List { json } => commands::cmd_dsp_preset_list(json),
            PresetCommands::Save { name, device } => commands::cmd_dsp_preset_save(&name, device),
            PresetCommands::Use { name, device } => {
                commands::cmd_dsp_preset_use(Some(name.as_str()).filter(|n| *n != "flat"), device)
            }
        },
        DspCommands::Revert { name } => commands::cmd_dsp_revert(&name),
        DspCommands::Copy { name, new } => commands::cmd_dsp_copy(&name, new.as_deref()),
        DspCommands::TunedFor { name, target } => {
            commands::cmd_dsp_tuned_for(&name, Some(target.as_str()).filter(|t| *t != "unknown"))
        }
        DspCommands::Eq { command } => match command {
            EqCommands::Plays { name, eqs } => commands::cmd_dsp_stack(&name, &eqs),
            EqCommands::Switch { name, eq, state } => {
                commands::cmd_dsp_layer(&name, &eq, state == "on")
            }
        },
        DspCommands::Stack { name, layers } => commands::cmd_dsp_stack(&name, &layers),
        DspCommands::Layer {
            stack,
            layer,
            state,
        } => commands::cmd_dsp_layer(&stack, &layer, state == "on"),
        DspCommands::Autoeq { command } => match command {
            AutoeqCommands::Search {
                query,
                limit,
                refresh,
            } => commands::cmd_dsp_autoeq_search(&query, limit, refresh),
            AutoeqCommands::Install {
                entry,
                source,
                device,
            } => commands::cmd_dsp_autoeq_install(&entry, source.as_deref(), device),
        },
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
) {
    let cfg = koan_core::config::Config::load_or_default();
    commands::evict_cache(&cfg, false);

    if let Some(ref url) = server {
        commands::cmd_play_remote(url);
    } else {
        let api_enabled = !cli.no_api && cfg.graphql.enabled;
        let api_opts = if api_enabled {
            Some(commands::ApiOptions {
                port: cli.port.or(Some(cfg.graphql.port)),
                bind: cli.bind.or(Some(cfg.graphql.bind)),
                subsonic: cli.subsonic,
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

    fn dsp(args: &[&str]) -> DspCommands {
        match Cli::try_parse_from(args).map(|c| c.command) {
            Ok(Some(Commands::Dsp { command: Some(cmd) })) => cmd,
            Ok(_) => panic!("{args:?} is not a dsp command"),
            Err(e) => panic!("{args:?}: {e}"),
        }
    }

    /// The EQ subcommands go by device, correction, tuning, EQ and preset;
    /// the old names still parse, out of the help, so scripts keep working.
    #[test]
    fn dsp_words_and_their_old_names() {
        assert!(matches!(
            dsp(&["koan", "dsp", "set", "Scarlett", "--correction", "Wharfedale", "--tuning", "Lush,Air"]),
            DspCommands::Set { correction: Some(c), tuning: Some(t), .. } if c == "Wharfedale" && t == ["Lush", "Air"]
        ));
        assert!(
            Cli::try_parse_from(["koan", "dsp", "set", "Scarlett"]).is_err(),
            "set names a correction, a tuning or both"
        );
        assert!(matches!(
            dsp(&["koan", "dsp", "show", "--json"]),
            DspCommands::Show {
                device: None,
                json: true
            }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "flat", "Scarlett"]),
            DspCommands::Flat { device: Some(_) }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "preset", "list", "--json"]),
            DspCommands::Preset {
                command: PresetCommands::List { json: true }
            }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "eq", "plays", "Desk", "HD 650", "Bass"]),
            DspCommands::Eq { command: EqCommands::Plays { eqs, .. } } if eqs == ["HD 650", "Bass"]
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "eq", "switch", "Desk", "Bass", "off"]),
            DspCommands::Eq {
                command: EqCommands::Switch { .. }
            }
        ));
        // The old names.
        assert!(matches!(
            dsp(&["koan", "dsp", "stack", "Desk", "Bass"]),
            DspCommands::Stack { .. }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "layer", "Desk", "Bass", "on"]),
            DspCommands::Layer { .. }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "use", "HD 650"]),
            DspCommands::Use { .. }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "clear"]),
            DspCommands::Clear { .. }
        ));
        assert!(matches!(
            dsp(&["koan", "dsp", "tuning", "none"]),
            DspCommands::Tuning { .. }
        ));
        for role in ["mixed", "baked"] {
            assert!(matches!(
                dsp(&["koan", "dsp", "role", "Lush", role]),
                DspCommands::Role { .. }
            ));
        }

        let mut cli = Cli::command();
        let dsp = cli.find_subcommand_mut("dsp").unwrap();
        let shown: Vec<&str> = dsp
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(|c| c.get_name())
            .collect();
        for gone in ["stack", "layer", "use", "clear", "tuning"] {
            assert!(
                !shown.contains(&gone),
                "{gone} is out of the help: {shown:?}"
            );
        }
        for word in ["show", "set", "flat", "list", "preset", "eq"] {
            assert!(shown.contains(&word), "{word}: {shown:?}");
        }
        let help = dsp.render_long_help().to_string();
        assert!(help.starts_with("Examples:"), "{help}");
    }

    /// `koan dsp show --json` keeps these keys, which scripts and assistants
    /// read.
    #[test]
    fn dsp_show_json_keys() {
        use koan_core::audio::dsp::profiles::{ChainView, Join, TuningView};
        let view = ChainView {
            device: "Scarlett".into(),
            correction: Some("Wharfedale".into()),
            target: None,
            tuning: vec![TuningView {
                name: "Lush".into(),
                on: true,
                made_for: Some("Neutral".into()),
                matched: Some(true),
                join: Some(Join::Matched),
                note: None,
            }],
            preset: None,
            edited: false,
            left_out: vec![],
            notes: None,
            flat: false,
            sentence: String::new(),
        };
        let value = serde_json::to_value(&view).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "correction",
                "device",
                "edited",
                "flat",
                "left_out",
                "notes",
                "preset",
                "sentence",
                "target",
                "tuning"
            ]
        );
        assert_eq!(
            value["tuning"][0],
            serde_json::json!({
                "name": "Lush",
                "on": true,
                "made_for": "Neutral",
                "matched": true,
                "join": { "state": "matched" },
                "note": null,
            })
        );
    }

    /// `koan dsp list --json` keeps these keys for each item.
    #[test]
    fn dsp_list_json_keys() {
        use koan_core::audio::dsp::profiles::Summary;
        use koan_core::config::DspRole;
        let item = commands::dsp_list_item(&Summary {
            name: "Lush".into(),
            devices: vec![],
            bands: 1,
            layers: 0,
            rates: vec![],
            problem: None,
            role: DspRole::Baked,
            measured: false,
            members: vec![],
            playing: None,
            preset: false,
            edited: false,
            used_on: vec!["Scarlett".into()],
            everywhere: false,
            held_by: vec![],
            scope_locked: None,
            graphics: 0,
            points: 0,
            made_for: None,
            join: None,
        });
        let mut keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["edited", "kind", "members", "name", "problem", "used_on"]
        );
        assert_eq!(item["kind"], "correction");
    }

    #[test]
    fn cli_play_jukebox_requires_server() {
        let result = Cli::try_parse_from(["koan", "play", "--jukebox"]);
        assert!(result.is_err());
    }
}

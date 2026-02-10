use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use indicatif::{ProgressBar, ProgressStyle};
use koan_core::audio::{buffer, device};
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
    /// Generate shell completions
    Completions {
        /// Shell to generate for
        shell: Shell,
    },
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
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "koan", &mut io::stdout());
        }
    }
}

/// Events from the input thread or the state watcher.
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

    // Send the whole queue — gapless transitions handled internally.
    tx.send(PlayerCommand::PlayQueue(paths.to_vec()))
        .expect("player thread died");

    wait_for_playing(&state);

    println!("controls: [space] pause/resume  [</>] seek 10s  [n] next  [q] quit\n");

    // Progress bar.
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{prefix} {bar:40.cyan/dim} {msg}")
            .unwrap()
            .progress_chars("━╸─"),
    );

    let quit = Arc::new(AtomicBool::new(false));

    let (ev_tx, ev_rx) = crossbeam_channel::unbounded::<Event>();

    // Input thread — raw mode, sends Key events.
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

    // Tick thread — drives progress bar updates and detects playback end.
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
    // Initial state was set before we entered — track might already be going after wait_for_playing.
    update_progress_bar(&pb, &state, &mut current_track);

    while let Ok(event) = ev_rx.recv() {
        match event {
            Event::Tick => {
                update_progress_bar(&pb, &state, &mut current_track);

                // Detect playback finished (queue exhausted, decode done).
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
                    // Escape sequence — consume `[` and direction byte.
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

    // Brief sleep so final output flushes.
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

    // Detect track change — print header for new track.
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

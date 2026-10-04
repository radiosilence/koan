use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use koan_core::config;
use koan_core::db::queries;
use koan_core::graphql_client::{GraphQLClient, GraphQLError};
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use owo_colors::OwoColorize;

use koan_tui::app::PickerAction;
use koan_tui::enqueue::enqueue_playlist;
use koan_tui::play::TuiCallbacks;

use super::{install_terminal_panic_hook, open_db, parse_dropped_paths, playlist_items_from_paths};
use crate::BufferedLogger;

/// Options for running the GraphQL/Subsonic API server alongside the TUI.
pub struct ApiOptions {
    pub port: Option<u16>,
    pub bind: Option<std::net::IpAddr>,
    pub subsonic: Option<u16>,
    pub playground: bool,
}

pub fn cmd_play(
    paths: &[PathBuf],
    ids: &[i64],
    album: Option<i64>,
    artist: Option<i64>,
    start_in_library: bool,
    clear_queue: bool,
    api_opts: Option<ApiOptions>,
) {
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

    let log_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    BufferedLogger::set_buffer(log_buffer.clone());

    let (state, _timeline, viz_snapshot, tx) = Player::spawn_for_listening();
    let tx_quit = tx.clone();

    // Spawn the API server on a background thread if requested.
    if let Some(opts) = api_opts {
        let db_path = config::db_path();
        let state_api = state.clone();
        let tx_api = tx.clone();
        std::thread::Builder::new()
            .name("koan-api".into())
            .spawn(move || {
                koan_server::graphql::start_api_background(
                    state_api,
                    tx_api,
                    db_path,
                    opts.port,
                    opts.bind,
                    opts.subsonic,
                    opts.playground,
                );
            })
            .expect("failed to spawn API server thread");
    }

    if let Ok(db) = koan_core::db::pool::shared().get() {
        if clear_queue {
            let _ = queries::clear_playback_state(&db.conn);
        }
        // Before any queue: the mode is the player's, kept with or without
        // one, and a queue arriving under shuffle is shuffled.
        if let Ok(mode) = queries::load_play_mode(&db.conn) {
            tx.send(PlayerCommand::RestorePlayMode(mode))
                .expect("player thread died");
        }
    }

    let mut expects_playback = track_ids.is_some() || !paths.is_empty();

    if let Some(ids) = track_ids {
        let tx_bg = tx.clone();
        std::thread::Builder::new()
            .name("koan-resolve".into())
            .spawn(move || {
                enqueue_playlist(ids, PickerAction::AppendAndPlay, tx_bg);
            })
            .expect("failed to spawn resolve thread");
    } else if !paths.is_empty() {
        for path in paths {
            if !path.exists() {
                eprintln!("{} {}", "not found:".red().bold(), path.display());
                std::process::exit(1);
            }
        }
        let owned_paths: Vec<PathBuf> = paths.to_vec();
        let tx_bg = tx.clone();
        std::thread::Builder::new()
            .name("koan-resolve".into())
            .spawn(move || {
                let mut audio_paths: Vec<PathBuf> = Vec::new();
                for path in &owned_paths {
                    if path.is_dir() {
                        let mut dir_files: Vec<PathBuf> = jwalk::WalkDir::new(path)
                            .follow_links(true)
                            .into_iter()
                            .filter_map(|e| e.ok())
                            .filter(|e| e.file_type().is_file())
                            .filter(|e| koan_core::index::metadata::is_audio_file(&e.path()))
                            .map(|e| e.path())
                            .collect();
                        dir_files.sort();
                        audio_paths.extend(dir_files);
                    } else {
                        audio_paths.push(path.clone());
                    }
                }
                if audio_paths.is_empty() {
                    return;
                }
                let items = playlist_items_from_paths(&audio_paths, None);
                if let Some(first) = items.first() {
                    let first_id = first.id;
                    tx_bg.send(PlayerCommand::AddToPlaylist(items)).ok();
                    tx_bg.send(PlayerCommand::Play(first_id)).ok();
                }
            })
            .expect("failed to spawn resolve thread");
    } else if !clear_queue
        && let Ok(db) = koan_core::db::pool::shared().get()
        && let Ok(Some(persisted)) = queries::load_playback_state(&db.conn)
    {
        let items: Vec<_> = persisted
            .items
            .iter()
            .map(|i| i.to_playlist_item())
            .collect();
        if !items.is_empty() {
            let cursor_id = persisted.cursor_path.as_ref().and_then(|cp| {
                items
                    .iter()
                    .find(|i| i.path.to_string_lossy() == *cp)
                    .map(|i| i.id)
            });
            tx.send(PlayerCommand::AddToPlaylist(items))
                .expect("player thread died");
            if let Some(cid) = cursor_id {
                // The player waits for a track still downloading, and opens it
                // at the position once it can.
                if persisted.position_ms > 0 || persisted.was_playing {
                    tx.send(PlayerCommand::Cue {
                        id: cid,
                        position_ms: persisted.position_ms,
                        play: persisted.was_playing,
                    })
                    .expect("player thread died");
                } else {
                    state.set_cursor(Some(cid));
                }
            }
            expects_playback = true;
        }
    }

    let callbacks = TuiCallbacks {
        sigint_received: crate::sigint_received,
        install_panic_hook: install_terminal_panic_hook,
        parse_dropped_paths,
        playlist_items_from_paths,
    };

    if let Err(e) = koan_tui::play::run_tui(
        state,
        viz_snapshot,
        tx,
        log_buffer,
        start_in_library,
        expects_playback,
        callbacks,
    ) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    // Saved above, playing if it was: a renderer left on our URL would play
    // out its buffer and stop, so it is stopped here and resumed next time.
    koan_core::player::commands::release_renderer(&tx_quit, Duration::from_millis(1500));

    BufferedLogger::clear_buffer();
    std::thread::sleep(Duration::from_millis(100));
}

pub fn cmd_play_remote(server_url: &str) {
    eprintln!("connecting to kōan server at {}...", server_url);

    let client = GraphQLClient::from_config(server_url);
    match client.library_stats() {
        Ok(stats) => {
            let total = stats["libraryStats"]["totalTracks"].as_i64().unwrap_or(0);
            let artists = stats["libraryStats"]["totalArtists"].as_i64().unwrap_or(0);
            let albums = stats["libraryStats"]["totalAlbums"].as_i64().unwrap_or(0);
            eprintln!(
                "connected — {} tracks, {} artists, {} albums",
                total, artists, albums
            );
        }
        Err(GraphQLError::Unauthorized(reason)) => {
            eprintln!(
                "{} {} refused the connection: {}",
                "error:".red().bold(),
                server_url,
                reason
            );
            if !client.has_session() {
                let cfg = config::Config::load().unwrap_or_default();
                if !cfg.auth.server.is_empty() {
                    eprintln!("the stored sign-in is for {}", cfg.auth.server);
                }
            }
            eprintln!(
                "sign in with: koan auth login --server {} --username <name>",
                server_url
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "{} failed to connect to {}: {}",
                "error:".red().bold(),
                server_url,
                e
            );
            std::process::exit(1);
        }
    }

    eprintln!("remote control — the server plays the audio");
    let (state, _timeline, viz_snapshot, cmd_tx) =
        koan_tui::remote_bridge::spawn_remote_bridge(client);

    let log_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    BufferedLogger::set_buffer(log_buffer.clone());

    std::thread::sleep(Duration::from_millis(300));

    let callbacks = TuiCallbacks {
        sigint_received: crate::sigint_received,
        install_panic_hook: install_terminal_panic_hook,
        parse_dropped_paths,
        playlist_items_from_paths,
    };

    if let Err(e) = koan_tui::play::run_tui(
        state,
        viz_snapshot,
        cmd_tx,
        log_buffer,
        true,
        false,
        callbacks,
    ) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    BufferedLogger::clear_buffer();
}

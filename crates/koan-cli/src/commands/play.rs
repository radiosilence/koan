use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use koan_core::config;
use koan_core::db::queries;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::LoadState;
use owo_colors::OwoColorize;

use koan_tui::app::PickerAction;
use koan_tui::backend::LocalBackend;
use koan_tui::download_queue::DownloadQueue;
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

    let (state, _timeline, viz_snapshot, tx) = Player::spawn();

    let download_queue = DownloadQueue::spawn(tx.clone(), state.clone(), log_buffer.clone());

    // Build the in-process GQL client — routes TUI data queries through the schema
    // instead of hitting the DB directly.
    let db_path = config::db_path();
    let schema = koan_server::build_schema(
        state.clone(),
        tx.clone(),
        db_path.clone(),
        Some(viz_snapshot.clone()),
    );
    let executor = koan_server::graphql::InProcessExecutor::new(schema);
    let gql_client = koan_core::graphql_client::GraphQLClient::new_with_executor(
        std::sync::Arc::new(executor),
        "http://localhost:4000",
    );

    // Spawn the API server on a background thread if requested.
    if let Some(opts) = api_opts {
        let state_api = state.clone();
        let tx_api = tx.clone();
        let viz_api = viz_snapshot.clone();
        let db_path_api = db_path.clone();
        std::thread::Builder::new()
            .name("koan-api".into())
            .spawn(move || {
                koan_server::graphql::start_api_background(koan_server::graphql::ApiServerOpts {
                    state: state_api,
                    cmd_tx: tx_api,
                    db_path: db_path_api,
                    port: opts.port,
                    bind: opts.bind,
                    subsonic_port: opts.subsonic,
                    playground: opts.playground,
                    viz: Some(viz_api),
                });
            })
            .expect("failed to spawn API server thread");
    }

    if clear_queue && let Ok(db) = koan_core::db::connection::Database::open(&config::db_path()) {
        let _ = queries::clear_playback_state(&db.conn);
    }

    let mut expects_playback = track_ids.is_some() || !paths.is_empty();
    let mut restored_position_ms: Option<u64> = None;

    if let Some(ids) = track_ids {
        let tx_bg = tx.clone();
        let dq_bg = download_queue.clone();
        std::thread::Builder::new()
            .name("koan-resolve".into())
            .spawn(move || {
                enqueue_playlist(ids, PickerAction::AppendAndPlay, tx_bg, dq_bg);
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
        && let Ok(db) = koan_core::db::connection::Database::open(&config::db_path())
        && let Ok(Some(persisted)) = queries::load_playback_state(&db.conn)
    {
        let items: Vec<_> = persisted
            .items
            .iter()
            .map(|i| i.to_playlist_item())
            .collect();
        if !items.is_empty() {
            let pending: Vec<(i64, koan_core::player::state::QueueItemId)> = items
                .iter()
                .filter(|i| matches!(i.load_state, LoadState::Pending))
                .filter_map(|i| i.db_id.map(|db_id| (db_id, i.id)))
                .collect();

            let cursor_id = persisted.cursor_path.as_ref().and_then(|cp| {
                items
                    .iter()
                    .find(|i| i.path.to_string_lossy() == *cp)
                    .map(|i| i.id)
            });
            tx.send(PlayerCommand::AddToPlaylist(items))
                .expect("player thread died");
            if let Some(cid) = cursor_id {
                state.set_cursor(Some(cid));
                restored_position_ms = Some(persisted.position_ms);
            }
            expects_playback = true;

            if !pending.is_empty() {
                log::info!(
                    "session restore: {} pending downloads submitted to queue",
                    pending.len()
                );
                download_queue.enqueue(pending);
            }
        }
    }

    let callbacks = TuiCallbacks {
        sigint_received: crate::sigint_received,
        install_panic_hook: install_terminal_panic_hook,
        parse_dropped_paths: |text| parse_dropped_paths(text),
        playlist_items_from_paths: |paths, progress| playlist_items_from_paths(paths, progress),
        open_db,
    };

    let backend = Arc::new(LocalBackend::new(
        state.clone(),
        viz_snapshot.clone(),
        tx.clone(),
    ));

    if let Err(e) = koan_tui::play::run_tui(
        state,
        viz_snapshot,
        tx,
        log_buffer,
        start_in_library,
        expects_playback,
        restored_position_ms,
        download_queue,
        callbacks,
        Some(gql_client),
        backend,
    ) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    BufferedLogger::clear_buffer();
    std::thread::sleep(Duration::from_millis(100));
}

pub fn cmd_play_remote(server_url: &str, jukebox: bool) {
    eprintln!("connecting to koan server at {}...", server_url);

    let client = koan_core::graphql_client::GraphQLClient::new(server_url);
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

    if jukebox {
        eprintln!("jukebox mode — server plays audio, client is remote control");
    }
    let (state, _timeline, viz_snapshot, cmd_tx) =
        koan_tui::remote_bridge::spawn_remote_bridge(server_url, jukebox);

    let log_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    BufferedLogger::set_buffer(log_buffer.clone());

    let download_queue = DownloadQueue::spawn(cmd_tx.clone(), state.clone(), log_buffer.clone());

    std::thread::sleep(Duration::from_millis(300));

    let callbacks = TuiCallbacks {
        sigint_received: crate::sigint_received,
        install_panic_hook: install_terminal_panic_hook,
        parse_dropped_paths: |text| parse_dropped_paths(text),
        playlist_items_from_paths: |paths, progress| playlist_items_from_paths(paths, progress),
        open_db,
    };

    // Remote mode: use HTTP-backed GraphQLClient (talks to the remote server).
    let remote_client = koan_core::graphql_client::GraphQLClient::new(server_url);

    // Wrap the remote bridge primitives in a LocalBackend — the remote bridge
    // already provides SharedPlayerState + VizSnapshot + Sender<PlayerCommand>,
    // so LocalBackend works fine here. A dedicated RemoteBackend will come later.
    let backend = Arc::new(LocalBackend::new(
        state.clone(),
        viz_snapshot.clone(),
        cmd_tx.clone(),
    ));

    if let Err(e) = koan_tui::play::run_tui(
        state,
        viz_snapshot,
        cmd_tx,
        log_buffer,
        true,
        false,
        None,
        download_queue,
        callbacks,
        Some(remote_client),
        backend,
    ) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    BufferedLogger::clear_buffer();
}

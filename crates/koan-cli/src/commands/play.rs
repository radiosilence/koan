use std::path::PathBuf;
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
use crate::Logger;

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

    Logger::set_quiet(true);

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

    // What this session starts with, being read on a thread of its own. The
    // TUI shows "loading" until the thread drops `reading`.
    let mut startup = None;
    let mut loading = None;
    if let Some(ids) = track_ids {
        let tx_bg = tx.clone();
        let (reading, done) = crossbeam_channel::bounded::<()>(0);
        loading = Some(done);
        startup = Some(
            std::thread::Builder::new()
                .name("koan-resolve".into())
                .spawn(move || {
                    let _reading = reading;
                    enqueue_playlist(ids, PickerAction::AppendAndPlay, None, tx_bg);
                })
                .expect("failed to spawn resolve thread"),
        );
    } else if !paths.is_empty() {
        for path in paths {
            if !path.exists() {
                eprintln!("{} {}", "not found:".red().bold(), path.display());
                std::process::exit(1);
            }
        }
        let owned_paths: Vec<PathBuf> = paths.to_vec();
        let tx_bg = tx.clone();
        let (reading, done) = crossbeam_channel::bounded::<()>(0);
        loading = Some(done);
        startup = Some(
            std::thread::Builder::new()
                .name("koan-resolve".into())
                .spawn(move || {
                    let _reading = reading;
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
                    if !items.is_empty() {
                        tx_bg.send(PlayerCommand::AddAndPlay(items)).ok();
                    }
                })
                .expect("failed to spawn resolve thread"),
        );
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
        }
    }

    // Reachable as a device: by the account's other devices through the
    // server, and by whatever is on this network, as `devices.*` says. Not
    // before the session it starts with is in the player: a device that
    // connected sooner would be told an empty queue, and could replace it
    // before the restore landed on top. Music named on the command line
    // plays here, whichever device the last run was controlling.
    {
        let (state, viz, tx) = (state.clone(), viz_snapshot.clone(), tx.clone());
        let here = startup.is_some();
        std::thread::Builder::new()
            .name("koan-serve".into())
            .spawn(move || {
                if let Some(startup) = startup {
                    let _ = startup.join();
                }
                let (applied, done) = crossbeam_channel::bounded(1);
                if tx.send(PlayerCommand::Barrier(applied)).is_ok() {
                    let _ = done.recv_timeout(Duration::from_secs(5));
                }
                let Some(identity) = koan_core::remote::link::LinkIdentity::terminal() else {
                    log::info!(
                        "another terminal on this machine is the terminal device; this one plays here only"
                    );
                    return;
                };
                koan_core::remote::control::serve(
                    identity,
                    state,
                    viz,
                    tx,
                    koan_tui::tasks::library_changed,
                    here,
                );
            })
            .expect("failed to spawn the serve thread");
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
        start_in_library,
        loading,
        callbacks,
    ) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    // Saved above, playing if it was: a renderer left on our URL would play
    // out its buffer and stop, so it is stopped here and resumed next time.
    koan_core::player::commands::release_renderer(&tx_quit, Duration::from_millis(1500));
    koan_core::remote::control::stop();

    Logger::set_quiet(false);
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

    Logger::set_quiet(true);

    std::thread::sleep(Duration::from_millis(300));

    let callbacks = TuiCallbacks {
        sigint_received: crate::sigint_received,
        install_panic_hook: install_terminal_panic_hook,
        parse_dropped_paths,
        playlist_items_from_paths,
    };

    if let Err(e) = koan_tui::play::run_tui(state, viz_snapshot, cmd_tx, true, None, callbacks) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    Logger::set_quiet(false);
}

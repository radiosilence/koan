use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use koan_core::config;
use koan_core::db::queries;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use owo_colors::OwoColorize;

use super::{
    enqueue_playlist, install_terminal_panic_hook, load_picker_items, make_album_picker_items,
    open_db, parse_dropped_paths, playlist_items_from_paths,
};
use crate::BufferedLogger;
use crate::tui;
use crate::tui::app::PickerAction;
use crate::tui::picker::{
    PickerItem, PickerKind, PickerPartKind, PickerState, all_tracks_sentinel,
    artist_id_from_sentinel, is_all_tracks_sentinel,
};

pub fn cmd_play(
    paths: &[PathBuf],
    ids: &[i64],
    album: Option<i64>,
    artist: Option<i64>,
    start_in_library: bool,
) {
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

    let (state, _timeline, tx) = Player::spawn();

    let expects_playback = track_ids.is_some() || !paths.is_empty();

    if let Some(ids) = track_ids {
        // Resolve ALL tracks in the background — the TUI starts immediately
        // with a loading overlay. No more blank terminal during downloads.
        let tx_bg = tx.clone();
        let log_bg = log_buffer.clone();
        let state_bg = state.clone();
        std::thread::Builder::new()
            .name("koan-resolve".into())
            .spawn(move || {
                // CLI play: always append and start playing.
                enqueue_playlist(ids, PickerAction::AppendAndPlay, tx_bg, log_bg, state_bg);
            })
            .expect("failed to spawn resolve thread");
    } else if !paths.is_empty() {
        // Raw file paths — expand directories recursively and filter to audio files.
        let mut audio_paths: Vec<PathBuf> = Vec::new();
        for path in paths {
            if !path.exists() {
                eprintln!("{} {}", "not found:".red().bold(), path.display());
                std::process::exit(1);
            }
            if path.is_dir() {
                let mut dir_files: Vec<PathBuf> = walkdir::WalkDir::new(path)
                    .follow_links(true)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file())
                    .filter(|e| koan_core::index::metadata::is_audio_file(e.path()))
                    .map(|e| e.into_path())
                    .collect();
                dir_files.sort();
                audio_paths.extend(dir_files);
            } else {
                audio_paths.push(path.clone());
            }
        }
        if audio_paths.is_empty() {
            eprintln!("no audio files found");
            std::process::exit(1);
        }
        let items = playlist_items_from_paths(&audio_paths, None);
        let first_id = items[0].id;
        tx.send(PlayerCommand::AddToPlaylist(items))
            .expect("player thread died");
        tx.send(PlayerCommand::Play(first_id))
            .expect("player thread died");
    }
    // No paths/ids and no library — just open the TUI empty.
    // User can add tracks via pickers (p/a/r) or library browser (l).

    // Run the Ratatui TUI immediately — don't wait for playback to start.
    // The TUI shows a loading overlay until playback begins.
    if let Err(e) = run_tui(state, tx, log_buffer, start_in_library, expects_playback) {
        eprintln!("{} {}", "tui error:".red().bold(), e);
    }

    BufferedLogger::clear_buffer();
    std::thread::sleep(Duration::from_millis(100));
}

fn run_tui(
    state: Arc<koan_core::player::state::SharedPlayerState>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buffer: Arc<Mutex<Vec<String>>>,
    start_in_library: bool,
    expects_playback: bool,
) -> std::io::Result<()> {
    use crossterm::{
        event::{
            DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        },
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use std::io;

    install_terminal_panic_hook();

    // Setup terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let db_path = config::db_path();
    let mut app = tui::app::App::new(state, tx.clone(), log_buffer, db_path);

    if expects_playback {
        app.loading_message = Some("loading...".into());
    }

    if start_in_library {
        app.open_library();
    }

    // Load favourites from database.
    app.load_favourites();

    // Media keys (macOS Control Center integration).
    let mut media = crate::media_keys::MediaKeyHandler::new(tx.clone(), app.state.clone());
    let mut last_track_path: Option<PathBuf> = None;

    loop {
        terminal.draw(|f| tui::ui::render(f, &mut app))?;

        let event = tui::event::poll(Duration::from_millis(50))?;

        // Process the first event, then drain any buffered events.
        // For mouse events, only keep the latest (coalesce move events).
        let mut last_mouse: Option<crossterm::event::MouseEvent> = None;
        match event {
            tui::event::Event::Key(key) => app.handle_key(key),
            tui::event::Event::Mouse(mouse) => { last_mouse = Some(mouse); }
            tui::event::Event::Paste(text) => {
                // Parse dropped/pasted paths (handles shell escaping, file:// URIs, etc).
                // Heavy work (walkdir + metadata read) runs on a background thread.
                // Insert at mouse position if hovering over queue, otherwise append.
                let tx_drop = tx.clone();
                let log_drop = app.log_buffer.clone();
                let insert_after = app.drop_target_queue_id();
                let progress = std::sync::Arc::new((
                    std::sync::atomic::AtomicUsize::new(0),
                    std::sync::atomic::AtomicUsize::new(0),
                ));
                app.drop_progress = Some(progress.clone());
                std::thread::Builder::new()
                    .name("koan-drop".into())
                    .spawn(move || {
                        let dropped = parse_dropped_paths(&text);
                        let mut audio_paths: Vec<PathBuf> = Vec::new();
                        for path in dropped {
                            if path.is_dir() {
                                let mut dir_files: Vec<PathBuf> = walkdir::WalkDir::new(&path)
                                    .follow_links(true)
                                    .into_iter()
                                    .filter_map(|e| e.ok())
                                    .filter(|e| e.file_type().is_file())
                                    .filter(|e| koan_core::index::metadata::is_audio_file(e.path()))
                                    .map(|e| e.into_path())
                                    .collect();
                                dir_files.sort();
                                audio_paths.extend(dir_files);
                            } else if path.is_file()
                                && koan_core::index::metadata::is_audio_file(&path)
                            {
                                audio_paths.push(path);
                            }
                        }
                        if !audio_paths.is_empty() {
                            let count = audio_paths.len();
                            progress
                                .1
                                .store(count, std::sync::atomic::Ordering::Relaxed);
                            let items = playlist_items_from_paths(&audio_paths, Some(&progress.0));
                            if let Some(after_id) = insert_after {
                                tx_drop
                                    .send(PlayerCommand::InsertInPlaylist {
                                        items,
                                        after: after_id,
                                    })
                                    .ok();
                            } else {
                                tx_drop.send(PlayerCommand::AddToPlaylist(items)).ok();
                            }
                            if let Ok(mut logs) = log_drop.lock() {
                                logs.push(format!("added {} files", count));
                            }
                        }
                        // Signal completion by setting processed == total.
                        let total = progress.1.load(std::sync::atomic::Ordering::Relaxed);
                        progress
                            .0
                            .store(total, std::sync::atomic::Ordering::Relaxed);
                    })
                    .ok();
            }
            tui::event::Event::Tick => {
                app.handle_tick();

                // Update media keys + pump macOS run loop for event dispatch.
                if let Some(ref mut mk) = media {
                    mk.update_playback(&app.state);
                    let current = app.state.track_info().map(|t| t.path.clone());
                    if current != last_track_path {
                        last_track_path = current.clone();
                        mk.update_metadata(&app.state, current.as_ref());
                    }
                }
                crate::media_keys::pump_run_loop();

                // Check for external quit request (e.g. macOS Control Center).
                if app.state.quit_requested() {
                    app.tx.send(PlayerCommand::Stop).ok();
                    app.quit = true;
                }
            }
        }

        // Drain any buffered events — coalesce mouse moves so we always
        // render with the latest mouse position.
        while crossterm::event::poll(Duration::ZERO)? {
            match crossterm::event::read()? {
                crossterm::event::Event::Mouse(m) => { last_mouse = Some(m); }
                crossterm::event::Event::Key(k) => {
                    app.handle_key(k);
                }
                crossterm::event::Event::Paste(text) => {
                    // Treat extra pastes same as primary — but typically rare.
                    let _ = text;
                }
                _ => {}
            }
        }

        // Process the coalesced mouse event (latest position wins).
        if let Some(mouse) = last_mouse {
            app.handle_mouse(mouse);
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
                    app.picker_result =
                        Some((PickerKind::Track, track_ids, PickerAction::AppendAndPlay));
                }
            } else {
                // Open album picker for this artist with an "all tracks" entry.
                let mut items = vec![PickerItem {
                    id: all_tracks_sentinel(artist_id),
                    display: "all tracks".to_string(),
                    match_text: "all tracks".into(),
                    parts: vec![("all tracks".into(), PickerPartKind::Plain)],
                }];
                items.extend(make_album_picker_items(&albums));
                app.mode = tui::app::Mode::Picker(PickerKind::Album);
                let picker = PickerState::new(PickerKind::Album, items, false);
                app.picker = Some(picker);
            }
        }

        // Handle picker result — enqueue in background.
        if let Some((kind, ids, action)) = app.picker_result.take() {
            let tx_bg = tx.clone();
            let log_bg = app.log_buffer.clone();
            let state_bg = app.state.clone();

            app.loading_message = Some("loading...".into());

            // Everything happens on a background thread — album expansion + downloads.
            std::thread::Builder::new()
                .name("koan-enqueue".into())
                .spawn(move || {
                    // Expand album IDs to track IDs if needed.
                    let track_ids = match kind {
                        PickerKind::Album => {
                            let db = open_db();
                            let mut expanded = Vec::new();
                            for album_id in &ids {
                                if is_all_tracks_sentinel(*album_id) {
                                    let aid = artist_id_from_sentinel(*album_id);
                                    let tracks = queries::tracks_for_artist(&db.conn, aid)
                                        .unwrap_or_default();
                                    expanded.extend(tracks.iter().map(|t| t.id));
                                    continue;
                                }
                                let tracks = queries::tracks_for_album(&db.conn, *album_id)
                                    .unwrap_or_default();
                                expanded.extend(tracks.iter().map(|t| t.id));
                            }
                            expanded
                        }
                        _ => ids,
                    };

                    if !track_ids.is_empty() {
                        enqueue_playlist(track_ids, action, tx_bg, log_bg, state_bg);
                    }
                })
                .ok();
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
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    Ok(())
}

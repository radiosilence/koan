use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use koan_core::config;
use koan_core::db::queries;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{LoadState, PlaylistItem, QueueItemId};
use owo_colors::OwoColorize;

use super::{
    enqueue_playlist, install_terminal_panic_hook, load_picker_items, make_album_picker_items,
    open_db,
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
        let items: Vec<PlaylistItem> = audio_paths
            .iter()
            .map(|p| {
                // Read tags so queue groups by album correctly.
                if let Ok(meta) = koan_core::index::metadata::read_metadata(p) {
                    PlaylistItem {
                        id: QueueItemId::new(),
                        path: p.clone(),
                        title: meta.title,
                        artist: meta.artist,
                        album_artist: meta.album_artist.unwrap_or_default(),
                        album: meta.album,
                        year: meta.date,
                        codec: meta.codec,
                        track_number: meta.track_number.map(|n| n as i64),
                        disc: meta.disc.map(|d| d as i64),
                        duration_ms: meta.duration_ms.map(|d| d as u64),
                        load_state: LoadState::Ready,
                    }
                } else {
                    // Fallback: no tags readable, just use filename.
                    let title = p
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    PlaylistItem {
                        id: QueueItemId::new(),
                        path: p.clone(),
                        title,
                        artist: String::new(),
                        album_artist: String::new(),
                        album: String::new(),
                        year: None,
                        codec: None,
                        track_number: None,
                        disc: None,
                        duration_ms: None,
                        load_state: LoadState::Ready,
                    }
                }
            })
            .collect();
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
        event::{DisableMouseCapture, EnableMouseCapture},
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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

    // Media keys (macOS Control Center integration).
    let mut media = crate::media_keys::MediaKeyHandler::new(tx.clone(), app.state.clone());
    let mut last_track_path: Option<PathBuf> = None;

    loop {
        terminal.draw(|f| tui::ui::render(f, &mut app))?;

        let event = tui::event::poll(Duration::from_millis(50))?;

        match event {
            tui::event::Event::Key(key) => app.handle_key(key),
            tui::event::Event::Mouse(mouse) => app.handle_mouse(mouse),
            tui::event::Event::Tick => {
                app.handle_tick();

                // Update media keys + pump macOS run loop for event dispatch.
                if let Some(ref mut mk) = media {
                    mk.update_playback(&app.state);
                    let current = app.state.track_info().map(|t| t.path.clone());
                    if current != last_track_path {
                        last_track_path = current;
                        mk.update_metadata(&app.state);
                    }
                }
                crate::media_keys::pump_run_loop();
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
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

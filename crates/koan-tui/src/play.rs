//! TUI event loop and run_tui() entry point.
//!
//! This module contains the core TUI event loop. The CLI (koan-cli) calls
//! `run_tui()` after setting up the player and any initial playback.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use koan_core::db::queries;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::LoadState;

use crate::app::{self, PickerAction};
use crate::download_queue::DownloadQueue;
use crate::enqueue::enqueue_playlist;
use crate::picker::{
    PickerItem, PickerKind, PickerPartKind, PickerState, all_tracks_sentinel,
    artist_id_from_sentinel, is_all_tracks_sentinel,
};
use crate::picker_items::{load_picker_items, make_album_picker_items};

/// Callbacks for CLI integration. The TUI library crate doesn't own
/// the logger or signal handler — koan-cli provides those.
pub struct TuiCallbacks {
    /// Returns true if Ctrl+C has been pressed (for graceful shutdown).
    pub sigint_received: fn() -> bool,
    /// Install a panic hook that restores the terminal on any thread panic.
    pub install_panic_hook: fn(),
    /// Parse dropped/pasted text into file paths (shell escaping, file:// URIs, etc).
    pub parse_dropped_paths: fn(&str) -> Vec<PathBuf>,
    /// Build PlaylistItems from file paths (DB lookup + disk metadata fallback).
    pub playlist_items_from_paths: fn(
        &[PathBuf],
        Option<&std::sync::atomic::AtomicUsize>,
    ) -> Vec<koan_core::player::state::PlaylistItem>,
    /// Open the database (exits on failure).
    pub open_db: fn() -> koan_core::db::connection::Database,
}

/// Save the current queue and playback position to DB.
fn save_playback_state_from_app(app: &app::App) {
    if let Some(ref client) = app.gql_client {
        if let Err(e) = client.save_playback_state() {
            log::warn!("failed to save playback state via GQL: {}", e);
        }
        return;
    }

    // Fallback: direct DB access.
    let (items, cursor) = app.state.snapshot_playlist();
    if items.is_empty() {
        if let Ok(db) = koan_core::db::connection::Database::open(&app.db_path) {
            let _ = koan_core::db::queries::clear_playback_state(&db.conn);
        }
        return;
    }
    let persisted: Vec<koan_core::db::queries::PersistedQueueItem> = items
        .iter()
        .map(koan_core::db::queries::PersistedQueueItem::from_playlist_item)
        .collect();

    let cursor_path = cursor
        .and_then(|cid| items.iter().find(|i| i.id == cid))
        .map(|i| i.path.to_string_lossy().into_owned());
    let position_ms = app.state.position_ms();

    match koan_core::db::connection::Database::open(&app.db_path) {
        Ok(db) => {
            if let Err(e) = koan_core::db::queries::save_playback_state(
                &db.conn,
                &persisted,
                cursor_path.as_deref(),
                position_ms,
            ) {
                log::warn!("failed to save playback state: {}", e);
            }
        }
        Err(e) => {
            log::warn!("failed to open db for autosave: {}", e);
        }
    }
}

/// Run the Ratatui TUI event loop. This is the main entry point for the TUI.
///
/// Called by koan-cli after spawning the player and setting up initial playback.
#[allow(clippy::too_many_arguments)]
pub fn run_tui(
    state: Arc<koan_core::player::state::SharedPlayerState>,
    viz_snapshot: Arc<koan_core::audio::viz::VizSnapshot>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buffer: Arc<Mutex<Vec<String>>>,
    start_in_library: bool,
    expects_playback: bool,
    restored_position_ms: Option<u64>,
    download_queue: DownloadQueue,
    callbacks: TuiCallbacks,
    gql_client: Option<koan_core::graphql_client::GraphQLClient>,
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

    (callbacks.install_panic_hook)();

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

    let db_path = koan_core::config::db_path();

    let target_fps = {
        let cfg = koan_core::config::Config::load().unwrap_or_default();
        cfg.playback.target_fps.max(1)
    };
    let frame_duration = Duration::from_micros(1_000_000 / target_fps as u64);
    let mut next_frame = std::time::Instant::now();

    let mut app = app::App::new(
        state,
        viz_snapshot,
        tx.clone(),
        log_buffer,
        db_path,
        target_fps,
        download_queue.clone(),
        gql_client,
    );

    if expects_playback {
        app.loading_message = Some("loading...".into());
    }

    if start_in_library {
        app.open_library();
    }

    app.load_favourites();

    let mut pending_seek: Option<u64> = restored_position_ms;

    let mut media = crate::media_keys::MediaKeyHandler::new(tx.clone(), app.state.clone());
    let mut last_track_path: Option<PathBuf> = None;

    let mut last_autosave = std::time::Instant::now();
    const AUTOSAVE_INTERVAL: Duration = Duration::from_millis(100);

    loop {
        if (callbacks.sigint_received)() {
            app.quit = true;
        }

        terminal.draw(|f| crate::ui::render(f, &mut app))?;

        let mut last_mouse: Option<crossterm::event::MouseEvent> = None;
        loop {
            let now = std::time::Instant::now();
            let remaining = next_frame.saturating_duration_since(now);

            if !crossterm::event::poll(remaining)? {
                break;
            }
            match crossterm::event::read()? {
                crossterm::event::Event::Key(key) => app.handle_key(key),
                crossterm::event::Event::Mouse(mouse) => {
                    last_mouse = Some(mouse);
                }
                crossterm::event::Event::Paste(text) => {
                    let tx_drop = tx.clone();
                    let log_drop = app.log_buffer.clone();
                    let insert_after = app.drop_target_queue_id();
                    let progress = std::sync::Arc::new((
                        std::sync::atomic::AtomicUsize::new(0),
                        std::sync::atomic::AtomicUsize::new(0),
                    ));
                    app.drop_progress = Some(progress.clone());
                    let parse_fn = callbacks.parse_dropped_paths;
                    let items_fn = callbacks.playlist_items_from_paths;
                    std::thread::Builder::new()
                        .name("koan-drop".into())
                        .spawn(move || {
                            let dropped = parse_fn(&text);
                            let mut audio_paths: Vec<PathBuf> = Vec::new();
                            for path in dropped {
                                if path.is_dir() {
                                    let mut dir_files: Vec<PathBuf> = jwalk::WalkDir::new(&path)
                                        .follow_links(true)
                                        .into_iter()
                                        .filter_map(|e| e.ok())
                                        .filter(|e| e.file_type().is_file())
                                        .filter(|e| {
                                            koan_core::index::metadata::is_audio_file(&e.path())
                                        })
                                        .map(|e| e.path())
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
                                let items = items_fn(&audio_paths, Some(&progress.0));
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
                            let total = progress.1.load(std::sync::atomic::Ordering::Relaxed);
                            progress
                                .0
                                .store(total, std::sync::atomic::Ordering::Relaxed);
                        })
                        .ok();
                }
                _ => {}
            }
        }

        if let Some(mouse) = last_mouse {
            app.handle_mouse(mouse);
        }

        app.handle_tick();

        if let Some(pos) = pending_seek
            && let Some(cid) = app.state.cursor()
            && app
                .state
                .item_load_state(cid)
                .is_some_and(|s| matches!(s, LoadState::Ready))
        {
            tx.send(PlayerCommand::Play(cid)).ok();
            tx.send(PlayerCommand::Seek(pos)).ok();
            tx.send(PlayerCommand::Pause).ok();
            pending_seek = None;
        }

        if let Some(ref mut mk) = media {
            mk.update_playback(&app.state);
            let current = app.state.track_info().map(|t| t.path.clone());
            let track_changed = current != last_track_path;
            let metadata_refreshed = app.state.take_metadata_refresh();
            if track_changed || metadata_refreshed {
                last_track_path = current.clone();
                mk.update_metadata(&app.state, current.as_ref());
            }
        }
        crate::media_keys::pump_run_loop();

        if app.state.quit_requested() {
            app.quit = true;
        }

        let now = std::time::Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        }
        next_frame += frame_duration;
        let now = std::time::Instant::now();
        if next_frame < now {
            next_frame = now;
        }

        if let app::Mode::Picker(kind) = &app.mode
            && app.picker.is_none()
        {
            let items = load_picker_items(*kind, app.gql_client.as_ref());
            let multi = matches!(kind, PickerKind::Track);
            app.picker = Some(PickerState::new(*kind, items, multi));
        }

        if let Some(artist_id) = app.artist_drill_down.take() {
            if let Some(ref client) = app.gql_client {
                // GQL path for artist drill-down.
                match client.albums_for_artist(artist_id) {
                    Ok(albums) if albums.is_empty() => {
                        let track_ids: Vec<i64> = client
                            .tracks_for_artist(artist_id)
                            .unwrap_or_default()
                            .iter()
                            .map(|t| t.id)
                            .collect();
                        if !track_ids.is_empty() {
                            app.picker_result = Some((
                                PickerKind::Track,
                                track_ids,
                                PickerAction::AppendAndPlay,
                            ));
                        }
                    }
                    Ok(albums) => {
                        let mut items = vec![PickerItem {
                            id: all_tracks_sentinel(artist_id),
                            display: "all tracks".to_string(),
                            match_text: "all tracks".into(),
                            parts: vec![("all tracks".into(), PickerPartKind::Plain)],
                        }];
                        items.extend(
                            crate::picker_items::make_album_picker_items_gql(&albums),
                        );
                        app.mode = app::Mode::Picker(PickerKind::Album);
                        let picker = PickerState::new(PickerKind::Album, items, false);
                        app.picker = Some(picker);
                    }
                    Err(e) => {
                        log::warn!("GQL albums_for_artist failed: {}", e);
                    }
                }
            } else {
                let db = (callbacks.open_db)();
                let albums =
                    queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();
                if albums.is_empty() {
                    let track_ids: Vec<i64> =
                        queries::tracks_for_artist(&db.conn, artist_id)
                            .unwrap_or_default()
                            .iter()
                            .map(|t| t.id)
                            .collect();
                    if !track_ids.is_empty() {
                        app.picker_result = Some((
                            PickerKind::Track,
                            track_ids,
                            PickerAction::AppendAndPlay,
                        ));
                    }
                } else {
                    let mut items = vec![PickerItem {
                        id: all_tracks_sentinel(artist_id),
                        display: "all tracks".to_string(),
                        match_text: "all tracks".into(),
                        parts: vec![("all tracks".into(), PickerPartKind::Plain)],
                    }];
                    items.extend(make_album_picker_items(&albums));
                    app.mode = app::Mode::Picker(PickerKind::Album);
                    let picker = PickerState::new(PickerKind::Album, items, false);
                    app.picker = Some(picker);
                }
            }
        }

        if let Some((kind, ids, action)) = app.picker_result.take() {
            let tx_bg = tx.clone();
            let dq_bg = download_queue.clone();
            let open_db = callbacks.open_db;
            let gql = app.gql_client.clone();

            app.loading_message = Some("loading...".into());

            std::thread::Builder::new()
                .name("koan-enqueue".into())
                .spawn(move || {
                    let track_ids = match kind {
                        PickerKind::Album => {
                            let mut expanded = Vec::new();
                            if let Some(ref client) = gql {
                                for album_id in &ids {
                                    if is_all_tracks_sentinel(*album_id) {
                                        let aid = artist_id_from_sentinel(*album_id);
                                        let tracks = client
                                            .tracks_for_artist(aid)
                                            .unwrap_or_default();
                                        expanded.extend(tracks.iter().map(|t| t.id));
                                        continue;
                                    }
                                    let tracks = client
                                        .tracks_for_album(*album_id)
                                        .unwrap_or_default();
                                    expanded.extend(tracks.iter().map(|t| t.id));
                                }
                            } else {
                                let db = open_db();
                                for album_id in &ids {
                                    if is_all_tracks_sentinel(*album_id) {
                                        let aid = artist_id_from_sentinel(*album_id);
                                        let tracks =
                                            queries::tracks_for_artist(&db.conn, aid)
                                                .unwrap_or_default();
                                        expanded.extend(tracks.iter().map(|t| t.id));
                                        continue;
                                    }
                                    let tracks =
                                        queries::tracks_for_album(&db.conn, *album_id)
                                            .unwrap_or_default();
                                    expanded.extend(tracks.iter().map(|t| t.id));
                                }
                            }
                            expanded
                        }
                        _ => ids,
                    };

                    if !track_ids.is_empty() {
                        enqueue_playlist(track_ids, action, tx_bg, dq_bg);
                    }
                })
                .ok();
        }

        if app.state_dirty && last_autosave.elapsed() >= AUTOSAVE_INTERVAL {
            save_playback_state_from_app(&app);
            app.state_dirty = false;
            last_autosave = std::time::Instant::now();
        }

        if app.quit {
            if app.has_played {
                save_playback_state_from_app(&app);
            }
            app.tx.send(PlayerCommand::Stop).ok();
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

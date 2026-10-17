//! TUI event loop and `run_tui()` entry point.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use koan_core::db::queries::{self, PersistedQueueItem};
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueItemId, SharedPlayerState};

use crate::app;

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
    pub playlist_items_from_paths:
        fn(&[PathBuf], Option<&AtomicUsize>) -> Vec<koan_core::player::state::PlaylistItem>,
}

/// What a write of the session covers. A session write includes the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Save {
    Position,
    Session,
}

/// Keeps the session saved: the whole queue when what is saved of it
/// changes, and otherwise the position about once a second and on every
/// play, pause, stop and track change. The writes are database transactions
/// and happen on a thread of their own, never in a frame.
struct Autosave {
    state: Arc<SharedPlayerState>,
    tx: Option<crossbeam_channel::Sender<Save>>,
    writer: Option<JoinHandle<()>>,
    /// Nothing is saved of the position before anything has played: a queue
    /// restored and not yet started would save over where it was left.
    has_played: bool,
    content: u64,
    cursor: Option<QueueItemId>,
    playback: PlaybackState,
    last_session: Instant,
    last_position: Instant,
}

impl Autosave {
    /// The queue at most this often, however fast it is edited.
    const SESSION_INTERVAL: Duration = Duration::from_millis(100);
    const POSITION_INTERVAL: Duration = Duration::from_secs(1);

    fn start(state: Arc<SharedPlayerState>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<Save>();
        let writer_state = state.clone();
        let writer = std::thread::Builder::new()
            .name("koan-autosave".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Whatever queued up behind it is one write: the state is
                    // read when it is written.
                    let save = rx.try_iter().fold(first, Save::max);
                    write(&writer_state, save);
                }
            })
            .ok();
        let now = Instant::now();
        Self {
            content: state.saved_version(),
            cursor: state.cursor(),
            playback: state.playback_state(),
            state,
            tx: Some(tx),
            writer,
            has_played: false,
            last_session: now,
            last_position: now,
        }
    }

    fn tick(&mut self) {
        let playback = self.state.playback_state();
        self.has_played |= playback == PlaybackState::Playing;
        let content = self.state.saved_version();
        let cursor = self.state.cursor();
        let save =
            if content != self.content && self.last_session.elapsed() >= Self::SESSION_INTERVAL {
                self.content = content;
                self.last_session = Instant::now();
                Save::Session
            } else if self.has_played
                && (playback != self.playback
                    || cursor != self.cursor
                    || (playback == PlaybackState::Playing
                        && self.last_position.elapsed() >= Self::POSITION_INTERVAL))
            {
                Save::Position
            } else {
                return;
            };
        self.cursor = cursor;
        self.playback = playback;
        self.last_position = Instant::now();
        self.send(save);
    }

    fn send(&self, save: Save) {
        if let Some(tx) = self.tx.as_ref() {
            tx.send(save).ok();
        }
    }

    /// Saves the session a last time, and waits until it is written.
    fn finish(mut self) {
        if self.has_played {
            self.send(Save::Session);
        }
        self.tx = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn write(state: &SharedPlayerState, save: Save) {
    let db = match koan_core::db::pool::shared().get() {
        Ok(db) => db,
        Err(e) => {
            log::warn!("failed to open db for autosave: {e}");
            return;
        }
    };
    let playing = state.playback_state() == PlaybackState::Playing;
    let result = match save {
        Save::Position => queries::save_playback_position(
            &db.conn,
            state.play_mode(),
            state
                .cursor_path()
                .map(|p| p.to_string_lossy().into_owned())
                .as_deref(),
            state.position_ms(),
            playing,
        ),
        Save::Session => {
            let (items, cursor) = state.snapshot_playlist();
            if items.is_empty() {
                // The mode outlives the queue, and may be what changed.
                queries::clear_playback_state(&db.conn).and_then(|()| {
                    queries::save_playback_position(&db.conn, state.play_mode(), None, 0, false)
                })
            } else {
                let persisted: Vec<PersistedQueueItem> = items
                    .iter()
                    .map(PersistedQueueItem::from_playlist_item)
                    .collect();
                let cursor_path = cursor
                    .and_then(|cid| items.iter().find(|i| i.id == cid))
                    .map(|i| i.path.to_string_lossy().into_owned());
                queries::save_playback_state(
                    &db.conn,
                    &persisted,
                    state.play_mode(),
                    cursor_path.as_deref(),
                    state.position_ms(),
                    playing,
                )
            }
        }
    };
    if let Err(e) = result {
        log::warn!("failed to save playback state: {e}");
    }
}

/// What the system's Now Playing was last told. It runs its own clock from the
/// playhead it is given, so it hears of a track, a play or pause and a seek,
/// not of every frame.
#[derive(Default)]
struct NowPlaying {
    track: Option<PathBuf>,
    playback: Option<PlaybackState>,
    position_ms: u64,
    at: Option<Instant>,
}

impl NowPlaying {
    /// A playhead this far from where its clock would have it was moved.
    const SEEKED_MS: u64 = 1000;

    fn sync(&mut self, mk: &mut crate::media_keys::MediaKeyHandler, state: &SharedPlayerState) {
        let track = state.track_info().map(|t| t.path);
        let metadata = state.take_metadata_refresh() || track != self.track;
        if metadata {
            mk.update_metadata(state, track.as_ref());
            self.track = track;
        }
        let playback = state.playback_state();
        let position_ms = state.position_ms();
        let expected = match (self.playback, self.at) {
            (Some(PlaybackState::Playing), Some(at)) => {
                self.position_ms + at.elapsed().as_millis() as u64
            }
            _ => self.position_ms,
        };
        if metadata
            || self.playback != Some(playback)
            || position_ms.abs_diff(expected) > Self::SEEKED_MS
        {
            mk.update_playback(state);
            self.playback = Some(playback);
            self.position_ms = position_ms;
            self.at = Some(Instant::now());
        }
    }
}

/// Adds the audio files in pasted or dropped text to the queue, at the row
/// under the pointer when it is over one. Folders add every audio file under
/// them, in path order.
fn import_dropped(app: &mut app::App, text: String, callbacks: &TuiCallbacks) {
    let tx = app.tx.clone();
    let reports = app.reports();
    let insert_after = app.drop_target_queue_id();
    let progress = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    app.drop_progress = Some(progress.clone());
    let parse = callbacks.parse_dropped_paths;
    let items_for = callbacks.playlist_items_from_paths;
    std::thread::Builder::new()
        .name("koan-drop".into())
        .spawn(move || {
            let mut audio_paths: Vec<PathBuf> = Vec::new();
            for path in parse(&text) {
                if path.is_dir() {
                    let mut dir_files: Vec<PathBuf> = jwalk::WalkDir::new(&path)
                        .follow_links(true)
                        .into_iter()
                        .filter_map(|e| e.ok())
                        .filter(|e| e.file_type().is_file())
                        .filter(|e| koan_core::index::metadata::is_audio_file(&e.path()))
                        .map(|e| e.path())
                        .collect();
                    dir_files.sort();
                    audio_paths.extend(dir_files);
                } else if path.is_file() && koan_core::index::metadata::is_audio_file(&path) {
                    audio_paths.push(path);
                }
            }
            let count = audio_paths.len();
            if count > 0 {
                progress.1.store(count, Ordering::Relaxed);
                let items = items_for(&audio_paths, Some(&progress.0));
                match insert_after {
                    Some(after) => tx.send(PlayerCommand::InsertInPlaylist { items, after }),
                    None => tx.send(PlayerCommand::AddToPlaylist(items)),
                }
                .ok();
                reports
                    .send(crate::tasks::Report::Say(format!(
                        "added {count} file{}",
                        if count == 1 { "" } else { "s" }
                    )))
                    .ok();
            } else {
                reports
                    .send(crate::tasks::Report::Say("no audio files in that".into()))
                    .ok();
            }
            // Done: the progress bar goes.
            progress.1.store(count.max(1), Ordering::Relaxed);
            progress.0.store(count.max(1), Ordering::Relaxed);
        })
        .ok();
}

/// Run the Ratatui TUI event loop.
///
/// Called by koan-cli after spawning the player and setting up initial
/// playback. "loading" shows until `loading` disconnects: its sender is held
/// by the thread reading what is to play into the queue.
pub fn run_tui(
    state: Arc<SharedPlayerState>,
    viz_snapshot: Arc<koan_core::audio::viz::VizSnapshot>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    start_in_library: bool,
    loading: Option<crossbeam_channel::Receiver<()>>,
    callbacks: TuiCallbacks,
) -> std::io::Result<()> {
    use crossterm::{
        event::{
            DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
            Event, MouseEventKind,
        },
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use std::io::{self, Write};

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
    /// Puts the terminal back however the loop exits — `?` included.
    struct TerminalGuard;

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                DisableBracketedPaste,
                crossterm::cursor::Show
            );
        }
    }

    let _restore = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let cfg = koan_core::config::Config::cached();
    let target_fps = cfg.playback.target_fps.max(1);
    let frame_duration = Duration::from_micros(1_000_000 / target_fps as u64);
    let mut next_frame = Instant::now();

    let mut app = app::App::new(state, viz_snapshot, tx, target_fps);
    // Before anything reads the keyboard: the terminal answers on stdin.
    app.art.gfx = crate::cover_art::Gfx::detect(cfg.playback.art_graphics);
    drop(cfg);
    if let Some(done) = loading {
        app.wait_for(done);
    }
    if start_in_library {
        app.open_library();
    }
    app.load_favourites();

    let mut media = crate::media_keys::MediaKeyHandler::new(
        app.tx.clone(),
        app.shown.clone(),
        app.view.local.clone(),
    );
    let mut now_playing = NowPlaying::default();
    // This player's session, whichever device the pages show.
    let mut autosave = Autosave::start(app.view.local.clone());

    let flush_deletes = |app: &mut app::App| {
        if let Some(deletes) = app.art.gfx.take_deletes() {
            let mut out = io::stdout();
            let _ = out.write_all(deletes.as_bytes());
            let _ = out.flush();
        }
    };

    loop {
        if (callbacks.sigint_received)() {
            app.quit = true;
        }

        app.follow_control();
        let frame_start = Instant::now();
        let mut built = Duration::ZERO;
        terminal.draw(|f| {
            crate::ui::render(f, &mut app);
            built = frame_start.elapsed();
        })?;
        let total = frame_start.elapsed();
        flush_deletes(&mut app);
        if total > Duration::from_millis(30) {
            log::debug!(
                "slow frame: {total:?} (build {built:?}, diff and write {:?}), covers {}",
                total - built,
                app.art.gfx.protocol_name()
            );
        }

        // Motion coalesces to the latest position; clicks and wheel ticks
        // each carry meaning and are dispatched in order.
        let mut pending_motion = None;
        while crossterm::event::poll(next_frame.saturating_duration_since(Instant::now()))? {
            let event = crossterm::event::read()?;
            if let Event::Mouse(mouse) = event
                && matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
            {
                pending_motion = Some(mouse);
                continue;
            }
            if let Some(m) = pending_motion.take() {
                app.handle_mouse(m);
            }
            match event {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                // Into the text field that has the keys, or else the queue.
                Event::Paste(text) if app.takes_text() => app.type_text(&text),
                Event::Paste(text) => import_dropped(&mut app, text, &callbacks),
                _ => {}
            }
        }
        if let Some(mouse) = pending_motion.take() {
            app.handle_mouse(mouse);
        }

        app.handle_tick();

        if let Some(ref mut mk) = media {
            now_playing.sync(mk, &app.state);
        }
        crate::media_keys::pump_run_loop();

        if app.view.local.quit_requested() {
            app.quit = true;
        }

        next_frame += frame_duration;
        let now = Instant::now();
        if next_frame < now {
            next_frame = now;
        }

        autosave.tick();

        if app.quit {
            autosave.finish();
            // Behind whatever the router still holds: a barrier through it
            // reaches this player once every command before it has gone where
            // it was going.
            let (reply, drained) = crossbeam_channel::bounded(1);
            if app.tx.send(PlayerCommand::Barrier(reply)).is_ok() {
                let _ = drained.recv_timeout(Duration::from_secs(1));
            }
            app.local_tx.send(PlayerCommand::Stop).ok();
            app.art.gfx.release_all();
            flush_deletes(&mut app);
            break;
        }
    }

    Ok(())
}

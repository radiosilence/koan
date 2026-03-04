use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    PlaybackState, QueueEntry, QueueEntryStatus, QueueItemId, SharedPlayerState,
    VisibleQueueSnapshot,
};

use super::library::LibraryState;
use super::picker::{PickerKind, PickerPartKind, PickerState, picker_results_rect};
use super::queue;
use super::theme::Theme;
use super::transport::TransportBar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    QueueEdit,
    Picker(PickerKind),
    LibraryBrowse,
    TrackInfo(usize),
    CoverArtZoom,
    ContextMenu,
    Organize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Play,
    Remove,
    ToggleFavourite,
    TrackInfo,
    Organize,
}

pub struct ContextMenuState {
    /// (action, label, hotkey char)
    pub actions: Vec<(ContextAction, &'static str, char)>,
    pub cursor: usize,
}

/// What to do when the picker confirms a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    /// Append to end of queue (don't start playing).
    Append,
    /// Append and immediately play the first added track.
    AppendAndPlay,
    /// Clear queue, add tracks, play from the top.
    ReplaceQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryFocus {
    Library,
    Queue,
}

pub struct DragState {
    pub from_index: usize,
    pub current_y: u16,
    /// True if we're dragging a multi-selection group.
    pub multi: bool,
    /// Offset of the clicked item within the sorted selection.
    /// E.g. if selection is [3,4,5] and you click on 4, anchor_offset = 1.
    pub anchor_offset: usize,
    /// The last desired group-start index during drag.
    /// Used to avoid redundant moves when the mouse hasn't crossed a row boundary.
    pub last_group_start: Option<usize>,
}

/// Which UI element the mouse cursor is currently hovering over.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum HoverZone {
    #[default]
    None,
    QueueItem(usize),
    LibraryItem(usize),
    SeekBar,
    ScrollbarQueue,
    TransportArt,
    TransportText,
    PanelDivider,
    PickerItem(usize),
    ContextMenuItem(usize),
}

/// Tracks the current mouse hover position and zone.
#[derive(Default)]
pub struct HoverState {
    pub column: u16,
    pub row: u16,
    pub zone: HoverZone,
}

/// Queue cursor, selection, scroll, drag, and cached snapshot.
#[derive(Default)]
pub struct QueueState {
    pub cursor: usize,
    pub scroll_offset: usize,
    pub selected_ids: HashSet<QueueItemId>,
    pub anchor_id: Option<QueueItemId>,
    pub drag: Option<DragState>,
    /// Cached visible queue snapshot — refreshed once per frame.
    pub(super) vq_cache: VisibleQueueSnapshot,
}

/// Cached layout rects from last render, used for mouse hit-testing.
#[derive(Default)]
pub struct LayoutRects {
    pub transport_area: ratatui::layout::Rect,
    pub queue_area: ratatui::layout::Rect,
    pub picker_area: ratatui::layout::Rect,
    pub track_info_area: ratatui::layout::Rect,
    pub library_area: ratatui::layout::Rect,
    pub now_playing_art_area: ratatui::layout::Rect,
    pub transport_text_area: ratatui::layout::Rect,
    pub seek_bar_start: u16,
    pub seek_bar_width: u16,
    pub context_menu_area: ratatui::layout::Rect,
    pub organize_area: ratatui::layout::Rect,
}

/// Cover art caches and stable height tracking.
#[derive(Default)]
pub struct ArtState {
    /// Cached cover art for track info modal.
    pub cover_art: super::cover_art::CoverArtCache,
    /// Cached cover art for now-playing transport display.
    pub now_playing_art: super::cover_art::CoverArtCache,
    /// Last computed art height so layout stays stable when art disappears.
    pub last_art_height: u16,
}

pub struct App {
    pub mode: Mode,
    pub state: Arc<SharedPlayerState>,
    pub tx: Sender<PlayerCommand>,
    pub quit: bool,

    /// Queue cursor, selection, scroll, and cached snapshot.
    pub queue: QueueState,

    // Picker state (when in Picker mode).
    pub picker: Option<PickerState>,

    // Spinner tick for download animation.
    pub spinner_tick: usize,

    // Double-click detection.
    pub last_click_time: Option<std::time::Instant>,
    pub last_click_idx: Option<usize>,

    // Log messages from background threads.
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub log_messages: Vec<String>,

    // Track whether we've ever been in Playing state.
    pub has_played: bool,

    // Theme.
    pub theme: Theme,

    /// Cached layout rects from last render for mouse hit-testing.
    pub layout: LayoutRects,

    // Picker result — set when picker confirms, consumed by main loop.
    // Tagged with the picker kind and action so album IDs can be expanded
    // and the right enqueue behaviour is applied.
    pub picker_result: Option<(PickerKind, Vec<i64>, PickerAction)>,

    pub artist_drill_down: Option<i64>,

    // Loading overlay message (e.g. "loading album...").
    pub loading_message: Option<String>,

    // Auto-scroll: track by path so index shifts from finished_paths don't trigger.
    pub last_playing_path: Option<PathBuf>,

    // Library browser.
    pub library: Option<LibraryState>,
    pub library_focus: LibraryFocus,
    pub db_path: PathBuf,

    /// Cover art caches.
    pub art: ArtState,

    /// Context menu state (when in ContextMenu mode).
    pub context_menu: Option<ContextMenuState>,

    /// Organize modal state (when in Organize mode).
    pub organize: Option<super::organize::OrganizeModalState>,

    /// Last known mouse row — for determining drop insertion point.
    pub last_mouse_row: Option<u16>,

    /// When Some(offset), the user is click-dragging the scrollbar thumb.
    /// The offset is the grab position within the thumb (0 = top of thumb).
    pub scrollbar_grab_offset: Option<u16>,

    /// True while a drag operation has an open undo batch.
    pub drag_undo_active: bool,

    /// Drop/paste import progress: (processed, total). Cleared when done.
    pub drop_progress: Option<Arc<(AtomicUsize, AtomicUsize)>>,

    /// Mouse hover state — updated on MouseEventKind::Moved.
    pub hover: HoverState,

    /// Ticker offset for scrolling long transport text.
    pub ticker_offset: usize,
    /// Counter for ticker animation speed (increments each tick).
    pub ticker_tick: u8,
    /// Tick interval divisor for ticker speed (derived from config ticker_fps).
    ticker_divisor: u8,
    /// Track path used to detect track changes for ticker reset.
    ticker_last_path: Option<PathBuf>,

    /// Set of favourite track paths, loaded from DB on startup.
    pub favourites: std::collections::HashSet<PathBuf>,
}

impl App {
    pub fn new(
        state: Arc<SharedPlayerState>,
        tx: Sender<PlayerCommand>,
        log_buffer: Arc<Mutex<Vec<String>>>,
        db_path: PathBuf,
    ) -> Self {
        Self {
            mode: Mode::Normal,
            state,
            tx,
            quit: false,
            queue: QueueState::default(),
            picker: None,
            spinner_tick: 0,
            last_click_time: None,
            last_click_idx: None,
            log_buffer,
            log_messages: Vec::new(),
            has_played: false,
            theme: Theme::default(),
            layout: LayoutRects::default(),
            loading_message: None,
            picker_result: None,
            artist_drill_down: None,
            last_playing_path: None,
            library: None,
            library_focus: LibraryFocus::Library,
            db_path,
            art: ArtState::default(),
            context_menu: None,
            organize: None,
            last_mouse_row: None,
            scrollbar_grab_offset: None,
            drag_undo_active: false,
            drop_progress: None,
            hover: HoverState::default(),
            ticker_offset: 0,
            ticker_tick: 0,
            ticker_divisor: {
                let cfg = koan_core::config::Config::load().unwrap_or_default();
                let fps = cfg.playback.ticker_fps.max(1);
                // Tick interval is 50ms (20 ticks/sec). Divisor = 20 / fps.
                (20u8 / fps).max(1)
            },
            ticker_last_path: None,
            favourites: std::collections::HashSet::new(),
        }
    }

    /// Load favourites from the database.
    pub fn load_favourites(&mut self) {
        if let Ok(db) = koan_core::db::connection::Database::open(&self.db_path)
            && let Ok(favs) = koan_core::db::queries::load_favourites(&db.conn)
        {
            self.favourites = favs;
        }
    }

    /// Toggle favourite status for a track path. Returns true if now favourite.
    pub fn toggle_favourite(&mut self, path: &std::path::Path) -> bool {
        if let Ok(db) = koan_core::db::connection::Database::open(&self.db_path)
            && let Ok(is_fav) = koan_core::db::queries::toggle_favourite(&db.conn, path)
        {
            if is_fav {
                self.favourites.insert(path.to_path_buf());
            } else {
                self.favourites.remove(path);
            }
            // Sync star/unstar to remote if this track has a remote_id.
            if let Ok(Some(remote_id)) = koan_core::db::queries::remote_id_for_path(&db.conn, path)
            {
                let cfg = koan_core::config::Config::load().unwrap_or_default();
                if cfg.remote.enabled && !cfg.remote.password.is_empty() {
                    let client = koan_core::remote::client::SubsonicClient::new(
                        &cfg.remote.url,
                        &cfg.remote.username,
                        &cfg.remote.password,
                    );
                    let rid = remote_id.clone();
                    let star = is_fav;
                    std::thread::spawn(move || {
                        let _ = if star {
                            client.star(&rid)
                        } else {
                            client.unstar(&rid)
                        };
                    });
                }
            }
            return is_fav;
        }
        false
    }

    pub fn handle_tick(&mut self) {
        // Refresh visible queue cache so all tick logic sees current state.
        self.refresh_visible_queue();

        self.spinner_tick = self.spinner_tick.wrapping_add(1);

        // Ticker animation: advance one character every 3 ticks (~150ms).
        // Reset when the playing track changes so new titles start from the beginning.
        {
            let current_playing = self
                .queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
                .map(|e| e.path.clone());
            if current_playing != self.ticker_last_path {
                self.ticker_last_path = current_playing;
                self.ticker_offset = 0;
                self.ticker_tick = 0;
            }
        }
        self.ticker_tick = self.ticker_tick.wrapping_add(1);
        if self.ticker_tick.is_multiple_of(self.ticker_divisor) {
            self.ticker_offset = self.ticker_offset.wrapping_add(1);
        }

        // Drain log buffer.
        if let Ok(mut logs) = self.log_buffer.lock() {
            self.log_messages.extend(logs.drain(..));
        }

        // Track playing state.
        if self.state.playback_state() == PlaybackState::Playing {
            self.has_played = true;
        }

        // Clear loading overlay once playback starts or pending queue populates.
        if self.loading_message.is_some()
            && (self.has_played || !self.queue.vq_cache.entries.is_empty())
        {
            self.loading_message = None;
        }

        // Tick picker if active.
        if let Some(ref mut picker) = self.picker {
            picker.tick();
        }

        // Tick organize modal — check for pending preview/execute results.
        if let Some(ref mut org) = self.organize
            && let Some(result) = org.check_pending()
        {
            match result {
                super::organize::OrganizeCompletionKind::Preview => {}
                super::organize::OrganizeCompletionKind::Execute => {
                    // Send path updates to the player.
                    let updates = org.take_path_updates();
                    if !updates.is_empty() {
                        self.tx.send(PlayerCommand::UpdatePaths(updates)).ok();
                    }
                }
            }
        }

        // Check drop progress — clear when done.
        if let Some(ref progress) = self.drop_progress {
            let done = progress.0.load(Ordering::Relaxed);
            let total = progress.1.load(Ordering::Relaxed);
            if total > 0 && done >= total {
                self.drop_progress = None;
            }
        }

        // Update now-playing cover art cache when track changes.
        if let Some(ref info) = self.state.track_info() {
            self.art.now_playing_art.get(&info.path);
        }

        // In normal mode, auto-scroll to playing track on actual track change.
        // Derive the playing track from the visible queue cache (atomic snapshot)
        // NOT from track_info directly — track_info changes before the visible
        // queue is rebuilt, causing a 1-frame scroll offset jump.
        if self.mode == Mode::Normal {
            let current_playing = self
                .queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
                .map(|e| e.path.clone());
            if current_playing != self.last_playing_path {
                self.last_playing_path = current_playing;
                if let Some(idx) = self
                    .queue
                    .vq_cache
                    .entries
                    .iter()
                    .position(|e| e.status == QueueEntryStatus::Playing)
                {
                    let visible_height = self.layout.queue_area.height.max(5) as usize;
                    self.queue.scroll_offset = queue::scroll_for_cursor(
                        &self.visible_queue(),
                        idx,
                        self.queue.scroll_offset,
                        visible_height,
                    );
                }
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl+C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.tx.send(PlayerCommand::Stop).ok();
            self.quit = true;
            return;
        }

        match &self.mode {
            Mode::Picker(_) => self.handle_picker_key(key),
            Mode::QueueEdit => self.handle_edit_key(key),
            Mode::LibraryBrowse => self.handle_library_key(key),
            Mode::TrackInfo(_) => self.handle_info_key(key),
            Mode::CoverArtZoom => self.handle_zoom_key(key),
            Mode::ContextMenu => self.handle_context_menu_key(key),
            Mode::Organize => self.handle_organize_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.tx.send(PlayerCommand::Stop).ok();
                self.quit = true;
            }
            KeyCode::Char(' ') => {
                if self.state.playback_state() == PlaybackState::Playing {
                    self.tx.send(PlayerCommand::Pause).ok();
                } else {
                    self.tx.send(PlayerCommand::Resume).ok();
                }
            }
            KeyCode::Char('>') | KeyCode::Char('n') => {
                self.tx.send(PlayerCommand::NextTrack).ok();
            }
            KeyCode::Char('<') => {
                self.tx.send(PlayerCommand::PrevTrack).ok();
            }
            KeyCode::Char('.') | KeyCode::Right => {
                let pos = self.state.position_ms();
                self.tx
                    .send(PlayerCommand::Seek(pos.saturating_add(10_000)))
                    .ok();
            }
            KeyCode::Char(',') | KeyCode::Left => {
                let pos = self.state.position_ms();
                self.tx
                    .send(PlayerCommand::Seek(pos.saturating_sub(10_000)))
                    .ok();
            }
            KeyCode::Char('e') => {
                self.mode = Mode::QueueEdit;
                // Sync selection to cursor so j/k work immediately.
                if self.queue.selected_ids.len() <= 1 {
                    self.select_single(self.queue.cursor);
                }
            }
            KeyCode::Char('p') => {
                self.open_picker(PickerKind::Track);
            }
            KeyCode::Char('a') => {
                self.open_picker(PickerKind::Album);
            }
            KeyCode::Char('r') => {
                self.open_picker(PickerKind::Artist);
            }
            KeyCode::Char('l') => {
                self.open_library();
            }
            KeyCode::Up => {
                let visible = self.visible_queue();
                if !visible.is_empty() {
                    self.queue.cursor = self.queue.cursor.saturating_sub(1);
                    self.select_single(self.queue.cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Down => {
                let visible = self.visible_queue();
                if !visible.is_empty() && self.queue.cursor + 1 < visible.len() {
                    self.queue.cursor += 1;
                    self.select_single(self.queue.cursor);
                    self.update_scroll();
                }
            }
            // Vim: page up/down, home/end.
            KeyCode::PageUp | KeyCode::Char('u')
                if key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.page_up(false);
            }
            KeyCode::PageDown | KeyCode::Char('d')
                if key.code == KeyCode::PageDown
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.page_down(false);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.jump_to_start(false);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.jump_to_end(false);
            }
            KeyCode::Enter => {
                self.play_at_cursor();
            }
            KeyCode::Char('i') => {
                self.open_track_info(self.queue.cursor);
            }
            KeyCode::Char('f') => {
                // Toggle favourite for the track at cursor.
                let visible = self.visible_queue();
                if let Some(entry) = visible.get(self.queue.cursor) {
                    let path = entry.path.clone();
                    self.toggle_favourite(&path);
                }
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Undo).ok();
            }
            KeyCode::Char('z') => {
                if self.art.now_playing_art.cached().is_some() {
                    self.mode = Mode::CoverArtZoom;
                }
            }
            KeyCode::Char('Z')
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
            {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            KeyCode::Char('/') => {
                self.open_queue_jump();
            }
            _ => {}
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.queue.selected_ids.clear();
                self.queue.anchor_id = None;
            }
            KeyCode::Char('q') => {
                self.tx.send(PlayerCommand::Stop).ok();
                self.quit = true;
            }
            KeyCode::Up => {
                self.queue.cursor = self.queue.cursor.saturating_sub(1);
                if shift {
                    self.extend_selection_to(self.queue.cursor);
                } else {
                    self.select_single(self.queue.cursor);
                }
                self.update_scroll();
            }
            KeyCode::Down => {
                let visible = self.visible_queue();
                if self.queue.cursor + 1 < visible.len() {
                    self.queue.cursor += 1;
                }
                if shift {
                    self.extend_selection_to(self.queue.cursor);
                } else {
                    self.select_single(self.queue.cursor);
                }
                self.update_scroll();
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                self.delete_selected();
            }
            KeyCode::Char('j') => {
                self.move_selected_down();
            }
            KeyCode::Char('k') => {
                self.move_selected_up();
            }
            KeyCode::Char('J') => {
                let visible_len = self.visible_queue().len();
                if self.queue.cursor + 1 < visible_len {
                    self.queue.cursor += 1;
                    self.extend_selection_to(self.queue.cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Char('K') => {
                if self.queue.cursor > 0 {
                    self.queue.cursor -= 1;
                    self.extend_selection_to(self.queue.cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Char('i') => {
                self.open_track_info(self.queue.cursor);
            }
            KeyCode::Char(' ') => {
                // Open context menu if there's a selection.
                if !self.queue.selected_ids.is_empty() {
                    self.open_context_menu();
                }
            }
            // Vim: page up/down, home/end.
            KeyCode::PageUp | KeyCode::Char('u')
                if key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.page_up(shift);
            }
            KeyCode::PageDown => {
                self.page_down(shift);
            }
            KeyCode::Home | KeyCode::Char('g') if !shift => {
                self.jump_to_start(false);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.jump_to_end(shift);
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Undo).ok();
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            KeyCode::Char('Z')
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
            {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            _ => {}
        }
    }

    fn handle_context_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.context_menu = None;
                self.mode = Mode::QueueEdit;
            }
            KeyCode::Up => {
                if let Some(ref mut menu) = self.context_menu {
                    menu.cursor = menu.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(ref mut menu) = self.context_menu
                    && menu.cursor + 1 < menu.actions.len()
                {
                    menu.cursor += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(menu) = self.context_menu.take()
                    && let Some((action, _, _)) = menu.actions.get(menu.cursor)
                {
                    self.execute_context_action(*action);
                } else {
                    self.mode = Mode::QueueEdit;
                }
            }
            KeyCode::Char(c) => {
                if c == 'q' || c == ' ' {
                    self.context_menu = None;
                    self.mode = Mode::QueueEdit;
                } else if let Some(menu) = self.context_menu.take() {
                    // Match hotkey char against menu actions.
                    if let Some((action, _, _)) = menu.actions.iter().find(|(_, _, h)| *h == c) {
                        self.execute_context_action(*action);
                    } else {
                        // No match — restore menu.
                        self.context_menu = Some(menu);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_organize_key(&mut self, key: KeyEvent) {
        let Some(ref mut org) = self.organize else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.organize = None;
                self.mode = Mode::QueueEdit;
            }
            KeyCode::Tab => {
                org.focus = match org.focus {
                    super::organize::OrganizeFocus::PatternList => {
                        super::organize::OrganizeFocus::Preview
                    }
                    super::organize::OrganizeFocus::Preview => {
                        super::organize::OrganizeFocus::RunButton
                    }
                    super::organize::OrganizeFocus::RunButton => {
                        super::organize::OrganizeFocus::PatternList
                    }
                };
            }
            KeyCode::Up => match org.focus {
                super::organize::OrganizeFocus::PatternList => {
                    if org.pattern_cursor > 0 {
                        org.pattern_cursor -= 1;
                        org.request_preview();
                    }
                }
                super::organize::OrganizeFocus::Preview => {
                    org.scroll = org.scroll.saturating_sub(1);
                }
                _ => {}
            },
            KeyCode::Down => match org.focus {
                super::organize::OrganizeFocus::PatternList => {
                    if org.pattern_cursor + 1 < org.patterns.len() {
                        org.pattern_cursor += 1;
                        org.request_preview();
                    }
                }
                super::organize::OrganizeFocus::Preview => {
                    org.scroll += 1;
                }
                _ => {}
            },
            KeyCode::Enter => {
                if !org.executing {
                    org.request_execute();
                }
            }
            _ => {}
        }
    }

    fn open_context_menu(&mut self) {
        let visible = self.visible_queue();
        let is_fav = visible
            .get(self.queue.cursor)
            .is_some_and(|e| self.favourites.contains(&e.path));
        let fav_label = if is_fav { "Unfavourite" } else { "Favourite" };
        let actions = vec![
            (ContextAction::Play, "Play", 'p'),
            (ContextAction::ToggleFavourite, fav_label, 'f'),
            (ContextAction::TrackInfo, "Track info", 'i'),
            (ContextAction::Remove, "Remove", 'd'),
            (ContextAction::Organize, "Organize files", 'o'),
        ];
        self.context_menu = Some(ContextMenuState { actions, cursor: 0 });
        self.mode = Mode::ContextMenu;
    }

    fn execute_context_action(&mut self, action: ContextAction) {
        self.mode = Mode::QueueEdit;
        match action {
            ContextAction::Play => {
                self.play_at_cursor();
            }
            ContextAction::Remove => {
                self.delete_selected();
            }
            ContextAction::ToggleFavourite => {
                let visible = self.visible_queue();
                if let Some(entry) = visible.get(self.queue.cursor) {
                    let path = entry.path.clone();
                    self.toggle_favourite(&path);
                }
            }
            ContextAction::TrackInfo => {
                self.open_track_info(self.queue.cursor);
            }
            ContextAction::Organize => {
                self.open_organize_modal();
            }
        }
    }

    fn open_organize_modal(&mut self) {
        let config = koan_core::config::Config::load().unwrap_or_default();
        let mut patterns: Vec<(String, String)> = config.organize.patterns.into_iter().collect();
        patterns.sort_by(|a, b| a.0.cmp(&b.0));

        // Collect selected queue entries' paths.
        let visible = self.visible_queue();
        let selected_paths: Vec<PathBuf> = visible
            .iter()
            .filter(|e| self.queue.selected_ids.contains(&e.id))
            .map(|e| e.path.clone())
            .collect();

        // Collect the QueueItemIds for the selection (needed for path updates later).
        let selected_ids: Vec<(koan_core::player::state::QueueItemId, PathBuf)> = visible
            .iter()
            .filter(|e| self.queue.selected_ids.contains(&e.id))
            .map(|e| (e.id, e.path.clone()))
            .collect();

        let org = super::organize::OrganizeModalState::new(patterns, selected_paths, selected_ids);
        self.organize = Some(org);
        self.mode = Mode::Organize;
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let Some(ref mut picker) = self.picker else {
            return;
        };

        // Determine if this keypress is a confirm action.
        let action = match (key.code, key.modifiers) {
            (KeyCode::Enter, m) if m.contains(KeyModifiers::CONTROL) => {
                Some(PickerAction::AppendAndPlay)
            }
            (KeyCode::Char('r'), m) if m.contains(KeyModifiers::CONTROL) => {
                Some(PickerAction::ReplaceQueue)
            }
            (KeyCode::Enter, _) => Some(PickerAction::Append),
            _ => None,
        };

        if let Some(action) = action {
            let ids = picker.confirm();
            let kind = picker.kind;
            self.picker = None;
            self.mode = Mode::Normal;

            if !ids.is_empty() {
                match kind {
                    PickerKind::Track | PickerKind::Album => {
                        self.picker_result = Some((kind, ids, action));
                    }
                    PickerKind::Artist => {
                        self.artist_drill_down = Some(ids[0]);
                    }
                    PickerKind::QueueJump => {
                        let idx = ids[0] as usize;
                        let visible = self.visible_queue();
                        if let Some(entry) = visible.get(idx) {
                            self.tx.send(PlayerCommand::Play(entry.id)).ok();
                            self.queue.cursor = idx;
                            // Scroll so the jumped-to track is near the top.
                            let visible_height = self.layout.queue_area.height.max(10) as usize;
                            self.queue.scroll_offset =
                                queue::scroll_cursor_to_top(&visible, idx, visible_height);
                        }
                    }
                }
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::Tab => picker.toggle_select(),
            KeyCode::Backspace => picker.backspace(),
            KeyCode::Char(c) => picker.type_char(c),
            _ => {}
        }
    }

    fn handle_info_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.art.cover_art.clear();
            }
            _ => {}
        }
    }

    fn handle_zoom_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('z') | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    fn open_track_info(&mut self, idx: usize) {
        let visible = self.visible_queue();
        if visible.is_empty() || idx >= visible.len() {
            return;
        }
        self.mode = Mode::TrackInfo(idx);
        // Prime the cache for this track's path.
        let path = visible[idx].path.clone();
        self.art.cover_art.get(&path);
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        // Track mouse row for drop insertion indicator.
        if self.is_in_rect(event.column, event.row, self.layout.queue_area) {
            self.last_mouse_row = Some(event.row);
        } else {
            self.last_mouse_row = None;
        }

        // Cover art zoom intercepts all mouse events.
        if self.mode == Mode::CoverArtZoom {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                self.mode = Mode::Normal;
            }
            return;
        }

        // Track info intercepts all mouse events when active.
        if matches!(self.mode, Mode::TrackInfo(_)) {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind
                && !self.is_in_rect(event.column, event.row, self.layout.track_info_area)
            {
                self.mode = Mode::Normal;
                self.art.cover_art.clear();
            }
            return;
        }

        // Organize modal intercepts all mouse events.
        if self.mode == Mode::Organize {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind
                && !self.is_in_rect(event.column, event.row, self.layout.organize_area)
            {
                self.organize = None;
                self.mode = Mode::QueueEdit;
            }
            return;
        }

        // Context menu intercepts all mouse events.
        if self.mode == Mode::ContextMenu {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                if self.is_in_rect(event.column, event.row, self.layout.context_menu_area) {
                    // Click on an action row — compute which one.
                    let row = (event
                        .row
                        .saturating_sub(self.layout.context_menu_area.y + 1))
                        as usize;
                    if let Some(ref mut menu) = self.context_menu
                        && row < menu.actions.len()
                    {
                        menu.cursor = row;
                        let action = menu.actions[row].0;
                        self.context_menu = None;
                        self.execute_context_action(action);
                    }
                } else {
                    self.context_menu = None;
                    self.mode = Mode::QueueEdit;
                }
            }
            return;
        }

        // Picker intercepts all mouse events when active.
        if let Mode::Picker(_) = &self.mode {
            let picker_area = self.layout.picker_area;
            let results = picker_results_rect(picker_area);
            let in_results = self.is_in_rect(event.column, event.row, results);
            let in_popup = self.is_in_rect(event.column, event.row, picker_area);

            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                if in_results {
                    if let Some(ref mut picker) = self.picker {
                        let visible_height = results.height as usize;
                        let start = if picker.cursor >= visible_height {
                            picker.cursor - visible_height + 1
                        } else {
                            0
                        };
                        let row_in_results = (event.row - results.y) as usize;
                        let item_idx = start + row_in_results;
                        if item_idx < picker.matched_count() {
                            let now = std::time::Instant::now();
                            let is_double = self.last_click_idx == Some(item_idx)
                                && self
                                    .last_click_time
                                    .is_some_and(|t| now.duration_since(t).as_millis() < 400);

                            if is_double {
                                self.last_click_idx = None;
                                self.last_click_time = None;
                                picker.cursor = item_idx;
                                let ids = picker.confirm();
                                let kind = picker.kind;
                                self.picker = None;
                                self.mode = Mode::Normal;
                                if !ids.is_empty() {
                                    match kind {
                                        PickerKind::Track | PickerKind::Album => {
                                            self.picker_result =
                                                Some((kind, ids, PickerAction::AppendAndPlay));
                                        }
                                        PickerKind::Artist => {
                                            self.artist_drill_down = Some(ids[0]);
                                        }
                                        PickerKind::QueueJump => {
                                            let idx = ids[0] as usize;
                                            let visible = self.visible_queue();
                                            if let Some(entry) = visible.get(idx) {
                                                self.tx.send(PlayerCommand::Play(entry.id)).ok();
                                                self.queue.cursor = idx;
                                                self.update_scroll();
                                            }
                                        }
                                    }
                                }
                            } else {
                                self.last_click_idx = Some(item_idx);
                                self.last_click_time = Some(now);
                                picker.cursor = item_idx;
                            }
                        }
                    }
                } else if !in_popup {
                    // Click outside picker -> close.
                    self.picker = None;
                    self.mode = Mode::Normal;
                }
            }

            // Scroll events fall through to existing scroll handler.
            if matches!(
                event.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            ) {
                // Fall through.
            } else {
                return;
            }
        }

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Library pane click.
                if self.mode == Mode::LibraryBrowse
                    && self.is_in_rect(event.column, event.row, self.layout.library_area)
                {
                    self.library_focus = LibraryFocus::Library;
                    if let Some(ref mut lib) = self.library {
                        let inner_x = self.layout.library_area.x + 1;
                        let inner_y = self.layout.library_area.y + 1;
                        let inner_h = self.layout.library_area.height.saturating_sub(2) as usize;
                        if event.row >= inner_y && (event.row - inner_y) < inner_h as u16 {
                            let row = (event.row - inner_y) as usize;
                            let col = event.column.saturating_sub(inner_x) as usize;
                            let item_idx = lib.scroll_offset + row;
                            if item_idx < lib.nodes.len() {
                                lib.cursor = item_idx;

                                // Click on arrow area (first ~4 chars) -> expand/collapse.
                                // Click on text -> double-click to enqueue.
                                if col < 4 {
                                    lib.toggle_expand();
                                    self.last_click_idx = None;
                                    self.last_click_time = None;
                                } else {
                                    let now = std::time::Instant::now();
                                    let is_double = self.last_click_idx == Some(item_idx)
                                        && self.last_click_time.is_some_and(|t| {
                                            now.duration_since(t).as_millis() < 400
                                        });
                                    if is_double {
                                        self.last_click_idx = None;
                                        self.last_click_time = None;
                                        let ids = lib.enqueue_all_under_cursor();
                                        if !ids.is_empty() {
                                            self.picker_result = Some((
                                                PickerKind::Track,
                                                ids,
                                                PickerAction::AppendAndPlay,
                                            ));
                                        }
                                    } else {
                                        self.last_click_idx = Some(item_idx);
                                        self.last_click_time = Some(now);
                                    }
                                }
                            }
                        }
                    }
                    return;
                }

                // Click on now-playing art -> zoom.
                if self.art.now_playing_art.cached().is_some()
                    && self.layout.now_playing_art_area.width > 0
                    && self.is_in_rect(event.column, event.row, self.layout.now_playing_art_area)
                {
                    self.mode = Mode::CoverArtZoom;
                    return;
                }

                // Seek bar click — only the first row of the transport area.
                if event.row == self.layout.transport_text_area.y
                    && event.column >= self.layout.transport_text_area.x
                    && event.column
                        < self.layout.transport_text_area.x + self.layout.transport_text_area.width
                    && let Some(info) = self.state.track_info()
                {
                    let click_x = event.column;
                    let dur = info.duration_ms;

                    if let Some(pos) = TransportBar::seek_from_click(
                        self.layout.seek_bar_start,
                        self.layout.seek_bar_width,
                        click_x,
                        dur,
                    ) {
                        self.tx.send(PlayerCommand::Seek(pos)).ok();
                    }
                    return;
                }

                // Scrollbar click — rightmost column of queue area (inside border).
                let q = self.layout.queue_area;
                let scrollbar_x = q.x + q.width - 1;
                if event.column == scrollbar_x && event.row > q.y && event.row + 1 < q.y + q.height
                {
                    let inner_y = q.y + 1;
                    let visible_height = q.height.saturating_sub(2) as usize;
                    let visible = self.visible_queue();
                    let total_lines = visible.len();

                    if total_lines > visible_height && visible_height > 0 {
                        let thumb_size = (visible_height * visible_height / total_lines).max(1);
                        let max_scroll = total_lines.saturating_sub(visible_height);
                        let thumb_offset = if max_scroll > 0 {
                            self.queue.scroll_offset * (visible_height - thumb_size) / max_scroll
                        } else {
                            0
                        };
                        let click_row = (event.row - inner_y) as usize;

                        if click_row >= thumb_offset && click_row < thumb_offset + thumb_size {
                            // Clicked on thumb — record grab offset, don't jump.
                            self.scrollbar_grab_offset = Some((click_row - thumb_offset) as u16);
                        } else {
                            // Clicked on track — jump, grab at thumb center.
                            self.scrollbar_grab_offset = Some((thumb_size / 2) as u16);
                            self.scroll_to_scrollbar_y(event.row);
                        }
                    }
                    return;
                }

                // Queue area click.
                if !self.is_in_rect(event.column, event.row, self.layout.queue_area) {
                    return;
                }

                // Switch focus to queue when clicking it in library mode.
                if self.mode == Mode::LibraryBrowse {
                    self.library_focus = LibraryFocus::Queue;
                }

                let visible = self.visible_queue();
                let Some(idx) = queue::QueueView::queue_index_at_y(
                    &visible,
                    self.layout.queue_area,
                    self.queue.scroll_offset,
                    event.row,
                ) else {
                    // Check if the click landed on an album header.
                    if let Some((first, last)) = queue::QueueView::album_group_at_y(
                        &visible,
                        self.layout.queue_area,
                        self.queue.scroll_offset,
                        event.row,
                    ) {
                        // Select all tracks in the album group.
                        self.queue.selected_ids.clear();
                        for i in first..=last {
                            if let Some(entry) = visible.get(i) {
                                self.queue.selected_ids.insert(entry.id);
                            }
                        }
                        self.queue.cursor = first;
                        if let Some(entry) = visible.get(first) {
                            self.queue.anchor_id = Some(entry.id);
                        }
                    }
                    return;
                };

                // Range-select: Shift or Ctrl (most terminals don't report Shift on mouse).
                let range_select = event.modifiers.contains(KeyModifiers::SHIFT)
                    || event.modifiers.contains(KeyModifiers::CONTROL);
                // Alt/Option for toggle-select (Cmd doesn't reach terminal).
                let toggle = event.modifiers.contains(KeyModifiers::ALT);

                // Mouse editing works in any mode — modality is keyboard-only.
                // Double-click plays; single click selects; drag reorders.
                let now = std::time::Instant::now();
                let is_double_click = self.last_click_idx == Some(idx)
                    && self
                        .last_click_time
                        .is_some_and(|t| now.duration_since(t).as_millis() < 400);

                if is_double_click {
                    // Double-click -> play the track at cursor.
                    self.last_click_idx = None;
                    self.last_click_time = None;
                    self.queue.cursor = idx;
                    self.play_at_cursor();
                } else {
                    self.last_click_idx = Some(idx);
                    self.last_click_time = Some(now);

                    if range_select {
                        self.extend_selection_to(idx);
                    } else if toggle {
                        self.toggle_selection(idx);
                    } else {
                        let id_at_idx = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
                        let already_selected =
                            id_at_idx.is_some_and(|id| self.queue.selected_ids.contains(&id));
                        if !already_selected {
                            // Only deselect others if clicking a NON-selected track.
                            // Clicking an already-selected track preserves the
                            // multi-selection so the user can drag the whole group.
                            self.select_single(idx);
                        }
                    }
                    self.queue.cursor = idx;

                    let multi = self.queue.selected_ids.len() > 1;
                    let anchor_offset = if multi {
                        let mut indices: Vec<usize> =
                            self.selected_indices().into_iter().collect();
                        indices.sort_unstable();
                        let first = indices.first().copied().unwrap_or(idx);
                        idx.saturating_sub(first)
                    } else {
                        0
                    };
                    self.queue.drag = Some(DragState {
                        from_index: idx,
                        current_y: event.row,
                        multi,
                        anchor_offset,
                        last_group_start: if multi {
                            let mut indices: Vec<usize> =
                                self.selected_indices().into_iter().collect();
                            indices.sort_unstable();
                            indices.first().copied()
                        } else {
                            Some(idx)
                        },
                    });
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.scrollbar_grab_offset.is_some() => {
                self.scroll_to_scrollbar_y(event.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let drag_info = self
                    .queue
                    .drag
                    .as_ref()
                    .map(|d| (d.from_index, d.multi, d.anchor_offset));
                if let Some((from_index, _multi, anchor_offset)) = drag_info {
                    if let Some(ref mut drag) = self.queue.drag {
                        drag.current_y = event.row;
                    }

                    if self.mode == Mode::QueueEdit
                        && self.is_in_rect(event.column, event.row, self.layout.queue_area)
                    {
                        // Edit mode: drag extends selection (shift-click workaround).
                        let visible = self.visible_queue();
                        if let Some(idx) = queue::QueueView::queue_index_at_y(
                            &visible,
                            self.layout.queue_area,
                            self.queue.scroll_offset,
                            event.row,
                        ) {
                            self.extend_selection_to(idx);
                            self.queue.cursor = idx;
                        }
                    } else if self.mode != Mode::QueueEdit
                        && self.is_in_rect(event.column, event.row, self.layout.queue_area)
                    {
                        // Normal mode: live reorder — move track(s) as mouse crosses rows.
                        let visible = self.visible_queue();
                        if let Some(to_idx) = queue::QueueView::queue_index_at_y(
                            &visible,
                            self.layout.queue_area,
                            self.queue.scroll_offset,
                            event.row,
                        ) {
                            if self.queue.selected_ids.len() > 1 {
                                // Multi-drag: compute desired group start from anchor offset
                                // so the clicked item stays under the mouse cursor.
                                let desired_start = to_idx.saturating_sub(anchor_offset);
                                let last_start = self
                                    .queue
                                    .drag
                                    .as_ref()
                                    .and_then(|d| d.last_group_start);

                                if Some(desired_start) != last_start {
                                    if !self.drag_undo_active {
                                        self.tx.send(PlayerCommand::BeginUndoBatch).ok();
                                        self.drag_undo_active = true;
                                    }

                                    // Compute target index for send_move_selected.
                                    let mut indices: Vec<usize> =
                                        self.selected_indices().into_iter().collect();
                                    indices.sort_unstable();
                                    let count = indices.len();
                                    let first = indices.first().copied().unwrap_or(0);

                                    let target = if desired_start > first {
                                        // Moving down: place after this index.
                                        (desired_start + count - 1)
                                            .min(visible.len().saturating_sub(1))
                                    } else {
                                        // Moving up: place at this index.
                                        desired_start
                                    };

                                    self.send_move_selected(target);
                                    if let Some(ref mut drag) = self.queue.drag {
                                        drag.last_group_start = Some(desired_start);
                                    }
                                }
                            } else {
                                // Single-track drag.
                                if to_idx != from_index {
                                    if !self.drag_undo_active {
                                        self.tx.send(PlayerCommand::BeginUndoBatch).ok();
                                        self.drag_undo_active = true;
                                    }
                                    self.send_move(from_index, to_idx);
                                    self.queue.cursor = to_idx;
                                    self.select_single(to_idx);
                                    if let Some(ref mut drag) = self.queue.drag {
                                        drag.from_index = to_idx;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Close undo batch if a drag reorder was active.
                if self.drag_undo_active {
                    self.tx.send(PlayerCommand::EndUndoBatch).ok();
                    self.drag_undo_active = false;
                }
                // Clear drag state — reorder already happened live during drag.
                self.queue.drag.take();
                self.scrollbar_grab_offset = None;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click on queue item -> context menu at click position.
                if self.is_in_rect(event.column, event.row, self.layout.queue_area) {
                    let visible = self.visible_queue();
                    if let Some(idx) = queue::QueueView::queue_index_at_y(
                        &visible,
                        self.layout.queue_area,
                        self.queue.scroll_offset,
                        event.row,
                    ) {
                        // Select the clicked item if not already selected.
                        let id_at_idx = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
                        if !id_at_idx.is_some_and(|id| self.queue.selected_ids.contains(&id)) {
                            self.select_single(idx);
                        }
                        self.queue.cursor = idx;

                        // Build context menu with relevant actions.
                        let is_fav = visible
                            .get(idx)
                            .is_some_and(|e| self.favourites.contains(&e.path));
                        let fav_label = if is_fav { "Unfavourite" } else { "Favourite" };
                        self.context_menu = Some(ContextMenuState {
                            actions: vec![
                                (ContextAction::Play, "Play", 'p'),
                                (ContextAction::ToggleFavourite, fav_label, 'f'),
                                (ContextAction::TrackInfo, "Track info", 'i'),
                                (ContextAction::Remove, "Remove", 'd'),
                                (ContextAction::Organize, "Organize files", 'o'),
                            ],
                            cursor: 0,
                        });
                        self.mode = Mode::ContextMenu;
                        // Store click position for positioned rendering.
                        self.hover.column = event.column;
                        self.hover.row = event.row;
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if let Mode::Picker(_) = &self.mode {
                    if let Some(ref mut picker) = self.picker {
                        picker.move_up();
                    }
                } else if self.mode == Mode::LibraryBrowse
                    && self.is_in_rect(event.column, event.row, self.layout.library_area)
                {
                    if let Some(ref mut lib) = self.library {
                        lib.move_up();
                    }
                } else {
                    self.queue.scroll_offset = self.queue.scroll_offset.saturating_sub(1);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Mode::Picker(_) = &self.mode {
                    if let Some(ref mut picker) = self.picker {
                        picker.move_down();
                    }
                } else if self.mode == Mode::LibraryBrowse
                    && self.is_in_rect(event.column, event.row, self.layout.library_area)
                {
                    if let Some(ref mut lib) = self.library {
                        lib.move_down();
                    }
                } else {
                    let visible_len = self.visible_queue().len();
                    let max_scroll = visible_len.saturating_sub(1);
                    self.queue.scroll_offset = (self.queue.scroll_offset + 1).min(max_scroll);
                }
            }
            MouseEventKind::Moved => {
                self.hover.column = event.column;
                self.hover.row = event.row;
                self.update_hover_zone();
            }
            _ => {}
        }
    }

    /// Compute the hover zone from current mouse position and layout rects.
    fn update_hover_zone(&mut self) {
        let col = self.hover.column;
        let row = self.hover.row;

        // Seek bar (first row of transport text area).
        if row == self.layout.transport_text_area.y
            && col >= self.layout.seek_bar_start
            && col < self.layout.seek_bar_start + self.layout.seek_bar_width
        {
            self.hover.zone = HoverZone::SeekBar;
            return;
        }

        // Transport art area.
        if self.layout.now_playing_art_area.width > 0
            && self.is_in_rect(col, row, self.layout.now_playing_art_area)
        {
            self.hover.zone = HoverZone::TransportArt;
            return;
        }

        // Transport text area.
        if self.is_in_rect(col, row, self.layout.transport_text_area) {
            self.hover.zone = HoverZone::TransportText;
            return;
        }

        // Queue scrollbar (rightmost column).
        let q = self.layout.queue_area;
        if q.width > 0 && col == q.x + q.width - 1 && row > q.y && row < q.y + q.height {
            self.hover.zone = HoverZone::ScrollbarQueue;
            return;
        }

        // Queue item.
        if self.is_in_rect(col, row, self.layout.queue_area) {
            let visible = self.visible_queue();
            if let Some(idx) = queue::QueueView::queue_index_at_y(
                &visible,
                self.layout.queue_area,
                self.queue.scroll_offset,
                row,
            ) {
                self.hover.zone = HoverZone::QueueItem(idx);
                return;
            }
        }

        // Library item.
        if self.mode == Mode::LibraryBrowse && self.is_in_rect(col, row, self.layout.library_area) {
            let inner_y = self.layout.library_area.y + 1;
            let inner_h = self.layout.library_area.height.saturating_sub(2) as usize;
            if row >= inner_y
                && (row - inner_y) < inner_h as u16
                && let Some(ref lib) = self.library
            {
                let item_idx = lib.scroll_offset + (row - inner_y) as usize;
                if item_idx < lib.nodes.len() {
                    self.hover.zone = HoverZone::LibraryItem(item_idx);
                    return;
                }
            }
        }

        self.hover.zone = HoverZone::None;
    }

    // --- Selection helpers ---

    /// Derive selected indices from selected IDs for the current visible queue.
    pub fn selected_indices(&self) -> HashSet<usize> {
        let visible = &self.queue.vq_cache.entries;
        visible
            .iter()
            .enumerate()
            .filter(|(_, e)| self.queue.selected_ids.contains(&e.id))
            .map(|(i, _)| i)
            .collect()
    }

    /// Get the index of the anchor in the current visible queue.
    fn anchor_index(&self) -> Option<usize> {
        let anchor = self.queue.anchor_id?;
        let visible = &self.queue.vq_cache.entries;
        visible.iter().position(|e| e.id == anchor)
    }

    /// Plain click / arrow: clear selection, select one track, set anchor.
    fn select_single(&mut self, idx: usize) {
        let id = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
        self.queue.selected_ids.clear();
        if let Some(id) = id {
            self.queue.selected_ids.insert(id);
            self.queue.anchor_id = Some(id);
        }
    }

    /// Shift-click/arrow: select range from anchor to idx (inclusive).
    fn extend_selection_to(&mut self, idx: usize) {
        let anchor = self.anchor_index().unwrap_or(self.queue.cursor);
        let lo = anchor.min(idx);
        let hi = anchor.max(idx);
        // Don't clear — shift extends. But we replace the range from anchor.
        self.queue.selected_ids.clear();
        let visible = &self.queue.vq_cache.entries;
        for i in lo..=hi {
            if let Some(e) = visible.get(i) {
                self.queue.selected_ids.insert(e.id);
            }
        }
        // Keep anchor where it was (only set if not already set).
        if self.queue.anchor_id.is_none()
            && let Some(e) = visible.get(anchor)
        {
            self.queue.anchor_id = Some(e.id);
        }
    }

    /// Alt-click: toggle one track in/out of selection set.
    fn toggle_selection(&mut self, idx: usize) {
        if let Some(e) = self.queue.vq_cache.entries.get(idx) {
            let id = e.id;
            if self.queue.selected_ids.contains(&id) {
                self.queue.selected_ids.remove(&id);
            } else {
                self.queue.selected_ids.insert(id);
            }
            // Move anchor to last toggled.
            self.queue.anchor_id = Some(id);
        }
    }

    /// Play the track at the current cursor position (Enter / double-click).
    fn play_at_cursor(&mut self) {
        let idx = self.queue.cursor;
        let visible = self.visible_queue();
        if let Some(entry) = visible.get(idx)
            && entry.status != QueueEntryStatus::Playing
        {
            self.tx.send(PlayerCommand::Play(entry.id)).ok();
        }
    }

    /// Delete all selected tracks.
    fn delete_selected(&mut self) {
        let ids: Vec<_> = if self.queue.selected_ids.is_empty() {
            // Nothing selected — delete the track under cursor.
            let visible = self.visible_queue();
            visible
                .get(self.queue.cursor)
                .map(|e| e.id)
                .into_iter()
                .collect()
        } else {
            self.queue.selected_ids.iter().copied().collect()
        };

        if !ids.is_empty() {
            self.tx
                .send(PlayerCommand::RemoveFromPlaylistBatch(ids))
                .ok();
        }

        self.queue.selected_ids.clear();
        let visible_len = self.visible_queue().len();
        if visible_len > 0 && self.queue.cursor >= visible_len {
            self.queue.cursor = visible_len - 1;
        }
    }

    /// Move all selected tracks down by one position.
    fn move_selected_down(&mut self) {
        let visible_len = self.visible_queue().len();

        // Single item: move the track under cursor.
        if self.queue.selected_ids.len() <= 1 {
            if self.queue.cursor + 1 < visible_len {
                self.send_move(self.queue.cursor, self.queue.cursor + 1);
                self.queue.cursor += 1;
                self.select_single(self.queue.cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
        indices.sort_unstable();

        let max_idx = indices.last().copied().unwrap_or(0);
        if max_idx + 1 >= visible_len {
            return;
        }
        let min_idx = indices.first().copied().unwrap_or(0);

        // Swap the item BELOW the group to ABOVE it — single atomic move.
        self.send_move(max_idx + 1, min_idx);

        // IDs are stable — selection follows automatically. Just update cursor.
        self.queue.cursor += 1;
        self.update_scroll();
    }

    /// Move all selected tracks up by one position.
    fn move_selected_up(&mut self) {
        // Single item: move the track under cursor.
        if self.queue.selected_ids.len() <= 1 {
            if self.queue.cursor > 0 {
                self.send_move(self.queue.cursor, self.queue.cursor - 1);
                self.queue.cursor -= 1;
                self.select_single(self.queue.cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
        indices.sort_unstable();

        let min_idx = indices.first().copied().unwrap_or(0);
        if min_idx == 0 {
            return;
        }
        let max_idx = indices.last().copied().unwrap_or(0);

        // Swap the item ABOVE the group to BELOW it — single atomic move.
        self.send_move(min_idx - 1, max_idx);

        // IDs are stable — selection follows automatically. Just update cursor.
        self.queue.cursor -= 1;
        self.update_scroll();
    }

    /// Send a batch move for all selected tracks to a target position.
    /// Used for multi-track drag.
    fn send_move_selected(&mut self, target_idx: usize) {
        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
        indices.sort_unstable();

        let visible = self.visible_queue();
        let ids: Vec<_> = indices
            .iter()
            .filter_map(|&i| visible.get(i).map(|e| e.id))
            .collect();
        let Some(target_entry) = visible.get(target_idx) else {
            return;
        };
        let target_id = target_entry.id;

        // Use MoveItemsInPlaylist to move the whole group atomically.
        let moving_down = target_idx > indices.first().copied().unwrap_or(0);
        self.tx
            .send(PlayerCommand::MoveItemsInPlaylist {
                ids,
                target: target_id,
                after: moving_down,
            })
            .ok();

        // IDs are stable — selected_ids already contains the right IDs.
        // Just update the cursor to the new visual position.
        let count = indices.len();
        let new_start = if moving_down {
            target_idx + 1 - count
        } else {
            target_idx
        };
        self.queue.cursor = if moving_down {
            new_start + count - 1
        } else {
            new_start
        };
    }

    /// Send a move command for a visible queue index pair.
    fn send_move(&self, from_visible: usize, to_visible: usize) {
        let visible = self.visible_queue();
        let Some(from_entry) = visible.get(from_visible) else {
            return;
        };
        let Some(to_entry) = visible.get(to_visible) else {
            return;
        };

        let after = to_visible > from_visible;
        self.tx
            .send(PlayerCommand::MoveInPlaylist {
                id: from_entry.id,
                target: to_entry.id,
                after,
            })
            .ok();
    }

    fn handle_library_key(&mut self, key: KeyEvent) {
        // When filter input is focused, route keys there first.
        if self.library.as_ref().is_some_and(|lib| lib.filter_active) {
            self.handle_library_filter_key(key);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                // If filter is non-empty, clear it first. Otherwise close library.
                if self.library.as_ref().is_some_and(|l| !l.filter.is_empty()) {
                    if let Some(ref mut lib) = self.library {
                        lib.clear_filter();
                    }
                } else {
                    self.library = None;
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char('q') => {
                self.tx.send(PlayerCommand::Stop).ok();
                self.quit = true;
            }
            KeyCode::Tab => {
                self.library_focus = match self.library_focus {
                    LibraryFocus::Library => LibraryFocus::Queue,
                    LibraryFocus::Queue => LibraryFocus::Library,
                };
            }
            KeyCode::Char(' ') => {
                if self.state.playback_state() == PlaybackState::Playing {
                    self.tx.send(PlayerCommand::Pause).ok();
                } else {
                    self.tx.send(PlayerCommand::Resume).ok();
                }
            }
            KeyCode::Char('>') | KeyCode::Char('n') => {
                self.tx.send(PlayerCommand::NextTrack).ok();
            }
            KeyCode::Char('<') => {
                self.tx.send(PlayerCommand::PrevTrack).ok();
            }
            _ => {
                if self.library_focus == LibraryFocus::Library {
                    self.handle_library_browse_key(key);
                }
            }
        }
    }

    fn handle_library_filter_key(&mut self, key: KeyEvent) {
        let Some(ref mut lib) = self.library else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                lib.clear_filter();
            }
            KeyCode::Enter => {
                lib.stop_filter();
            }
            KeyCode::Backspace => {
                if lib.filter.is_empty() {
                    lib.stop_filter();
                } else {
                    lib.filter_backspace();
                }
            }
            KeyCode::Char(c) => {
                lib.filter_type_char(c);
            }
            _ => {}
        }
    }

    fn handle_library_browse_key(&mut self, key: KeyEvent) {
        let Some(ref mut lib) = self.library else {
            return;
        };
        match key.code {
            KeyCode::Up => lib.move_up(),
            KeyCode::Down => lib.move_down(),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(ids) = lib.expand_or_enter() {
                    self.picker_result =
                        Some((PickerKind::Track, ids, PickerAction::AppendAndPlay));
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                lib.collapse_or_parent();
            }
            KeyCode::Char('a') => {
                let ids = lib.enqueue_all_under_cursor();
                if !ids.is_empty() {
                    self.picker_result =
                        Some((PickerKind::Track, ids, PickerAction::AppendAndPlay));
                }
            }
            KeyCode::Char('f') | KeyCode::Char('/') => {
                lib.start_filter();
            }
            _ => {}
        }
    }

    pub fn open_library(&mut self) {
        if self.library.is_none() {
            self.library = Some(LibraryState::new(&self.db_path));
        }
        self.mode = Mode::LibraryBrowse;
        self.library_focus = LibraryFocus::Library;
    }

    fn open_picker(&mut self, kind: PickerKind) {
        self.mode = Mode::Picker(kind);
    }

    fn open_queue_jump(&mut self) {
        let visible = self.visible_queue();
        if visible.is_empty() {
            return;
        }
        let items: Vec<super::picker::PickerItem> = visible
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let has_artist = !e.artist.is_empty() && e.artist != e.album_artist;
                let display = if has_artist {
                    format!("{} \u{2014} {}", e.artist, e.title)
                } else {
                    e.title.clone()
                };
                let parts = if has_artist {
                    vec![
                        (e.artist.clone(), PickerPartKind::Artist),
                        (" \u{2014} ".into(), PickerPartKind::Separator),
                        (e.title.clone(), PickerPartKind::Title),
                    ]
                } else {
                    vec![(e.title.clone(), PickerPartKind::Title)]
                };
                let match_text = format!("{} {} {} {}", e.artist, e.album_artist, e.album, e.title);
                super::picker::PickerItem {
                    id: i as i64,
                    display,
                    match_text,
                    parts,
                }
            })
            .collect();
        self.picker = Some(PickerState::new(PickerKind::QueueJump, items, false));
        self.mode = Mode::Picker(PickerKind::QueueJump);
    }

    /// Move cursor up by one page.
    fn page_up(&mut self, extend: bool) {
        let page_size = self.layout.queue_area.height.max(5) as usize;
        self.queue.cursor = self.queue.cursor.saturating_sub(page_size);
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Move cursor down by one page.
    fn page_down(&mut self, extend: bool) {
        let visible_len = self.visible_queue().len();
        let page_size = self.layout.queue_area.height.max(5) as usize;
        if visible_len > 0 {
            self.queue.cursor = (self.queue.cursor + page_size).min(visible_len - 1);
        }
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Jump cursor to start of queue.
    fn jump_to_start(&mut self, extend: bool) {
        self.queue.cursor = 0;
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Jump cursor to end of queue.
    fn jump_to_end(&mut self, extend: bool) {
        let visible_len = self.visible_queue().len();
        if visible_len > 0 {
            self.queue.cursor = visible_len - 1;
        }
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    fn update_scroll(&mut self) {
        let visible = self.visible_queue();
        let visible_height = self.layout.queue_area.height.max(10) as usize;
        self.queue.scroll_offset = queue::scroll_for_cursor(
            &visible,
            self.queue.cursor,
            self.queue.scroll_offset,
            visible_height,
        );
    }

    /// Scroll the queue based on a click/drag position on the scrollbar.
    /// Uses the same thumb math as queue.rs rendering so dragging is consistent.
    fn scroll_to_scrollbar_y(&mut self, y: u16) {
        let q = self.layout.queue_area;
        let inner_y = q.y + 1;
        let visible_height = q.height.saturating_sub(2) as usize;
        if visible_height == 0 {
            return;
        }

        let total_lines = self.visible_queue().len();
        if total_lines <= visible_height {
            return;
        }

        let thumb_size = (visible_height * visible_height / total_lines).max(1);
        let max_scroll = total_lines.saturating_sub(visible_height);
        let track_range = visible_height.saturating_sub(thumb_size);
        if track_range == 0 {
            return;
        }

        let grab_offset = self.scrollbar_grab_offset.unwrap_or(0) as usize;
        let desired_thumb_top = (y.saturating_sub(inner_y) as usize).saturating_sub(grab_offset);
        let clamped = desired_thumb_top.min(track_range);
        self.queue.scroll_offset = (clamped * max_scroll / track_range).min(max_scroll);
    }

    fn is_in_rect(&self, x: u16, y: u16, rect: ratatui::layout::Rect) -> bool {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }

    /// Get the drop indicator index — where external drops would insert.
    /// Only active when mouse is over queue and no internal drag is happening.
    pub fn drop_indicator_index(&self) -> Option<usize> {
        if self.queue.drag.is_some() {
            return None; // internal drag takes precedence
        }
        let row = self.last_mouse_row?;
        let visible = self.visible_queue();
        queue::QueueView::queue_index_at_y(
            &visible,
            self.layout.queue_area,
            self.queue.scroll_offset,
            row,
        )
    }

    /// Get the QueueItemId at the drop indicator position (for InsertInPlaylist).
    pub fn drop_target_queue_id(&self) -> Option<koan_core::player::state::QueueItemId> {
        let idx = self.drop_indicator_index()?;
        self.queue.vq_cache.entries.get(idx).map(|e| e.id)
    }

    /// Refresh the cached visible queue snapshot from shared state.
    /// Call once per frame before any queue-related reads.
    pub fn refresh_visible_queue(&mut self) {
        self.queue.vq_cache = self.state.derive_visible_queue();
    }

    pub fn visible_queue(&self) -> Vec<QueueEntry> {
        self.queue.vq_cache.entries.clone()
    }
}

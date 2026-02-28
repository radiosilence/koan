use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    PlaybackState, QueueEntry, QueueEntryStatus, SharedPlayerState, VisibleQueueSnapshot,
};

use super::library::LibraryState;
use super::picker::{PickerKind, PickerState, picker_results_rect};
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
}

pub struct App {
    pub mode: Mode,
    pub state: Arc<SharedPlayerState>,
    pub tx: Sender<PlayerCommand>,
    pub quit: bool,

    // Queue state.
    pub queue_cursor: usize,
    pub queue_scroll_offset: usize,

    // Selection state (Finder-style).
    pub selected_indices: HashSet<usize>,
    pub anchor_index: Option<usize>,

    // Picker state (when in Picker mode).
    pub picker: Option<PickerState>,

    // Mouse state.
    pub drag: Option<DragState>,

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

    // Transport area rect, stored after render for click-to-seek.
    pub transport_area: ratatui::layout::Rect,
    // Queue area rect, stored after render for mouse interaction.
    pub queue_area: ratatui::layout::Rect,

    // Picker overlay area, stored after render for mouse interaction.
    pub picker_area: ratatui::layout::Rect,

    // Track info overlay area, stored after render for mouse interaction.
    pub track_info_area: ratatui::layout::Rect,

    // Picker result — set when picker confirms, consumed by main loop.
    // Tagged with the picker kind so album IDs can be expanded to track IDs.
    pub picker_result: Option<(PickerKind, Vec<i64>)>,

    pub artist_drill_down: Option<i64>,

    // Loading overlay message (e.g. "loading album...").
    pub loading_message: Option<String>,

    // Auto-scroll: track by path so index shifts from finished_paths don't trigger.
    pub last_playing_path: Option<PathBuf>,

    // Library browser.
    pub library: Option<LibraryState>,
    pub library_focus: LibraryFocus,
    pub library_area: ratatui::layout::Rect,
    pub db_path: PathBuf,

    /// Cached visible queue snapshot — refreshed once per frame.
    /// All queue-related methods use this for consistency within a frame.
    vq_cache: VisibleQueueSnapshot,
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
            queue_cursor: 0,
            queue_scroll_offset: 0,
            selected_indices: HashSet::new(),
            anchor_index: None,
            picker: None,
            drag: None,
            spinner_tick: 0,
            last_click_time: None,
            last_click_idx: None,
            log_buffer,
            log_messages: Vec::new(),
            has_played: false,
            theme: Theme::default(),
            transport_area: ratatui::layout::Rect::default(),
            queue_area: ratatui::layout::Rect::default(),
            loading_message: None,
            picker_area: ratatui::layout::Rect::default(),
            track_info_area: ratatui::layout::Rect::default(),
            picker_result: None,
            artist_drill_down: None,
            last_playing_path: None,
            library: None,
            library_focus: LibraryFocus::Library,
            library_area: ratatui::layout::Rect::default(),
            db_path,
            vq_cache: VisibleQueueSnapshot::default(),
        }
    }

    pub fn handle_tick(&mut self) {
        // Refresh visible queue cache so all tick logic sees current state.
        self.refresh_visible_queue();

        self.spinner_tick = self.spinner_tick.wrapping_add(1);

        // Drain log buffer.
        if let Ok(mut logs) = self.log_buffer.lock() {
            self.log_messages.extend(logs.drain(..));
        }

        // Track playing state.
        if self.state.playback_state() == PlaybackState::Playing {
            self.has_played = true;
        }

        // Clear loading overlay once playback starts or pending queue populates.
        if self.loading_message.is_some() && (self.has_played || !self.vq_cache.entries.is_empty())
        {
            self.loading_message = None;
        }

        // Tick picker if active.
        if let Some(ref mut picker) = self.picker {
            picker.tick();
        }

        // In normal mode, auto-scroll to playing track on actual track change.
        // Derive the playing track from the visible queue cache (atomic snapshot)
        // NOT from track_info directly — track_info changes before the visible
        // queue is rebuilt, causing a 1-frame scroll offset jump.
        if self.mode == Mode::Normal {
            let current_playing = self
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
                .map(|e| e.path.clone());
            if current_playing != self.last_playing_path {
                self.last_playing_path = current_playing;
                if let Some(idx) = self
                    .vq_cache
                    .entries
                    .iter()
                    .position(|e| e.status == QueueEntryStatus::Playing)
                {
                    let visible_height = self.queue_area.height.max(5) as usize;
                    self.queue_scroll_offset = queue::scroll_for_cursor(
                        &self.visible_queue(),
                        idx,
                        self.queue_scroll_offset,
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
                let min = self.queue_cursor_min();
                if self.queue_cursor < min {
                    self.queue_cursor = min;
                }
                // Sync selection to cursor so j/k work immediately.
                if self.selected_indices.len() <= 1 {
                    self.select_single(self.queue_cursor);
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
                    self.queue_cursor = self.queue_cursor.saturating_sub(1);
                    self.select_single(self.queue_cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Down => {
                let visible = self.visible_queue();
                if !visible.is_empty() && self.queue_cursor + 1 < visible.len() {
                    self.queue_cursor += 1;
                    self.select_single(self.queue_cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Enter => {
                self.play_at_cursor();
            }
            KeyCode::Char('i') => {
                let visible = self.visible_queue();
                if !visible.is_empty() && self.queue_cursor < visible.len() {
                    self.mode = Mode::TrackInfo(self.queue_cursor);
                }
            }
            _ => {}
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let min = self.queue_cursor_min();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.selected_indices.clear();
                self.anchor_index = None;
            }
            KeyCode::Char('q') => {
                self.tx.send(PlayerCommand::Stop).ok();
                self.quit = true;
            }
            KeyCode::Up => {
                self.queue_cursor = self.queue_cursor.saturating_sub(1).max(min);
                if shift {
                    self.extend_selection_to(self.queue_cursor);
                } else {
                    self.select_single(self.queue_cursor);
                }
                self.update_scroll();
            }
            KeyCode::Down => {
                let visible = self.visible_queue();
                if self.queue_cursor + 1 < visible.len() {
                    self.queue_cursor += 1;
                }
                if shift {
                    self.extend_selection_to(self.queue_cursor);
                } else {
                    self.select_single(self.queue_cursor);
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
                if self.queue_cursor + 1 < visible_len {
                    self.queue_cursor += 1;
                    self.extend_selection_to(self.queue_cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Char('K') => {
                if self.queue_cursor > min {
                    self.queue_cursor -= 1;
                    self.extend_selection_to(self.queue_cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Char('i') => {
                let visible = self.visible_queue();
                if !visible.is_empty() && self.queue_cursor < visible.len() {
                    self.mode = Mode::TrackInfo(self.queue_cursor);
                }
            }
            _ => {}
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let Some(ref mut picker) = self.picker else {
            return;
        };

        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let ids = picker.confirm();
                let kind = picker.kind;
                self.picker = None;
                self.mode = Mode::Normal;

                if !ids.is_empty() {
                    match kind {
                        PickerKind::Track | PickerKind::Album => {
                            self.picker_result = Some((kind, ids));
                        }
                        PickerKind::Artist => {
                            self.artist_drill_down = Some(ids[0]);
                        }
                    }
                }
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
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        // Track info intercepts all mouse events when active.
        if matches!(self.mode, Mode::TrackInfo(_)) {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind
                && !self.is_in_rect(event.column, event.row, self.track_info_area)
            {
                self.mode = Mode::Normal;
            }
            return;
        }

        // Picker intercepts all mouse events when active.
        if let Mode::Picker(_) = &self.mode {
            let picker_area = self.picker_area;
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
                                            self.picker_result = Some((kind, ids));
                                        }
                                        PickerKind::Artist => {
                                            self.artist_drill_down = Some(ids[0]);
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
                    // Click outside picker → close.
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
                    && self.is_in_rect(event.column, event.row, self.library_area)
                {
                    self.library_focus = LibraryFocus::Library;
                    if let Some(ref mut lib) = self.library {
                        let inner_x = self.library_area.x + 1;
                        let inner_y = self.library_area.y + 1;
                        let inner_h = self.library_area.height.saturating_sub(2) as usize;
                        if event.row >= inner_y && (event.row - inner_y) < inner_h as u16 {
                            let row = (event.row - inner_y) as usize;
                            let col = event.column.saturating_sub(inner_x) as usize;
                            let item_idx = lib.scroll_offset + row;
                            if item_idx < lib.nodes.len() {
                                lib.cursor = item_idx;

                                // Click on arrow area (first ~4 chars) → expand/collapse.
                                // Click on text → double-click to enqueue.
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
                                            self.picker_result = Some((PickerKind::Track, ids));
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

                // Transport area click -> seek.
                if self.is_in_rect(event.column, event.row, self.transport_area)
                    && let Some(info) = self.state.track_info()
                    && let Some(pos) = TransportBar::seek_from_click(
                        self.transport_area,
                        event.column,
                        &info,
                        self.state.position_ms(),
                    )
                {
                    self.tx.send(PlayerCommand::Seek(pos)).ok();
                    return;
                }

                // Queue area click.
                if !self.is_in_rect(event.column, event.row, self.queue_area) {
                    return;
                }

                // Switch focus to queue when clicking it in library mode.
                if self.mode == Mode::LibraryBrowse {
                    self.library_focus = LibraryFocus::Queue;
                }

                let visible = self.visible_queue();
                let Some(idx) = queue::QueueView::queue_index_at_y(
                    &visible,
                    self.queue_area,
                    self.queue_scroll_offset,
                    event.row,
                ) else {
                    return;
                };

                let offset = self.queue_edit_offset();
                let shift = event.modifiers.contains(KeyModifiers::SHIFT);
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
                    // Double-click → play the track at cursor.
                    self.last_click_idx = None;
                    self.last_click_time = None;
                    self.queue_cursor = idx;
                    self.play_at_cursor();
                } else {
                    self.last_click_idx = Some(idx);
                    self.last_click_time = Some(now);

                    // Only select/drag upcoming (editable) tracks.
                    if idx >= offset {
                        if shift {
                            self.extend_selection_to(idx);
                        } else if toggle {
                            self.toggle_selection(idx);
                        } else {
                            self.select_single(idx);
                        }
                        self.queue_cursor = idx;

                        let multi = self.selected_indices.len() > 1;
                        self.drag = Some(DragState {
                            from_index: idx,
                            current_y: event.row,
                            multi,
                        });
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(ref mut drag) = self.drag {
                    drag.current_y = event.row;

                    // Shift-drag: extend selection continuously.
                    if event.modifiers.contains(KeyModifiers::SHIFT)
                        && self.is_in_rect(event.column, event.row, self.queue_area)
                    {
                        let visible = self.visible_queue();
                        if let Some(idx) = queue::QueueView::queue_index_at_y(
                            &visible,
                            self.queue_area,
                            self.queue_scroll_offset,
                            event.row,
                        ) {
                            let offset = self.queue_edit_offset();
                            if idx >= offset {
                                self.extend_selection_to(idx);
                                self.queue_cursor = idx;
                            }
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag) = self.drag.take() {
                    let visible = self.visible_queue();
                    let Some(to_idx) = queue::QueueView::queue_index_at_y(
                        &visible,
                        self.queue_area,
                        self.queue_scroll_offset,
                        drag.current_y,
                    ) else {
                        return;
                    };

                    let offset = self.queue_edit_offset();
                    if drag.from_index < offset || to_idx < offset {
                        return;
                    }

                    if to_idx == drag.from_index {
                        return; // click, not drag
                    }

                    if drag.multi && self.selected_indices.len() > 1 {
                        self.move_selected_to(to_idx);
                    } else {
                        self.send_move(drag.from_index, to_idx);
                        self.queue_cursor = to_idx;
                        self.select_single(to_idx);
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if let Mode::Picker(_) = &self.mode {
                    if let Some(ref mut picker) = self.picker {
                        picker.move_up();
                        picker.move_up();
                        picker.move_up();
                    }
                } else if self.mode == Mode::LibraryBrowse
                    && self.is_in_rect(event.column, event.row, self.library_area)
                {
                    if let Some(ref mut lib) = self.library {
                        lib.move_up();
                        lib.move_up();
                        lib.move_up();
                    }
                } else {
                    self.queue_scroll_offset = self.queue_scroll_offset.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Mode::Picker(_) = &self.mode {
                    if let Some(ref mut picker) = self.picker {
                        picker.move_down();
                        picker.move_down();
                        picker.move_down();
                    }
                } else if self.mode == Mode::LibraryBrowse
                    && self.is_in_rect(event.column, event.row, self.library_area)
                {
                    if let Some(ref mut lib) = self.library {
                        lib.move_down();
                        lib.move_down();
                        lib.move_down();
                    }
                } else {
                    // Clamp scroll to prevent scrolling past end.
                    let visible_len = self.visible_queue().len();
                    let max_scroll = visible_len.saturating_sub(1);
                    self.queue_scroll_offset = (self.queue_scroll_offset + 3).min(max_scroll);
                }
            }
            _ => {}
        }
    }

    // --- Selection helpers ---

    /// Plain click / arrow: clear selection, select one track, set anchor.
    fn select_single(&mut self, idx: usize) {
        self.selected_indices.clear();
        self.selected_indices.insert(idx);
        self.anchor_index = Some(idx);
    }

    /// Shift-click/arrow: select range from anchor to idx (inclusive).
    fn extend_selection_to(&mut self, idx: usize) {
        let anchor = self.anchor_index.unwrap_or(self.queue_cursor);
        let lo = anchor.min(idx);
        let hi = anchor.max(idx);
        // Don't clear — shift extends. But we replace the range from anchor.
        self.selected_indices.clear();
        for i in lo..=hi {
            self.selected_indices.insert(i);
        }
        // Keep anchor where it was.
        if self.anchor_index.is_none() {
            self.anchor_index = Some(anchor);
        }
    }

    /// Alt-click: toggle one track in/out of selection set.
    fn toggle_selection(&mut self, idx: usize) {
        if self.selected_indices.contains(&idx) {
            self.selected_indices.remove(&idx);
        } else {
            self.selected_indices.insert(idx);
        }
        // Move anchor to last toggled.
        self.anchor_index = Some(idx);
    }

    /// Play the track at the current cursor position (Enter / double-click).
    fn play_at_cursor(&mut self) {
        let idx = self.queue_cursor;
        let visible = self.visible_queue();
        if let Some(entry) = visible.get(idx)
            && entry.status != QueueEntryStatus::Playing
        {
            self.tx.send(PlayerCommand::Play(entry.id)).ok();
        }
    }

    /// Delete all selected tracks.
    fn delete_selected(&mut self) {
        let indices: Vec<usize> = if self.selected_indices.is_empty() {
            vec![self.queue_cursor]
        } else {
            self.selected_indices.iter().copied().collect()
        };

        let visible = self.visible_queue();
        for idx in &indices {
            if let Some(entry) = visible.get(*idx) {
                self.tx
                    .send(PlayerCommand::RemoveFromPlaylist(entry.id))
                    .ok();
            }
        }

        self.selected_indices.clear();
        let min = self.queue_cursor_min();
        let visible_len = self.visible_queue().len();
        if visible_len > min && self.queue_cursor >= visible_len {
            self.queue_cursor = visible_len - 1;
        }
    }

    /// Move all selected tracks down by one position.
    fn move_selected_down(&mut self) {
        let visible_len = self.visible_queue().len();
        let offset = self.queue_edit_offset();

        // Single item: move the track under cursor.
        if self.selected_indices.len() <= 1 {
            if self.queue_cursor + 1 < visible_len && self.queue_cursor >= offset {
                self.send_move(self.queue_cursor, self.queue_cursor + 1);
                self.queue_cursor += 1;
                self.select_single(self.queue_cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();

        let max_idx = indices.last().copied().unwrap_or(0);
        if max_idx + 1 >= visible_len {
            return;
        }
        let min_idx = indices.first().copied().unwrap_or(0);

        // Swap the item BELOW the group to ABOVE it — single atomic move.
        if max_idx + 1 < visible_len && min_idx >= offset {
            self.send_move(max_idx + 1, min_idx);
        }

        let new_selected: HashSet<usize> = indices.iter().map(|&i| i + 1).collect();
        self.selected_indices = new_selected;
        self.queue_cursor += 1;
        self.anchor_index = self.anchor_index.map(|a| a + 1);
        self.update_scroll();
    }

    /// Move all selected tracks up by one position.
    fn move_selected_up(&mut self) {
        let offset = self.queue_edit_offset();

        // Single item: move the track under cursor.
        if self.selected_indices.len() <= 1 {
            if self.queue_cursor > offset {
                self.send_move(self.queue_cursor, self.queue_cursor - 1);
                self.queue_cursor -= 1;
                self.select_single(self.queue_cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();

        let min_idx = indices.first().copied().unwrap_or(offset);
        if min_idx <= offset {
            return;
        }
        let max_idx = indices.last().copied().unwrap_or(0);

        // Swap the item ABOVE the group to BELOW it — single atomic move.
        self.send_move(min_idx - 1, max_idx);

        let new_selected: HashSet<usize> = indices.iter().map(|&i| i - 1).collect();
        self.selected_indices = new_selected;
        self.queue_cursor -= 1;
        self.anchor_index = self.anchor_index.map(|a| a.saturating_sub(1));
        self.update_scroll();
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

    /// Move all selected tracks so the group lands at `target_idx` (visible space).
    fn move_selected_to(&mut self, target_idx: usize) {
        if self.selected_indices.is_empty() {
            return;
        }

        let edit_offset = self.queue_edit_offset();
        let visible = self.visible_queue();
        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();

        // Collect IDs of selected items (in order).
        let ids: Vec<_> = indices
            .iter()
            .filter(|&&i| i >= edit_offset)
            .filter_map(|&i| visible.get(i).map(|e| e.id))
            .collect();

        if ids.is_empty() {
            return;
        }

        let count = ids.len();
        let group_start = *indices.first().unwrap();

        // Find the target entry to place relative to.
        let (target_id, after) = if target_idx < group_start {
            // Moving up: place before the item at target_idx.
            let Some(entry) = visible.get(target_idx.max(edit_offset)) else {
                return;
            };
            (entry.id, false)
        } else {
            // Moving down: place after the item at target_idx.
            let Some(entry) = visible.get(target_idx) else {
                return;
            };
            (entry.id, true)
        };

        self.tx
            .send(PlayerCommand::MoveItemsInPlaylist {
                ids,
                target: target_id,
                after,
            })
            .ok();

        // Update local selection to where the group will land.
        let new_start = if target_idx < group_start {
            target_idx.max(edit_offset)
        } else {
            target_idx + 1 - count
        };
        self.selected_indices = (new_start..new_start + count).collect();
        self.queue_cursor = target_idx;
        self.anchor_index = Some(new_start);
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
                    self.picker_result = Some((PickerKind::Track, ids));
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                lib.collapse_or_parent();
            }
            KeyCode::Char('a') => {
                let ids = lib.enqueue_all_under_cursor();
                if !ids.is_empty() {
                    self.picker_result = Some((PickerKind::Track, ids));
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

    fn update_scroll(&mut self) {
        let visible = self.visible_queue();
        let visible_height = self.queue_area.height.max(10) as usize;
        self.queue_scroll_offset = queue::scroll_for_cursor(
            &visible,
            self.queue_cursor,
            self.queue_scroll_offset,
            visible_height,
        );
    }

    fn is_in_rect(&self, x: u16, y: u16, rect: ratatui::layout::Rect) -> bool {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }

    /// Get the drag target index (for visual feedback in queue).
    pub fn drag_target_index(&self) -> Option<usize> {
        let drag = self.drag.as_ref()?;
        let visible = self.visible_queue();
        queue::QueueView::queue_index_at_y(
            &visible,
            self.queue_area,
            self.queue_scroll_offset,
            drag.current_y,
        )
    }

    /// Refresh the cached visible queue snapshot from shared state.
    /// Call once per frame before any queue-related reads.
    pub fn refresh_visible_queue(&mut self) {
        self.vq_cache = self.state.derive_visible_queue();
    }

    pub fn visible_queue(&self) -> Vec<QueueEntry> {
        self.vq_cache.entries.clone()
    }

    /// Offset into visible_queue() where the player queue entries start
    /// (after finished + playing).
    pub fn queue_edit_offset(&self) -> usize {
        self.vq_cache.finished_count + usize::from(self.vq_cache.has_playing)
    }

    /// Minimum cursor position in edit mode (can reach playing track).
    pub fn queue_cursor_min(&self) -> usize {
        self.vq_cache.finished_count
    }
}

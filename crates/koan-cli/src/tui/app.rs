use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, SharedPlayerState};

use super::library::LibraryState;
use super::picker::{PickerKind, PickerState};
use super::queue;
use super::theme::Theme;
use super::transport::TransportBar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    QueueEdit,
    Picker(PickerKind),
    LibraryBrowse,
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

    // Picker result — set when picker confirms, consumed by main loop.
    pub picker_result: Option<Vec<i64>>,

    pub artist_drill_down: Option<i64>,

    // Auto-scroll on track change only.
    pub last_playing_idx: Option<usize>,

    // Library browser.
    pub library: Option<LibraryState>,
    pub library_focus: LibraryFocus,
    pub library_area: ratatui::layout::Rect,
    pub db_path: PathBuf,
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
            log_buffer,
            log_messages: Vec::new(),
            has_played: false,
            theme: Theme::default(),
            transport_area: ratatui::layout::Rect::default(),
            queue_area: ratatui::layout::Rect::default(),
            picker_result: None,
            artist_drill_down: None,
            last_playing_idx: None,
            library: None,
            library_focus: LibraryFocus::Library,
            library_area: ratatui::layout::Rect::default(),
            db_path,
        }
    }

    pub fn handle_tick(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);

        // Drain log buffer.
        if let Ok(mut logs) = self.log_buffer.lock() {
            self.log_messages.extend(logs.drain(..));
        }

        // Track playing state.
        if self.state.playback_state() == PlaybackState::Playing {
            self.has_played = true;
        }

        // Tick picker if active.
        if let Some(ref mut picker) = self.picker {
            picker.tick();
        }

        // In normal mode, auto-scroll to playing track on track change.
        if self.mode == Mode::Normal {
            let queue = self.state.full_queue();
            let playing_idx = queue
                .iter()
                .position(|e| e.status == koan_core::player::state::QueueEntryStatus::Playing);
            if playing_idx != self.last_playing_idx {
                self.last_playing_idx = playing_idx;
                if let Some(idx) = playing_idx {
                    let visible_height = self.queue_area.height.max(5) as usize;
                    self.queue_scroll_offset = queue::scroll_for_cursor(
                        &queue,
                        idx,
                        self.queue_scroll_offset,
                        visible_height,
                    );
                }
            }
        }

        // Auto-quit when playback finishes.
        if self.has_played
            && self.state.playback_state() == PlaybackState::Stopped
            && self.state.track_info().is_none()
            && self.state.full_queue().is_empty()
        {
            self.quit = true;
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
                self.selected_indices.clear();
                self.anchor_index = None;
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
            _ => {}
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
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
                let prev = self.queue_cursor;
                self.queue_cursor = self.queue_cursor.saturating_sub(1);
                if shift {
                    self.extend_selection_to(self.queue_cursor);
                } else {
                    self.select_single(self.queue_cursor);
                }
                let _ = prev; // silence
                self.update_scroll();
            }
            KeyCode::Down => {
                let queue = self.state.full_queue();
                if self.queue_cursor + 1 < queue.len() {
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
                // Shift+J: extend selection down.
                let queue = self.state.full_queue();
                if self.queue_cursor + 1 < queue.len() {
                    self.queue_cursor += 1;
                    self.extend_selection_to(self.queue_cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Char('K') => {
                // Shift+K: extend selection up.
                if self.queue_cursor > 0 {
                    self.queue_cursor -= 1;
                    self.extend_selection_to(self.queue_cursor);
                    self.update_scroll();
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
                            self.picker_result = Some(ids);
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

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
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

                let queue = self.state.full_queue();
                let Some(idx) = queue::QueueView::queue_index_at_y(
                    &queue,
                    self.queue_area,
                    self.queue_scroll_offset,
                    event.row,
                ) else {
                    return;
                };

                let shift = event.modifiers.contains(KeyModifiers::SHIFT);
                // Alt/Option for toggle-select (Cmd doesn't reach terminal).
                let toggle = event.modifiers.contains(KeyModifiers::ALT);

                if self.mode == Mode::QueueEdit {
                    if shift {
                        // Shift-click: range selection from anchor.
                        self.extend_selection_to(idx);
                        self.queue_cursor = idx;
                    } else if toggle {
                        // Alt-click: toggle individual track.
                        self.toggle_selection(idx);
                        self.queue_cursor = idx;
                    } else {
                        // Plain click: select single, set anchor, start drag.
                        self.select_single(idx);
                        self.queue_cursor = idx;
                    }

                    // Start drag — multi if multiple are selected.
                    let multi = self.selected_indices.len() > 1;
                    self.drag = Some(DragState {
                        from_index: idx,
                        current_y: event.row,
                        multi,
                    });
                } else if shift {
                    // Shift-click in normal mode -> switch to edit + range.
                    self.mode = Mode::QueueEdit;
                    if self.anchor_index.is_none() {
                        self.anchor_index = Some(self.queue_cursor);
                    }
                    self.extend_selection_to(idx);
                    self.queue_cursor = idx;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(ref mut drag) = self.drag {
                    let prev_y = drag.current_y;
                    drag.current_y = event.row;

                    // Shift-drag: extend selection continuously.
                    if event.modifiers.contains(KeyModifiers::SHIFT)
                        && self.is_in_rect(event.column, event.row, self.queue_area)
                    {
                        let queue = self.state.full_queue();
                        if let Some(idx) = queue::QueueView::queue_index_at_y(
                            &queue,
                            self.queue_area,
                            self.queue_scroll_offset,
                            event.row,
                        ) {
                            self.extend_selection_to(idx);
                            self.queue_cursor = idx;
                        }
                    }
                    let _ = prev_y;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag) = self.drag.take() {
                    let queue = self.state.full_queue();
                    let Some(to_idx) = queue::QueueView::queue_index_at_y(
                        &queue,
                        self.queue_area,
                        self.queue_scroll_offset,
                        drag.current_y,
                    ) else {
                        return;
                    };

                    if to_idx == drag.from_index {
                        return; // click, not drag — selection already handled on Down
                    }

                    if drag.multi && self.selected_indices.len() > 1 {
                        // Multi-drag: move all selected tracks as a group.
                        self.move_selected_to(to_idx);
                    } else {
                        // Single drag.
                        self.tx
                            .send(PlayerCommand::MoveInQueue {
                                from: drag.from_index,
                                to: to_idx,
                            })
                            .ok();
                        self.queue_cursor = to_idx;
                        self.select_single(to_idx);
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                self.queue_scroll_offset = self.queue_scroll_offset.saturating_sub(3);
            }
            MouseEventKind::ScrollDown => {
                self.queue_scroll_offset += 3;
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

    /// Delete all selected tracks from the queue (highest index first).
    fn delete_selected(&mut self) {
        if self.selected_indices.is_empty() {
            // Fall back to cursor.
            self.tx
                .send(PlayerCommand::RemoveFromQueue(self.queue_cursor))
                .ok();
            return;
        }
        // Remove from highest index first so indices stay valid.
        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in indices {
            self.tx.send(PlayerCommand::RemoveFromQueue(idx)).ok();
        }
        self.selected_indices.clear();
        // Clamp cursor.
        let queue_len = self.state.full_queue().len();
        if queue_len > 0 && self.queue_cursor >= queue_len {
            self.queue_cursor = queue_len - 1;
        }
    }

    /// Move all selected tracks down by one position.
    fn move_selected_down(&mut self) {
        let queue = self.state.full_queue();
        if self.selected_indices.is_empty() {
            // Single cursor move.
            if self.queue_cursor + 1 < queue.len() {
                self.tx
                    .send(PlayerCommand::MoveInQueue {
                        from: self.queue_cursor,
                        to: self.queue_cursor + 1,
                    })
                    .ok();
                self.queue_cursor += 1;
                self.select_single(self.queue_cursor);
                self.update_scroll();
            }
            return;
        }

        // Move group down: process from bottom to top.
        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();

        // Can't move if the bottom-most is already at the end.
        if indices.last().copied().unwrap_or(0) + 1 >= queue.len() {
            return;
        }

        // Move from bottom to top to avoid index shifts.
        let mut new_selected = HashSet::new();
        for &idx in indices.iter().rev() {
            self.tx
                .send(PlayerCommand::MoveInQueue {
                    from: idx,
                    to: idx + 1,
                })
                .ok();
            new_selected.insert(idx + 1);
        }
        self.selected_indices = new_selected;
        self.queue_cursor += 1;
        self.anchor_index = self.anchor_index.map(|a| a + 1);
        self.update_scroll();
    }

    /// Move all selected tracks up by one position.
    fn move_selected_up(&mut self) {
        if self.selected_indices.is_empty() {
            if self.queue_cursor > 0 {
                self.tx
                    .send(PlayerCommand::MoveInQueue {
                        from: self.queue_cursor,
                        to: self.queue_cursor - 1,
                    })
                    .ok();
                self.queue_cursor -= 1;
                self.select_single(self.queue_cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();

        if indices.first().copied().unwrap_or(0) == 0 {
            return;
        }

        // Move from top to bottom.
        let mut new_selected = HashSet::new();
        for &idx in &indices {
            self.tx
                .send(PlayerCommand::MoveInQueue {
                    from: idx,
                    to: idx - 1,
                })
                .ok();
            new_selected.insert(idx - 1);
        }
        self.selected_indices = new_selected;
        self.queue_cursor -= 1;
        self.anchor_index = self.anchor_index.map(|a| a.saturating_sub(1));
        self.update_scroll();
    }

    /// Move all selected tracks so the group lands at `target_idx`.
    fn move_selected_to(&mut self, target_idx: usize) {
        if self.selected_indices.is_empty() {
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();

        let group_start = *indices.first().unwrap();

        if target_idx < group_start {
            // Moving up: process top to bottom.
            let mut new_selected = HashSet::new();
            for (offset, &idx) in indices.iter().enumerate() {
                let dest = target_idx + offset;
                self.tx
                    .send(PlayerCommand::MoveInQueue {
                        from: idx,
                        to: dest,
                    })
                    .ok();
                new_selected.insert(dest);
            }
            self.selected_indices = new_selected;
            self.queue_cursor = target_idx;
            self.anchor_index = Some(target_idx);
        } else {
            // Moving down: process bottom to top.
            let count = indices.len();
            let mut new_selected = HashSet::new();
            for (offset, &idx) in indices.iter().rev().enumerate() {
                let dest = target_idx.saturating_sub(offset);
                self.tx
                    .send(PlayerCommand::MoveInQueue {
                        from: idx,
                        to: dest,
                    })
                    .ok();
                new_selected.insert(dest);
            }
            self.selected_indices = new_selected;
            self.queue_cursor = target_idx;
            self.anchor_index = Some(target_idx.saturating_sub(count - 1));
        }
    }

    fn handle_library_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.library = None;
                self.mode = Mode::Normal;
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

    fn handle_library_browse_key(&mut self, key: KeyEvent) {
        let Some(ref mut lib) = self.library else {
            return;
        };
        match key.code {
            KeyCode::Up => lib.move_up(),
            KeyCode::Down => lib.move_down(),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(ids) = lib.expand_or_enter() {
                    self.picker_result = Some(ids);
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                lib.collapse_or_parent();
            }
            KeyCode::Char('a') => {
                let ids = lib.enqueue_all_under_cursor();
                if !ids.is_empty() {
                    self.picker_result = Some(ids);
                }
            }
            _ => {}
        }
    }

    fn open_library(&mut self) {
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
        let queue = self.state.full_queue();
        let visible_height = self.queue_area.height.max(10) as usize;
        self.queue_scroll_offset = queue::scroll_for_cursor(
            &queue,
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
        let queue = self.state.full_queue();
        queue::QueueView::queue_index_at_y(
            &queue,
            self.queue_area,
            self.queue_scroll_offset,
            drag.current_y,
        )
    }
}

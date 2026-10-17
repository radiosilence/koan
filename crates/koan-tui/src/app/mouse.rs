//! The pointer. An open overlay takes every event; otherwise a click, a drag
//! or the wheel goes to the region it lands in: the key hints, the EQ, the
//! sidebar or tab bar, the page, the transport, the queue.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Margin, Position, Rect};

use koan_core::player::commands::PlayerCommand;

use super::{App, DragState, HoverZone, Mode, NavTarget, Page, PickerAction, browse, queue};
use crate::picker::picker_results_rect;
use crate::playlists_page::PlAction;

/// What a click landed on, so a second click is a double click only on the
/// same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Click {
    QueueRow(usize),
    QueueHeading(usize),
    PickerRow(usize),
}

/// The longest wait between the clicks of a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

fn hit(rect: Rect, col: u16, row: u16) -> bool {
    rect.contains(Position::new(col, row))
}

/// A modal's rows, inside its border.
fn inside(area: Rect) -> Rect {
    area.inner(Margin::new(1, 1))
}

impl App {
    /// True when `click` is the second of a double click. Otherwise it is
    /// remembered as the first.
    fn double_click(&mut self, click: Click) -> bool {
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(c, at)| c == click && now.duration_since(at) < DOUBLE_CLICK);
        self.last_click = (!double).then_some((click, now));
        double
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        if self.resize_transport(event) {
            return;
        }
        self.last_mouse_row =
            hit(self.layout.queue_area, event.column, event.row).then_some(event.row);
        // A drag ends with the button, whatever opened over it meanwhile.
        if event.kind == MouseEventKind::Up(MouseButton::Left) {
            // The reorder happened live during the drag; it is one undo.
            if self.drag_undo_active {
                self.tx.send(PlayerCommand::EndUndoBatch).ok();
                self.drag_undo_active = false;
            }
            self.queue.drag = None;
            self.scrollbar_grab_offset = None;
            self.browse_drag = false;
        }
        if self.overlay_mouse(event) {
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.left_click(event),
            MouseEventKind::Drag(MouseButton::Left) => self.left_drag(event),
            MouseEventKind::Down(MouseButton::Right) => self.right_click(event),
            MouseEventKind::ScrollUp => self.scroll(event, -1),
            MouseEventKind::ScrollDown => self.scroll(event, 1),
            MouseEventKind::Moved => {
                self.hover.column = event.column;
                self.hover.row = event.row;
                self.update_hover_zone();
            }
            _ => {}
        }
    }

    /// Dragging the hairline above the transport resizes the cover. True
    /// when `event` was part of that.
    fn resize_transport(&mut self, event: MouseEvent) -> bool {
        match (event.kind, self.transport_drag) {
            (MouseEventKind::Down(MouseButton::Left), _) => {
                let t = self.layout.transport_area;
                if t.height > 0
                    && self.layout.transport_rule_y == Some(event.row)
                    && event.column >= t.x
                    && event.column < t.x + t.width
                {
                    self.transport_drag = Some((event.row, self.art_size));
                }
                false
            }
            (MouseEventKind::Drag(MouseButton::Left), Some((start_row, original))) => {
                // A row of drag is two units of art size: the cover is half
                // as tall as it is wide.
                let delta = start_row as i32 - event.row as i32;
                self.art_size = (original as i32 + delta * 2).clamp(4, 80) as u16;
                true
            }
            (MouseEventKind::Up(MouseButton::Left), Some(_)) => {
                let size = self.art_size;
                let _ = koan_core::config::Config::persist(|cfg| cfg.playback.art_size = size);
                self.transport_drag = None;
                true
            }
            (_, Some(_)) => {
                self.transport_drag = None;
                false
            }
            _ => false,
        }
    }

    /// An open overlay takes every pointer event: a click on a row chooses
    /// it, a click outside closes it, the wheel moves through it. True when
    /// one was open.
    fn overlay_mouse(&mut self, event: MouseEvent) -> bool {
        let (col, row) = (event.column, event.row);
        let down = event.kind == MouseEventKind::Down(MouseButton::Left);
        let wheel = match event.kind {
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::ScrollDown => 1,
            _ => 0,
        };
        match self.mode {
            Mode::Normal => return false,
            Mode::CoverArtZoom => {
                if down {
                    self.pop_mode();
                }
            }
            Mode::HelpModal => {
                if down {
                    self.pop_mode();
                } else {
                    self.help_scroll = self.help_scroll.saturating_add_signed(3 * wheel);
                }
            }
            // Over the fullscreen visualiser, where nothing else is drawn.
            Mode::VizPicker => {}
            Mode::ActionsMenu => {
                let area = self.layout.modal_area;
                let rows = inside(area);
                if down && !hit(area, col, row) {
                    self.close_actions_menu();
                } else if down && hit(rows, col, row) {
                    let top = self
                        .actions_menu
                        .as_ref()
                        .map_or(0, |m| m.top(rows.height as usize));
                    self.run_menu_item(top + (row - rows.y) as usize);
                } else if wheel != 0
                    && let Some(m) = self.actions_menu.as_mut()
                {
                    m.move_by(wheel);
                }
            }
            Mode::FilterPanel => {
                let area = self.layout.modal_area;
                let rows = inside(area);
                if down && !hit(area, col, row) {
                    if !self.filter_panel.as_mut().is_some_and(|p| p.back()) {
                        self.close_filter_panel();
                    }
                } else if down && hit(rows, col, row) {
                    let at = (row - rows.y) as usize;
                    let changed = self.filter_panel.as_mut().is_some_and(|panel| {
                        if panel.picking() {
                            let at = panel.pick_top(rows.height as usize) + at;
                            panel.choose(at)
                        } else {
                            panel.activate(at, true)
                        }
                    });
                    if changed {
                        self.filters_changed();
                    }
                }
            }
            Mode::ReplaceQueue => {
                let area = self.layout.modal_area;
                if down && !hit(area, col, row) {
                    self.answer_replace(crate::replace_dialog::Choice::Cancel);
                } else if down
                    && let Some(choice) = self.replace_dialog.as_ref().and_then(|d| d.hit(col, row))
                {
                    self.answer_replace(choice);
                }
            }
            Mode::NamePrompt => {
                let area = self.layout.modal_area;
                if down && !hit(area, col, row) {
                    self.close_name_prompt();
                } else if down && hit(crate::playlist_modals::prompt_ok_rect(area), col, row) {
                    self.confirm_name_prompt();
                }
            }
            Mode::PlaylistChooser => {
                let area = self.layout.modal_area;
                let rows = inside(area);
                if down && !hit(area, col, row) {
                    self.close_chooser();
                } else if down && hit(rows, col, row) {
                    let top = self
                        .chooser
                        .as_ref()
                        .map_or(0, |c| c.top(rows.height as usize));
                    self.apply_chooser(top + (row - rows.y) as usize);
                } else if wheel != 0
                    && let Some(c) = self.chooser.as_mut()
                {
                    c.move_by(wheel);
                }
            }
            Mode::TrackInfo(_) | Mode::TrackDetails => {
                if down && !hit(self.layout.track_info_area, col, row) {
                    self.close_info();
                }
            }
            Mode::Organize => {
                if down && !hit(self.layout.organize_area, col, row) {
                    self.organize = None;
                    self.pop_mode();
                }
            }
            Mode::ContextMenu => {
                let rows = inside(self.layout.context_menu_area);
                if down && hit(rows, col, row) {
                    let at = (row - rows.y) as usize;
                    if let Some(action) = self
                        .context_menu
                        .as_ref()
                        .and_then(|m| m.actions.get(at))
                        .map(|a| a.0)
                        .filter(|a| *a != crate::app::ContextAction::Separator)
                    {
                        self.context_menu = None;
                        self.execute_context_action(action);
                    }
                } else if down && !hit(self.layout.context_menu_area, col, row) {
                    self.context_menu = None;
                    self.pop_mode();
                }
            }
            Mode::DeviceSelector | Mode::DspSelector | Mode::PlayOn => {
                let area = self.layout.selector_area;
                let rows = inside(area);
                if down && !hit(area, col, row) {
                    self.dsp_selector = None;
                    self.device_selector = None;
                    self.play_on = None;
                    self.pop_mode();
                } else if down && hit(rows, col, row) {
                    let at = (row - rows.y) as usize;
                    if self.mode == Mode::DspSelector {
                        let top = self
                            .dsp_selector
                            .as_ref()
                            .map_or(0, |s| s.top(rows.height as usize));
                        self.apply_dsp(top + at);
                    } else if let Some(p) = self.play_on.as_mut() {
                        if at < p.choices.len() {
                            p.cursor = at;
                            self.handle_play_on_key(KeyEvent::from(KeyCode::Enter));
                        }
                    } else if let Some(picker) = self.device_selector.as_mut()
                        && let Some(idx) = picker.row_at(col, row)
                    {
                        let picked = picker.click(idx);
                        self.apply_output(picked);
                    }
                } else if wheel != 0 {
                    if let Some(sel) = self.dsp_selector.as_mut() {
                        sel.move_by(wheel);
                    } else if let Some(p) = self.play_on.as_mut() {
                        p.move_by(wheel);
                    } else if let Some(picker) = self.device_selector.as_mut() {
                        picker.scroll(wheel);
                    }
                }
            }
            Mode::Picker(_) => self.picker_mouse(event),
        }
        true
    }

    /// A click on a tab shows it, a click on a result picks it and a second
    /// plays it; a click outside closes the picker; the wheel moves through
    /// the results.
    fn picker_mouse(&mut self, event: MouseEvent) {
        let (col, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.move_up();
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.move_down();
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let area = self.layout.picker_area;
                if !hit(area, col, row) {
                    self.close_picker();
                    return;
                }
                let Some(picker) = self.picker.as_mut() else {
                    return;
                };
                let labels = picker.tab_labels();
                if let Some(tab) = crate::picker::picker_tab_rects(area, &labels)
                    .iter()
                    .position(|r| hit(*r, col, row))
                {
                    picker.set_tab(tab);
                    return;
                }
                let results = picker_results_rect(area, !labels.is_empty());
                if !hit(results, col, row) {
                    return;
                }
                // The results scroll as the overlay draws them: the cursor
                // on the last row once it is past the first screen.
                let shown = results.height as usize;
                let start = (picker.cursor + 1).saturating_sub(shown);
                let at = start + (row - results.y) as usize;
                if at >= picker.matched_count() {
                    return;
                }
                // A click picks or lets go; a second plays that row alone.
                if self.double_click(Click::PickerRow(at)) {
                    if let Some(picker) = self.picker.as_mut() {
                        picker.pick_only(at);
                    }
                    self.confirm_picker(PickerAction::AppendAndPlay);
                } else if let Some(picker) = self.picker.as_mut() {
                    picker.toggle_at(at);
                }
            }
            _ => {}
        }
    }

    fn left_click(&mut self, event: MouseEvent) {
        let (col, row) = (event.column, event.row);

        // A key hint along the foot is that key.
        if let Some(key) = self
            .layout
            .hint_hits
            .iter()
            .find(|(r, _)| hit(*r, col, row))
            .map(|(_, k)| *k)
        {
            self.handle_key(key);
            return;
        }

        // Wherever the EQ is shown, a click opens the chooser.
        if self.layout.eq_hits.iter().any(|r| hit(*r, col, row)) {
            self.open_dsp_selector();
            return;
        }

        if let Some(target) = self.nav_target_at(col, row) {
            self.activate(target);
            return;
        }

        if self.shell_click(col, row) {
            return;
        }

        // The transport's controls and seek bar, and Now playing's.
        if let Some(c) = self
            .layout
            .transport
            .at(col, row)
            .or_else(|| self.layout.now_playing.at(col, row))
        {
            self.press(c);
            return;
        }
        if let Some(bar) = [self.layout.transport.seek, self.layout.now_playing.seek]
            .into_iter()
            .find(|r| hit(*r, col, row))
        {
            // The duration the bar was drawn against, not the probed one: a
            // partial file reports short, and a click would then land
            // somewhere other than where it was aimed.
            if let Some(pos) = crate::transport::seek_from_click(
                bar,
                col,
                self.state.duration_ms(),
                self.state.seek_ceiling_ms(),
            ) {
                self.seek_to(pos);
            }
            return;
        }

        if self.page() != Page::Queue && hit(self.layout.browse_area, col, row) {
            self.page_click(col, row, event.modifiers);
            return;
        }

        if self.art.now_playing_art.cached().is_some()
            && hit(self.layout.now_playing_art_area, col, row)
        {
            self.push_mode(Mode::CoverArtZoom);
            return;
        }

        if self.on_queue_scrollbar(col, row) {
            self.grab_queue_scrollbar(row);
            return;
        }

        if hit(self.layout.queue_area, col, row) {
            self.queue_click(event);
        }
    }

    /// A click on the page in front, which is not the queue.
    fn page_click(&mut self, col: u16, row: u16, modifiers: KeyModifiers) {
        match self.page() {
            Page::Playlists => {
                let p = &mut self.playlists_page;
                let mut action = PlAction::None;
                if p.on_scrollbar(col, row) {
                    self.browse_drag = true;
                    p.scroll_to_bar(row);
                } else if let Some(hit) = p.hit(col, row) {
                    action = p.click(hit);
                }
                self.apply_playlist(action);
            }
            Page::Playlist(_) => {
                let mut action = PlAction::None;
                if let Some(p) = self.playlist_page.as_mut() {
                    if p.on_scrollbar(col, row) {
                        self.browse_drag = true;
                        p.scroll_to_bar(row);
                    } else if let Some(hit) = p.hit(col, row) {
                        action = p.click(hit);
                    }
                }
                self.apply_playlist(action);
            }
            Page::Search | Page::Favourites | Page::Recent | Page::Downloaded => {
                let mut action = browse::Action::None;
                let mut dragging = false;
                if let Some(p) = self.shelf_mut() {
                    if p.on_scrollbar(col, row) {
                        dragging = true;
                        p.scroll_to_bar(row);
                    } else if let Some(hit) = p.hit(col, row) {
                        action = p.click(hit);
                    }
                }
                self.browse_drag |= dragging;
                self.apply_browse(action);
            }
            Page::History => {
                let mut action = browse::Action::None;
                let p = &mut self.history_page;
                if p.on_scrollbar(col, row) {
                    self.browse_drag = true;
                    p.scroll_to_bar(row);
                } else if let Some(hit) = p.hit(col, row) {
                    action = p.click(hit, modifiers);
                }
                self.apply_browse(action);
            }
            Page::Settings => {
                let p = &mut self.settings_page;
                let mut outcome = None;
                if p.on_scrollbar(col, row) {
                    self.browse_drag = true;
                    p.scroll_to_bar(row);
                } else if let Some(hit) = p.hit(col, row) {
                    outcome = p.click(hit);
                }
                if let Some(outcome) = outcome {
                    self.apply_outcome(outcome);
                }
            }
            Page::Album(_) => {
                let mut action = browse::Action::None;
                if let Some(p) = self.album_page.as_mut() {
                    if p.on_scrollbar(col, row) {
                        self.browse_drag = true;
                        p.scroll_to_bar(row);
                    } else if let Some(hit) = p.hit(col, row) {
                        action = p.click(hit);
                    }
                }
                self.apply_browse(action);
            }
            Page::Albums | Page::Artists | Page::Tracks | Page::Artist(_) => {
                let mut dragging = false;
                let mut action = browse::Action::None;
                if let Some(browse) = self.browse_mut() {
                    if browse.on_scrollbar(col, row) {
                        dragging = true;
                        browse.scroll_to_bar(row);
                    } else if let Some(hit) = browse.hit(col, row) {
                        action = browse.click(hit);
                    }
                }
                self.browse_drag |= dragging;
                self.apply_browse(action);
            }
            Page::Downloads => {
                if let Some(hit) = self.downloads_page.hit(col, row) {
                    let action = self.downloads_page.click(hit);
                    self.apply_browse(action);
                }
            }
            Page::Queue | Page::NowPlaying => {}
        }
    }

    /// The queue's scrollbar: the last column of the queue.
    fn on_queue_scrollbar(&self, col: u16, row: u16) -> bool {
        let q = self.layout.queue_area;
        q.width > 0 && col == q.x + q.width - 1 && row >= q.y && row < q.y + q.height
    }

    /// A click on the queue's scrollbar: on the thumb it takes hold of it
    /// where it was clicked, elsewhere the thumb jumps there, held by its
    /// middle. In eighths of a cell, as the bar is drawn.
    fn grab_queue_scrollbar(&mut self, row: u16) {
        let visible_height = self.queue_visible_height();
        let total_lines = queue::display_line_count(&self.queue.layout);
        if total_lines <= visible_height || visible_height == 0 {
            return;
        }
        let vis8 = visible_height * 8;
        let thumb_8 = (vis8 * visible_height / total_lines).max(8);
        let max_scroll = total_lines.saturating_sub(visible_height);
        let track_8 = vis8.saturating_sub(thumb_8);
        let thumb_top_8 = if max_scroll > 0 && track_8 > 0 {
            self.queue.scroll_offset * track_8 / max_scroll
        } else {
            0
        };
        let click_8 = (row - self.layout.queue_area.y) as usize * 8 + 4;
        if click_8 >= thumb_top_8 && click_8 < thumb_top_8 + thumb_8 {
            self.scrollbar_grab_offset = Some((click_8 - thumb_top_8) as u16);
        } else {
            self.scrollbar_grab_offset = Some((thumb_8 / 2) as u16);
            self.scroll_to_scrollbar_y(row);
        }
    }

    /// A click in the queue: a row's heart or menu button, a heading's
    /// group, or a row. A second click plays; a press starts a drag. The
    /// names are the shell's links.
    fn queue_click(&mut self, event: MouseEvent) {
        let (col, row) = (event.column, event.row);
        let area = self.layout.queue_area;
        let scroll = self.queue.scroll_offset;
        let layout = &self.queue.layout;
        let visible = &self.queue.vq_cache.entries;
        if let Some(id) = queue::QueueView::heart_at(visible, layout, area, scroll, col, row)
            .and_then(|i| visible[i].db_id)
        {
            self.toggle_favourite(id);
            return;
        }
        let at = queue::QueueView::queue_index_at_y(layout, area, scroll, row);
        if let Some(idx) = at
            && col == queue::menu_button_x(area)
        {
            self.open_queue_menu(idx, col, row);
            return;
        }
        let Some(idx) = at else {
            let Some((first, last)) = queue::QueueView::album_group_at_y(layout, area, scroll, row)
            else {
                return;
            };
            if self.double_click(Click::QueueHeading(first)) {
                // A second click on a heading plays the album from its top.
                self.queue.cursor = first;
                self.play_at_cursor();
            } else {
                self.select_group(first, last);
                // A drag from here moves the album.
                self.queue.drag = Some(DragState {
                    anchor_offset: 0,
                    group_start: first,
                });
            }
            return;
        };

        if self.double_click(Click::QueueRow(idx)) {
            self.queue.cursor = idx;
            self.play_at_cursor();
            return;
        }
        // Shift or Ctrl selects a range (most terminals report no Shift with
        // a click), Alt adds or takes one away (Cmd never reaches a terminal).
        if event
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL)
        {
            self.extend_selection_to(idx);
        } else if event.modifiers.contains(KeyModifiers::ALT) {
            self.toggle_selection(idx);
        } else if !self.is_selected(idx) {
            // A click on a row already selected keeps the selection, so the
            // whole of it can be dragged.
            self.select_single(idx);
        }
        self.queue.cursor = idx;

        // A drag moves the selection, with the track grabbed in it.
        let entries = &self.queue.vq_cache.entries;
        let selected = &self.queue.selected_ids;
        self.queue.drag = entries
            .get(idx)
            .filter(|e| selected.contains(&e.id))
            .and_then(|_| entries.iter().position(|e| selected.contains(&e.id)))
            .map(|first| DragState {
                anchor_offset: entries[first..idx]
                    .iter()
                    .filter(|e| selected.contains(&e.id))
                    .count(),
                group_start: first,
            });
    }

    fn is_selected(&self, idx: usize) -> bool {
        self.queue
            .vq_cache
            .entries
            .get(idx)
            .is_some_and(|e| self.queue.selected_ids.contains(&e.id))
    }

    /// Selects the record whose rows are `first..=last`, anchored at its top.
    fn select_group(&mut self, first: usize, last: usize) {
        let entries = &self.queue.vq_cache.entries;
        self.queue.selected_ids = entries
            .iter()
            .take(last + 1)
            .skip(first)
            .map(|e| e.id)
            .collect();
        self.queue.anchor_id = entries.get(first).map(|e| e.id);
        self.queue.cursor = first;
    }

    fn left_drag(&mut self, event: MouseEvent) {
        let row = event.row;
        if self.browse_drag {
            match self.page() {
                Page::Playlists => self.playlists_page.scroll_to_bar(row),
                Page::Playlist(_) => {
                    if let Some(p) = self.playlist_page.as_mut() {
                        p.scroll_to_bar(row);
                    }
                }
                Page::Search | Page::Favourites | Page::Recent | Page::Downloaded => {
                    if let Some(p) = self.shelf_mut() {
                        p.scroll_to_bar(row);
                    }
                }
                Page::History => self.history_page.scroll_to_bar(row),
                Page::Settings => self.settings_page.scroll_to_bar(row),
                Page::Album(_) => {
                    if let Some(p) = self.album_page.as_mut() {
                        p.scroll_to_bar(row);
                    }
                }
                _ => {
                    if let Some(browse) = self.browse_mut() {
                        browse.scroll_to_bar(row);
                    }
                }
            }
            return;
        }
        if self.scrollbar_grab_offset.is_some() {
            self.scroll_to_scrollbar_y(row);
            return;
        }
        if self.queue.drag.is_none() || !hit(self.layout.queue_area, event.column, row) {
            return;
        }
        let Some(idx) = queue::QueueView::queue_index_at_y(
            &self.queue.layout,
            self.layout.queue_area,
            self.queue.scroll_offset,
            row,
        ) else {
            return;
        };
        // It reorders live: the selection follows the pointer, the
        // grabbed track under it.
        let Some(drag) = self.queue.drag.as_mut() else {
            return;
        };
        let start = idx.saturating_sub(drag.anchor_offset);
        if start == drag.group_start {
            return;
        }
        drag.group_start = start;
        let grabbed = drag.anchor_offset;
        self.begin_drag_undo();
        if let Some(start) = self.place_selected(start) {
            self.queue.cursor = start + grabbed;
        }
    }

    /// The moves of one drag undo together.
    fn begin_drag_undo(&mut self) {
        if !self.drag_undo_active {
            self.tx.send(PlayerCommand::BeginUndoBatch).ok();
            self.drag_undo_active = true;
        }
    }

    fn right_click(&mut self, event: MouseEvent) {
        let (col, row) = (event.column, event.row);
        if self.page() != Page::Queue && hit(self.layout.browse_area, col, row) {
            // On a record or a track, a shortcut for adding it to the queue.
            if let Some(browse) = self.browse_mut()
                && let Some(browse::Hit::Item(idx)) = browse.hit(col, row)
            {
                let action = browse.append_at(idx);
                self.apply_browse(action);
                self.say("added to the queue");
            }
            return;
        }
        if !hit(self.layout.queue_area, col, row) {
            return;
        }
        let area = self.layout.queue_area;
        let scroll = self.queue.scroll_offset;
        let layout = &self.queue.layout;
        if let Some(idx) = queue::QueueView::queue_index_at_y(layout, area, scroll, row) {
            self.open_queue_menu(idx, col, row);
        } else if let Some((first, last)) =
            queue::QueueView::album_group_at_y(layout, area, scroll, row)
        {
            // On a heading: the menu is for the whole album.
            self.select_group(first, last);
            self.open_context_menu(Some((col, row)), true);
        }
    }

    /// The menu of actions for the queue row `idx`, opened at `col`, `row`.
    /// A right click opens it, and so does the row's own button.
    fn open_queue_menu(&mut self, idx: usize, col: u16, row: u16) {
        if !self.is_selected(idx) {
            self.select_single(idx);
        }
        self.queue.cursor = idx;
        self.open_context_menu(Some((col, row)), false);
    }

    fn scroll(&mut self, event: MouseEvent, delta: isize) {
        let (col, row) = (event.column, event.row);
        if self.page() != Page::Queue && hit(self.layout.browse_area, col, row) {
            match self.page() {
                Page::Playlists => self.playlists_page.scroll_by(3 * delta),
                Page::Downloads => self.downloads_page.scroll_by(delta),
                Page::Playlist(_) => {
                    if let Some(p) = self.playlist_page.as_mut() {
                        p.scroll_by(3 * delta);
                    }
                }
                Page::Search | Page::Favourites | Page::Recent | Page::Downloaded => {
                    if let Some(p) = self.shelf_mut() {
                        p.scroll_by(3 * delta);
                    }
                }
                Page::History => self.history_page.scroll_by(3 * delta),
                Page::Settings => self.settings_page.scroll_by(3 * delta),
                Page::Album(_) => {
                    if let Some(p) = self.album_page.as_mut() {
                        p.scroll_by(3 * delta);
                    }
                }
                _ => {
                    if let Some(browse) = self.browse_mut() {
                        browse.wheel(delta > 0);
                    }
                }
            }
        } else if self.page() == Page::Queue {
            let total = queue::display_line_count(&self.queue.layout);
            let max = total.saturating_sub(self.queue_visible_height());
            self.queue.scroll_offset = self
                .queue
                .scroll_offset
                .saturating_add_signed(delta)
                .min(max);
        }
    }

    fn nav_target_at(&self, col: u16, row: u16) -> Option<NavTarget> {
        self.layout
            .nav_hits
            .iter()
            .find(|(r, _)| hit(*r, col, row))
            .map(|(_, t)| *t)
    }

    /// What the pointer is over, against the layout last drawn.
    pub(super) fn update_hover_zone(&mut self) {
        self.hover.zone = self.zone_at(self.hover.column, self.hover.row);
    }

    fn zone_at(&self, col: u16, row: u16) -> HoverZone {
        let layout = &self.layout;
        if hit(layout.queue_area, col, row)
            && !self.on_queue_scrollbar(col, row)
            && let Some(idx) = queue::QueueView::queue_index_at_y(
                &self.queue.layout,
                layout.queue_area,
                self.queue.scroll_offset,
                row,
            )
        {
            return HoverZone::QueueItem(idx);
        }
        if let Some(target) = self.nav_target_at(col, row) {
            return HoverZone::Nav(target);
        }
        if self.page() != Page::Queue && hit(layout.browse_area, col, row) {
            let zone = match self.page() {
                Page::Playlists => self.playlists_page.hit(col, row).map(HoverZone::Playlists),
                Page::Playlist(_) => self
                    .playlist_page
                    .as_ref()
                    .and_then(|p| p.hit(col, row))
                    .map(HoverZone::Playlist),
                Page::Search | Page::Favourites | Page::Recent | Page::Downloaded => self
                    .shelf()
                    .and_then(|p| p.hit(col, row))
                    .map(HoverZone::Shelf),
                Page::History => self.history_page.hit(col, row).map(HoverZone::History),
                Page::Downloads => self.downloads_page.hit(col, row).map(HoverZone::Downloads),
                Page::Settings => self.settings_page.hit(col, row).map(HoverZone::Settings),
                Page::Album(_) => self
                    .album_page
                    .as_ref()
                    .and_then(|p| p.hit(col, row))
                    .map(HoverZone::Album),
                _ => self
                    .browse()
                    .and_then(|b| b.hit(col, row))
                    .map(HoverZone::Browse),
            };
            if let Some(zone) = zone {
                return zone;
            }
        }
        HoverZone::None
    }

    /// Scroll the queue based on a click/drag position on the scrollbar.
    /// Uses the same sub-pixel (1/8th-cell) thumb math as queue.rs rendering.
    fn scroll_to_scrollbar_y(&mut self, y: u16) {
        let inner_y = self.layout.queue_area.y;
        let visible_height = self.queue_visible_height();
        if visible_height == 0 {
            return;
        }
        let total_lines = queue::display_line_count(&self.queue.layout);
        if total_lines <= visible_height {
            return;
        }

        let vis8 = visible_height * 8;
        let thumb_8 = (vis8 * visible_height / total_lines).max(8);
        let max_scroll = total_lines.saturating_sub(visible_height);
        let track_8 = vis8.saturating_sub(thumb_8);
        if track_8 == 0 {
            return;
        }

        let grab_offset_8 = self.scrollbar_grab_offset.unwrap_or(0) as usize;
        // Mouse Y is in whole cells — convert to eighths (center of cell).
        let mouse_8 = (y.saturating_sub(inner_y) as usize) * 8 + 4;
        let desired_thumb_top_8 = mouse_8.saturating_sub(grab_offset_8);
        let clamped = desired_thumb_top_8.min(track_8);
        self.queue.scroll_offset = (clamped * max_scroll / track_8).min(max_scroll);
    }

    /// Get the drop indicator index — where external drops would insert.
    /// Only active when mouse is over queue and no internal drag is happening.
    pub fn drop_indicator_index(&self) -> Option<usize> {
        if self.queue.drag.is_some() {
            return None; // internal drag takes precedence
        }
        queue::QueueView::queue_index_at_y(
            &self.queue.layout,
            self.layout.queue_area,
            self.queue.scroll_offset,
            self.last_mouse_row?,
        )
    }

    /// Get the QueueItemId at the drop indicator position (for InsertInPlaylist).
    pub fn drop_target_queue_id(&self) -> Option<koan_core::player::state::QueueItemId> {
        let idx = self.drop_indicator_index()?;
        self.queue.vq_cache.entries.get(idx).map(|e| e.id)
    }
}

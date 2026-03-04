use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};

use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Track,
    Album,
    Artist,
    QueueJump,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerPartKind {
    Artist,
    Album,
    Title,
    Date,
    TrackNum,
    Duration,
    Separator,
    Codec,
    Plain,
}

pub struct PickerItem {
    pub id: i64,
    pub display: String,
    pub match_text: String,
    pub parts: Vec<(String, PickerPartKind)>,
}

// --- "All tracks for artist" sentinel encoding ---
// When an artist drill-down shows an album picker, the "all tracks" entry
// encodes the artist_id as a negative value in the PickerItem.id field.
// These helpers make the encoding/decoding explicit.

/// Encode an artist_id into the sentinel value used for "all tracks" picker items.
pub fn all_tracks_sentinel(artist_id: i64) -> i64 {
    -artist_id
}

/// Check whether a picker result ID is the "all tracks for artist" sentinel.
pub fn is_all_tracks_sentinel(id: i64) -> bool {
    id < 0
}

/// Extract the original artist_id from an "all tracks" sentinel value.
pub fn artist_id_from_sentinel(id: i64) -> i64 {
    id.unsigned_abs() as i64
}

pub struct PickerState {
    pub kind: PickerKind,
    pub query: String,
    pub cursor: usize,
    pub selected: Vec<usize>,
    pub items: Vec<PickerItem>,
    nucleo: Nucleo<u32>,
    pub multi: bool,
}

impl PickerState {
    pub fn new(kind: PickerKind, items: Vec<PickerItem>, multi: bool) -> Self {
        let nucleo: Nucleo<u32> = Nucleo::new(Config::DEFAULT, std::sync::Arc::new(|| {}), None, 1);

        let injector = nucleo.injector();
        for (i, item) in items.iter().enumerate() {
            let text = item.match_text.clone();
            injector.push(i as u32, |_val, cols| {
                cols[0] = text.into();
            });
        }

        let mut state = Self {
            kind,
            query: String::new(),
            cursor: 0,
            selected: Vec::new(),
            items,
            nucleo,
            multi,
        };
        state.nucleo.tick(10);
        state
    }

    pub fn prompt(&self) -> &str {
        match self.kind {
            PickerKind::Track => "enqueue>",
            PickerKind::Album => "album>",
            PickerKind::Artist => "artist>",
            PickerKind::QueueJump => "jump>",
        }
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.reparse(true);
        self.cursor = 0;
    }

    pub fn backspace(&mut self) {
        if self.query.pop().is_some() {
            self.reparse(false);
            self.cursor = 0;
        }
    }

    pub fn move_up(&mut self) {
        let count = self.matched_count();
        if count == 0 {
            return;
        }
        if self.cursor == 0 {
            self.cursor = count - 1; // wrap to bottom
        } else {
            self.cursor -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let count = self.matched_count();
        if count == 0 {
            return;
        }
        if self.cursor + 1 >= count {
            self.cursor = 0; // wrap to top
        } else {
            self.cursor += 1;
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.cursor = self.cursor.saturating_sub(page_size);
    }

    pub fn page_down(&mut self, page_size: usize) {
        let count = self.matched_count();
        if count > 0 {
            self.cursor = (self.cursor + page_size).min(count - 1);
        }
    }

    pub fn jump_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn jump_to_end(&mut self) {
        let count = self.matched_count();
        if count > 0 {
            self.cursor = count - 1;
        }
    }

    pub fn toggle_select(&mut self) {
        if !self.multi {
            return;
        }
        if let Some(idx) = self.data_index_at_cursor() {
            if let Some(pos) = self.selected.iter().position(|&s| s == idx) {
                self.selected.remove(pos);
            } else {
                self.selected.push(idx);
            }
            self.move_down();
        }
    }

    /// Get the selected item IDs on Enter.
    pub fn confirm(&self) -> Vec<i64> {
        if self.multi && !self.selected.is_empty() {
            return self.selected.iter().map(|&i| self.items[i].id).collect();
        }
        if let Some(idx) = self.data_index_at_cursor() {
            vec![self.items[idx].id]
        } else {
            vec![]
        }
    }

    pub fn matched_count(&self) -> usize {
        self.nucleo.snapshot().matched_item_count() as usize
    }

    fn data_index_at_cursor(&self) -> Option<usize> {
        let snap = self.nucleo.snapshot();
        snap.get_matched_item(self.cursor as u32)
            .map(|item| *item.data as usize)
    }

    fn reparse(&mut self, append: bool) {
        self.nucleo.pattern.reparse(
            0,
            &self.query,
            CaseMatching::Smart,
            Normalization::Smart,
            append,
        );
        self.nucleo.tick(10);
    }

    pub fn tick(&mut self) {
        self.nucleo.tick(10);
    }
}

/// Calculate the popup rect centered in the given area.
pub fn picker_popup_rect(area: Rect) -> Rect {
    let popup_width = (area.width as f32 * 0.6).max(40.0).min(area.width as f32) as u16;
    let popup_height = (area.height as f32 * 0.7).max(10.0).min(area.height as f32) as u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(x, y, popup_width, popup_height)
}

/// Given the popup rect, return the results list area (inner minus query and hint rows).
pub fn picker_results_rect(popup: Rect) -> Rect {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(popup);
    if inner.height < 3 {
        return Rect::default();
    }
    // query (1) + results (inner.height - 2) + hints (1)
    Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(2),
    )
}

pub struct PickerOverlay<'a> {
    state: &'a PickerState,
    theme: &'a Theme,
}

impl<'a> PickerOverlay<'a> {
    pub fn new(state: &'a PickerState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for PickerOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Center overlay at ~60% width, ~70% height.
        let popup_width = (area.width as f32 * 0.6).max(40.0).min(area.width as f32) as u16;
        let popup_height = (area.height as f32 * 0.7).max(10.0).min(area.height as f32) as u16;
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Clear the area behind the popup.
        Clear.render(popup_area, buf);

        let prompt = self.state.prompt();
        let matched = self.state.matched_count();
        let total = self.state.items.len();

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(vec![Span::styled(
                format!(" {} ", prompt),
                self.theme.album_header_artist.add_modifier(Modifier::BOLD),
            )]));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        if inner.height < 3 {
            return;
        }

        // Layout: query line (1) + results (remaining - 1 for hint)
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

        // Query line.
        let query_line = Line::from(vec![
            Span::styled(" > ", self.theme.track_playing),
            Span::raw(&self.state.query),
            Span::styled(format!("  {}/{}", matched, total), self.theme.hint_desc),
        ]);
        Paragraph::new(query_line).render(chunks[0], buf);

        // Results.
        let snap = self.state.nucleo.snapshot();
        let visible_height = chunks[1].height as usize;
        let start = if self.state.cursor >= visible_height {
            self.state.cursor - visible_height + 1
        } else {
            0
        };
        let end = (start + visible_height).min(matched);

        for (row, i) in (start..end).enumerate() {
            if let Some(item) = snap.get_matched_item(i as u32) {
                let idx = *item.data as usize;
                let is_cursor = i == self.state.cursor;
                let is_selected = self.state.selected.contains(&idx);

                let row_style = if is_cursor {
                    self.theme.picker_cursor
                } else if is_selected {
                    self.theme.album_header_album
                } else {
                    self.theme.track_normal
                };

                let marker = if is_selected && !is_cursor {
                    "\u{25C6} "
                } else if is_cursor {
                    "\u{25B8} "
                } else {
                    "  "
                };

                let item = &self.state.items[idx];

                // Fill entire row with the style for a visible highlight bar.
                let row_y = chunks[1].y + row as u16;
                for col in chunks[1].x..chunks[1].x + chunks[1].width {
                    buf[(col, row_y)].set_style(row_style);
                }

                let mut spans = vec![
                    Span::styled(" ", row_style),
                    Span::styled(marker, row_style),
                ];

                if item.parts.is_empty() {
                    spans.push(Span::styled(item.display.clone(), row_style));
                } else {
                    for (text, kind) in &item.parts {
                        let part_style = self.theme.picker_part_style(*kind).patch(row_style);
                        spans.push(Span::styled(text.clone(), part_style));
                    }
                }

                let line = Line::from(spans);
                buf.set_line(chunks[1].x, row_y, &line, chunks[1].width);
            }
        }

        // Hint bar.
        let hints = if self.state.multi {
            vec![
                Span::styled(" \u{2191}\u{2193}", self.theme.hint_key),
                Span::styled(" navigate  ", self.theme.hint_desc),
                Span::styled("tab", self.theme.hint_key),
                Span::styled(" select  ", self.theme.hint_desc),
                Span::styled("enter", self.theme.hint_key),
                Span::styled(" confirm  ", self.theme.hint_desc),
                Span::styled("esc", self.theme.hint_key),
                Span::styled(" cancel", self.theme.hint_desc),
            ]
        } else {
            vec![
                Span::styled(" \u{2191}\u{2193}", self.theme.hint_key),
                Span::styled(" navigate  ", self.theme.hint_desc),
                Span::styled("enter", self.theme.hint_key),
                Span::styled(" select  ", self.theme.hint_desc),
                Span::styled("esc", self.theme.hint_key),
                Span::styled(" cancel", self.theme.hint_desc),
            ]
        };
        let hint_line = Line::from(hints);
        buf.set_line(chunks[2].x, chunks[2].y, &hint_line, chunks[2].width);
    }
}

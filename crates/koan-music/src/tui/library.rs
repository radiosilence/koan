use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use koan_core::db::connection::Database;
use koan_core::db::queries;

use super::theme::Theme;
use super::transport::format_time;

#[derive(Debug, Clone)]
pub enum LibraryNode {
    Artist {
        id: i64,
        name: String,
        expanded: bool,
    },
    Album {
        id: i64,
        title: String,
        year: Option<String>,
        expanded: bool,
    },
    Track {
        id: i64,
        title: String,
        number: Option<i32>,
        duration_ms: Option<i64>,
    },
}

pub struct LibraryState {
    pub nodes: Vec<LibraryNode>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub db_path: PathBuf,
    /// Active filter text. When non-empty, only matching nodes are shown.
    pub filter: String,
    /// Whether the filter input box is focused (typing mode).
    pub filter_active: bool,
    /// Cached unfiltered artist list — avoids re-querying DB on every filter keystroke.
    all_artists: Vec<LibraryNode>,
}

impl LibraryState {
    pub fn new(db_path: &Path) -> Self {
        let mut state = Self {
            nodes: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            db_path: db_path.to_path_buf(),
            filter: String::new(),
            filter_active: false,
            all_artists: Vec::new(),
        };
        state.load_artists();
        state
    }

    fn open_db(&self) -> Option<Database> {
        Database::open(&self.db_path).ok()
    }

    fn load_artists(&mut self) {
        let Some(db) = self.open_db() else { return };
        let artists = queries::all_artists(&db.conn).unwrap_or_default();
        self.all_artists = artists
            .into_iter()
            .map(|a| LibraryNode::Artist {
                id: a.id,
                name: a.name,
                expanded: false,
            })
            .collect();
        self.nodes = self.all_artists.clone();
    }

    /// Filter the cached artist list (case-insensitive substring). No DB query.
    pub fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.nodes = self.all_artists.clone();
        } else {
            let query = self.filter.to_lowercase();
            self.nodes = self
                .all_artists
                .iter()
                .filter(|node| match node {
                    LibraryNode::Artist { name, .. } => name.to_lowercase().contains(&query),
                    _ => false,
                })
                .cloned()
                .collect();
        }

        self.cursor = 0;
        self.scroll_offset = 0;
    }

    pub fn start_filter(&mut self) {
        self.filter_active = true;
    }

    pub fn stop_filter(&mut self) {
        self.filter_active = false;
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_active = false;
        self.apply_filter();
    }

    pub fn filter_type_char(&mut self, c: char) {
        self.filter.push(c);
        self.apply_filter();
    }

    pub fn filter_backspace(&mut self) {
        self.filter.pop();
        self.apply_filter();
    }

    pub fn expand_or_enter(&mut self) -> Option<Vec<i64>> {
        if self.cursor >= self.nodes.len() {
            return None;
        }
        match &self.nodes[self.cursor] {
            LibraryNode::Artist {
                expanded: false, ..
            } => {
                self.expand_artist();
                None
            }
            LibraryNode::Album {
                expanded: false, ..
            } => {
                self.expand_album();
                None
            }
            LibraryNode::Track { id, .. } => Some(vec![*id]),
            _ => None,
        }
    }

    pub fn collapse_or_parent(&mut self) {
        if self.cursor >= self.nodes.len() {
            return;
        }
        match &self.nodes[self.cursor] {
            LibraryNode::Artist { expanded: true, .. }
            | LibraryNode::Album { expanded: true, .. } => {
                self.collapse_at_cursor();
            }
            LibraryNode::Album { .. } | LibraryNode::Track { .. } => {
                self.jump_to_parent();
            }
            LibraryNode::Artist {
                expanded: false, ..
            } => {}
        }
    }

    fn expand_artist(&mut self) {
        let LibraryNode::Artist { id, .. } = &self.nodes[self.cursor] else {
            return;
        };
        let artist_id = *id;

        let Some(db) = self.open_db() else { return };
        let albums = queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();

        if let LibraryNode::Artist { expanded, .. } = &mut self.nodes[self.cursor] {
            *expanded = true;
        }

        let insert_pos = self.cursor + 1;
        let new_nodes: Vec<LibraryNode> = albums
            .into_iter()
            .map(|a| {
                let year = a.date.as_deref().and_then(|d| {
                    if d.len() >= 4 {
                        Some(d[..4].to_string())
                    } else {
                        None
                    }
                });
                LibraryNode::Album {
                    id: a.id,
                    title: a.title,
                    year,
                    expanded: false,
                }
            })
            .collect();

        for (i, node) in new_nodes.into_iter().enumerate() {
            self.nodes.insert(insert_pos + i, node);
        }
    }

    fn expand_album(&mut self) {
        let LibraryNode::Album { id, .. } = &self.nodes[self.cursor] else {
            return;
        };
        let album_id = *id;

        let Some(db) = self.open_db() else { return };
        let tracks = queries::tracks_for_album(&db.conn, album_id).unwrap_or_default();

        if let LibraryNode::Album { expanded, .. } = &mut self.nodes[self.cursor] {
            *expanded = true;
        }

        let insert_pos = self.cursor + 1;
        let new_nodes: Vec<LibraryNode> = tracks
            .into_iter()
            .map(|t| LibraryNode::Track {
                id: t.id,
                title: t.title,
                number: t.track_number,
                duration_ms: t.duration_ms,
            })
            .collect();

        for (i, node) in new_nodes.into_iter().enumerate() {
            self.nodes.insert(insert_pos + i, node);
        }
    }

    fn collapse_at_cursor(&mut self) {
        let depth = node_depth(&self.nodes[self.cursor]);

        match &mut self.nodes[self.cursor] {
            LibraryNode::Artist { expanded, .. } | LibraryNode::Album { expanded, .. } => {
                *expanded = false;
            }
            LibraryNode::Track { .. } => return,
        }

        let mut remove_count = 0;
        let start = self.cursor + 1;
        while start + remove_count < self.nodes.len()
            && node_depth(&self.nodes[start + remove_count]) > depth
        {
            remove_count += 1;
        }
        if remove_count > 0 {
            self.nodes.drain(start..start + remove_count);
        }
    }

    fn jump_to_parent(&mut self) {
        let depth = node_depth(&self.nodes[self.cursor]);
        for i in (0..self.cursor).rev() {
            if node_depth(&self.nodes[i]) < depth {
                self.cursor = i;
                return;
            }
        }
    }

    /// Toggle expand/collapse at cursor. Returns true if toggled.
    pub fn toggle_expand(&mut self) -> bool {
        if self.cursor >= self.nodes.len() {
            return false;
        }
        match &self.nodes[self.cursor] {
            LibraryNode::Artist { expanded, .. } | LibraryNode::Album { expanded, .. } => {
                if *expanded {
                    self.collapse_at_cursor();
                } else {
                    match &self.nodes[self.cursor] {
                        LibraryNode::Artist { .. } => self.expand_artist(),
                        LibraryNode::Album { .. } => self.expand_album(),
                        _ => {}
                    }
                }
                true
            }
            LibraryNode::Track { .. } => false,
        }
    }

    pub fn move_up(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        if self.cursor == 0 {
            self.cursor = self.nodes.len() - 1; // wrap to bottom
        } else {
            self.cursor -= 1;
        }
        self.ensure_visible();
    }

    pub fn move_down(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        if self.cursor + 1 >= self.nodes.len() {
            self.cursor = 0; // wrap to top
        } else {
            self.cursor += 1;
        }
        self.ensure_visible();
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.cursor = self.cursor.saturating_sub(page_size);
        self.ensure_visible();
    }

    pub fn page_down(&mut self, page_size: usize) {
        if !self.nodes.is_empty() {
            self.cursor = (self.cursor + page_size).min(self.nodes.len() - 1);
        }
        self.ensure_visible();
    }

    pub fn jump_to_start(&mut self) {
        self.cursor = 0;
        self.ensure_visible();
    }

    pub fn jump_to_end(&mut self) {
        if !self.nodes.is_empty() {
            self.cursor = self.nodes.len() - 1;
        }
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
    }

    pub fn update_scroll(&mut self, visible_height: usize) {
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor.saturating_sub(visible_height) + 1;
        }
    }

    pub fn enqueue_all_under_cursor(&self) -> Vec<i64> {
        if self.cursor >= self.nodes.len() {
            return vec![];
        }
        let Some(db) = self.open_db() else {
            return vec![];
        };
        match &self.nodes[self.cursor] {
            LibraryNode::Artist { id, .. } => queries::tracks_for_artist(&db.conn, *id)
                .unwrap_or_default()
                .iter()
                .map(|t| t.id)
                .collect(),
            LibraryNode::Album { id, .. } => queries::tracks_for_album(&db.conn, *id)
                .unwrap_or_default()
                .iter()
                .map(|t| t.id)
                .collect(),
            LibraryNode::Track { id, .. } => vec![*id],
        }
    }
}

fn node_depth(node: &LibraryNode) -> usize {
    match node {
        LibraryNode::Artist { .. } => 0,
        LibraryNode::Album { .. } => 1,
        LibraryNode::Track { .. } => 2,
    }
}

// --- Widget ---

pub struct LibraryView<'a> {
    state: &'a LibraryState,
    theme: &'a Theme,
    focused: bool,
    filter_focused: bool,
    hover_index: Option<usize>,
}

impl<'a> LibraryView<'a> {
    pub fn new(state: &'a LibraryState, theme: &'a Theme, focused: bool) -> Self {
        Self {
            state,
            theme,
            focused,
            filter_focused: state.filter_active,
            hover_index: None,
        }
    }

    pub fn with_hover(mut self, hover_index: Option<usize>) -> Self {
        self.hover_index = hover_index;
        self
    }
}

impl Widget for LibraryView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_style = if self.focused {
            self.theme.library_artist
        } else {
            self.theme.hint_desc
        };

        let title_text = if self.state.filter.is_empty() {
            " library ".to_string()
        } else {
            format!(" library [{}] ", self.state.filter)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(vec![Span::styled(title_text, title_style)]));

        let inner = block.inner(area);
        block.render(area, buf);

        // Reserve bottom row for filter input when filter is focused.
        let show_filter_bar = self.filter_focused;
        let content_height = if show_filter_bar {
            inner.height.saturating_sub(1) as usize
        } else {
            inner.height as usize
        };

        if self.state.nodes.is_empty() {
            let msg = if self.state.filter.is_empty() {
                " empty — run koan scan"
            } else {
                " no matches"
            };
            let line = Line::from(Span::styled(msg, self.theme.hint_desc));
            buf.set_line(inner.x, inner.y, &line, inner.width);
        } else {
            let scroll = self.state.scroll_offset;
            let end = (scroll + content_height).min(self.state.nodes.len());

            for (row, i) in (scroll..end).enumerate() {
                let node = &self.state.nodes[i];
                let is_cursor = self.focused && !self.filter_focused && i == self.state.cursor;
                let is_hovered = self.hover_index == Some(i) && !is_cursor;

                // Fill entire row with cursor style first for full-width highlight bar.
                if is_cursor {
                    let row_y = inner.y + row as u16;
                    for col in inner.x..inner.x + inner.width {
                        buf[(col, row_y)].set_style(self.theme.library_cursor);
                    }
                } else if is_hovered {
                    let row_y = inner.y + row as u16;
                    for col in inner.x..inner.x + inner.width {
                        buf[(col, row_y)].set_style(self.theme.library_hover);
                    }
                }

                let line = render_node(node, is_cursor, self.theme);
                buf.set_line(inner.x, inner.y + row as u16, &line, inner.width);
            }
        }

        // Render filter input bar at bottom of inner area.
        if show_filter_bar {
            let filter_y = inner.y + inner.height.saturating_sub(1);
            let filter_line = Line::from(vec![
                Span::styled("\u{F002} ", self.theme.hint_key), // nerd font search icon
                Span::styled(self.state.filter.clone(), self.theme.library_artist),
                Span::styled("\u{2588}", self.theme.library_cursor), // block cursor
            ]);
            buf.set_line(inner.x, filter_y, &filter_line, inner.width);
        }
    }
}

fn render_node<'a>(node: &LibraryNode, is_cursor: bool, theme: &Theme) -> Line<'a> {
    match node {
        LibraryNode::Artist { name, expanded, .. } => {
            let arrow = if *expanded { "\u{F0D7} " } else { "\u{F0DA} " }; //
            let style = if is_cursor {
                theme.library_cursor
            } else {
                theme.library_artist
            };
            Line::from(vec![
                Span::styled(if is_cursor { ">" } else { " " }, theme.library_cursor),
                Span::styled(arrow, theme.hint_desc),
                Span::styled(name.clone(), style),
            ])
        }
        LibraryNode::Album {
            title,
            year,
            expanded,
            ..
        } => {
            let arrow = if *expanded { "\u{F0D7} " } else { "\u{F0DA} " }; //
            let year_str = year
                .as_deref()
                .map(|y| format!("({}) ", y))
                .unwrap_or_default();
            let style = if is_cursor {
                theme.library_cursor
            } else {
                theme.library_album
            };
            Line::from(vec![
                Span::styled(if is_cursor { ">" } else { " " }, theme.library_cursor),
                Span::raw("  "),
                Span::styled(arrow, theme.hint_desc),
                Span::styled(year_str, theme.hint_desc),
                Span::styled(title.clone(), style),
            ])
        }
        LibraryNode::Track {
            title,
            number,
            duration_ms,
            ..
        } => {
            let num = number
                .map(|n| format!("{:02} ", n))
                .unwrap_or_else(|| "   ".to_string());
            let dur = duration_ms
                .map(|d| format_time(d as u64))
                .unwrap_or_default();
            let style = if is_cursor {
                theme.library_cursor
            } else {
                theme.library_track
            };
            let num_style = if is_cursor {
                theme.library_cursor
            } else {
                theme.track_number
            };
            let dur_style = if is_cursor {
                theme.library_cursor
            } else {
                theme.hint_desc
            };
            Line::from(vec![
                Span::styled(if is_cursor { ">" } else { " " }, theme.library_cursor),
                Span::raw("    "),
                Span::styled(num, num_style),
                Span::styled(title.clone(), style),
                Span::raw("  "),
                Span::styled(dur, dur_style),
            ])
        }
    }
}

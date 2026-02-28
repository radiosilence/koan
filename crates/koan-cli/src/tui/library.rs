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
}

impl LibraryState {
    pub fn new(db_path: &Path) -> Self {
        let mut state = Self {
            nodes: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            db_path: db_path.to_path_buf(),
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
        self.nodes = artists
            .into_iter()
            .map(|a| LibraryNode::Artist {
                id: a.id,
                name: a.name,
                expanded: false,
            })
            .collect();
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

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.ensure_visible();
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.nodes.len() {
            self.cursor += 1;
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
}

impl<'a> LibraryView<'a> {
    pub fn new(state: &'a LibraryState, theme: &'a Theme, focused: bool) -> Self {
        Self {
            state,
            theme,
            focused,
        }
    }
}

impl Widget for LibraryView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_style = if self.focused {
            self.theme.library_artist
        } else {
            self.theme.hint_desc
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(vec![Span::styled(" library ", title_style)]));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.state.nodes.is_empty() {
            let line = Line::from(Span::styled(" empty — run koan scan", self.theme.hint_desc));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        let visible_height = inner.height as usize;
        let scroll = self.state.scroll_offset;
        let end = (scroll + visible_height).min(self.state.nodes.len());

        for (row, i) in (scroll..end).enumerate() {
            let node = &self.state.nodes[i];
            let is_cursor = self.focused && i == self.state.cursor;

            let line = render_node(node, is_cursor, self.theme);
            buf.set_line(inner.x, inner.y + row as u16, &line, inner.width);
        }
    }
}

fn render_node<'a>(node: &LibraryNode, is_cursor: bool, theme: &Theme) -> Line<'a> {
    match node {
        LibraryNode::Artist { name, expanded, .. } => {
            let arrow = if *expanded { "v " } else { "> " };
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
            let arrow = if *expanded { "v " } else { "> " };
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

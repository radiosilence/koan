use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, Widget};

use koan_core::organize::OrganizeResult;
use koan_core::player::state::QueueItemId;

use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizeFocus {
    PatternList,
    Preview,
    RunButton,
}

/// What kind of background result just completed.
pub enum OrganizeCompletionKind {
    Preview,
    Execute,
}

/// Payload from a completed background operation.
pub struct OrganizePendingResult {
    pub kind: OrganizeCompletionKind,
    pub preview: Option<OrganizeResult>,
    pub error: Option<String>,
    /// Path updates (queue_item_id → new_path) from execute.
    pub path_updates: Vec<(QueueItemId, PathBuf)>,
}

pub struct OrganizeModalState {
    /// Named patterns (name, format_string), sorted by name.
    pub patterns: Vec<(String, String)>,
    pub pattern_cursor: usize,
    pub preview: Option<OrganizeResult>,
    pub scroll: usize,
    pub status: Option<String>,
    pub executing: bool,
    pub focus: OrganizeFocus,

    /// Background result channel.
    pending: Arc<Mutex<Option<OrganizePendingResult>>>,

    /// File paths to organize (from queue selection).
    selected_paths: Vec<PathBuf>,
    /// Queue item IDs + paths for updating playlist after organize.
    queue_entries: Vec<(QueueItemId, PathBuf)>,
    /// Completed path updates to send to player.
    completed_path_updates: Vec<(QueueItemId, PathBuf)>,
}

impl OrganizeModalState {
    pub fn new(
        patterns: Vec<(String, String)>,
        selected_paths: Vec<PathBuf>,
        queue_entries: Vec<(QueueItemId, PathBuf)>,
    ) -> Self {
        let pending: Arc<Mutex<Option<OrganizePendingResult>>> = Arc::new(Mutex::new(None));

        let mut state = Self {
            patterns,
            pattern_cursor: 0,
            preview: None,
            scroll: 0,
            status: None,
            executing: false,
            focus: OrganizeFocus::PatternList,
            pending,
            selected_paths,
            queue_entries,
            completed_path_updates: Vec::new(),
        };

        state.request_preview();
        state
    }

    fn current_pattern(&self) -> Option<&str> {
        self.patterns
            .get(self.pattern_cursor)
            .map(|(_, fmt)| fmt.as_str())
    }

    pub fn request_preview(&mut self) {
        let Some(pattern) = self.current_pattern().map(|s| s.to_string()) else {
            self.preview = None;
            self.status = Some("No patterns configured".into());
            return;
        };

        if self.selected_paths.is_empty() {
            self.preview = None;
            return;
        }

        let pending = self.pending.clone();
        let paths = self.selected_paths.clone();

        self.status = Some("Computing preview...".into());

        std::thread::Builder::new()
            .name("koan-org-preview".into())
            .spawn(move || {
                let preview =
                    koan_core::organize::preview_for_paths(&paths, &pattern, None);

                if let Ok(mut p) = pending.lock() {
                    match preview {
                        Ok(result) => {
                            *p = Some(OrganizePendingResult {
                                kind: OrganizeCompletionKind::Preview,
                                preview: Some(result),
                                error: None,
                                path_updates: Vec::new(),
                            });
                        }
                        Err(e) => {
                            *p = Some(OrganizePendingResult {
                                kind: OrganizeCompletionKind::Preview,
                                preview: None,
                                error: Some(e.to_string()),
                                path_updates: Vec::new(),
                            });
                        }
                    }
                }
            })
            .ok();
    }

    pub fn request_execute(&mut self) {
        let Some(pattern) = self.current_pattern().map(|s| s.to_string()) else {
            return;
        };

        if self.selected_paths.is_empty() {
            return;
        }

        self.executing = true;
        self.status = Some("Moving files...".into());

        let pending = self.pending.clone();
        let paths = self.selected_paths.clone();
        let queue_entries = self.queue_entries.clone();

        std::thread::Builder::new()
            .name("koan-org-exec".into())
            .spawn(move || {
                let result =
                    koan_core::organize::execute_for_paths(&paths, &pattern, None);

                if let Ok(mut p) = pending.lock() {
                    match result {
                        Ok(result) => {
                            // Build path update map: match QueueItemIds to moved files.
                            let mut path_updates = Vec::new();
                            for file_move in &result.moves {
                                if let Some((qid, _)) =
                                    queue_entries.iter().find(|(_, p)| *p == file_move.from)
                                {
                                    path_updates.push((*qid, file_move.to.clone()));
                                }
                            }

                            let moved_count = result.moves.len();
                            let error_count = result.errors.len();
                            let skipped = result.skipped;
                            let mut parts = vec![format!("Moved {moved_count} files")];
                            if skipped > 0 {
                                parts.push(format!("{skipped} unchanged"));
                            }
                            if error_count > 0 {
                                parts.push(format!("{error_count} errors"));
                            }
                            let status = Some(parts.join(", "));

                            *p = Some(OrganizePendingResult {
                                kind: OrganizeCompletionKind::Execute,
                                preview: Some(result),
                                error: status,
                                path_updates,
                            });
                        }
                        Err(e) => {
                            *p = Some(OrganizePendingResult {
                                kind: OrganizeCompletionKind::Execute,
                                preview: None,
                                error: Some(e.to_string()),
                                path_updates: Vec::new(),
                            });
                        }
                    }
                }
            })
            .ok();
    }

    /// Check for a completed background result. Returns the kind if something completed.
    pub fn check_pending(&mut self) -> Option<OrganizeCompletionKind> {
        let result = {
            let mut guard = self.pending.lock().ok()?;
            guard.take()?
        };

        let kind = match result.kind {
            OrganizeCompletionKind::Preview => {
                self.preview = result.preview;
                if let Some(err) = result.error {
                    self.status = Some(err);
                } else {
                    self.status = None;
                }
                OrganizeCompletionKind::Preview
            }
            OrganizeCompletionKind::Execute => {
                self.executing = false;
                self.completed_path_updates = result.path_updates;
                if let Some(msg) = result.error {
                    self.status = Some(msg);
                }

                // Update selected_paths and queue_entries to reflect moved files,
                // so re-preview operates on the new locations.
                if let Some(ref exec_result) = result.preview {
                    for file_move in &exec_result.moves {
                        for path in self.selected_paths.iter_mut() {
                            if *path == file_move.from {
                                *path = file_move.to.clone();
                            }
                        }
                        for (_, path) in self.queue_entries.iter_mut() {
                            if *path == file_move.from {
                                *path = file_move.to.clone();
                            }
                        }
                    }
                }

                // Re-run preview against updated paths so the UI reflects reality.
                self.preview = None;
                self.request_preview();

                OrganizeCompletionKind::Execute
            }
        };

        Some(kind)
    }

    /// Take the completed path updates for sending to the player.
    pub fn take_path_updates(&mut self) -> Vec<(QueueItemId, PathBuf)> {
        std::mem::take(&mut self.completed_path_updates)
    }
}

// --- Rendering ---

pub struct OrganizeOverlay<'a> {
    state: &'a OrganizeModalState,
    theme: &'a Theme,
}

impl<'a> OrganizeOverlay<'a> {
    pub fn new(state: &'a OrganizeModalState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

/// Compute the popup rect for the organize modal.
pub fn organize_popup_rect(area: Rect) -> Rect {
    let w = ((area.width as f32 * 0.75) as u16).max(50).min(area.width);
    let h = ((area.height as f32 * 0.70) as u16).max(16).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

impl Widget for OrganizeOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = organize_popup_rect(area);
        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Organize \u{2014} Move/Rename ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        if inner.height < 4 {
            return;
        }

        // Layout: patterns (top, ~30%) | preview (middle, flex) | status + button (bottom, 2)
        let pattern_height = (self.state.patterns.len() as u16 + 1).min(inner.height / 3).max(2);
        let chunks = Layout::vertical([
            Constraint::Length(pattern_height),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

        // --- Pattern list ---
        let pat_focused = self.state.focus == OrganizeFocus::PatternList;
        if self.state.patterns.is_empty() {
            let msg = Line::from(Span::styled(
                " No patterns in config.toml",
                self.theme.hint_desc,
            ));
            Paragraph::new(msg).render(chunks[0], buf);
        } else {
            for (i, (name, fmt)) in self.state.patterns.iter().enumerate() {
                if i >= chunks[0].height as usize {
                    break;
                }
                let is_selected = i == self.state.pattern_cursor;
                let style = if is_selected && pat_focused {
                    Style::default()
                        .fg(ratatui::style::Color::Black)
                        .bg(ratatui::style::Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    self.theme.hint_desc
                };

                let max_fmt_len = chunks[0]
                    .width
                    .saturating_sub(name.len() as u16 + 5) as usize;
                let abbrev_fmt = if fmt.len() > max_fmt_len {
                    format!("{}...", &fmt[..max_fmt_len.saturating_sub(3)])
                } else {
                    fmt.clone()
                };

                let indicator = if is_selected { "\u{25b6} " } else { "  " };
                let line = Line::from(vec![
                    Span::styled(indicator, style),
                    Span::styled(name.as_str(), style),
                    Span::styled(" \u{2014} ", self.theme.hint_desc),
                    Span::styled(abbrev_fmt, self.theme.hint_desc),
                ]);
                let row_rect = Rect::new(chunks[0].x, chunks[0].y + i as u16, chunks[0].width, 1);
                Paragraph::new(line).render(row_rect, buf);
            }
        }

        // --- Preview table ---
        let preview_focused = self.state.focus == OrganizeFocus::Preview;
        let preview_border_style = if preview_focused {
            self.theme.hint_key
        } else {
            self.theme.hint_desc
        };
        let preview_block = Block::default()
            .borders(Borders::TOP)
            .border_style(preview_border_style)
            .title(" Preview ");
        let preview_inner = preview_block.inner(chunks[1]);
        preview_block.render(chunks[1], buf);

        if let Some(ref result) = self.state.preview {
            if result.moves.is_empty() && result.errors.is_empty() {
                let msg = if result.skipped > 0 {
                    format!(" All {} files already at target paths", result.skipped)
                } else {
                    " No files to move".into()
                };
                let line = Line::from(Span::styled(msg, self.theme.hint_desc));
                Paragraph::new(line).render(preview_inner, buf);
            } else if result.moves.is_empty() && !result.errors.is_empty() {
                // All moves failed — show the errors.
                let visible_rows = preview_inner.height as usize;
                let scroll = self.state.scroll.min(result.errors.len().saturating_sub(1));
                let lines: Vec<Line> = result
                    .errors
                    .iter()
                    .skip(scroll)
                    .take(visible_rows)
                    .map(|(path, err)| {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        Line::from(vec![
                            Span::styled(
                                format!(" {name}: "),
                                Style::default().fg(ratatui::style::Color::Red),
                            ),
                            Span::styled(err.as_str(), self.theme.hint_desc),
                        ])
                    })
                    .collect();
                Paragraph::new(lines).render(preview_inner, buf);
            } else {
                // Build combined row list: moves + errors.
                let total_rows = result.moves.len() + result.errors.len();
                let visible_rows = preview_inner.height as usize;
                let scroll = self.state.scroll.min(total_rows.saturating_sub(1));
                let widths = [Constraint::Percentage(45), Constraint::Percentage(55)];

                let move_rows = result.moves.iter().map(|m| {
                    let from_name = m
                        .from
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let to_display = m.to.to_string_lossy().into_owned();
                    Row::new(vec![from_name, to_display])
                });

                let error_rows = result.errors.iter().map(|(path, err)| {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    Row::new(vec![name, format!("ERROR: {err}")])
                        .style(Style::default().fg(ratatui::style::Color::Red))
                });

                let rows: Vec<Row> = move_rows
                    .chain(error_rows)
                    .skip(scroll)
                    .take(visible_rows)
                    .collect();

                let table = Table::new(rows, widths)
                    .header(
                        Row::new(vec!["Source", "Destination"])
                            .style(Style::default().add_modifier(Modifier::BOLD)),
                    )
                    .column_spacing(1);
                Widget::render(table, preview_inner, buf);
            }
        } else if self.state.status.is_some() {
            // Status shown below
        } else {
            let msg = Line::from(Span::styled(" No preview", self.theme.hint_desc));
            Paragraph::new(msg).render(preview_inner, buf);
        }

        // --- Status + button ---
        let status_area = Rect::new(chunks[2].x, chunks[2].y, chunks[2].width, 1);
        if let Some(ref status) = self.state.status {
            let line = Line::from(Span::styled(
                format!(" {status}"),
                self.theme.hint_desc,
            ));
            Paragraph::new(line).render(status_area, buf);
        }

        // Count line + run button.
        let button_area = Rect::new(
            chunks[2].x,
            chunks[2].y + 1,
            chunks[2].width,
            1,
        );
        let (move_count, error_count, skipped_count) = self
            .state
            .preview
            .as_ref()
            .map_or((0, 0, 0), |r| (r.moves.len(), r.errors.len(), r.skipped));
        let btn_focused = self.state.focus == OrganizeFocus::RunButton;
        let btn_style = if btn_focused {
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(ratatui::style::Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            self.theme.hint_desc
        };

        let btn_text = if self.state.executing {
            " [Running...] "
        } else {
            " [Run] "
        };

        let mut count_parts = vec![format!("{} to move", move_count)];
        if skipped_count > 0 {
            count_parts.push(format!("{} unchanged", skipped_count));
        }
        if error_count > 0 {
            count_parts.push(format!("{} errors", error_count));
        }
        let count_text = format!(" {}  ", count_parts.join(", "));

        let line = Line::from(vec![
            Span::styled(count_text, self.theme.hint_desc),
            Span::styled(btn_text, btn_style),
            Span::styled(
                "  tab:focus  \u{2191}\u{2193}:navigate  enter:run  esc:cancel",
                self.theme.hint_desc,
            ),
        ]);
        Paragraph::new(line).render(button_area, buf);
    }
}

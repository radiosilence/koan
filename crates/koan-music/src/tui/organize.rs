use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

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
                let preview = koan_core::organize::preview_for_paths(&paths, &pattern, None);

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
                let result = koan_core::organize::execute_for_paths(&paths, &pattern, None);

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

// --- Path diff helpers (extracted for testability) ---

/// Find the longest common path prefix (at `/` boundaries) across a set of path strings.
fn common_path_prefix(paths: &[String]) -> String {
    if paths.len() < 2 {
        return String::new();
    }
    let first = &paths[0];
    let mut prefix_chars = first.chars().count();
    for p in &paths[1..] {
        prefix_chars = first
            .chars()
            .zip(p.chars())
            .take(prefix_chars)
            .take_while(|(a, b)| a == b)
            .count();
    }
    // Convert char count back to byte offset.
    let prefix_bytes: usize = first.chars().take(prefix_chars).map(|c| c.len_utf8()).sum();
    let prefix = &first[..prefix_bytes];
    // Walk back to last '/' boundary so we don't cut mid-component.
    match prefix.rfind('/') {
        Some(i) => first[..=i].to_string(),
        None => String::new(),
    }
}

/// Find the shared prefix length (at `/` boundaries) between two strings.
/// Returns a **byte offset** into `a` (and `b`) at the last `/` boundary.
fn shared_prefix_len(a: &str, b: &str) -> usize {
    let shared_chars = a
        .chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .count();
    // Convert char count to byte offset.
    let shared_bytes: usize = a.chars().take(shared_chars).map(|c| c.len_utf8()).sum();
    match a[..shared_bytes].rfind('/') {
        Some(i) => i + 1,
        None => 0,
    }
}

/// Truncate a string to at most `max` display chars, adding `…` if truncated.
fn truncate_path(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
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
    let h = ((area.height as f32 * 0.70) as u16)
        .max(16)
        .min(area.height);
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
        let pattern_height = (self.state.patterns.len() as u16 + 1)
            .min(inner.height / 3)
            .max(2);
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

                let max_fmt_len = chunks[0].width.saturating_sub(name.len() as u16 + 5) as usize;
                let abbrev_fmt = truncate_path(fmt, max_fmt_len);

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
                // Before/after diff view: each move = 2 lines (before + after).
                let usable_width = preview_inner.width.saturating_sub(2) as usize;

                // Find longest common path prefix across all from/to paths to strip for display.
                let all_paths: Vec<String> = result
                    .moves
                    .iter()
                    .flat_map(|m| {
                        [
                            m.from.to_string_lossy().into_owned(),
                            m.to.to_string_lossy().into_owned(),
                        ]
                    })
                    .collect();
                let common_prefix = common_path_prefix(&all_paths);

                let strip = |p: &std::path::PathBuf| -> String {
                    let s = p.to_string_lossy();
                    s.strip_prefix(&common_prefix)
                        .unwrap_or(&s)
                        .to_string()
                };

                // Build all display lines.
                let mut all_lines: Vec<Line> = Vec::new();

                for m in &result.moves {
                    let from_rel = strip(&m.from);
                    let to_rel = strip(&m.to);
                    let shared = shared_prefix_len(&from_rel, &to_rel);

                    // Before line: dim (DarkGray), shows old relative path.
                    let before_str = truncate_path(&from_rel, usable_width.saturating_sub(2));
                    all_lines.push(Line::from(vec![
                        Span::styled("  ", self.theme.hint_desc),
                        Span::styled(before_str, self.theme.hint_desc),
                    ]));

                    // After line: shared path prefix in normal colour, changed part in green+bold.
                    let common_part = to_rel[..shared].to_string();
                    let changed_part = if shared < to_rel.len() {
                        to_rel[shared..].to_string()
                    } else {
                        String::new()
                    };
                    let arrow_width = 2; // "→ "
                    let remaining = usable_width.saturating_sub(arrow_width);
                    let common_display = truncate_path(&common_part, remaining);
                    let changed_display =
                        truncate_path(&changed_part, remaining.saturating_sub(common_display.len()));

                    all_lines.push(Line::from(vec![
                        Span::styled("\u{2192} ", Style::default().fg(Color::DarkGray)),
                        Span::styled(common_display, Style::default()),
                        Span::styled(
                            changed_display,
                            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }

                for (path, err) in &result.errors {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    all_lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {name}: "),
                            Style::default().fg(Color::Red),
                        ),
                        Span::styled(err.as_str(), self.theme.hint_desc),
                    ]));
                }

                let total_lines = all_lines.len();
                let visible_rows = preview_inner.height as usize;
                let scroll = self.state.scroll.min(total_lines.saturating_sub(1));

                let visible: Vec<Line> = all_lines
                    .into_iter()
                    .skip(scroll)
                    .take(visible_rows)
                    .collect();

                Paragraph::new(visible).render(preview_inner, buf);
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
            let line = Line::from(Span::styled(format!(" {status}"), self.theme.hint_desc));
            Paragraph::new(line).render(status_area, buf);
        }

        // Count line + run button.
        let button_area = Rect::new(chunks[2].x, chunks[2].y + 1, chunks[2].width, 1);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_path_prefix_basic() {
        let paths = vec![
            "/music/artist/album/01.flac".into(),
            "/music/artist/album/02.flac".into(),
        ];
        assert_eq!(common_path_prefix(&paths), "/music/artist/album/");
    }

    #[test]
    fn test_common_path_prefix_different_albums() {
        let paths = vec![
            "/music/artist/old-album/01.flac".into(),
            "/music/artist/new-album/01.flac".into(),
        ];
        assert_eq!(common_path_prefix(&paths), "/music/artist/");
    }

    #[test]
    fn test_common_path_prefix_no_common() {
        let paths = vec!["/a/file.flac".into(), "/b/file.flac".into()];
        assert_eq!(common_path_prefix(&paths), "/");
    }

    #[test]
    fn test_common_path_prefix_single_path() {
        let paths = vec!["/music/track.flac".into()];
        assert_eq!(common_path_prefix(&paths), "");
    }

    #[test]
    fn test_common_path_prefix_empty() {
        let paths: Vec<String> = vec![];
        assert_eq!(common_path_prefix(&paths), "");
    }

    #[test]
    fn test_shared_prefix_len_same_dir() {
        assert_eq!(
            shared_prefix_len("artist/old-album/01.flac", "artist/new-album/01.flac"),
            7 // "artist/"
        );
    }

    #[test]
    fn test_shared_prefix_len_no_shared_dir() {
        assert_eq!(shared_prefix_len("foo/a.flac", "bar/a.flac"), 0);
    }

    #[test]
    fn test_shared_prefix_len_identical() {
        assert_eq!(
            shared_prefix_len("artist/album/track.flac", "artist/album/track.flac"),
            13 // "artist/album/"
        );
    }

    #[test]
    fn test_truncate_path_fits() {
        assert_eq!(truncate_path("short", 10), "short");
    }

    #[test]
    fn test_truncate_path_exact() {
        assert_eq!(truncate_path("exact", 5), "exact");
    }

    #[test]
    fn test_truncate_path_truncated() {
        let result = truncate_path("very-long-path-name.flac", 10);
        assert_eq!(result, "very-long…");
        assert!(result.len() <= 13); // 9 ascii + multibyte ellipsis
    }

    // --- Unicode torture tests ---
    // Filenames in the wild contain fullwidth Japanese, CJK, emoji, Arabic
    // ligatures, combining diacritics (Zalgo), and zero-width joiners.
    // Every helper must handle these without panicking.

    #[test]
    fn test_truncate_path_fullwidth_japanese() {
        // Fullwidth chars are 3 bytes each — the original bug.
        let s = "Ｊ Ｕ Ｒ Ａ Ｓ Ｓ Ｉ Ｃ";
        let result = truncate_path(s, 5);
        assert_eq!(result.chars().count(), 5); // 4 chars + ellipsis
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_path_cjk() {
        let s = "島のクラッシュ.m4a";
        let result = truncate_path(s, 4);
        assert_eq!(result.chars().count(), 4);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_path_emoji() {
        let s = "🎵🎶🎸🎹🎷🎺🎻.flac";
        let result = truncate_path(s, 4);
        assert_eq!(result.chars().count(), 4);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_path_arabic_bismillah() {
        // ﷽ (U+FDFD) — one of the widest Unicode glyphs.
        let s = "﷽/track.flac";
        let result = truncate_path(s, 3);
        assert_eq!(result.chars().count(), 3);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_path_combining_diacritics_zalgo() {
        // Zalgo text: base char + many combining marks.
        let s = "Z\u{0337}\u{0327}\u{0310}\u{0324}a\u{033A}\u{0303}lgo/track.flac";
        // Should not panic — combining marks are separate chars.
        let result = truncate_path(s, 6);
        assert!(result.chars().count() <= 6);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_path_emoji_zwj_sequence() {
        // Family emoji with zero-width joiners: 👨‍👩‍👧‍👦 (7 codepoints, 1 glyph)
        let s = "👨\u{200D}👩\u{200D}👧\u{200D}👦/track.flac";
        let result = truncate_path(s, 5);
        assert!(result.chars().count() <= 5);
        // Must not panic on ZWJ boundaries.
    }

    #[test]
    fn test_truncate_path_flag_emoji() {
        // Flag emoji: regional indicators 🇯🇵 (2 codepoints)
        let s = "🇯🇵🇺🇸🇬🇧/music.flac";
        let result = truncate_path(s, 4);
        assert!(result.chars().count() <= 4);
    }

    #[test]
    fn test_common_path_prefix_cjk_paths() {
        let paths = vec![
            "/音楽/アーティスト/アルバム/01.flac".into(),
            "/音楽/アーティスト/アルバム/02.flac".into(),
        ];
        assert_eq!(common_path_prefix(&paths), "/音楽/アーティスト/アルバム/");
    }

    #[test]
    fn test_common_path_prefix_fullwidth_diverge() {
        // Paths diverge inside fullwidth text — must not split mid-char.
        let paths = vec![
            "/music/Ｊ Ｕ Ｒ Ａ/01.m4a".into(),
            "/music/Ｊ Ｕ Ｒ Ｂ/01.m4a".into(),
        ];
        let prefix = common_path_prefix(&paths);
        assert_eq!(prefix, "/music/");
        // Verify the prefix is valid UTF-8 (no mid-char slice).
        assert!(prefix.is_char_boundary(prefix.len()));
    }

    #[test]
    fn test_common_path_prefix_emoji_folders() {
        let paths = vec![
            "/🎵/🎸/track.flac".into(),
            "/🎵/🎹/track.flac".into(),
        ];
        assert_eq!(common_path_prefix(&paths), "/🎵/");
    }

    #[test]
    fn test_shared_prefix_len_cjk() {
        let a = "アーティスト/古いアルバム/01.flac";
        let b = "アーティスト/新しいアルバム/01.flac";
        let shared = shared_prefix_len(a, b);
        // Should return byte offset after "アーティスト/"
        assert_eq!(&a[..shared], "アーティスト/");
    }

    #[test]
    fn test_shared_prefix_len_emoji() {
        let a = "🎵/old/track.flac";
        let b = "🎵/new/track.flac";
        let shared = shared_prefix_len(a, b);
        assert_eq!(&a[..shared], "🎵/");
    }

    #[test]
    fn test_shared_prefix_len_mixed_width() {
        // Mix of ASCII and multi-byte before the divergence point.
        let a = "artist-名前/album-A/01.flac";
        let b = "artist-名前/album-B/01.flac";
        let shared = shared_prefix_len(a, b);
        assert_eq!(&a[..shared], "artist-名前/");
    }

    #[test]
    fn test_truncate_path_extreme_zalgo() {
        // Cthulhu-tier combining modifiers: each base char has dozens of
        // combining diacritical marks stacked on it.
        let zalgo = "Ǫ\u{0337}\u{0327}\u{0310}\u{0324}\u{0332}\u{0347}\u{0353}\u{035A}\u{0317}H\u{0336}\u{0321}\u{0308}\u{0303}\u{0342}\u{0326}\u{032E}\u{0330} \u{0335}\u{0322}\u{0307}\u{030C}\u{0328}\u{0316}\u{0331}M\u{0334}\u{0323}\u{030A}\u{0325}\u{0339}\u{032D}\u{0348}Y\u{0335}\u{0309}\u{030B}\u{0304}\u{0327}\u{0316}\u{0331}/track.flac";
        // Must not panic.
        let result = truncate_path(zalgo, 8);
        assert!(result.chars().count() <= 8);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_common_path_prefix_zalgo_folder() {
        let zalgo_dir = "/music/Z\u{0337}\u{0327}a\u{033A}\u{0303}l\u{0335}g\u{0321}o\u{0336}";
        let paths = vec![
            format!("{}/album-A/01.flac", zalgo_dir),
            format!("{}/album-B/01.flac", zalgo_dir),
        ];
        let prefix = common_path_prefix(&paths);
        assert_eq!(prefix, format!("{}/", zalgo_dir));
    }

    #[test]
    fn test_shared_prefix_len_zalgo() {
        let base = "Z\u{0337}\u{0327}\u{0310}a\u{033A}\u{0303}lgo";
        let a = format!("{}/old/track.flac", base);
        let b = format!("{}/new/track.flac", base);
        let shared = shared_prefix_len(&a, &b);
        assert_eq!(&a[..shared], format!("{}/", base));
    }

    #[test]
    fn test_truncate_path_skin_tone_emoji() {
        // Emoji with skin tone modifier: 👩🏽 = 👩 + 🏽 (2 codepoints)
        let s = "👩\u{1F3FD}👨\u{1F3FB}👧\u{1F3FE}/music/track.flac";
        let result = truncate_path(s, 5);
        assert!(result.chars().count() <= 5);
    }

    #[test]
    fn test_all_helpers_with_realworld_vaporwave() {
        // Real filename from the crash: fullwidth + CJK + standard ASCII.
        let from = "/music/Valet Girls/(2015) Ｊ Ｕ Ｒ Ａ Ｓ Ｓ Ｉ Ｃ Ｐ Ａ Ｒ Ｌ Ｏ Ｒ [AAC]/01. Valet Girls - ｆｒｉｅｎｄｌｙ ｓｋｉｅｓ 島のクラッシュ.m4a";
        let to = "/music/Valet Girls/(2015) Ｊ Ｕ Ｒ Ａ Ｓ Ｓ Ｉ Ｃ Ｐ Ａ Ｒ Ｌ Ｏ Ｒ [ALAC]/01. Valet Girls - ｆｒｉｅｎｄｌｙ ｓｋｉｅｓ 島のクラッシュ.m4a";
        let paths = vec![from.to_string(), to.to_string()];

        // None of these should panic.
        let prefix = common_path_prefix(&paths);
        assert!(prefix.ends_with('/'));

        let from_rel = from.strip_prefix(&prefix).unwrap_or(from);
        let to_rel = to.strip_prefix(&prefix).unwrap_or(to);
        let shared = shared_prefix_len(from_rel, to_rel);
        // shared must be a valid byte offset into both strings.
        assert!(from_rel.is_char_boundary(shared));
        assert!(to_rel.is_char_boundary(shared));

        let truncated = truncate_path(from_rel, 40);
        assert!(truncated.chars().count() <= 40);
    }
}

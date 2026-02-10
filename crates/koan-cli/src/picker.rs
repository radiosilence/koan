use std::io::{self, Write};

use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use owo_colors::OwoColorize;

/// A single item in the picker — display string + associated ID.
pub struct PickerItem {
    pub id: i64,
    pub display: String,
    /// Plain text for matching (no ANSI escapes).
    pub match_text: String,
}

/// Result of running the picker.
pub enum PickerResult {
    /// User selected one or more items.
    Selected(Vec<i64>),
    /// User cancelled (Esc).
    Cancelled,
}

/// In-process fuzzy picker. Renders inline with ANSI escapes, uses nucleo for matching.
/// Playback continues uninterrupted — we just consume key events differently.
pub struct Picker {
    items: Vec<PickerItem>,
    prompt: String,
    multi: bool,
}

fn term_size() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            (ws.ws_col, ws.ws_row)
        } else {
            (80, 24)
        }
    }
}

/// Strip ANSI escape sequences and truncate to `max_width` visible characters.
/// Returns a string that renders at most `max_width` columns.
fn truncate_ansi(s: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut visible = 0;
    let mut in_escape = false;

    for c in s.chars() {
        if in_escape {
            out.push(c);
            if c.is_ascii_alphabetic() || c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            out.push(c);
            continue;
        }
        if visible >= max_width {
            break;
        }
        out.push(c);
        visible += 1;
    }
    out
}

impl Picker {
    pub fn new(items: Vec<PickerItem>, prompt: &str, multi: bool) -> Self {
        Self {
            items,
            prompt: prompt.to_string(),
            multi,
        }
    }

    /// Run the picker, blocking until the user selects or cancels.
    /// `read_key` is called to get the next keypress — this lets the caller
    /// control the input source (same raw mode stdin as playback).
    pub fn run(self, read_key: &mut dyn FnMut() -> Option<PickerKey>) -> PickerResult {
        let (term_w, term_h) = term_size();
        // Reserve 2 lines for prompt + hint bar, leave 1 line margin.
        let max_visible = (term_h as usize).saturating_sub(3);
        let height = max_visible + 2; // prompt + results + hint
        let width = term_w as usize;

        let nucleo: Nucleo<u32> = Nucleo::new(
            Config::DEFAULT,
            std::sync::Arc::new(|| {}),
            None,
            1, // single column
        );

        // Inject all items.
        let injector = nucleo.injector();
        for (i, item) in self.items.iter().enumerate() {
            let text = item.match_text.clone();
            injector.push(i as u32, |_val, cols| {
                cols[0] = text.into();
            });
        }

        let mut query = String::new();
        let mut cursor: usize = 0;
        let mut selected: Vec<usize> = Vec::new();
        let mut nucleo = nucleo;

        // Initial tick to populate.
        nucleo.tick(10);

        // Reserve screen space and save the top-of-picker cursor position.
        {
            let mut stdout = io::stdout().lock();
            // Save position, print blank lines to scroll if needed, then position cursor.
            let _ = write!(stdout, "\x1b[?25l"); // hide cursor
            for _ in 0..height {
                let _ = stdout.write_all(b"\n");
            }
            // Move back up to top of reserved area.
            let _ = write!(stdout, "\x1b[{}A", height);
            // Save this position — all renders will restore to here.
            let _ = write!(stdout, "\x1b[s");
            let _ = stdout.flush();
        }

        self.render(&nucleo, &query, cursor, &selected, max_visible, width);

        loop {
            let Some(key) = read_key() else {
                self.clear_display(height);
                return PickerResult::Cancelled;
            };

            match key {
                PickerKey::Char(c) => {
                    query.push(c);
                    nucleo.pattern.reparse(
                        0,
                        &query,
                        CaseMatching::Smart,
                        Normalization::Smart,
                        true,
                    );
                    cursor = 0;
                }
                PickerKey::Backspace => {
                    if query.pop().is_some() {
                        nucleo.pattern.reparse(
                            0,
                            &query,
                            CaseMatching::Smart,
                            Normalization::Smart,
                            false,
                        );
                        cursor = 0;
                    }
                }
                PickerKey::Up => {
                    cursor = cursor.saturating_sub(1);
                }
                PickerKey::Down => {
                    let snap = nucleo.snapshot();
                    let count = snap.matched_item_count() as usize;
                    if cursor + 1 < count {
                        cursor += 1;
                    }
                }
                PickerKey::Tab if self.multi => {
                    let snap = nucleo.snapshot();
                    if let Some(item) = snap.get_matched_item(cursor as u32) {
                        let idx = *item.data;
                        if let Some(pos) = selected.iter().position(|&s| s == idx as usize) {
                            selected.remove(pos);
                        } else {
                            selected.push(idx as usize);
                        }
                        let count = snap.matched_item_count() as usize;
                        if cursor + 1 < count {
                            cursor += 1;
                        }
                    }
                }
                PickerKey::Enter => {
                    self.clear_display(height);
                    if self.multi && !selected.is_empty() {
                        let ids: Vec<i64> = selected.iter().map(|&i| self.items[i].id).collect();
                        return PickerResult::Selected(ids);
                    }
                    let snap = nucleo.snapshot();
                    if let Some(item) = snap.get_matched_item(cursor as u32) {
                        let idx = *item.data as usize;
                        return PickerResult::Selected(vec![self.items[idx].id]);
                    }
                    return PickerResult::Cancelled;
                }
                PickerKey::Escape => {
                    self.clear_display(height);
                    return PickerResult::Cancelled;
                }
                _ => {}
            }

            nucleo.tick(10);
            self.render(&nucleo, &query, cursor, &selected, max_visible, width);
        }
    }

    fn render(
        &self,
        nucleo: &Nucleo<u32>,
        query: &str,
        cursor: usize,
        selected: &[usize],
        max_visible: usize,
        width: usize,
    ) {
        let snap = nucleo.snapshot();
        let matched = snap.matched_item_count() as usize;
        let total = snap.item_count() as usize;

        let mut out = String::new();

        // Hide cursor + restore to saved position (top of picker area).
        out.push_str("\x1b[?25l\x1b[u");

        // Prompt line — clear then write.
        let prompt_line = format!(
            "  {} {}{}",
            self.prompt.cyan().bold(),
            query,
            format!("  {}/{}", matched, total).dimmed(),
        );
        out.push_str(&format!(
            "\x1b[2K{}\r\n",
            truncate_ansi(&prompt_line, width)
        ));

        // Results.
        let start = if cursor >= max_visible {
            cursor - max_visible + 1
        } else {
            0
        };
        let end = (start + max_visible).min(matched);

        for i in start..end {
            if let Some(item) = snap.get_matched_item(i as u32) {
                let idx = *item.data as usize;
                let is_cursor = i == cursor;
                let is_selected = selected.contains(&idx);

                let marker = if is_selected {
                    "◆ ".green().to_string()
                } else if is_cursor {
                    "▸ ".cyan().to_string()
                } else {
                    "  ".to_string()
                };

                let line = &self.items[idx].display;
                let full = if is_cursor {
                    format!("  {}{}", marker, line.bold())
                } else {
                    format!("  {}{}", marker, line)
                };
                out.push_str(&format!("\x1b[2K{}\r\n", truncate_ansi(&full, width)));
            }
        }

        // Pad remaining lines.
        let rendered = end - start;
        for _ in rendered..max_visible {
            out.push_str("\x1b[2K\r\n");
        }

        // Hint bar.
        let hint = if self.multi {
            format!(
                "  {}  {}  {}",
                "↑↓ navigate".dimmed(),
                "tab select".dimmed(),
                "enter confirm  esc cancel".dimmed()
            )
        } else {
            format!(
                "  {}  {}",
                "↑↓ navigate".dimmed(),
                "enter select  esc cancel".dimmed()
            )
        };
        out.push_str(&format!("\x1b[2K{}", truncate_ansi(&hint, width)));

        // Show cursor.
        out.push_str("\x1b[?25h");

        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    fn clear_display(&self, height: usize) {
        let mut out = String::new();
        // Restore to saved position.
        out.push_str("\x1b[u");
        for _ in 0..height {
            out.push_str("\x1b[2K\n");
        }
        // Move back to saved position.
        out.push_str("\x1b[u");
        // Show cursor.
        out.push_str("\x1b[?25h");

        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }
}

/// Key events the picker understands.
pub enum PickerKey {
    Char(char),
    Backspace,
    Up,
    Down,
    Tab,
    Enter,
    Escape,
}

/// Parse raw bytes from stdin into PickerKey.
/// Handles escape sequences for arrow keys.
pub fn parse_key(byte: u8, read_more: &mut dyn FnMut() -> Option<u8>) -> Option<PickerKey> {
    match byte {
        0x1b => {
            let Some(b'[') = read_more() else {
                return Some(PickerKey::Escape);
            };
            match read_more() {
                Some(b'A') => Some(PickerKey::Up),
                Some(b'B') => Some(PickerKey::Down),
                _ => None,
            }
        }
        0x0d | 0x0a => Some(PickerKey::Enter),
        0x7f | 0x08 => Some(PickerKey::Backspace),
        0x09 => Some(PickerKey::Tab),
        0x03 => Some(PickerKey::Escape), // Ctrl-C
        b if (0x20..0x7f).contains(&b) => Some(PickerKey::Char(b as char)),
        _ => None,
    }
}

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
        let max_visible: usize = 20;

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

        self.render(&nucleo, &query, cursor, &selected, max_visible);

        loop {
            let Some(key) = read_key() else {
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
                    // Toggle selection on current item.
                    let snap = nucleo.snapshot();
                    if let Some(item) = snap.get_matched_item(cursor as u32) {
                        let idx = *item.data;
                        if let Some(pos) = selected.iter().position(|&s| s == idx as usize) {
                            selected.remove(pos);
                        } else {
                            selected.push(idx as usize);
                        }
                        // Move cursor down after toggle.
                        let count = snap.matched_item_count() as usize;
                        if cursor + 1 < count {
                            cursor += 1;
                        }
                    }
                }
                PickerKey::Enter => {
                    self.clear_display(max_visible);
                    if self.multi && !selected.is_empty() {
                        let ids: Vec<i64> = selected.iter().map(|&i| self.items[i].id).collect();
                        return PickerResult::Selected(ids);
                    }
                    // Single select — use cursor position.
                    let snap = nucleo.snapshot();
                    if let Some(item) = snap.get_matched_item(cursor as u32) {
                        let idx = *item.data as usize;
                        return PickerResult::Selected(vec![self.items[idx].id]);
                    }
                    return PickerResult::Cancelled;
                }
                PickerKey::Escape => {
                    self.clear_display(max_visible);
                    return PickerResult::Cancelled;
                }
                _ => {}
            }

            nucleo.tick(10);
            self.render(&nucleo, &query, cursor, &selected, max_visible);
        }
    }

    fn render(
        &self,
        nucleo: &Nucleo<u32>,
        query: &str,
        cursor: usize,
        selected: &[usize],
        max_visible: usize,
    ) {
        let snap = nucleo.snapshot();
        let matched = snap.matched_item_count() as usize;
        let total = snap.item_count() as usize;

        let mut out = String::new();

        // Move to start of picker area and clear.
        // Save cursor, move up to clear previous render, then redraw.
        out.push_str("\x1b[s"); // save cursor
        out.push_str(&format!("\x1b[{}A", max_visible + 2)); // move up
        for _ in 0..max_visible + 2 {
            out.push_str("\x1b[2K\n"); // clear line
        }
        out.push_str(&format!("\x1b[{}A", max_visible + 2)); // move back up

        // Prompt line.
        out.push_str(&format!(
            "  {} {}{}\n",
            self.prompt.cyan().bold(),
            query,
            format!("  {}/{}", matched, total).dimmed(),
        ));

        // Results.
        let visible_count = matched.min(max_visible);
        // Window around cursor.
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
                if is_cursor {
                    out.push_str(&format!("  {}{}\n", marker, line.bold()));
                } else {
                    out.push_str(&format!("  {}{}\n", marker, line));
                }
            }
        }

        // Pad remaining lines.
        for _ in visible_count..max_visible {
            out.push_str("\x1b[2K\n");
        }

        // Bottom border.
        let hint = if self.multi {
            format!(
                "{}  {}  {}",
                "↑↓ navigate".dimmed(),
                "tab select".dimmed(),
                "enter confirm  esc cancel".dimmed()
            )
        } else {
            format!(
                "{}  {}",
                "↑↓ navigate".dimmed(),
                "enter select  esc cancel".dimmed()
            )
        };
        out.push_str(&format!("  {}", hint));

        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    fn clear_display(&self, max_visible: usize) {
        let mut out = String::new();
        out.push_str(&format!("\x1b[{}A", max_visible + 2));
        for _ in 0..max_visible + 2 {
            out.push_str("\x1b[2K\n");
        }
        out.push_str(&format!("\x1b[{}A", max_visible + 2));

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
            // Escape sequence or bare Escape.
            let Some(b'[') = read_more() else {
                return Some(PickerKey::Escape);
            };
            match read_more() {
                Some(b'A') => Some(PickerKey::Up),
                Some(b'B') => Some(PickerKey::Down),
                _ => None, // ignore other sequences
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

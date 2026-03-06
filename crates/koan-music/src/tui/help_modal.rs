use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::theme::Theme;

struct Section {
    title: &'static str,
    bindings: &'static [(&'static str, &'static str)],
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Playback",
        bindings: &[
            ("space", "Play / Pause"),
            ("< >", "Previous / Next track"),
            (", .", "Seek backward / forward"),
            ("+ -", "Volume up / down"),
        ],
    },
    Section {
        title: "Navigation",
        bindings: &[
            ("p", "Track picker"),
            ("a", "Album picker"),
            ("r", "Artist picker"),
            ("l", "Library browser"),
            ("/", "Search queue"),
            ("e", "Edit queue"),
            ("z", "Zoom cover art"),
            ("d", "Output device"),
        ],
    },
    Section {
        title: "Queue Edit (e)",
        bindings: &[
            ("\u{2191}\u{2193}", "Navigate"),
            ("S-\u{2191}\u{2193}", "Extend selection"),
            ("C-a", "Select all"),
            ("d", "Delete selected"),
            ("j / k", "Move selected up/down"),
            ("C-z / C-y", "Undo / Redo"),
            ("space", "Context actions"),
            ("i", "Track info"),
            ("g / G", "Jump to top / end"),
            ("PgUp/Dn", "Page up / down"),
        ],
    },
    Section {
        title: "Picker (p/a/r)",
        bindings: &[
            ("\u{2191}\u{2193}", "Navigate results"),
            ("enter", "Append to queue"),
            ("C-enter", "Append & play"),
            ("C-r", "Replace queue"),
            ("tab", "Toggle multi-select"),
        ],
    },
    Section {
        title: "Library (l)",
        bindings: &[
            ("\u{2191}\u{2193}", "Navigate tree"),
            ("\u{2192} / enter", "Expand node"),
            ("\u{2190}", "Collapse node"),
            ("a", "Enqueue selection"),
            ("f", "Filter"),
            ("tab", "Switch focus"),
        ],
    },
    Section {
        title: "General",
        bindings: &[
            ("?", "This help"),
            ("q", "Quit"),
            ("esc", "Close modal / cancel"),
        ],
    },
];

pub struct HelpModalOverlay<'a> {
    theme: &'a Theme,
}

impl<'a> HelpModalOverlay<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl Widget for HelpModalOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = 72u16.min(area.width.saturating_sub(4));
        let content_lines = SECTIONS.iter().map(|s| 1 + s.bindings.len()).sum::<usize>()
            + SECTIONS.len().saturating_sub(1); // blank lines between sections
        let h = (content_lines as u16 + 2).min(area.height.saturating_sub(2)); // +2 for border
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect::new(x, y, w, h);

        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Keybindings ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        // Render in two columns.
        let col_width = inner.width / 2;
        let col_height = inner.height as usize;

        // Build all lines first.
        let mut lines: Vec<Line> = Vec::new();
        for (si, section) in SECTIONS.iter().enumerate() {
            if si > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                format!(" {}", section.title),
                self.theme
                    .hint_key
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            for (key, desc) in section.bindings {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:>12}", key),
                        self.theme.hint_key.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", desc), self.theme.hint_desc),
                ]));
            }
        }

        // Split into two columns.
        let mid = lines.len().div_ceil(2);
        for (i, line) in lines.iter().enumerate() {
            let (col_x, row) = if i < mid {
                (inner.x, i)
            } else {
                (inner.x + col_width, i - mid)
            };
            if row >= col_height {
                continue;
            }
            let row_area = Rect::new(col_x, inner.y + row as u16, col_width, 1);
            Paragraph::new(line.clone()).render(row_area, buf);
        }
    }
}

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
            ("s", "Shuffle on / off"),
            ("R", "Repeat: off / queue / one"),
            ("T", "Sleep: 15 / 30 / 45 / 60 min / track / record / off"),
            (", .", "Seek backward / forward"),
        ],
    },
    Section {
        title: "Pages",
        bindings: &[
            (
                "0-9",
                "Queue / playing / albums / artists / faves / playlists / recent / history / settings / search (0)",
            ),
            ("tab", "Next page (S-tab previous)"),
            ("[ ]", "Back / forward (also A-\u{2190} A-\u{2192})"),
            ("esc", "Leave the page"),
            ("I", "Covers: images / half blocks"),
        ],
    },
    Section {
        title: "Navigation",
        bindings: &[
            ("p", "Track picker"),
            ("a", "Album picker"),
            ("r", "Artist picker"),
            ("l", "Artists page"),
            ("/", "Search queue"),
            ("e", "Edit queue"),
            ("z", "Zoom cover art"),
            ("D", "Output device"),
            ("C", "Play on: control another kōan, or move the music"),
            ("E", "Choose the EQ"),
            ("w", "Add to a playlist (W: save the queue as one)"),
            ("B", "Bypass the EQ, or put it back"),
            ("A / O", "Artist / album of the selection"),
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
        title: "Artists, albums, favourites, recent",
        bindings: &[
            (
                "\u{2191}\u{2193}\u{2190}\u{2192}",
                "Move (tree: expand / collapse)",
            ),
            ("enter", "Open (tree: expand, or play a track)"),
            ("P", "Play"),
            ("a", "Append (C-r replaces the queue)"),
            ("N", "Play next"),
            ("m", "Actions"),
            ("/", "Filter"),
            ("v / o", "Tiles or rows / sort order"),
        ],
    },
    Section {
        title: "Playlists",
        bindings: &[
            ("P", "Play"),
            ("c", "New playlist"),
            ("K / J", "Move a track up / down"),
        ],
    },
    Section {
        title: "Toggles",
        bindings: &[
            ("L", "Lyrics panel"),
            ("V", "Visualiser on/off"),
            ("v", "Visualiser picker"),
            ("M", "Visualiser mode cycle"),
            ("F", "Visualiser fullscreen"),
            ("S", "Bass shake on/off"),
            ("X", "Matrix overlay on/off"),
            ("f", "Favourite track"),
        ],
    },
    Section {
        title: "General",
        bindings: &[
            ("?", "This help (\u{2191}\u{2193} scroll)"),
            ("q", "Quit"),
            ("esc", "Close modal / cancel"),
        ],
    },
];

pub struct HelpModalOverlay<'a> {
    theme: &'a Theme,
    scroll: usize,
}

impl<'a> HelpModalOverlay<'a> {
    pub fn new(theme: &'a Theme, scroll: usize) -> Self {
        Self { theme, scroll }
    }
}

/// The popup in `area`, and how many rows it shows of each column.
fn layout(area: Rect) -> (Rect, usize) {
    let w = 72u16.min(area.width.saturating_sub(4));
    let h = (column_len() as u16 + 2).min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    (Rect::new(x, y, w, h), h.saturating_sub(2) as usize)
}

/// Lines in a column: the sections, a blank line between each, split in two.
fn column_len() -> usize {
    let lines = SECTIONS.iter().map(|s| 1 + s.bindings.len()).sum::<usize>()
        + SECTIONS.len().saturating_sub(1);
    lines.div_ceil(2)
}

/// How far the help scrolls in `area` before its end is on screen.
pub fn max_scroll(area: Rect) -> usize {
    column_len().saturating_sub(layout(area).1)
}

impl Widget for HelpModalOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (popup, col_height) = layout(area);

        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Keybindings ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        let col_width = inner.width / 2;
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

        // Two columns, scrolled together.
        let mid = column_len();
        for (i, line) in lines.into_iter().enumerate() {
            let (col_x, row) = if i < mid {
                (inner.x, i)
            } else {
                (inner.x + col_width, i - mid)
            };
            let Some(row) = row.checked_sub(self.scroll).filter(|r| *r < col_height) else {
                continue;
            };
            let row_area = Rect::new(col_x, inner.y + row as u16, col_width, 1);
            Paragraph::new(line).render(row_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_help_can_be_scrolled_into_view() {
        let short = Rect::new(0, 0, 100, 20);
        let (_, shown) = layout(short);
        assert!(shown < column_len());
        assert_eq!(max_scroll(short) + shown, column_len());
        // Tall enough, nothing scrolls.
        assert_eq!(max_scroll(Rect::new(0, 0, 100, 200)), 0);
    }
}

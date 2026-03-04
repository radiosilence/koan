use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::app::Mode;
use super::theme::Theme;

pub struct HintBar<'a> {
    mode: &'a Mode,
    theme: &'a Theme,
}

impl<'a> HintBar<'a> {
    pub fn new(mode: &'a Mode, theme: &'a Theme) -> Self {
        Self { mode, theme }
    }
}

impl Widget for HintBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let hints: Vec<(&str, &str)> = match self.mode {
            Mode::Normal => vec![
                ("space", "pause"),
                ("<>", "skip"),
                (",.", "seek"),
                ("/", "search"),
                ("p", "track"),
                ("a", "album"),
                ("r", "artist"),
                ("l", "library"),
                ("e", "edit"),
                ("i", "info"),
                ("f", "\u{2605}fav"),
                ("z", "art"),
                ("C-z", "undo"),
                ("q", "quit"),
            ],
            Mode::QueueEdit => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("S-\u{2191}\u{2193}", "select"),
                ("d", "delete"),
                ("j/k", "move"),
                ("C-z/y", "undo/redo"),
                ("space", "actions"),
                ("i", "info"),
                ("g/G", "top/end"),
                ("PgUp/Dn", "page"),
                ("esc", "done"),
            ],
            Mode::TrackInfo(_) => vec![("esc", "close"), ("i", "close")],
            Mode::CoverArtZoom => vec![("esc", "close"), ("z", "close")],
            Mode::LibraryBrowse => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("\u{2192}/enter", "expand"),
                ("\u{2190}", "collapse"),
                ("a", "enqueue"),
                ("f", "filter"),
                ("tab", "focus"),
                ("space", "pause"),
                ("esc", "close"),
            ],
            Mode::ContextMenu => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "select"),
                ("esc", "cancel"),
            ],
            Mode::Organize => vec![
                ("tab", "focus"),
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "run"),
                ("esc", "cancel"),
            ],
            Mode::Picker(_) => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "append"),
                ("C-ret", "play"),
                ("C-r", "replace"),
                ("tab", "multi"),
                ("esc", "cancel"),
            ],
        };

        let mut spans: Vec<Span> = Vec::new();
        for (i, (key, desc)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", self.theme.hint_desc));
            }
            spans.push(Span::styled(
                format!("[{}]", key),
                self.theme.hint_key.add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!(" {}", desc), self.theme.hint_desc));
        }

        let line = Line::from(spans);
        let line_widget = ratatui::widgets::Paragraph::new(line);
        line_widget.render(area, buf);
    }
}

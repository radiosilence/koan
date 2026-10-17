use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::theme::Theme;

struct Section {
    title: &'static str,
    bindings: &'static [(&'static str, &'static str)],
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Playback",
        bindings: &[
            ("space", "Play / Pause, on every page"),
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
                "Pages, as the sidebar lists them: search (0), queue (1) … settings (9)",
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
            ("z", "Zoom cover art"),
            ("D", "Output device"),
            ("C", "Play on: control another kōan, or move the music"),
            ("E", "EQ page (click the transport's EQ for presets)"),
            ("w", "Add to a playlist (W: save the queue as one)"),
            ("B", "Bypass the EQ (kept as a preset), or put it back"),
            ("A / O", "Artist / album of the selection"),
        ],
    },
    Section {
        title: "Queue",
        bindings: &[
            ("\u{2191}\u{2193} j k", "Move the cursor"),
            ("t", "Tag the row, or let it go (History too)"),
            ("S-\u{2191}\u{2193}", "Extend the selection (S-click too)"),
            ("C-a / esc", "Select all / clear the selection"),
            ("enter", "Play the row"),
            ("J / K", "Move the selection down / up"),
            ("d", "Remove the selection"),
            ("m", "Actions for the selection"),
            ("v", "Albums or a row per track"),
            ("o", "Scroll to what is playing"),
            ("C-z / C-y", "Undo / Redo"),
            ("C-x", "Clear the queue"),
            ("i", "Track info"),
            ("g / G", "Jump to top / end"),
            ("PgUp/Dn", "Page up / down"),
        ],
    },
    Section {
        title: "Picker (p/a/r)",
        bindings: &[
            ("\u{2191}\u{2193}", "Move through the results"),
            ("\u{2190}\u{2192}", "Tracks, albums or artists"),
            ("tab", "Pick the row, or let it go"),
            ("enter", "Add to the queue"),
            ("C-enter", "Add and play"),
            ("C-r", "Replace the queue"),
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
        title: "EQ (E)",
        bindings: &[
            ("enter", "Choose: device, preset, correction, open an EQ"),
            ("space", "Switch an EQ on or off"),
            ("K / J", "Move an EQ up / down"),
            ("x", "Take an EQ out of the filters, or remove a band"),
            (
                "\u{2190}\u{2192} / tab",
                "Move between a band's type, Hz, dB and Q",
            ),
            ("+ -  ] [", "Nudge the figure: fine / coarse"),
            ("0-9 / enter", "Type a figure (1k, -3.5, 0.7)"),
            ("a", "Add a band"),
            (
                "n / i",
                "New EQ / import a file (or drop one on the terminal)",
            ),
            ("p / s", "Preset / save as preset"),
            ("m", "Manage every correction, EQ and preset"),
            ("r c R D", "Rename / save as new / reset to file / delete"),
        ],
    },
    Section {
        title: "Playlists",
        bindings: &[
            ("P", "Play"),
            ("c", "New playlist"),
            ("K / J", "Move a track up / down"),
            ("v", "Albums or a row per track"),
        ],
    },
    Section {
        title: "Toggles",
        bindings: &[
            ("L", "Lyrics panel"),
            ("V", "Visualiser: off, or which one"),
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

/// A line of the help: a blank between sections, a section's title, or a
/// key with a line of what it does (the key on the first line only).
enum HelpLine {
    Blank,
    Title(&'static str),
    Binding(&'static str, String),
}

/// The widest key, which every key is right-aligned to.
fn key_width() -> usize {
    use crate::text::width;
    SECTIONS
        .iter()
        .flat_map(|s| s.bindings.iter())
        .map(|(k, _)| width(k))
        .max()
        .unwrap_or(0)
}

/// The popup in `area`, and its two columns' lines, each description
/// wrapped to its column. A column breaks between sections or bindings,
/// never inside one.
fn layout(area: Rect) -> (Rect, Vec<HelpLine>, Vec<HelpLine>) {
    let w = 104u16.min(area.width.saturating_sub(4));
    let col_width = w.saturating_sub(2) / 2;
    // A cell of margin, the key, two cells, the description, a cell spare.
    let desc_w = (col_width as usize).saturating_sub(key_width() + 4).max(8);
    let mut blocks: Vec<Vec<HelpLine>> = Vec::new();
    for (si, section) in SECTIONS.iter().enumerate() {
        let mut first = Vec::new();
        if si > 0 {
            first.push(HelpLine::Blank);
        }
        first.push(HelpLine::Title(section.title));
        blocks.push(first);
        for (key, desc) in section.bindings {
            let block = crate::text::wrap(desc, desc_w)
                .into_iter()
                .enumerate()
                .map(|(i, part)| HelpLine::Binding(if i == 0 { key } else { "" }, part))
                .collect();
            blocks.push(block);
        }
    }
    let total: usize = blocks.iter().map(Vec::len).sum();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for block in blocks {
        // A section's title stays with its first binding.
        let titled = matches!(left.last(), Some(HelpLine::Title(_)));
        if right.is_empty() && (left.len() < total.div_ceil(2) || titled) {
            left.extend(block);
        } else {
            right.extend(block);
        }
    }
    let rows = left.len().max(right.len());
    let h = (rows as u16 + 2).min(area.height.saturating_sub(2));
    (crate::shell::centred(area, w, h), left, right)
}

/// How far the help scrolls in `area` before its end is on screen.
pub fn max_scroll(area: Rect) -> usize {
    let (popup, left, right) = layout(area);
    left.len()
        .max(right.len())
        .saturating_sub(popup.height.saturating_sub(2) as usize)
}

impl Widget for HelpModalOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        use crate::text::fit_pad_right;
        let (popup, left, right) = layout(area);
        crate::shell::clear(buf, popup);
        let block = crate::shell::frame("keys", self.theme);
        let inner = block.inner(popup);
        block.render(popup, buf);
        let col_width = inner.width / 2;
        let key_w = key_width();
        let line = |l: &HelpLine| match l {
            HelpLine::Blank => Line::default(),
            HelpLine::Title(t) => Line::from(Span::styled(
                format!(" {}", t.to_lowercase()),
                self.theme.heading,
            )),
            HelpLine::Binding(key, desc) => Line::from(vec![
                Span::styled(
                    format!(" {}", fit_pad_right(key, key_w)),
                    self.theme.hint_key.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {desc}"), self.theme.hint_desc),
            ]),
        };
        // The two columns, scrolled together.
        let rows = inner.height as usize;
        for (x, column) in [(inner.x, &left), (inner.x + col_width, &right)] {
            for (row, l) in column.iter().skip(self.scroll).take(rows).enumerate() {
                let r = Rect::new(x, inner.y + row as u16, col_width, 1);
                Paragraph::new(line(l)).render(r, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_help_can_be_scrolled_into_view() {
        let short = Rect::new(0, 0, 100, 20);
        let (popup, left, right) = layout(short);
        let shown = popup.height as usize - 2;
        let lines = left.len().max(right.len());
        assert!(shown < lines);
        assert_eq!(max_scroll(short) + shown, lines);
        // Tall enough, nothing scrolls.
        assert_eq!(max_scroll(Rect::new(0, 0, 100, 200)), 0);
    }

    #[test]
    fn descriptions_wrap_rather_than_cut_and_the_frame_stays_square() {
        let theme = Theme::default();
        for (w, h) in [(120u16, 40u16), (80, 24), (50, 20), (200, 60)] {
            let area = Rect::new(0, 0, w, h);
            let max = max_scroll(area);
            let mut seen = String::new();
            for scroll in 0..=max {
                let mut buf = Buffer::empty(area);
                HelpModalOverlay::new(&theme, scroll).render(area, &mut buf);
                let (popup, _, _) = layout(area);
                let right = popup.x + popup.width - 1;
                for y in popup.y + 1..popup.y + popup.height - 1 {
                    assert_eq!(buf[(right, y)].symbol(), "\u{2502}", "{w}x{h} row {y}");
                    let row: String = (popup.x..right).map(|x| buf[(x, y)].symbol()).collect();
                    seen.push_str(&row);
                    seen.push(' ');
                }
            }
            if w < 80 {
                continue;
            }
            // Every word of every description is there whole.
            for (_, desc) in SECTIONS.iter().flat_map(|s| s.bindings.iter()) {
                for word in desc.split_whitespace() {
                    assert!(seen.contains(word), "{w}x{h}: {word:?} of {desc:?} missing");
                }
            }
        }
    }
}

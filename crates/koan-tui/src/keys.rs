use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::app::Mode;
use super::nav::Page;
use super::theme::Theme;

pub struct HintBar<'a> {
    mode: &'a Mode,
    page: Page,
    filterable: bool,
    theme: &'a Theme,
}

impl<'a> HintBar<'a> {
    pub fn new(mode: &'a Mode, page: Page, filterable: bool, theme: &'a Theme) -> Self {
        Self {
            mode,
            page,
            filterable,
            theme,
        }
    }
}

/// The keys the hint bar shows for a mode and a page, with what each does.
/// `filterable` is whether the page's filters panel opens.
pub fn hints(mode: &Mode, page: Page, filterable: bool) -> Vec<(&'static str, &'static str)> {
    {
        match mode {
            Mode::Normal => match page {
                Page::Queue => vec![
                    ("space", "pick"),
                    ("ret", "play"),
                    ("c", "pause"),
                    ("p", "find"),
                    ("m", "actions"),
                    ("J/K", "reorder"),
                    ("d", "remove"),
                    ("t", "view"),
                    ("?", "help"),
                ],
                Page::NowPlaying => vec![
                    ("space", "pause"),
                    ("<>", "skip"),
                    ("\u{2190}\u{2192}", "seek"),
                    ("E", "eq"),
                    ("B", "bypass eq"),
                    ("M", "visualiser"),
                    ("F", "fullscreen"),
                    ("0-9", "pages"),
                    ("esc", "back"),
                    ("?", "help"),
                ],
                Page::Album(_) => vec![
                    ("\u{2191}\u{2193}", "move"),
                    ("ret", "play from here"),
                    ("P", "play"),
                    ("S", "shuffle"),
                    ("a", "add"),
                    ("N", "next"),
                    ("m", "actions"),
                    ("f", "favourite"),
                    ("H", "fav album"),
                    ("i", "info"),
                    ("A", "artist"),
                    ("esc", "back"),
                ],
                Page::Albums | Page::Artists | Page::Tracks | Page::Artist(_) => {
                    let mut hints = vec![
                        ("\u{2191}\u{2193}\u{2190}\u{2192}", "move"),
                        ("ret", if page == Page::Tracks { "play" } else { "open" }),
                        ("P", "play"),
                        ("a", "add"),
                        ("N", "next"),
                        ("f", "favourite"),
                        ("m", "actions"),
                        ("w", "playlist"),
                        ("A", "artist"),
                        ("/", "filter"),
                    ];
                    if filterable {
                        hints.push(("C-f", "filters"));
                    }
                    if matches!(page, Page::Albums | Page::Tracks) {
                        hints.push(("o", "sort"));
                    }
                    if matches!(page, Page::Albums | Page::Artist(_)) {
                        hints.push(("v", "view"));
                    }
                    hints.extend([("0-9", "pages"), ("esc", "back")]);
                    hints
                }
                Page::Favourites | Page::Recent | Page::Downloaded => vec![
                    ("\u{2191}\u{2193}\u{2190}\u{2192}", "move"),
                    ("ret", "open/play"),
                    ("P", "play"),
                    ("a", "add"),
                    ("N", "next"),
                    ("f", "favourite"),
                    ("m", "actions"),
                    ("v", "view"),
                    ("0-9", "pages"),
                    ("esc", "back"),
                ],
                Page::Search => vec![
                    ("type", "search"),
                    ("ret", "results/open"),
                    ("\u{2191}\u{2193}\u{2190}\u{2192}", "move"),
                    ("P", "play"),
                    ("a", "add"),
                    ("m", "actions"),
                    ("v", "view"),
                    ("esc", "clear/back"),
                ],
                Page::History => vec![
                    ("\u{2191}\u{2193}", "move"),
                    ("ret", "play"),
                    ("a", "add"),
                    ("N", "next"),
                    ("space", "pick"),
                    ("S-\u{2191}\u{2193}", "pick run"),
                    ("m", "actions"),
                    ("x", "forget"),
                    ("i", "info"),
                    ("D", "clear"),
                    ("A", "artist"),
                    ("O", "album"),
                    ("/", "filter"),
                    ("E", "eq"),
                    ("0-9", "pages"),
                    ("esc", "back"),
                ],
                Page::Playlists => vec![
                    ("\u{2191}\u{2193}", "move"),
                    ("ret", "open"),
                    ("P", "play"),
                    ("a", "add"),
                    ("w", "playlist"),
                    ("c", "new"),
                    ("r", "rename"),
                    ("x", "delete"),
                    ("m", "actions"),
                    ("/", "filter"),
                    ("E", "eq"),
                    ("0-9", "pages"),
                    ("esc", "back"),
                ],
                Page::Playlist(_) => vec![
                    ("\u{2191}\u{2193}", "move"),
                    ("ret", "play from here"),
                    ("P", "play all"),
                    ("K/J", "move up/down"),
                    ("x", "remove"),
                    ("C-z/y", "undo/redo"),
                    ("m", "actions"),
                    ("t", "view"),
                    ("w", "playlist"),
                    ("esc", "back"),
                ],
                Page::Downloads => {
                    vec![("0-9", "pages"), ("esc", "back"), ("?", "help")]
                }
                Page::Settings => vec![
                    ("\u{2191}\u{2193}", "move"),
                    ("ret", "open"),
                    ("\u{2190}\u{2192}", "change"),
                    ("m", "actions"),
                    ("x", "remove"),
                    ("E", "eq"),
                    ("0-9", "pages"),
                    ("esc", "back"),
                ],
            },
            Mode::TrackInfo(_) | Mode::TrackDetails => vec![("esc", "close"), ("i", "close")],
            Mode::CoverArtZoom => vec![("esc", "close"), ("z", "close")],
            Mode::NamePrompt => vec![("ret", "keep"), ("esc", "cancel")],
            Mode::FilterPanel => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "change"),
                ("esc", "close"),
            ],
            Mode::ActionsMenu => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "do it"),
                ("esc", "close"),
            ],
            Mode::PlaylistChooser => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "add here"),
                ("esc", "cancel"),
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
            Mode::HelpModal => vec![
                ("\u{2191}\u{2193}", "scroll"),
                ("esc", "close"),
                ("?", "close"),
            ],
            Mode::VizPicker => vec![
                ("\u{2191}\u{2193}", "preview"),
                ("ret", "select"),
                ("esc", "cancel"),
            ],
            Mode::DspSelector => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "use"),
                ("esc", "close"),
            ],
            Mode::DeviceSelector => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "select"),
                ("esc", "cancel"),
            ],
            Mode::PlayOn => vec![
                ("\u{2191}\u{2193}", "navigate"),
                ("ret", "control"),
                ("m", "move music here"),
                ("x", "forget"),
                ("esc", "close"),
            ],
            Mode::Picker(_) => vec![
                ("\u{2191}\u{2193}", "move"),
                ("\u{2190}\u{2192}", "kind"),
                ("tab", "pick"),
                ("ret", "add"),
                ("C-ret", "add & play"),
                ("C-r", "replace"),
                ("esc", "cancel"),
            ],
        }
    }
}

/// The key press a hint's label stands for, when it names exactly one.
pub fn label_event(label: &str) -> Option<KeyEvent> {
    let plain = |code| Some(KeyEvent::new(code, KeyModifiers::NONE));
    match label {
        "space" => plain(KeyCode::Char(' ')),
        "ret" => plain(KeyCode::Enter),
        "esc" => plain(KeyCode::Esc),
        "tab" => plain(KeyCode::Tab),
        "C-r" => Some(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        "C-z/y" | "C-a" => None,
        l if l.chars().count() == 1 => plain(KeyCode::Char(l.chars().next()?)),
        _ => None,
    }
}

/// Each hint's cells and the key it stands for, laid out as `render` draws
/// them, so a click on a hint is the key it names.
pub fn hits(mode: &Mode, page: Page, filterable: bool, area: Rect) -> Vec<(Rect, KeyEvent)> {
    let mut x = area.x;
    let mut out = Vec::new();
    for (i, (key, desc)) in fitting(mode, page, filterable, area.width)
        .iter()
        .enumerate()
    {
        if i > 0 {
            x += 2;
        }
        let w = (key.chars().count() + 2 + 1 + desc.chars().count()) as u16;
        if let Some(event) = label_event(key) {
            out.push((Rect::new(x, area.y, w, 1), event));
        }
        x += w;
    }
    out
}

/// The hints that fit `width` whole: one that would be cut is left out, so
/// a bar never ends mid-word.
fn fitting(
    mode: &Mode,
    page: Page,
    filterable: bool,
    width: u16,
) -> Vec<(&'static str, &'static str)> {
    let mut used = 0usize;
    let mut out = Vec::new();
    for (key, desc) in hints(mode, page, filterable) {
        let w =
            key.chars().count() + 2 + 1 + desc.chars().count() + if out.is_empty() { 0 } else { 2 };
        if used + w > width as usize {
            break;
        }
        used += w;
        out.push((key, desc));
    }
    out
}

impl Widget for HintBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let hints = fitting(self.mode, self.page, self.filterable, area.width);
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

use ratatui::style::{Color, Modifier, Style};

use super::picker::PickerPartKind;

/// The kōan theme's colours (`docs/design/koan-theme.md`). Every style below
/// is made from these, so a screen cannot reach for a colour of its own.
#[derive(Clone, Copy)]
pub struct Tokens {
    /// Body text.
    pub ink: Color,
    /// Titles and the playing track's name.
    pub strong: Color,
    /// Secondary text and unselected navigation.
    pub muted: Color,
    /// Hairlines.
    pub rule: Color,
    /// A raised row: the cursor's.
    pub surface: Color,
    /// Selection, progress, the playing row. The cover's, while one plays.
    pub accent: Color,
    /// Errors and destructive actions.
    pub bad: Color,
}

/// The accent when nothing is playing, or the cover has no colour to give.
pub const MINT: (u8, u8, u8) = (0x7d, 0xd3, 0xa7);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Palette {
    /// The kōan theme, in 24-bit colour where the terminal has it.
    Koan { truecolor: bool },
    /// The terminal's own sixteen colours.
    Ansi,
}

impl Palette {
    /// `name` is `playback.tui_theme`.
    pub fn detect(name: &str) -> Self {
        if name.eq_ignore_ascii_case("ansi") {
            return Self::Ansi;
        }
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let truecolor = colorterm.contains("truecolor")
            || colorterm.contains("24bit")
            || std::env::var("TERM_PROGRAM").is_ok_and(|t| {
                matches!(t.as_str(), "iTerm.app" | "WezTerm" | "ghostty" | "vscode")
            })
            || std::env::var("TERM").is_ok_and(|t| t.contains("kitty") || t.contains("ghostty"));
        Self::Koan { truecolor }
    }
}

pub struct Theme {
    pub palette: Palette,
    pub tokens: Tokens,
    pub warning: Color,
    pub album_header_artist: Style,
    pub album_header_album: Style,
    pub track_playing: Style,
    pub track_normal: Style,
    pub track_cursor: Style,
    pub track_selected: Style,
    pub track_number: Style,
    pub picker_cursor: Style,
    pub hint_key: Style,
    pub hint_desc: Style,
    pub progress_filled: Style,
    pub progress_empty: Style,
    pub status_playing: Style,
    pub status_paused: Style,
    pub status_stopped: Style,
    pub spinner: Style,
    pub failed: Style,
    pub library_artist: Style,
    pub library_album: Style,
    pub library_track: Style,
    pub library_cursor: Style,
    pub track_hover: Style,
    pub library_hover: Style,
    pub favourite: Style,
    pub spectrum_low: Style,
    pub spectrum_mid: Style,
    pub spectrum_high: Style,
    pub spectrum_peak: Style,
    pub scrollbar_thumb: Color,
    pub scrollbar_bg: Color,
    /// A page's title.
    pub heading: Style,
    /// The hairline after it.
    pub rule: Style,
    /// The bar on a selected row's leading edge.
    pub bar: Style,
}

/// The nearest of the 256-colour palette's colour cube and grey ramp.
fn indexed(r: u8, g: u8, b: u8) -> Color {
    let level = |v: u8| ((v as u16 * 5 + 127) / 255) as u8;
    let cube = 16 + 36 * level(r) + 6 * level(g) + level(b);
    let grey = (((r as u16 + g as u16 + b as u16) / 3).saturating_sub(8) / 10).min(23) as u8;
    let is_grey = r.abs_diff(g) < 10 && g.abs_diff(b) < 10;
    Color::Indexed(if is_grey { 232 + grey } else { cube })
}

impl Theme {
    pub fn new(palette: Palette, accent: (u8, u8, u8)) -> Self {
        match palette {
            Palette::Ansi => Self::ansi(),
            Palette::Koan { truecolor } => Self::koan(truecolor, accent),
        }
    }

    fn koan(truecolor: bool, accent: (u8, u8, u8)) -> Self {
        let c = |r, g, b| {
            if truecolor {
                Color::Rgb(r, g, b)
            } else {
                indexed(r, g, b)
            }
        };
        let t = Tokens {
            ink: c(0xcc, 0xcc, 0xcc),
            strong: c(0xff, 0xff, 0xff),
            muted: c(0x91, 0x91, 0x91),
            rule: c(0x38, 0x38, 0x38),
            surface: c(0x2a, 0x2a, 0x2a),
            accent: c(accent.0, accent.1, accent.2),
            bad: c(0xef, 0x6b, 0x73),
        };
        let bold = Modifier::BOLD;
        Self {
            palette: Palette::Koan { truecolor },
            tokens: t,
            warning: c(0xe5, 0xc0, 0x7b),
            album_header_artist: Style::new().fg(t.strong).add_modifier(bold),
            album_header_album: Style::new().fg(t.ink),
            track_playing: Style::new().fg(t.accent),
            track_normal: Style::new().fg(t.ink),
            track_cursor: Style::new().fg(t.accent).add_modifier(bold),
            track_selected: Style::new().fg(t.accent),
            track_number: Style::new().fg(t.muted),
            picker_cursor: Style::new().fg(t.accent).bg(t.surface).add_modifier(bold),
            hint_key: Style::new().fg(t.ink),
            hint_desc: Style::new().fg(t.muted),
            progress_filled: Style::new().fg(t.accent),
            progress_empty: Style::new().fg(t.rule),
            status_playing: Style::new().fg(t.accent),
            status_paused: Style::new().fg(c(0xe5, 0xc0, 0x7b)),
            status_stopped: Style::new().fg(t.muted),
            spinner: Style::new().fg(t.accent),
            failed: Style::new().fg(t.bad),
            library_artist: Style::new().fg(t.strong).add_modifier(bold),
            library_album: Style::new().fg(t.ink),
            library_track: Style::new().fg(t.ink),
            library_cursor: Style::new().fg(t.accent).bg(t.surface).add_modifier(bold),
            track_hover: Style::new().fg(t.strong).add_modifier(Modifier::UNDERLINED),
            library_hover: Style::new().fg(t.strong).add_modifier(Modifier::UNDERLINED),
            favourite: Style::new().fg(t.accent),
            spectrum_low: Style::new().fg(t.muted),
            spectrum_mid: Style::new().fg(t.accent),
            spectrum_high: Style::new().fg(t.strong),
            spectrum_peak: Style::new().fg(t.strong),
            scrollbar_thumb: t.muted,
            scrollbar_bg: t.rule,
            heading: Style::new().fg(t.strong),
            rule: Style::new().fg(t.rule),
            bar: Style::new().fg(t.accent),
        }
    }

    fn ansi() -> Self {
        let t = Tokens {
            ink: Color::Reset,
            strong: Color::White,
            muted: Color::DarkGray,
            rule: Color::DarkGray,
            surface: Color::Reset,
            accent: Color::Cyan,
            bad: Color::Red,
        };
        Self {
            palette: Palette::Ansi,
            tokens: t,
            warning: Color::Yellow,
            album_header_artist: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            album_header_album: Style::new().fg(Color::Green),
            track_playing: Style::new().fg(Color::Cyan),
            track_normal: Style::new(),
            track_cursor: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            track_selected: Style::new().fg(Color::Blue),
            track_number: Style::new().fg(Color::DarkGray),
            picker_cursor: Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            hint_key: Style::new().add_modifier(Modifier::BOLD),
            hint_desc: Style::new().fg(Color::DarkGray),
            progress_filled: Style::new().fg(Color::Cyan),
            progress_empty: Style::new().fg(Color::DarkGray),
            status_playing: Style::new().fg(Color::Cyan),
            status_paused: Style::new().fg(Color::Yellow),
            status_stopped: Style::new().fg(Color::DarkGray),
            spinner: Style::new().fg(Color::Cyan),
            failed: Style::new().fg(Color::Red),
            library_artist: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            library_album: Style::new().fg(Color::Green),
            library_track: Style::new(),
            library_cursor: Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            track_hover: Style::new().add_modifier(Modifier::UNDERLINED),
            library_hover: Style::new().add_modifier(Modifier::UNDERLINED),
            favourite: Style::new().fg(Color::Yellow),
            spectrum_low: Style::new().fg(Color::Green),
            spectrum_mid: Style::new().fg(Color::Yellow),
            spectrum_high: Style::new().fg(Color::Red),
            spectrum_peak: Style::new().fg(Color::White),
            scrollbar_thumb: Color::White,
            scrollbar_bg: Color::Rgb(0x1c, 0x1c, 0x1e),
            heading: Style::new().add_modifier(Modifier::BOLD),
            rule: Style::new().fg(Color::DarkGray),
            bar: Style::new().fg(Color::Cyan),
        }
    }

    pub fn picker_part_style(&self, kind: PickerPartKind) -> Style {
        let t = &self.tokens;
        let ansi = self.palette == Palette::Ansi;
        match kind {
            PickerPartKind::Artist if ansi => Style::new().fg(Color::Cyan),
            PickerPartKind::Album if ansi => Style::new().fg(Color::Green),
            PickerPartKind::Date if ansi => Style::new().fg(Color::Yellow),
            PickerPartKind::Artist => Style::new().fg(t.strong),
            PickerPartKind::Album => Style::new().fg(t.accent),
            PickerPartKind::Title | PickerPartKind::Plain => Style::new(),
            PickerPartKind::Date
            | PickerPartKind::TrackNum
            | PickerPartKind::Duration
            | PickerPartKind::Separator
            | PickerPartKind::Codec => Style::new().fg(t.muted),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::koan(true, MINT)
    }
}

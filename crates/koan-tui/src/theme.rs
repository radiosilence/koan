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
    /// The accent where it is text: `ink` when the cover's does not reach
    /// 4.5:1 on the grounds.
    pub accent_text: Color,
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

/// The nearest of the 256-colour palette's colour cube and grey ramp. The
/// cube's levels are xterm's, which are not evenly spaced.
fn indexed(r: u8, g: u8, b: u8) -> Color {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| (0..6).min_by_key(|&i| LEVELS[i].abs_diff(v)).unwrap_or(0);
    let distance = |(x, y, z): (u8, u8, u8)| {
        [(x, r), (y, g), (z, b)]
            .iter()
            .map(|&(a, b)| u32::from(a.abs_diff(b)).pow(2))
            .sum::<u32>()
    };
    let (lr, lg, lb) = (level(r), level(g), level(b));
    let cube = (LEVELS[lr], LEVELS[lg], LEVELS[lb]);
    let mean = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let step = (mean.saturating_sub(3) / 10).min(23) as u8;
    let grey = 8 + 10 * step;
    if distance((grey, grey, grey)) < distance(cube) {
        Color::Indexed(232 + step)
    } else {
        Color::Indexed(16 + 36 * lr as u8 + 6 * lg as u8 + lb as u8)
    }
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
        let ink = c(0xcc, 0xcc, 0xcc);
        let t = Tokens {
            ink,
            strong: c(0xff, 0xff, 0xff),
            muted: c(0x91, 0x91, 0x91),
            rule: c(0x38, 0x38, 0x38),
            surface: c(0x2a, 0x2a, 0x2a),
            accent: c(accent.0, accent.1, accent.2),
            accent_text: if super::accent::reads_as_text(accent) {
                c(accent.0, accent.1, accent.2)
            } else {
                ink
            },
            bad: c(0xef, 0x6b, 0x73),
        };
        let bold = Modifier::BOLD;
        let warning = c(0xe5, 0xc0, 0x7b);
        Self {
            palette: Palette::Koan { truecolor },
            tokens: t,
            warning,
            album_header_artist: Style::new().fg(t.strong).add_modifier(bold),
            album_header_album: Style::new().fg(t.ink),
            track_playing: Style::new().fg(t.accent_text),
            track_normal: Style::new().fg(t.ink),
            track_cursor: Style::new().fg(t.accent_text).add_modifier(bold),
            track_selected: Style::new().fg(t.accent_text),
            track_number: Style::new().fg(t.muted),
            picker_cursor: Style::new()
                .fg(t.accent_text)
                .bg(t.surface)
                .add_modifier(bold),
            hint_key: Style::new().fg(t.ink),
            hint_desc: Style::new().fg(t.muted),
            progress_filled: Style::new().fg(t.accent),
            progress_empty: Style::new().fg(t.rule),
            status_playing: Style::new().fg(t.accent_text),
            status_paused: Style::new().fg(warning),
            status_stopped: Style::new().fg(t.muted),
            spinner: Style::new().fg(t.accent),
            failed: Style::new().fg(t.bad),
            library_artist: Style::new().fg(t.strong).add_modifier(bold),
            library_album: Style::new().fg(t.ink),
            library_track: Style::new().fg(t.ink),
            library_cursor: Style::new()
                .fg(t.accent_text)
                .bg(t.surface)
                .add_modifier(bold),
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
            accent_text: Color::Cyan,
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
            scrollbar_bg: Color::DarkGray,
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
            PickerPartKind::Album => Style::new().fg(t.accent_text),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_256_colour_is_on_xterm_levels() {
        // muted, #919191: the grey ramp's 145.
        assert_eq!(indexed(0x91, 0x91, 0x91), Color::Indexed(232 + 14));
        // 135 is a cube level; an even spacing would pick 153.
        assert_eq!(indexed(135, 0, 0), Color::Indexed(16 + 36 * 2));
        // Mint: 125/211/167 is nearest 135/215/175.
        assert_eq!(
            indexed(0x7d, 0xd3, 0xa7),
            Color::Indexed(16 + 36 * 2 + 6 * 4 + 3)
        );
        assert_eq!(indexed(0, 0, 0), Color::Indexed(16));
        assert_eq!(indexed(255, 255, 255), Color::Indexed(231));
    }

    #[test]
    fn an_accent_that_fails_as_text_leaves_text_in_ink() {
        let t = Theme::new(Palette::Koan { truecolor: true }, (40, 60, 200));
        assert_eq!(t.tokens.accent, Color::Rgb(40, 60, 200));
        assert_eq!(t.track_playing.fg, Some(t.tokens.ink));
        assert_eq!(t.progress_filled.fg, Some(t.tokens.accent));
        let mint = Theme::new(Palette::Koan { truecolor: true }, MINT);
        assert_eq!(mint.track_playing.fg, Some(mint.tokens.accent));
    }

    #[test]
    fn the_ansi_palette_uses_only_the_terminal_colours() {
        let t = Theme::new(Palette::Ansi, MINT);
        let rgb = |c: Color| matches!(c, Color::Rgb(..) | Color::Indexed(_));
        assert!(!rgb(t.scrollbar_bg) && !rgb(t.scrollbar_thumb) && !rgb(t.tokens.accent));
    }
}

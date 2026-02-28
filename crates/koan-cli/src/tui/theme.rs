use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
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
}

impl Default for Theme {
    fn default() -> Self {
        Self {
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
        }
    }
}

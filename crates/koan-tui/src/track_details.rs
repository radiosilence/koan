//! What the library knows of a track, for any list to show: the same fields
//! as the queue's track info, from the track's row rather than the queue's
//! entry, with where the file is as well.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use koan_core::db::queries::TrackRow;

use super::theme::Theme;
use super::transport::format_time;

/// Where the popup sits in `area`, for drawing and for a click outside it.
pub fn popup_rect(area: Rect) -> Rect {
    let w = (area.width * 7 / 10).max(40).min(area.width);
    let h = (area.height * 7 / 10).max(14).min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

/// The label and value of each line shown for `t`.
fn fields(t: &TrackRow) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("Title", t.title.clone()),
        ("Artist", t.artist_name.clone()),
    ];
    if !t.album_artist_name.is_empty() && t.album_artist_name != t.artist_name {
        out.push(("Album Artist", t.album_artist_name.clone()));
    }
    out.push(("Album", t.album_title.clone()));
    if let Some(n) = t.track_number {
        out.push(("Track #", n.to_string()));
    }
    if let Some(d) = t.disc {
        out.push(("Disc", d.to_string()));
    }
    if let Some(ms) = t.duration_ms {
        out.push(("Duration", format_time(ms as u64)));
    }
    if let Some(g) = t.genre.as_ref().filter(|g| !g.is_empty()) {
        out.push(("Genre", g.clone()));
    }
    if let Some(c) = &t.codec {
        out.push(("Codec", c.clone()));
    }
    if let Some(r) = t.sample_rate {
        out.push(("Sample Rate", format!("{r} Hz")));
    }
    if let Some(b) = t.bit_depth {
        out.push(("Bit Depth", b.to_string()));
    }
    if let Some(b) = t.bitrate {
        out.push(("Bitrate", format!("{b} kbps")));
    }
    if let Some(c) = t.channels {
        out.push(("Channels", c.to_string()));
    }
    out.push((
        "Source",
        match t.source.as_str() {
            "remote" => "server".to_string(),
            "local" => "file".to_string(),
            other => other.to_string(),
        },
    ));
    if let Some(p) = &t.path {
        out.push(("Path", p.clone()));
    }
    if let Some(p) = &t.cached_path {
        out.push(("Downloaded to", p.clone()));
    }
    out
}

pub struct TrackDetailsOverlay<'a> {
    track: &'a TrackRow,
    theme: &'a Theme,
}

impl<'a> TrackDetailsOverlay<'a> {
    pub fn new(track: &'a TrackRow, theme: &'a Theme) -> Self {
        Self { track, theme }
    }
}

impl Widget for TrackDetailsOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = popup_rect(area);
        Clear.render(popup, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(
                " track info ",
                self.theme.album_header_artist.add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(popup);
        block.render(popup, buf);
        if inner.height < 2 {
            return;
        }
        let key = self.theme.hint_key.add_modifier(Modifier::BOLD);
        let lines: Vec<Line> = fields(self.track)
            .into_iter()
            .map(|(label, value)| {
                Line::from(vec![
                    Span::styled(format!(" {label:<14}"), key),
                    Span::styled(value, self.theme.track_normal),
                ])
            })
            .collect();
        let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(body, buf);
        let hint = Line::from(vec![
            Span::styled(" [esc]", key),
            Span::styled(" close", self.theme.hint_desc),
        ]);
        Paragraph::new(hint).render(
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            buf,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> TrackRow {
        TrackRow {
            id: 1,
            album_id: Some(1),
            artist_id: Some(2),
            artist_name: "artist".into(),
            album_artist_name: "artist".into(),
            album_title: "record".into(),
            disc: Some(1),
            track_number: Some(3),
            title: "song".into(),
            duration_ms: Some(200_000),
            path: Some("/music/song.flac".into()),
            codec: Some("FLAC".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: None,
            genre: Some("rock".into()),
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        }
    }

    #[test]
    fn what_the_library_knows_is_listed() {
        let shown: Vec<_> = fields(&track()).into_iter().map(|(l, _)| l).collect();
        for want in ["Title", "Album", "Genre", "Sample Rate", "Source", "Path"] {
            assert!(shown.contains(&want), "{want}");
        }
        assert!(!shown.contains(&"Album Artist"), "same as the artist");
        assert!(!shown.contains(&"Downloaded to"), "nothing downloaded");
    }

    #[test]
    fn it_draws_at_any_size() {
        let t = track();
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        for w in 0..80 {
            for h in 0..24 {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                TrackDetailsOverlay::new(&t, &theme).render(area, &mut buf);
            }
        }
    }
}

//! Track info: what is known of a track, as labelled lines in a popup. A queue
//! entry shows its status and, while it plays, what the decoder reports; a
//! track picked from any other list shows its library row, with where the
//! file is as well.

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use koan_core::db::queries::TrackRow;
use koan_core::player::state::{QueueEntry, QueueEntryStatus, TrackInfo};

use super::cover_art::CoverArt;
use super::theme::Theme;
use super::transport::format_time;

type Fields = Vec<(&'static str, String)>;

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

/// What the queue knows of `entry`, with the decoder's report while it plays.
pub fn entry_fields(entry: &QueueEntry, playing: Option<&TrackInfo>) -> Fields {
    let mut out = vec![
        ("Title", entry.title.clone()),
        ("Artist", entry.artist.clone()),
    ];
    if !entry.album_artist.is_empty() && entry.album_artist != entry.artist {
        out.push(("Album Artist", entry.album_artist.clone()));
    }
    out.push(("Album", entry.album.clone()));
    if let Some(year) = &entry.year {
        out.push(("Year", year.clone()));
    }
    if let Some(n) = entry.track_number {
        out.push(("Track #", n.to_string()));
    }
    if let Some(d) = entry.disc {
        out.push(("Disc", d.to_string()));
    }
    if let Some(ms) = entry.duration_ms {
        out.push(("Duration", format_time(ms)));
    }
    if let Some(c) = &entry.codec {
        out.push(("Codec", c.clone()));
    }
    if let Some(info) = playing {
        out.push(("Sample Rate", format!("{} Hz", info.sample_rate)));
        if let Some(b) = info.bit_depth {
            out.push(("Bit Depth", b.to_string()));
        }
        if let Some(kbps) = info.bitrate_kbps {
            out.push(("Bitrate", format!("{kbps} kbps")));
        }
        out.push(("Channels", info.channels.to_string()));
    }
    let status = match entry.status {
        QueueEntryStatus::Queued => "Queued",
        QueueEntryStatus::Playing => "Playing",
        QueueEntryStatus::Played => "Played",
        QueueEntryStatus::Downloading => "Downloading",
        QueueEntryStatus::PriorityPending => "Up Next",
        QueueEntryStatus::Failed => "Failed",
    };
    out.push(("Status", status.into()));
    out.push(("Path", entry.path.display().to_string()));
    out
}

/// What the library knows of `t`.
pub fn row_fields(t: &TrackRow) -> Fields {
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

pub struct TrackInfoOverlay<'a> {
    fields: Fields,
    /// The cover, and the squash it is drawn with.
    cover: Option<(&'a DynamicImage, f32)>,
    theme: &'a Theme,
}

impl<'a> TrackInfoOverlay<'a> {
    pub fn new(fields: Fields, cover: Option<(&'a DynamicImage, f32)>, theme: &'a Theme) -> Self {
        Self {
            fields,
            cover,
            theme,
        }
    }
}

impl Widget for TrackInfoOverlay<'_> {
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

        // The cover takes a square on the left: half blocks give two pixels a
        // cell vertically, roughly a cell's 2:1 shape.
        let text = match self.cover {
            Some((img, squash)) if inner.width > 30 => {
                let side = inner.height.min(inner.width / 3);
                CoverArt::new(img)
                    .shaped(squash)
                    .render(Rect::new(inner.x + 1, inner.y, side, side), buf);
                Rect::new(
                    inner.x + side + 2,
                    inner.y,
                    inner.width.saturating_sub(side + 3),
                    inner.height,
                )
            }
            _ => inner,
        };

        let key = self.theme.hint_key.add_modifier(Modifier::BOLD);
        let lines: Vec<Line> = self
            .fields
            .into_iter()
            .map(|(label, value)| {
                Line::from(vec![
                    Span::styled(format!(" {label:<14}"), key),
                    Span::styled(value, self.theme.track_normal),
                ])
            })
            .collect();
        let body = Rect::new(text.x, text.y, text.width, text.height - 1);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(body, buf);
        let hint = Line::from(vec![
            Span::styled(" [esc]", key),
            Span::styled(" close", self.theme.hint_desc),
        ]);
        Paragraph::new(hint).render(
            Rect::new(text.x, text.y + text.height - 1, text.width, 1),
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
        let shown: Vec<_> = row_fields(&track()).into_iter().map(|(l, _)| l).collect();
        for want in ["Title", "Album", "Genre", "Sample Rate", "Source", "Path"] {
            assert!(shown.contains(&want), "{want}");
        }
        assert!(!shown.contains(&"Album Artist"), "same as the artist");
        assert!(!shown.contains(&"Downloaded to"), "nothing downloaded");
    }

    #[test]
    fn it_draws_at_any_size() {
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        let cover = DynamicImage::new_rgb8(8, 8);
        for w in 0..80 {
            for h in 0..24 {
                let area = Rect::new(0, 0, w, h);
                for with_cover in [None, Some((&cover, 1.0))] {
                    let mut buf = Buffer::empty(area);
                    TrackInfoOverlay::new(row_fields(&track()), with_cover, &theme)
                        .render(area, &mut buf);
                }
            }
        }
    }
}

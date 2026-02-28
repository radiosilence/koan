use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use koan_core::player::state::{QueueEntry, QueueEntryStatus, TrackInfo};

use super::cover_art::CoverArt;
use super::theme::Theme;

pub struct TrackInfoOverlay<'a> {
    entry: &'a QueueEntry,
    track_info: Option<&'a TrackInfo>,
    cover_art: Option<&'a DynamicImage>,
    theme: &'a Theme,
}

impl<'a> TrackInfoOverlay<'a> {
    pub fn new(
        entry: &'a QueueEntry,
        track_info: Option<&'a TrackInfo>,
        cover_art: Option<&'a DynamicImage>,
        theme: &'a Theme,
    ) -> Self {
        Self {
            entry,
            track_info,
            cover_art,
            theme,
        }
    }
}

fn format_duration(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}:{:02}", mins, secs)
}

fn status_str(status: QueueEntryStatus) -> &'static str {
    match status {
        QueueEntryStatus::Queued => "Queued",
        QueueEntryStatus::Playing => "Playing",
        QueueEntryStatus::Played => "Played",
        QueueEntryStatus::Downloading => "Downloading",
        QueueEntryStatus::PriorityPending => "Up Next",
        QueueEntryStatus::Failed => "Failed",
    }
}

impl Widget for TrackInfoOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup_width = (area.width as f32 * 0.7).max(40.0).min(area.width as f32) as u16;
        let popup_height = (area.height as f32 * 0.7).max(14.0).min(area.height as f32) as u16;
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(vec![Span::styled(
                " track info ",
                self.theme.album_header_artist.add_modifier(Modifier::BOLD),
            )]));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        if inner.height < 3 {
            return;
        }

        // Layout: if we have cover art, put it on the left.
        let (text_area, art_area) = if self.cover_art.is_some() && inner.width > 30 {
            // Art takes a square-ish area on the left. Width ≈ height works
            // because halfblocks give 2 vertical pixels per cell, roughly
            // matching the ~2:1 cell aspect ratio.
            let art_size = inner.height.saturating_sub(1).min(inner.width / 3);
            let art_rect = Rect::new(inner.x + 1, inner.y, art_size, art_size);
            let text_rect = Rect::new(
                inner.x + art_size + 2,
                inner.y,
                inner.width.saturating_sub(art_size + 3),
                inner.height,
            );
            (text_rect, Some(art_rect))
        } else {
            (inner, None)
        };

        // Render cover art.
        if let (Some(img), Some(art_rect)) = (self.cover_art, art_area) {
            CoverArt::new(img).render(art_rect, buf);
        }

        // Render text fields.
        let key_style = self.theme.hint_key.add_modifier(Modifier::BOLD);
        let val_style = self.theme.track_normal;

        let mut lines: Vec<Line> = Vec::new();

        let mut field = |label: &str, value: &str| {
            lines.push(Line::from(vec![
                Span::styled(format!(" {:<14}", label), key_style),
                Span::styled(value.to_string(), val_style),
            ]));
        };

        field("Title", &self.entry.title);
        field("Artist", &self.entry.artist);

        if !self.entry.album_artist.is_empty() && self.entry.album_artist != self.entry.artist {
            field("Album Artist", &self.entry.album_artist);
        }

        field("Album", &self.entry.album);

        if let Some(ref year) = self.entry.year {
            field("Year", year);
        }

        if let Some(num) = self.entry.track_number {
            field("Track #", &num.to_string());
        }

        if let Some(disc) = self.entry.disc {
            field("Disc", &disc.to_string());
        }

        if let Some(ms) = self.entry.duration_ms {
            field("Duration", &format_duration(ms));
        }

        if let Some(ref codec) = self.entry.codec {
            field("Codec", codec);
        }

        // Audio details from TrackInfo (only for currently playing track).
        if let Some(info) = self.track_info {
            field("Sample Rate", &format!("{} Hz", info.sample_rate));
            field("Bit Depth", &info.bit_depth.to_string());
            field("Channels", &info.channels.to_string());
        }

        field("Status", status_str(self.entry.status));

        // Blank line before path.
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(format!(" {:<14}", "Path"), key_style),
            Span::styled(self.entry.path.display().to_string(), val_style),
        ]));

        // Hint bar at bottom.
        let hint_line = Line::from(vec![
            Span::styled(" [esc]", self.theme.hint_key.add_modifier(Modifier::BOLD)),
            Span::styled(" close", self.theme.hint_desc),
        ]);

        // Split text area into content + hint row.
        let content_height = text_area.height.saturating_sub(1);
        let content_area = Rect::new(text_area.x, text_area.y, text_area.width, content_height);
        let hint_area = Rect::new(
            text_area.x,
            text_area.y + content_height,
            text_area.width,
            1,
        );

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content_area, buf);

        Paragraph::new(hint_line).render(hint_area, buf);
    }
}

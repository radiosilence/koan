//! Track info: everything the library knows of a track, as each of its
//! sources says it — the file's tags, the server's entry, the ReplayGain in
//! the file — and its ids, as the apps' track info shows it. A queue entry
//! adds where it stands in the queue and, while it plays, what the decoder
//! reports.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::{Block, Borders, Widget};

use koan_core::db::queries::{self, TrackMeta};
use koan_core::player::state::{QueueEntry, QueueEntryStatus, TrackInfo};

use super::cover_art::{Cover, Gfx};
use super::text::{fit, width, wrap};
use super::theme::Theme;
use super::transport::format_time;

type Fields = Vec<(&'static str, String)>;
pub type Groups = Vec<(&'static str, Fields)>;

/// What is known of a track, read once when it is asked for.
#[derive(Debug, Clone, Default)]
pub struct Details {
    pub title: String,
    groups: Groups,
    /// The first line shown, for a list longer than the popup.
    pub scroll: usize,
}

impl Details {
    /// Reads the track's sources, its ids and the ReplayGain in its file.
    pub fn load(track_id: i64) -> Option<Self> {
        let db = koan_core::db::pool::shared().get().ok()?;
        let row = queries::get_track_row(&db.conn, track_id).ok()??;
        let mut groups: Groups = queries::sources_of_track(&db.conn, track_id)
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                let kind = if m.source == "remote" {
                    "server"
                } else {
                    "file"
                };
                (kind, source_fields(m))
            })
            .collect();
        let gain: Fields = row
            .path
            .as_deref()
            .and_then(|p| koan_core::audio::replaygain::read_tags(std::path::Path::new(p)).ok())
            .map(|rg| {
                [
                    (
                        "track gain",
                        rg.track_gain_db.map(|g| format!("{g:+.2} dB")),
                    ),
                    ("track peak", rg.track_peak.map(|p| format!("{p:.6}"))),
                    (
                        "album gain",
                        rg.album_gain_db.map(|g| format!("{g:+.2} dB")),
                    ),
                    ("album peak", rg.album_peak.map(|p| format!("{p:.6}"))),
                ]
                .into_iter()
                .filter_map(|(name, value)| value.map(|v| (name, v)))
                .collect()
            })
            .unwrap_or_default();
        if !gain.is_empty() {
            groups.push(("replaygain", gain));
        }
        let mut ids = vec![("track id", track_id.to_string())];
        if let Some(uid) = queries::uids_for(&db.conn, queries::UidKind::Track, [track_id])
            .ok()
            .and_then(|mut u| u.remove(&track_id))
        {
            ids.push(("uid", uid));
        }
        if let Some(p) = &row.cached_path {
            ids.push(("downloaded to", p.clone()));
        }
        groups.push(("library", ids));
        Some(Self {
            title: row.title,
            groups,
            scroll: 0,
        })
    }

    pub fn groups(&self) -> Groups {
        self.groups.clone()
    }

    /// What the popup lists for a queue entry: where it stands, then what
    /// the library knows of it, or what the entry carries when the library
    /// has no row for it.
    pub fn with_entry(&self, entry: &QueueEntry, playing: Option<&TrackInfo>) -> Groups {
        let status = match entry.status {
            QueueEntryStatus::Queued => "queued",
            QueueEntryStatus::Playing => "playing",
            QueueEntryStatus::Played => "played",
            QueueEntryStatus::Downloading => "downloading",
            QueueEntryStatus::PriorityPending => "up next",
            QueueEntryStatus::Failed => "failed",
        };
        let mut queue = vec![("status", status.to_string())];
        if let Some(why) = &entry.error {
            queue.push(("why", why.clone()));
        }
        if let Some(info) = playing {
            queue.push(("sample rate", format!("{} Hz", info.sample_rate)));
            if let Some(b) = info.bit_depth {
                queue.push(("bit depth", format!("{b} bit")));
            }
            if let Some(kbps) = info.bitrate_kbps {
                queue.push(("bitrate", format!("{kbps} kbps")));
            }
            queue.push(("channels", info.channels.to_string()));
        }
        let mut groups = vec![("queue", queue)];
        if self.groups.is_empty() {
            groups.push(("entry", entry_fields(entry)));
        } else {
            groups.extend(self.groups.iter().cloned());
        }
        groups
    }
}

/// What a source says, by name, as the apps show it.
fn source_fields(m: TrackMeta) -> Fields {
    let text = |v: Option<String>| v.filter(|v| !v.is_empty());
    let number = |v: Option<i32>| v.map(|v| v.to_string());
    let modified = m
        .mtime
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-d %b %Y %H:%M")
                .to_string()
        });
    // The stream URL carries the account's credentials, so it is left out.
    [
        ("title", Some(m.title)),
        ("artist", Some(m.artist)),
        ("album artist", text(m.album_artist)),
        ("album", Some(m.album)),
        ("date", text(m.date)),
        ("disc", number(m.disc)),
        ("track", number(m.track_number)),
        ("genre", text(m.genre)),
        ("label", text(m.label)),
        ("length", m.duration_ms.map(|ms| format_time(ms as u64))),
        ("codec", text(m.codec)),
        ("sample rate", m.sample_rate.map(|r| format!("{r} Hz"))),
        ("bit depth", m.bit_depth.map(|b| format!("{b} bit"))),
        ("channels", number(m.channels)),
        ("bitrate", m.bitrate.map(|b| format!("{b} kbps"))),
        ("size", m.size_bytes.map(|b| format!("{b} bytes"))),
        ("modified", modified),
        ("path", text(m.path)),
        ("server id", text(m.remote_id)),
        ("server album id", text(m.album_remote_id)),
        ("server artist id", text(m.artist_remote_id)),
        ("musicbrainz recording", text(m.mbid)),
        ("musicbrainz release", text(m.album_mbid)),
        ("album added", text(m.album_added_at)),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|v| (name, v)))
    .collect()
}

/// What a queue entry carries of its track.
fn entry_fields(entry: &QueueEntry) -> Fields {
    let mut out = vec![
        ("title", entry.title.clone()),
        ("artist", entry.artist.clone()),
    ];
    if !entry.album_artist.is_empty() && entry.album_artist != entry.artist {
        out.push(("album artist", entry.album_artist.clone()));
    }
    out.push(("album", entry.album.clone()));
    if let Some(year) = &entry.year {
        out.push(("date", year.clone()));
    }
    if let Some(d) = entry.disc {
        out.push(("disc", d.to_string()));
    }
    if let Some(n) = entry.track_number {
        out.push(("track", n.to_string()));
    }
    if let Some(ms) = entry.duration_ms {
        out.push(("length", format_time(ms)));
    }
    if let Some(c) = &entry.codec {
        out.push(("codec", c.clone()));
    }
    out.push(("path", entry.path.display().to_string()));
    out
}

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

/// The width of the labels' column.
const LABEL_W: usize = 22;

pub struct TrackInfoOverlay<'a> {
    title: &'a str,
    groups: Groups,
    scroll: usize,
    /// The cover, and what draws it.
    cover: Option<(&'a Cover, &'a mut Gfx)>,
    theme: &'a Theme,
}

impl<'a> TrackInfoOverlay<'a> {
    pub fn new(
        title: &'a str,
        groups: Groups,
        scroll: usize,
        cover: Option<(&'a Cover, &'a mut Gfx)>,
        theme: &'a Theme,
    ) -> Self {
        Self {
            title,
            groups,
            scroll,
            cover,
            theme,
        }
    }
}

/// A line of the popup: a group's heading, or a label and a line of its
/// value.
enum Line {
    Blank,
    Heading(&'static str),
    Field(&'static str, String),
}

impl Widget for TrackInfoOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = popup_rect(area);
        crate::shell::clear(buf, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(ratatui::text::Span::styled(
                " track info ",
                self.theme.album_header_artist.add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(popup);
        block.render(popup, buf);
        if inner.height < 3 || inner.width < 4 {
            return;
        }

        // The cover, square in the terminal's cells, at the top left.
        let mut text = Rect::new(inner.x + 1, inner.y, inner.width - 2, inner.height);
        if let Some((cover, gfx)) = self.cover
            && inner.width > 50
        {
            let tallest = (f32::from(inner.height - 1) * 2.0 / gfx.squash()) as u16;
            let art_w = (inner.width / 4).min(tallest).max(1);
            let art_h = gfx.rows_for(art_w).min(inner.height);
            gfx.draw(
                cover.key,
                &cover.image,
                Rect::new(inner.x + 1, inner.y, art_w, art_h),
                buf,
                false,
            );
            text = Rect::new(
                inner.x + art_w + 3,
                inner.y,
                inner.width.saturating_sub(art_w + 4),
                inner.height,
            );
        }
        let w = text.width as usize;
        let strong = self.theme.track_normal.add_modifier(Modifier::BOLD);
        buf.set_stringn(text.x, text.y, fit(self.title, w), w, strong);

        // A long value wraps under itself; the labels keep their column.
        let label_w = LABEL_W.min(w / 3);
        let value_w = w.saturating_sub(label_w + 1).max(1);
        let mut lines = Vec::new();
        for (name, fields) in self.groups {
            lines.push(Line::Blank);
            lines.push(Line::Heading(name));
            for (label, value) in fields {
                for (i, part) in wrap(&value, value_w).into_iter().enumerate() {
                    lines.push(Line::Field(if i == 0 { label } else { "" }, part));
                }
            }
        }
        let body = Rect::new(
            text.x,
            text.y + 1,
            text.width,
            text.height.saturating_sub(2),
        );
        let rows = body.height as usize;
        let scroll = self.scroll.min(lines.len().saturating_sub(rows));
        for (r, line) in lines.iter().skip(scroll).take(rows).enumerate() {
            let y = body.y + r as u16;
            match line {
                Line::Blank => {}
                Line::Heading(name) => {
                    let style = self.theme.heading.add_modifier(Modifier::BOLD);
                    buf.set_stringn(body.x, y, name, w, style);
                    for cx in body.x + 1 + width(name) as u16..body.x + body.width {
                        buf[(cx, y)].set_char('\u{2500}').set_style(self.theme.rule);
                    }
                }
                Line::Field(label, value) => {
                    let label = fit(label, label_w);
                    buf.set_stringn(body.x, y, &label, label_w, self.theme.hint_desc);
                    let x = body.x + label_w as u16 + 1;
                    buf.set_stringn(x, y, value, value_w, self.theme.track_normal);
                }
            }
        }
        let hint = if lines.len() > rows {
            " esc close \u{b7} \u{2191}\u{2193} scroll"
        } else {
            " esc close"
        };
        let foot = text.y + text.height - 1;
        buf.set_stringn(text.x, foot, hint, w, self.theme.hint_desc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::player::state::QueueItemId;

    fn meta() -> TrackMeta {
        TrackMeta {
            title: "song".into(),
            artist: "artist".into(),
            album_artist: None,
            album: "album".into(),
            date: Some("2007".into()),
            disc: Some(1),
            track_number: Some(3),
            genre: Some(String::new()),
            label: None,
            duration_ms: Some(200_000),
            codec: Some("FLAC".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: None,
            size_bytes: Some(1234),
            mtime: None,
            path: Some("/music/a very long path/that wraps/song.flac".into()),
            server_path: None,
            source: "local".into(),
            remote_id: None,
            remote_url: Some("https://server/rest/stream?p=secret".into()),
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_mbid: None,
            album_added_at: None,
            credits: Default::default(),
        }
    }

    fn details() -> Details {
        Details {
            title: "song".into(),
            groups: vec![
                ("file", source_fields(meta())),
                ("library", vec![("track id", "7".into())]),
            ],
            scroll: 0,
        }
    }

    fn entry() -> QueueEntry {
        QueueEntry {
            id: QueueItemId::new(),
            db_id: Some(7),
            playlist_entry_id: None,
            path: "/music/song.flac".into(),
            title: "song".into(),
            artist: "artist".into(),
            album_artist: String::new(),
            album: "album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            status: QueueEntryStatus::Failed,
            download_progress: None,
            error: Some("the server is unreachable".into()),
        }
    }

    #[test]
    fn a_source_says_what_it_has_and_never_its_stream_url() {
        let fields = source_fields(meta());
        let names: Vec<_> = fields.iter().map(|(n, _)| *n).collect();
        for want in [
            "title",
            "album",
            "date",
            "sample rate",
            "bit depth",
            "path",
            "size",
        ] {
            assert!(names.contains(&want), "{want}");
        }
        assert!(!names.contains(&"genre"), "an empty tag is left out");
        assert!(fields.iter().all(|(_, v)| !v.contains("secret")));
    }

    #[test]
    fn a_queue_entry_leads_with_where_it_stands_and_why() {
        let groups = details().with_entry(&entry(), None);
        assert_eq!(groups[0].0, "queue");
        assert_eq!(groups[0].1[0], ("status", "failed".into()));
        assert_eq!(groups[0].1[1].0, "why");
        assert_eq!(groups[1].0, "file");
        let bare = Details::default().with_entry(&entry(), None);
        assert_eq!(bare[1].0, "entry", "what the entry carries, with no row");
    }

    #[test]
    fn it_draws_at_any_size() {
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        let cover = Cover {
            key: 1,
            image: image::DynamicImage::new_rgb8(8, 8),
            accent: None,
        };
        let mut gfx = Gfx::halfblocks();
        let d = details();
        for w in 0..120 {
            for h in 0..30 {
                let area = Rect::new(0, 0, w, h);
                for with_cover in [false, true] {
                    for scroll in [0, 100] {
                        let mut buf = Buffer::empty(area);
                        let cover = with_cover.then_some((&cover, &mut gfx));
                        TrackInfoOverlay::new(&d.title, d.groups(), scroll, cover, &theme)
                            .render(area, &mut buf);
                    }
                }
            }
        }
    }
}

//! The track row and the album heading the queue and a playlist's page share,
//! laid out in fixed columns as the apps' rows are: marker, status, number,
//! title and who it is by, then availability, heart, format and length
//! against the right edge. Every width here is measured in terminal cells.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::shell::{Link, Links};
use super::text::{fit, width};
use super::theme::Theme;
use super::transport::format_time;

/// Writes `text` at `x`, cut to `room` cells; where it ends.
fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, room: usize, style: Style) -> u16 {
    let shown = fit(text, room);
    buf.set_stringn(x, y, &shown, room, style);
    x + width(&shown) as u16
}

/// Writes `text` so that it ends at `end`.
fn put_right(buf: &mut Buffer, end: u16, y: u16, text: &str, room: usize, style: Style) {
    let shown = fit(text, room);
    let x = end.saturating_sub(width(&shown) as u16);
    buf.set_stringn(x, y, &shown, room, style);
}

/// Writes `text` as a link when there is one to draw it with; where it ends.
#[allow(clippy::too_many_arguments)]
fn put_link(
    buf: &mut Buffer,
    links: Option<&Links>,
    link: Option<Link>,
    x: u16,
    y: u16,
    text: &str,
    room: usize,
    style: Style,
) -> u16 {
    match (links, link) {
        (Some(links), Some(link)) => x + links.draw(buf, x, y, text, room as u16, style, link),
        _ => put(buf, x, y, text, room, style),
    }
}

/// What the row's left edge says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Marker {
    #[default]
    None,
    Cursor,
    Selected,
    /// Where something dropped here would land.
    Drop,
}

/// The status column: where the track is in playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    None,
    Playing,
    Played,
    Queued,
    /// Next in line for a download, ahead of the rest.
    Pending,
    Failed,
}

/// Where the track's bytes are, as the availability column draws it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Avail {
    #[default]
    None,
    /// Arriving; the fraction when the size is known.
    Transferring(Option<f64>),
    /// Offline, with no file here.
    NotHere,
    /// On the server, and downloaded.
    Kept,
    /// On the server only; downloads when played.
    Server,
    /// A file in the library.
    Local,
}

impl Avail {
    /// From where the library says the bytes are.
    pub fn of(on_server: bool, on_disk: bool, offline: bool) -> Self {
        match (on_server, on_disk) {
            (_, false) if offline => Avail::NotHere,
            (true, true) => Avail::Kept,
            (true, false) => Avail::Server,
            (false, true) => Avail::Local,
            (false, false) => Avail::None,
        }
    }
}

/// One track row's content.
#[derive(Debug, Clone, Default)]
pub struct Track<'a> {
    pub marker: Marker,
    pub status: Status,
    /// The number column; `None` leaves it out, as an ungrouped row does.
    pub number: Option<String>,
    pub title: &'a str,
    /// Who it is by, after the title, and where that leads.
    pub artist: Option<(&'a str, Option<Link>)>,
    /// The album, after the artist, and where that leads.
    pub album: Option<(&'a str, Option<Link>)>,
    pub avail: Avail,
    /// `Some` for a library track: whether it is a favourite.
    pub favourite: Option<bool>,
    pub codec: Option<&'a str>,
    pub duration_ms: Option<u64>,
    /// Played, or not playable here: drawn quieter.
    pub dim: bool,
    pub selected: bool,
    pub hovered: bool,
    pub current: bool,
    pub spinner: usize,
}

const SPINNER: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
];

/// Where the columns sit in a row `width` cells wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub status: u16,
    pub number: u16,
    /// Where the title starts on a numbered row.
    pub text: u16,
    /// Where the right-hand columns start; text ends two cells before.
    pub right: u16,
    pub avail: Option<u16>,
    pub heart: Option<u16>,
    pub codec: Option<u16>,
    /// Where the length column ends.
    pub end: u16,
}

const AVAIL_W: u16 = 4;
const CODEC_W: u16 = 4;
const LEN_W: u16 = 7;
const NUMBER_W: u16 = 5;

impl Columns {
    /// Narrow rows give up the format, then availability and the heart, so
    /// the title keeps room.
    pub fn new(x: u16, width: u16) -> Self {
        let end = x + width;
        let mut right = end.saturating_sub(LEN_W);
        let codec = (width >= 48).then(|| {
            right = right.saturating_sub(CODEC_W + 1);
            right
        });
        let (avail, heart) = if width >= 34 {
            right = right.saturating_sub(2);
            let heart = right;
            right = right.saturating_sub(AVAIL_W + 1);
            (Some(right), Some(heart))
        } else {
            (None, None)
        };
        Self {
            status: x + 1,
            number: x + 3,
            text: x + 3 + NUMBER_W,
            right: right.max(x + 3),
            avail,
            heart,
            codec,
            end,
        }
    }
}

/// Draws a track row across `area` (one row high), its names as links
/// through `links`.
pub fn track(buf: &mut Buffer, area: Rect, theme: &Theme, t: &Track, links: Option<&Links>) {
    if area.width < 8 || area.height == 0 {
        return;
    }
    let y = area.y;
    let cols = Columns::new(area.x, area.width);
    let ground = if t.selected {
        Some(theme.track_selected)
    } else if t.hovered {
        Some(theme.library_hover)
    } else {
        None
    };
    if let Some(ground) = ground {
        for x in area.x..area.x + area.width {
            buf[(x, y)].set_style(ground);
        }
    }
    let on = |style: Style| ground.map_or(style, |g| style.patch(g));
    let quiet = on(theme.hint_desc);
    let text = if t.dim {
        quiet
    } else if t.current && !t.selected {
        on(theme.track_playing.add_modifier(Modifier::BOLD))
    } else if t.marker == Marker::Cursor {
        on(theme.track_cursor)
    } else {
        on(theme.track_normal)
    };

    let (marker, marker_style) = match t.marker {
        Marker::None => (' ', quiet),
        Marker::Cursor => ('\u{258c}', theme.bar),
        Marker::Selected => ('\u{2502}', on(theme.track_selected)),
        Marker::Drop => ('\u{25be}', theme.hint_key),
    };
    buf[(area.x, y)].set_char(marker).set_style(marker_style);

    let (glyph, glyph_style) = match t.status {
        Status::None => (' ', quiet),
        Status::Playing => ('\u{25b6}', on(theme.track_playing)),
        Status::Played => ('\u{2713}', quiet),
        Status::Queued => ('\u{b7}', quiet),
        Status::Pending => ('\u{2193}', on(theme.track_playing)),
        Status::Failed => ('!', on(theme.failed)),
    };
    buf[(cols.status, y)].set_char(glyph).set_style(glyph_style);

    let mut x = cols.number;
    if let Some(n) = &t.number {
        put_right(
            buf,
            cols.text - 1,
            y,
            n,
            (NUMBER_W - 1) as usize,
            on(theme.track_number),
        );
        x = cols.text;
    }
    let text_end = cols.right.saturating_sub(2).max(x);
    x = put(buf, x, y, t.title, (text_end - x) as usize, text);
    let mut first = true;
    for (name, link) in [t.artist, t.album].into_iter().flatten() {
        if name.is_empty() {
            continue;
        }
        let sep = if first { "  " } else { " \u{2014} " };
        first = false;
        let room = text_end.saturating_sub(x) as usize;
        if room <= width(sep) + 1 {
            break;
        }
        let at = put(buf, x, y, sep, room, quiet);
        x = put_link(buf, links, link, at, y, name, room - width(sep), quiet);
    }

    if let Some(ax) = cols.avail {
        let end = ax + AVAIL_W;
        match t.avail {
            Avail::None => {}
            Avail::Transferring(Some(f)) => put_right(
                buf,
                end,
                y,
                &format!("{}%", (f * 100.0) as u32),
                4,
                on(theme.spinner),
            ),
            Avail::Transferring(None) => {
                let c = SPINNER[t.spinner % SPINNER.len()];
                buf[(end - 1, y)].set_char(c).set_style(on(theme.spinner));
            }
            Avail::NotHere => {
                buf[(end - 1, y)].set_char('\u{2298}').set_style(quiet);
            }
            Avail::Kept => {
                buf[(end - 1, y)]
                    .set_char('\u{25cf}')
                    .set_style(on(theme.track_normal));
            }
            Avail::Server => {
                buf[(end - 1, y)].set_char('\u{25cb}').set_style(quiet);
            }
            Avail::Local => {
                buf[(end - 1, y)].set_char('\u{25aa}').set_style(quiet);
            }
        }
    }
    if let (Some(hx), Some(fav)) = (cols.heart, t.favourite) {
        if fav {
            buf[(hx, y)]
                .set_char('\u{2665}')
                .set_style(on(theme.favourite));
        } else if t.hovered {
            buf[(hx, y)].set_char('\u{2661}').set_style(quiet);
        }
    }
    if let (Some(cx), Some(codec)) = (cols.codec, t.codec) {
        put(buf, cx, y, &codec.to_uppercase(), CODEC_W as usize, quiet);
    }
    if let Some(ms) = t.duration_ms {
        put_right(buf, cols.end, y, &format_time(ms), LEN_W as usize, quiet);
    }
}

/// One album heading's content.
#[derive(Debug, Clone, Copy, Default)]
pub struct Heading<'a> {
    pub album: (&'a str, Option<Link>),
    pub artist: (&'a str, Option<Link>),
    /// Said against the right edge, under the length column.
    pub facts: &'a str,
    pub selected: bool,
}

/// Draws an album heading across `area`, its names as links through `links`.
pub fn heading(buf: &mut Buffer, area: Rect, theme: &Theme, h: &Heading, links: Option<&Links>) {
    if area.width < 8 || area.height == 0 {
        return;
    }
    let y = area.y;
    if h.selected {
        for x in area.x..area.x + area.width {
            buf[(x, y)].set_style(theme.track_selected);
        }
    }
    let cols = Columns::new(area.x, area.width);
    let (album, album_link) = h.album;
    let (artist, artist_link) = h.artist;
    // The facts give way before the album's name does.
    let facts = if (width(album) + width(h.facts) + 6) as u16 <= area.width {
        h.facts
    } else {
        ""
    };
    let facts_x = cols.end.saturating_sub(width(facts) as u16);
    let text_end = facts_x.saturating_sub(2).max(cols.status);
    let room = |x: u16| text_end.saturating_sub(x) as usize;
    let bold = theme.album_header_album.add_modifier(Modifier::BOLD);
    let mut x = put_link(
        buf,
        links,
        album_link,
        cols.status,
        y,
        album,
        room(cols.status),
        bold,
    );
    if !artist.is_empty() && room(x) > 3 {
        x = put(buf, x, y, "  ", room(x), theme.hint_desc);
        put_link(
            buf,
            links,
            artist_link,
            x,
            y,
            artist,
            room(x),
            theme.album_header_artist,
        );
    }
    if facts_x > cols.status + 8 {
        buf.set_string(facts_x, y, facts, theme.hint_desc);
    }
}

/// "2007 · 11 tracks · 59:10 · FLAC": what a heading says of its run.
pub fn facts(year: Option<&str>, tracks: usize, ms: u64, codec: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(year) = year.filter(|y| !y.is_empty()) {
        parts.push(year.to_string());
    }
    parts.push(super::text::count(tracks as u64, "track"));
    if ms > 0 {
        parts.push(format_time(ms));
    }
    if let Some(codec) = codec {
        parts.push(codec.to_uppercase());
    }
    parts.join(" \u{b7} ")
}

/// A long length as people say one: "3 days 2:10:05", "2:10:05", "59:10".
pub fn total(ms: u64) -> String {
    let days = ms / 86_400_000;
    let rest = format_time(ms % 86_400_000);
    match days {
        0 => rest,
        1 => format!("1 day {rest}"),
        d => format!("{d} days {rest}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167))
    }

    fn line(buf: &Buffer, y: u16) -> String {
        let mut out = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let s = buf[(x, y)].symbol();
            skip = width(s).saturating_sub(1);
            out.push_str(s);
        }
        out
    }

    #[test]
    fn lengths_line_up_whatever_the_title() {
        let theme = theme();
        let area = Rect::new(0, 0, 80, 3);
        let mut buf = Buffer::empty(area);
        let titles = [
            "short",
            "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{66f2}",
            &"long ".repeat(40),
        ];
        for (y, title) in titles.iter().enumerate() {
            track(
                &mut buf,
                Rect::new(0, y as u16, 80, 1),
                &theme,
                &Track {
                    number: Some("12".into()),
                    title,
                    artist: Some(("someone", None)),
                    codec: Some("flac"),
                    duration_ms: Some(61_000),
                    favourite: Some(true),
                    ..Default::default()
                },
                None,
            );
        }
        for y in 0..3 {
            let l = line(&buf, y);
            assert!(l.ends_with("   1:01"), "{y}: {l:?}");
            let cols = Columns::new(0, 80);
            assert_eq!(
                buf[(cols.heart.unwrap(), y)].symbol(),
                "\u{2665}",
                "the heart in its column on row {y}"
            );
            assert_eq!(buf[(cols.codec.unwrap(), y)].symbol(), "F");
        }
        assert!(
            line(&buf, 2).contains('\u{2026}'),
            "a long title is cut with an ellipsis"
        );
    }

    #[test]
    fn numbers_are_right_aligned_and_titles_share_a_column() {
        let theme = theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 2));
        for (y, n) in ["2", "1-12"].iter().enumerate() {
            track(
                &mut buf,
                Rect::new(0, y as u16, 60, 1),
                &theme,
                &Track {
                    number: Some(n.to_string()),
                    title: "title",
                    ..Default::default()
                },
                None,
            );
        }
        let cols = Columns::new(0, 60);
        for y in 0..2 {
            assert_eq!(buf[(cols.text, y)].symbol(), "t");
            assert_eq!(
                buf[(cols.text - 2, y)].symbol(),
                "2",
                "numbers end in one column"
            );
        }
    }

    #[test]
    fn the_links_cover_the_names_drawn() {
        let theme = theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        let links = Links::default();
        links.begin((0, 0));
        track(
            &mut buf,
            Rect::new(0, 0, 80, 1),
            &theme,
            &Track {
                title: "song",
                artist: Some(("who", Some(Link::ArtistOfTrack(1)))),
                album: Some(("what", Some(Link::AlbumOfTrack(1)))),
                ..Default::default()
            },
            Some(&links),
        );
        let l = line(&buf, 0);
        let at = |name: &str| width(&l[..l.find(name).expect(name)]) as u16;
        assert_eq!(links.at(at("who"), 0), Some(Link::ArtistOfTrack(1)));
        assert_eq!(links.at(at("what") + 3, 0), Some(Link::AlbumOfTrack(1)));
        assert_eq!(links.at(at("song"), 0), None, "the title is not a link");
    }

    #[test]
    fn a_heading_puts_its_facts_under_the_lengths() {
        let theme = theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        let facts = facts(Some("2007"), 11, 3_550_000, Some("flac"));
        assert_eq!(facts, "2007 \u{b7} 11 tracks \u{b7} 59:10 \u{b7} FLAC");
        heading(
            &mut buf,
            Rect::new(0, 0, 80, 1),
            &theme,
            &Heading {
                album: ("Album", None),
                artist: ("Artist", None),
                facts: &facts,
                ..Default::default()
            },
            None,
        );
        let l = line(&buf, 0);
        assert!(l.ends_with(&facts), "{l:?}");
        assert!(l.starts_with(" Album  Artist"), "{l:?}");
    }

    #[test]
    fn counts_and_totals_read_as_words() {
        assert_eq!(total(3_550_000), "59:10");
        assert_eq!(total(3 * 86_400_000 + 7_805_000), "3 days 2:10:05");
    }

    #[test]
    fn rows_draw_at_any_width() {
        let theme = theme();
        for w in 0..90 {
            let mut buf = Buffer::empty(Rect::new(0, 0, w.max(1), 1));
            let area = Rect::new(0, 0, w, 1);
            track(
                &mut buf,
                area,
                &theme,
                &Track {
                    number: Some("1-02".into()),
                    title: "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{66f2}",
                    artist: Some(("artist", None)),
                    album: Some(("album", None)),
                    avail: Avail::Transferring(Some(0.42)),
                    favourite: Some(false),
                    codec: Some("flac"),
                    duration_ms: Some(4_000_000),
                    marker: Marker::Cursor,
                    status: Status::Playing,
                    ..Default::default()
                },
                None,
            );
            heading(
                &mut buf,
                area,
                &theme,
                &Heading {
                    album: ("album", None),
                    artist: ("artist", None),
                    facts: "2007 \u{b7} 2 tracks",
                    selected: true,
                },
                None,
            );
        }
    }
}

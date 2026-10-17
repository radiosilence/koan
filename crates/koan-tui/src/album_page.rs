//! One album, as the apps' album page lays it out: the cover, and beside it
//! the title and heart, the artist (a link) with the year, codec and length,
//! and play, shuffle, add and play next; then the tracks, under a heading per
//! disc when there is more than one. Every button is a key too.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use koan_core::db::queries::{self, AlbumRow, TrackRow};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::actions_menu::playable;
use super::app::PickerAction;
use super::browse::{Action, Thumbs, album_year, track_playable};
use super::cover_art::{Gfx, album_key};
use super::rows::{self, Columns, Emphasis, Row, Target};
use super::theme::Theme;
use super::transport::format_time;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// Columns the scrollbar takes: two drawn and one of gap.
const BAR_W: u16 = 3;

/// What the pointer can be over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumHit {
    Track(usize),
    Link(Target),
    Heart,
    Play,
    Shuffle,
    Add,
    Next,
}

/// A line of the track list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Disc(i32),
    Track(usize),
}

#[derive(Default, Clone)]
struct Geom {
    hits: Vec<(Rect, AlbumHit)>,
    list: Rect,
    bar: Rect,
}

pub struct AlbumPage {
    pub album: AlbumRow,
    tracks: Vec<TrackRow>,
    lines: Vec<Line>,
    /// Whether the album is a favourite.
    pub favourite: bool,
    /// The favourite tracks, as the page opened, for the menu.
    fav_tracks: HashSet<i64>,
    cursor: usize,
    /// The first line shown.
    scroll: usize,
    follow: bool,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// The tracks as lines: a heading before each disc when there is more than
/// one.
fn lines_of(tracks: &[TrackRow]) -> Vec<Line> {
    let multi = tracks
        .iter()
        .filter_map(|t| t.disc)
        .collect::<HashSet<_>>()
        .len()
        > 1;
    let mut lines = Vec::with_capacity(tracks.len() + 4);
    let mut disc = None;
    for (i, t) in tracks.iter().enumerate() {
        if multi && t.disc != disc {
            disc = t.disc;
            lines.push(Line::Disc(t.disc.unwrap_or(0)));
        }
        lines.push(Line::Track(i));
    }
    lines
}

impl AlbumPage {
    pub fn load(id: i64) -> Option<Self> {
        let db = koan_core::db::pool::shared().get().ok()?;
        let album = queries::get_album(&db.conn, id).ok()??;
        let tracks = queries::tracks_for_album(&db.conn, id).unwrap_or_default();
        let favourite = queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER)
            .is_ok_and(|set| set.contains(&id));
        let mut page = Self::new(album, tracks, favourite);
        page.fav_tracks =
            queries::load_favourites(&db.conn, queries::LOCAL_USER).unwrap_or_default();
        Some(page)
    }

    fn new(album: AlbumRow, tracks: Vec<TrackRow>, favourite: bool) -> Self {
        Self {
            album,
            lines: lines_of(&tracks),
            tracks,
            favourite,
            fav_tracks: HashSet::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
            last_click: None,
        }
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn line_of(&self, track: usize) -> usize {
        self.lines
            .iter()
            .position(|l| *l == Line::Track(track))
            .unwrap_or(0)
    }

    fn keep_cursor_visible(&mut self) {
        let line = self.line_of(self.cursor);
        // The disc heading over the first track of a disc is shown with it.
        let top = match line.checked_sub(1).map(|l| self.lines[l]) {
            Some(Line::Disc(_)) => line - 1,
            _ => line,
        };
        let rows = self.rows();
        if top < self.scroll {
            self.scroll = top;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.tracks.len().saturating_sub(1));
        self.follow = true;
        self.keep_cursor_visible();
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self.lines.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    fn all(&self) -> Vec<i64> {
        self.tracks.iter().map(|t| t.id).collect()
    }

    fn play_all(&self, how: PickerAction) -> Action {
        let ids = self.all();
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, how)
        }
    }

    /// The album, played from the track at `idx`: what Enter does, so an
    /// album can be started part-way through with the rest of it queued.
    fn play_from(&self, idx: usize) -> Action {
        if idx >= self.tracks.len() {
            return Action::None;
        }
        Action::Play(self.all(), PickerAction::ReplaceFrom(idx))
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        let track = self.tracks.get(self.cursor);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => return Some(self.play_from(self.cursor)),
            KeyCode::Char('P') => return Some(self.play_all(PickerAction::AppendAndPlay)),
            KeyCode::Char('a') => return Some(self.play_all(PickerAction::Append)),
            KeyCode::Char('N') => return Some(self.play_all(PickerAction::PlayNext)),
            KeyCode::Char('r') if ctrl => return Some(self.play_all(PickerAction::ReplaceQueue)),
            KeyCode::Char('S') => return Some(Action::Shuffle(self.all())),
            KeyCode::Char('A') => {
                // The track's own artist on a compilation, else the album's.
                let artist = track
                    .and_then(|t| t.artist_id)
                    .unwrap_or(self.album.artist_id);
                return Some(Action::OpenArtist(artist));
            }
            KeyCode::Char('H') => return Some(Action::ToggleAlbum(self.album.id)),
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('w') => return Some(Action::Choose(self.all())),
            KeyCode::Char('d') => return Some(Action::Download(self.all())),
            KeyCode::Char('u') => return Some(Action::RemoveDownload(self.all())),
            KeyCode::Char('i') => return Some(track.map_or(Action::None, |t| Action::Info(t.id))),
            KeyCode::Char('f') => {
                return Some(track.map_or(Action::None, |t| Action::Favourite(t.id)));
            }
            _ => return None,
        }
        Some(Action::None)
    }

    /// What can be done with the track at `idx`, as the apps' track menu
    /// has it; the album's own actions are the buttons under its title.
    fn menu(&self, idx: usize) -> Action {
        let Some(t) = self.tracks.get(idx) else {
            return Action::None;
        };
        let thing = track_playable(t, self.fav_tracks.contains(&t.id));
        Action::Menu(t.title.clone(), playable(&thing))
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<AlbumHit> {
        if let Some((_, h)) = self.geom.hits.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*h);
        }
        let list = self.geom.list;
        if inside(list, col, row) {
            return match self.lines.get(self.scroll + (row - list.y) as usize) {
                Some(Line::Track(i)) => Some(AlbumHit::Track(*i)),
                _ => None,
            };
        }
        None
    }

    pub fn click(&mut self, hit: AlbumHit) -> Action {
        match hit {
            AlbumHit::Link(Target::Artist(id)) => Action::OpenArtist(id),
            AlbumHit::Link(Target::Album(id)) => Action::OpenAlbum(id),
            AlbumHit::Heart => Action::ToggleAlbum(self.album.id),
            AlbumHit::Play => self.play_all(PickerAction::ReplaceQueue),
            AlbumHit::Shuffle => Action::Shuffle(self.all()),
            AlbumHit::Add => self.play_all(PickerAction::Append),
            AlbumHit::Next => self.play_all(PickerAction::PlayNext),
            AlbumHit::Track(idx) => {
                let double = self
                    .last_click
                    .is_some_and(|(i, at)| i == idx && at.elapsed() < DOUBLE_CLICK);
                self.move_to(idx);
                if double {
                    self.last_click = None;
                    self.play_from(idx)
                } else {
                    self.last_click = Some((idx, Instant::now()));
                    Action::None
                }
            }
        }
    }

    pub fn on_scrollbar(&self, col: u16, row: u16) -> bool {
        inside(self.geom.bar, col, row)
    }

    pub fn scroll_to_bar(&mut self, row: u16) {
        self.follow = false;
        let b = self.geom.bar;
        if b.height == 0 {
            return;
        }
        let max = self.lines.len().saturating_sub(self.rows());
        let frac = row.saturating_sub(b.y) as f64 / b.height.saturating_sub(1).max(1) as f64;
        self.scroll = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
    }

    /// "album", how many tracks, and its actions.
    pub fn toolbar(&self) -> crate::shell::Toolbar {
        use crossterm::event::{KeyCode, KeyEvent};
        let bar = crate::shell::Toolbar::new("album")
            .count(super::text::count(self.tracks.len() as u64, "track"));
        if self.tracks.is_empty() {
            return bar;
        }
        bar.item(crate::shell::ToolItem::new(
            "\u{22ef} actions",
            KeyEvent::from(KeyCode::Char('m')),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        favourites: &HashSet<i64>,
        hover: Option<AlbumHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 8 || area.height < 2 {
            return;
        }
        thumbs.begin_frame();
        let mut hits = Vec::new();
        let body = super::shell::body(area);

        // The header: the cover on the left when there is room, text beside it.
        let art_w = if body.width >= 50 {
            (body.width / 4).clamp(12, 24)
        } else {
            0
        };
        let art_h = gfx.rows_for(art_w);
        let header_h = art_h.max(5).min(body.height);
        if art_w > 0 {
            let art = Rect::new(body.x, body.y, art_w, art_h.min(body.height));
            if let Some(img) = thumbs.get(self.album.id) {
                gfx.draw(album_key(self.album.id), img, art, buf, false);
            } else {
                super::browse::placeholder(art, art.height, buf, theme, &self.album.title);
            }
        }
        let tx = body.x + if art_w > 0 { art_w + 3 } else { 0 };
        let tw = (body.x + body.width).saturating_sub(tx);
        let in_header = |y: u16| y < body.y + header_h;
        let title = rows::put(
            buf,
            tx,
            body.y,
            tw.saturating_sub(3),
            &self.album.title,
            theme.heading.add_modifier(Modifier::BOLD),
        );
        let (heart, heart_style) = if self.favourite {
            ("\u{2665}", theme.favourite)
        } else {
            ("\u{2661}", theme.hint_desc)
        };
        let hx = title.x + title.width + 2;
        if hx < tx + tw {
            buf.set_string(
                hx,
                body.y,
                heart,
                rows::link(heart_style, hover == Some(AlbumHit::Heart)),
            );
            hits.push((Rect::new(hx, body.y, 1, 1), AlbumHit::Heart));
        }
        if in_header(body.y + 1) {
            // "Artist · 2007 · FLAC · 59:10", the artist a link.
            let y = body.y + 1;
            let target = Target::Artist(self.album.artist_id);
            let artist = rows::put(
                buf,
                tx,
                y,
                tw,
                &self.album.artist_name,
                rows::link(theme.track_playing, hover == Some(AlbumHit::Link(target))),
            );
            hits.push((artist, AlbumHit::Link(target)));
            let total_ms: i64 = self.tracks.iter().filter_map(|t| t.duration_ms).sum();
            let facts: String = [
                album_year(self.album.date.as_deref()),
                self.album.codec.clone().filter(|c| !c.is_empty()),
                (total_ms > 0).then(|| format_time(total_ms as u64)),
            ]
            .into_iter()
            .flatten()
            .map(|f| format!(" \u{b7} {f}"))
            .collect();
            let after = artist.x + artist.width;
            rows::put(
                buf,
                after,
                y,
                (tx + tw).saturating_sub(after),
                &facts,
                theme.hint_desc,
            );
        }

        // The buttons. Play is the one prominent action of the page.
        let by = body.y + 3;
        if in_header(by) {
            // A button's padding sits outside the text column, so its label
            // lines up with the title.
            let mut x = tx.saturating_sub(1);
            for (label, hit, prominent) in [
                ("\u{25b6} play", AlbumHit::Play, true),
                ("shuffle", AlbumHit::Shuffle, false),
                ("+ add", AlbumHit::Add, false),
                ("play next", AlbumHit::Next, false),
            ] {
                let text = format!(" {label} ");
                let w = text.width() as u16;
                if x + w > tx + tw {
                    break;
                }
                let base = if prominent {
                    theme.track_cursor
                } else {
                    theme.track_normal
                };
                let at = rows::put(buf, x, by, w, &text, rows::link(base, hover == Some(hit)));
                hits.push((at, hit));
                x += w + 1;
            }
        }

        // The tracks.
        let list_y = body.y + header_h + 1;
        let list_h = (body.y + body.height).saturating_sub(list_y);
        if list_h == 0 {
            self.geom.hits = hits;
            return;
        }
        let rows_on = list_h as usize;
        let bar = self.lines.len() > rows_on && body.width > 20;
        let list = Rect::new(
            body.x,
            list_y,
            body.width - if bar { BAR_W } else { 0 },
            list_h,
        );
        self.geom.list = list;
        if self.follow {
            self.keep_cursor_visible();
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(rows_on));
        // A track's artist is shown only where it is not the album's.
        let mixed = self
            .tracks
            .iter()
            .any(|t| t.artist_name != self.album.artist_name);
        let lead = self
            .tracks
            .iter()
            .filter_map(|t| t.track_number)
            .max()
            .map_or(2, |n| (n.max(1).ilog10() + 1).max(2) as u16);
        let cols = Columns::new(list, lead, 7, mixed, false);
        let hover_link = match hover {
            Some(AlbumHit::Link(t)) => Some(t),
            _ => None,
        };
        let mut links = Vec::new();
        for r in 0..rows_on {
            let Some(line) = self.lines.get(self.scroll + r) else {
                break;
            };
            let y = list.y + r as u16;
            let row = Rect::new(list.x, y, list.width, 1);
            match *line {
                Line::Disc(n) => {
                    rows::heading(buf, row, theme, &format!("disc {n}"), "", false, false);
                }
                Line::Track(i) => {
                    let t = &self.tracks[i];
                    let number = t.track_number.map(|n| n.to_string()).unwrap_or_default();
                    let time = t
                        .duration_ms
                        .map(|d| format_time(d as u64))
                        .unwrap_or_default();
                    let emphasis =
                        Emphasis::of(i == self.cursor, hover == Some(AlbumHit::Track(i)));
                    Row {
                        favourite: favourites.contains(&t.id),
                        lead: &number,
                        name: &t.title,
                        artist: mixed.then_some((t.artist_name.as_str(), t.artist_id)),
                        detail: None,
                        tail: &time,
                    }
                    .draw(buf, &cols, row, theme, emphasis, hover_link, &mut links);
                }
            }
        }
        hits.extend(links.into_iter().map(|(r, t)| (r, AlbumHit::Link(t))));

        if bar {
            let x = body.x + body.width - 1;
            self.geom.bar = Rect::new(x - 2, list.y, BAR_W, list.height);
            let total = self.lines.len();
            let h = list.height as usize;
            let thumb = (h * rows_on / total).max(1);
            let top = (h - thumb) * self.scroll / (total - rows_on).max(1);
            for row in 0..h {
                let on = row >= top && row < top + thumb;
                for cx in [x - 1, x] {
                    buf[(cx, list.y + row as u16)]
                        .set_char(if on { '\u{2588}' } else { ' ' })
                        .set_style(if on {
                            Style::new().fg(theme.scrollbar_thumb)
                        } else {
                            Style::new().bg(theme.scrollbar_bg)
                        });
                }
            }
        }
        self.geom.hits = hits;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::tests::album;

    fn track(id: i64, disc: Option<i32>, n: i32, title: &str) -> TrackRow {
        TrackRow {
            disc,
            track_number: Some(n),
            title: title.into(),
            artist_id: Some(9),
            ..crate::browse::tests::track(id, 1)
        }
    }

    fn page(n: usize) -> AlbumPage {
        let mut a = album(1, "an album", "artist");
        a.artist_id = 9;
        let tracks = (0..n)
            .map(|i| track(100 + i as i64, None, i as i32 + 1, &format!("song {i}")))
            .collect();
        AlbumPage::new(a, tracks, false)
    }

    fn draw(p: &mut AlbumPage, w: u16, h: u16) -> Buffer {
        koan_core::config::isolate_config_for_tests();
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(
            area,
            &mut buf,
            &Theme::default(),
            &mut Gfx::halfblocks(),
            &mut Thumbs::new(),
            &HashSet::new(),
            None,
        );
        buf
    }

    fn hit_of(p: &AlbumPage, want: AlbumHit) -> Rect {
        p.geom
            .hits
            .iter()
            .find(|(_, h)| *h == want)
            .unwrap_or_else(|| panic!("{want:?} drawn"))
            .0
    }

    #[test]
    fn enter_plays_the_album_from_the_cursor() {
        let mut p = page(5);
        draw(&mut p, 80, 30);
        let key = |c| crossterm::event::KeyEvent::from(c);
        p.handle_key(key(crossterm::event::KeyCode::Down));
        p.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(
            p.handle_key(key(crossterm::event::KeyCode::Enter)),
            Some(Action::Play(
                vec![100, 101, 102, 103, 104],
                PickerAction::ReplaceFrom(2)
            ))
        );
    }

    #[test]
    fn the_artist_and_the_buttons_are_links() {
        let mut p = page(5);
        draw(&mut p, 80, 30);
        let a = hit_of(&p, AlbumHit::Link(Target::Artist(9)));
        assert_eq!(p.hit(a.x, a.y), Some(AlbumHit::Link(Target::Artist(9))));
        assert_eq!(
            p.click(AlbumHit::Link(Target::Artist(9))),
            Action::OpenArtist(9)
        );
        for b in [
            AlbumHit::Play,
            AlbumHit::Shuffle,
            AlbumHit::Add,
            AlbumHit::Next,
        ] {
            let r = hit_of(&p, b);
            assert_eq!(p.hit(r.x + 1, r.y), Some(b));
        }
        let Action::Play(ids, PickerAction::ReplaceQueue) = p.click(AlbumHit::Play) else {
            panic!("expected a play");
        };
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn the_menu_is_the_apps_track_menu() {
        let mut p = page(3);
        draw(&mut p, 100, 30);
        let Action::Menu(title, items) = p.menu(1) else {
            panic!("a menu");
        };
        assert_eq!(title, "song 1");
        let labels: Vec<&str> = items.iter().map(|(l, _)| l.as_str()).collect();
        for want in [
            "Play",
            "Play next",
            "Favourite track",
            "Track info",
            "Go to artist",
        ] {
            assert!(labels.contains(&want), "{want}");
        }
        assert!(
            p.toolbar()
                .items
                .iter()
                .any(|i| i.key.code == crossterm::event::KeyCode::Char('m'))
        );
    }

    #[test]
    fn the_heart_and_h_favourite_the_album() {
        let mut p = page(5);
        draw(&mut p, 100, 30);
        let key = |c| crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c));
        assert_eq!(p.handle_key(key('H')), Some(Action::ToggleAlbum(1)));
        let heart = hit_of(&p, AlbumHit::Heart);
        assert_eq!(p.hit(heart.x, heart.y), Some(AlbumHit::Heart));
    }

    #[test]
    fn discs_get_headings_and_the_cursor_skips_them() {
        let mut a = album(1, "a double", "artist");
        a.artist_id = 9;
        let tracks = vec![
            track(1, Some(1), 1, "one"),
            track(2, Some(1), 2, "two"),
            track(3, Some(2), 1, "three"),
        ];
        let mut p = AlbumPage::new(a, tracks, false);
        assert_eq!(
            p.lines,
            vec![
                Line::Disc(1),
                Line::Track(0),
                Line::Track(1),
                Line::Disc(2),
                Line::Track(2)
            ]
        );
        let buf = draw(&mut p, 100, 30);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("disc 2"));
        let list = p.geom.list;
        assert_eq!(p.hit(list.x + 5, list.y), None, "a heading is not a track");
        assert_eq!(p.hit(list.x + 5, list.y + 4), Some(AlbumHit::Track(2)));
    }

    #[test]
    fn the_scrollbar_scrolls_the_tracks() {
        let mut p = page(80);
        draw(&mut p, 80, 20);
        let b = p.geom.bar;
        assert!(p.on_scrollbar(b.x, b.y));
        p.scroll_to_bar(b.y + b.height - 1);
        assert_eq!(p.scroll, 80 - p.rows());
        p.scroll_to_bar(b.y);
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn durations_end_in_one_column() {
        let mut a = album(1, "an album", "artist");
        a.artist_id = 9;
        let tracks = vec![
            TrackRow {
                duration_ms: Some(65_000),
                ..track(1, None, 1, "short")
            },
            TrackRow {
                duration_ms: Some(3_725_000),
                ..track(2, None, 12, "a very long track indeed")
            },
        ];
        let mut p = AlbumPage::new(a, tracks, false);
        let buf = draw(&mut p, 90, 30);
        let list = p.geom.list;
        let end_of = |y: u16| {
            (list.x..list.x + list.width)
                .rev()
                .find(|&x| buf[(x, y)].symbol() != " ")
                .unwrap()
        };
        assert_eq!(end_of(list.y), end_of(list.y + 1));
    }

    #[test]
    fn it_draws_at_any_size() {
        let mut p = page(30);
        for w in [1u16, 7, 8, 20, 44, 80, 200] {
            for h in [1u16, 2, 3, 6, 12, 40] {
                draw(&mut p, w, h);
            }
        }
    }
}

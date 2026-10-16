//! One record: its cover, who it is by, what it holds, and its tracks. The
//! artist's name is a link to their page; the three buttons are what the keys
//! `P`, `a` and `Ctrl+R` do, so nothing here needs a right click.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use koan_core::db::queries::{self, AlbumRow, TrackRow};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::app::PickerAction;
use super::browse::{Action, Thumbs};
use super::cover_art::{Gfx, album_key};
use super::theme::Theme;
use super::transport::format_time;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// What the pointer can be over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumHit {
    Track(usize),
    Artist,
    Heart,
    Play,
    Add,
    Replace,
}

#[derive(Default, Clone, Copy)]
struct Geom {
    artist: Rect,
    heart: Rect,
    play: Rect,
    add: Rect,
    replace: Rect,
    list: Rect,
    bar: Rect,
    total: usize,
}

pub struct AlbumPage {
    pub album: AlbumRow,
    tracks: Vec<TrackRow>,
    /// Whether the record is a favourite.
    pub favourite: bool,
    cursor: usize,
    scroll: usize,
    follow: bool,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

impl AlbumPage {
    pub fn load(id: i64) -> Option<Self> {
        let db = koan_core::db::pool::shared().get().ok()?;
        let album = queries::get_album(&db.conn, id).ok()??;
        let tracks = queries::tracks_for_album(&db.conn, id).unwrap_or_default();
        let favourite = queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER)
            .is_ok_and(|set| set.contains(&id));
        Some(Self {
            album,
            tracks,
            favourite,
            cursor: 0,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
            last_click: None,
        })
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.tracks.len().saturating_sub(1));
        self.follow = true;
        let rows = self.rows();
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + rows {
            self.scroll = self.cursor + 1 - rows;
        }
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self.tracks.len().saturating_sub(self.rows());
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

    /// The tracks from the cursor to the end: what Enter plays, so a record
    /// can be started part-way through.
    fn play_from_cursor(&self) -> Action {
        let ids: Vec<i64> = self.tracks[self.cursor.min(self.tracks.len())..]
            .iter()
            .map(|t| t.id)
            .collect();
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, PickerAction::AppendAndPlay)
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => return Some(self.play_from_cursor()),
            KeyCode::Char('P') => return Some(self.play_all(PickerAction::AppendAndPlay)),
            KeyCode::Char('a') => return Some(self.play_all(PickerAction::Append)),
            KeyCode::Char('N') => return Some(self.play_all(PickerAction::PlayNext)),
            KeyCode::Char('r') if ctrl => return Some(self.play_all(PickerAction::ReplaceQueue)),
            KeyCode::Char('A') => return Some(Action::OpenArtist(self.album.artist_id)),
            KeyCode::Char('H') => return Some(Action::ToggleAlbum(self.album.id)),
            KeyCode::Char('w') => return Some(Action::Choose(self.all())),
            KeyCode::Char('d') => return Some(Action::Download(self.all())),
            KeyCode::Char('u') => return Some(Action::RemoveDownload(self.all())),
            KeyCode::Char('i') => {
                return Some(
                    self.tracks
                        .get(self.cursor)
                        .map_or(Action::None, |t| Action::Info(t.id)),
                );
            }
            KeyCode::Char('f') => {
                return Some(
                    self.tracks
                        .get(self.cursor)
                        .map_or(Action::None, |t| Action::Favourite(t.id)),
                );
            }
            _ => return None,
        }
        Some(Action::None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<AlbumHit> {
        let g = &self.geom;
        if inside(g.artist, col, row) {
            return Some(AlbumHit::Artist);
        }
        if inside(g.heart, col, row) {
            return Some(AlbumHit::Heart);
        }
        for (r, hit) in [
            (g.play, AlbumHit::Play),
            (g.add, AlbumHit::Add),
            (g.replace, AlbumHit::Replace),
        ] {
            if inside(r, col, row) {
                return Some(hit);
            }
        }
        if inside(g.list, col, row) {
            let idx = self.scroll + (row - g.list.y) as usize;
            return (idx < self.tracks.len()).then_some(AlbumHit::Track(idx));
        }
        None
    }

    pub fn click(&mut self, hit: AlbumHit) -> Action {
        match hit {
            AlbumHit::Artist => Action::OpenArtist(self.album.artist_id),
            AlbumHit::Heart => Action::ToggleAlbum(self.album.id),
            AlbumHit::Play => self.play_all(PickerAction::AppendAndPlay),
            AlbumHit::Add => self.play_all(PickerAction::Append),
            AlbumHit::Replace => self.play_all(PickerAction::ReplaceQueue),
            AlbumHit::Track(idx) => {
                let double = self
                    .last_click
                    .is_some_and(|(i, at)| i == idx && at.elapsed() < DOUBLE_CLICK);
                self.move_to(idx);
                if double {
                    self.last_click = None;
                    self.play_from_cursor()
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
        let max = self.geom.total.saturating_sub(self.rows());
        let frac = row.saturating_sub(b.y) as f64 / b.height.saturating_sub(1).max(1) as f64;
        self.scroll = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
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
        if area.width < 8 || area.height < 3 {
            return;
        }
        self.geom = Geom::default();
        thumbs.begin_frame();
        super::ui::heading(buf, area, theme, "album", "");
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);

        // The header: cover on the left when there is room, text beside it.
        let art_w = if body.width >= 44 {
            (body.width / 3).min(24)
        } else {
            0
        };
        let art_h = (art_w / 2).max(u16::from(art_w > 0));
        let header_h = art_h.max(6).min(body.height);
        let art = Rect::new(body.x + 1, body.y, art_w, art_h.min(body.height));
        if art_w > 0 {
            if let Some(img) = thumbs.get(self.album.id) {
                gfx.draw(album_key(self.album.id), img, art, buf, false);
            } else {
                super::browse::placeholder(art, art.height, buf, theme, &self.album.title);
            }
        }
        let tx = body.x + if art_w > 0 { art_w + 3 } else { 1 };
        let tw = (body.x + body.width).saturating_sub(tx) as usize;
        let put = |buf: &mut Buffer, y: u16, text: &str, style: Style| {
            if y < body.y + header_h {
                buf.set_stringn(tx, y, text, tw, style);
            }
        };
        let (heart, heart_style) = if self.favourite {
            ("\u{2665}", theme.favourite)
        } else {
            ("\u{2661}", theme.hint_desc)
        };
        let heart_style = if hover == Some(AlbumHit::Heart) {
            heart_style.add_modifier(Modifier::UNDERLINED)
        } else {
            heart_style
        };
        buf.set_string(tx, body.y, heart, heart_style);
        self.geom.heart = Rect::new(tx, body.y, 2, 1);
        buf.set_stringn(
            tx + 2,
            body.y,
            &self.album.title,
            tw.saturating_sub(2),
            theme.heading.add_modifier(Modifier::BOLD),
        );
        let artist_style = if hover == Some(AlbumHit::Artist) {
            theme.track_playing.add_modifier(Modifier::UNDERLINED)
        } else {
            theme.track_playing
        };
        if body.y + 1 < body.y + header_h {
            let name = &self.album.artist_name;
            put(buf, body.y + 1, name, artist_style);
            let w = (name.chars().count().min(tw)) as u16;
            self.geom.artist = Rect::new(tx, body.y + 1, w, 1);
        }
        let year = super::library::album_year(self.album.date.as_deref()).unwrap_or_default();
        let total_ms: i64 = self.tracks.iter().filter_map(|t| t.duration_ms).sum();
        let facts = [
            (!year.is_empty()).then_some(year),
            self.album.codec.clone().filter(|c| !c.is_empty()),
            Some(format!("{} tracks", self.tracks.len())),
            (total_ms > 0).then(|| format_time(total_ms as u64)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
        put(buf, body.y + 2, &facts, theme.hint_desc);

        // The buttons. Play is the one prominent action of the page.
        let by = body.y + 4;
        if by < body.y + header_h {
            let mut x = tx;
            let end = body.x + body.width;
            for (label, hit, prominent) in [
                (" play ", AlbumHit::Play, true),
                (" add ", AlbumHit::Add, false),
                (" replace queue ", AlbumHit::Replace, false),
            ] {
                let w = label.chars().count() as u16;
                if x + w + 2 > end {
                    break;
                }
                let base = if prominent {
                    theme.track_cursor
                } else {
                    theme.track_normal
                };
                let style = if hover == Some(hit) {
                    base.add_modifier(Modifier::UNDERLINED)
                } else {
                    base
                };
                let text = format!("[{label}]");
                buf.set_string(x, by, &text, style);
                let rect = Rect::new(x, by, w + 2, 1);
                match hit {
                    AlbumHit::Play => self.geom.play = rect,
                    AlbumHit::Add => self.geom.add = rect,
                    _ => self.geom.replace = rect,
                }
                x += w + 4;
            }
        }

        // The tracks.
        let list_y = body.y + header_h + 1;
        let list_h = (body.y + body.height).saturating_sub(list_y);
        if list_h == 0 {
            return;
        }
        let list = Rect::new(body.x, list_y, body.width, list_h);
        self.geom.list = list;
        if self.follow {
            self.move_to(self.cursor);
        }
        let rows = list_h as usize;
        self.scroll = self.scroll.min(self.tracks.len().saturating_sub(rows));
        let multi_disc = self.tracks.iter().any(|t| t.disc.is_some_and(|d| d > 1));
        let show_artist = self
            .tracks
            .iter()
            .any(|t| t.artist_name != self.album.artist_name);
        for r in 0..rows {
            let idx = self.scroll + r;
            let Some(t) = self.tracks.get(idx) else { break };
            let y = list.y + r as u16;
            let selected = idx == self.cursor;
            let hovered = hover == Some(AlbumHit::Track(idx));
            if selected {
                for cx in list.x..list.x + list.width {
                    buf[(cx, y)].set_style(theme.library_cursor);
                }
                buf[(list.x, y)].set_char('\u{258c}').set_style(theme.bar);
            }
            let row_style = if selected {
                theme.library_cursor
            } else if hovered {
                theme.library_hover
            } else {
                theme.track_normal
            };
            let dim = if selected {
                theme.library_cursor
            } else {
                theme.hint_desc
            };
            if favourites.contains(&t.id) {
                buf.set_string(
                    list.x + 1,
                    y,
                    "\u{2665}",
                    if selected {
                        theme.library_cursor
                    } else {
                        theme.favourite
                    },
                );
            }
            let number = match (multi_disc, t.disc, t.track_number) {
                (true, Some(d), Some(n)) => format!("{d}-{n:02}"),
                (_, _, Some(n)) => format!("{n:02}"),
                _ => String::new(),
            };
            buf.set_stringn(list.x + 3, y, &number, 5, dim);
            let tail = t
                .duration_ms
                .map(|d| format_time(d as u64))
                .unwrap_or_default();
            let tail_w = tail.chars().count() as u16;
            let end = list.x + list.width;
            let title_x = list.x + 9;
            let room = end.saturating_sub(title_x + tail_w + 3) as usize;
            let title_w = if show_artist { room * 3 / 5 } else { room };
            buf.set_stringn(title_x, y, &t.title, title_w, row_style);
            if show_artist {
                let ax = title_x + title_w as u16 + 2;
                let aw = end.saturating_sub(ax + tail_w + 2) as usize;
                if aw > 0 {
                    buf.set_stringn(ax, y, &t.artist_name, aw, dim);
                }
            }
            if tail_w + 2 < list.width {
                buf.set_string(end - tail_w - 2, y, &tail, dim);
            }
        }

        // Scrollbar.
        let total = self.tracks.len();
        if total > rows && list.width > 6 {
            let x = list.x + list.width - 1;
            self.geom.bar = Rect::new(x - 2, list.y, 3, list.height);
            self.geom.total = total;
            let h = list.height as usize;
            let thumb = (h * rows / total).max(1);
            let top = (h - thumb) * self.scroll / (total - rows).max(1);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, n: i32, title: &str) -> TrackRow {
        TrackRow {
            id,
            album_id: Some(1),
            artist_id: Some(1),
            artist_name: "artist".into(),
            album_artist_name: "artist".into(),
            album_title: "record".into(),
            disc: None,
            track_number: Some(n),
            title: title.into(),
            duration_ms: Some(200_000),
            path: None,
            codec: Some("FLAC".into()),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        }
    }

    fn page(n: usize) -> AlbumPage {
        AlbumPage {
            album: AlbumRow {
                id: 1,
                title: "record".into(),
                artist_id: 9,
                artist_name: "artist".into(),
                date: Some("2001-01-01".into()),
                total_discs: None,
                total_tracks: None,
                codec: Some("FLAC".into()),
                label: None,
                remote_id: None,
                added_at: None,
                on_device: None,
            },
            favourite: false,
            tracks: (0..n)
                .map(|i| track(100 + i as i64, i as i32 + 1, &format!("song {i}")))
                .collect(),
            cursor: 0,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
            last_click: None,
        }
    }

    fn draw(p: &mut AlbumPage, w: u16, h: u16) {
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
    }

    #[test]
    fn enter_plays_from_the_cursor_to_the_end() {
        let mut p = page(5);
        draw(&mut p, 80, 30);
        p.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Down,
        ));
        p.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Down,
        ));
        let Some(Action::Play(ids, PickerAction::AppendAndPlay)) = p.handle_key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Enter),
        ) else {
            panic!("expected a play");
        };
        assert_eq!(ids, vec![102, 103, 104]);
    }

    #[test]
    fn the_artist_and_the_buttons_are_links() {
        let mut p = page(5);
        draw(&mut p, 80, 30);
        let a = p.geom.artist;
        assert_eq!(p.hit(a.x, a.y), Some(AlbumHit::Artist));
        assert_eq!(p.click(AlbumHit::Artist), Action::OpenArtist(9));
        let play = p.geom.play;
        assert_eq!(p.hit(play.x + 1, play.y), Some(AlbumHit::Play));
        let Action::Play(ids, PickerAction::ReplaceQueue) = p.click(AlbumHit::Replace) else {
            panic!("expected a replace");
        };
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn the_heart_and_h_favourite_the_record() {
        let mut p = page(5);
        draw(&mut p, 100, 30);
        let key = |c| crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c));
        assert_eq!(p.handle_key(key('H')), Some(Action::ToggleAlbum(1)));
        let heart = p.geom.heart;
        assert_eq!(p.hit(heart.x, heart.y), Some(AlbumHit::Heart));
        assert_eq!(p.click(AlbumHit::Heart), Action::ToggleAlbum(1));
    }

    #[test]
    fn n_i_d_and_u_act_on_the_record_and_the_track() {
        let mut p = page(3);
        draw(&mut p, 100, 30);
        let key = |c| crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c));
        assert!(matches!(
            p.handle_key(key('N')),
            Some(Action::Play(ids, PickerAction::PlayNext)) if ids.len() == 3
        ));
        assert!(matches!(p.handle_key(key('i')), Some(Action::Info(_))));
        assert!(matches!(p.handle_key(key('d')), Some(Action::Download(ids)) if ids.len() == 3));
        assert!(
            matches!(p.handle_key(key('u')), Some(Action::RemoveDownload(ids)) if ids.len() == 3)
        );
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
    fn it_draws_at_any_size() {
        let mut p = page(30);
        for w in [1u16, 7, 8, 20, 44, 80, 200] {
            for h in [1u16, 2, 3, 6, 12, 40] {
                draw(&mut p, w, h);
            }
        }
    }
}

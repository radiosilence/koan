//! The top of an artist's page, as the apps lay it out: their photograph when
//! there is one, and beside it how many albums and the heart, play, shuffle,
//! add and play next, and the opening of their Wikipedia article. The page's
//! toolbar carries their name.

use crossbeam_channel::{Receiver, unbounded};
use image::DynamicImage;
use koan_core::artist_info::{self, ArtistInfo};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::cover_art::{Gfx, album_key};
use crate::rows;
use crate::theme::Theme;

/// What the pointer can be over in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderHit {
    Play,
    Shuffle,
    Add,
    Next,
    Heart,
    More,
    Wikipedia,
}

enum Update {
    Info(Option<ArtistInfo>),
    Photo(DynamicImage),
    Done,
}

/// Cells between the photograph and the text beside it.
const GAP: u16 = 3;
/// The row the buttons sit on, under the count and a gap.
const BUTTON_ROW: u16 = 2;
/// The row the bio starts on, after the buttons and a gap.
const BIO_ROW: u16 = 4;
/// Lines of bio shown before "more".
const FOLDED: u16 = 4;
/// The widest the bio is set, for lines that can be read.
const BIO_MAX: usize = 96;

pub struct ArtistHeader {
    id: i64,
    name: String,
    holds: String,
    info: Option<ArtistInfo>,
    /// The bio broken into lines, and the width it was broken for.
    bio: Option<(usize, Vec<String>)>,
    photo: Option<DynamicImage>,
    loading: bool,
    favourite: bool,
    expanded: bool,
    rx: Receiver<Update>,
    hits: Vec<(Rect, HeaderHit)>,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row == r.y
}

/// `text` broken at spaces into lines of at most `width` cells.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let need = line.width() + usize::from(!line.is_empty()) + word.width();
        if need > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

impl ArtistHeader {
    /// Starts reading what is known of the artist: the database first, then
    /// the network, then the photograph.
    pub fn new(id: i64) -> Self {
        let (tx, rx) = unbounded();
        std::thread::Builder::new()
            .name("koan-artist-info".into())
            .spawn(move || {
                let pool = koan_core::db::pool::shared();
                if let Ok(db) = pool.get() {
                    let held = artist_info::cached(&db.conn, id).ok().flatten();
                    tx.send(Update::Info(held)).ok();
                }
                let fetched = pool
                    .get()
                    .ok()
                    .and_then(|db| artist_info::fetch(&db.conn, id).ok().flatten());
                let has_image = fetched.as_ref().is_some_and(|i| i.image_url.is_some());
                tx.send(Update::Info(fetched)).ok();
                if has_image
                    && let Ok(db) = pool.get()
                    && let Ok(Some(bytes)) = artist_info::image(&db.conn, id)
                    && let Ok(img) = image::load_from_memory(&bytes)
                {
                    tx.send(Update::Photo(img.thumbnail(512, 512))).ok();
                }
                tx.send(Update::Done).ok();
            })
            .ok();
        Self {
            id,
            name: String::new(),
            holds: String::new(),
            info: None,
            bio: None,
            photo: None,
            loading: true,
            favourite: false,
            expanded: false,
            rx,
            hits: Vec::new(),
        }
    }

    /// Takes what the lookup has produced so far; true when something came.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.rx.try_recv() {
            changed = true;
            match update {
                Update::Info(info) => {
                    self.info = info.or(self.info.take());
                    self.bio = None;
                }
                Update::Photo(img) => self.photo = Some(img),
                Update::Done => self.loading = false,
            }
        }
        changed
    }

    /// Who the page is about, and what it shows of them: "12 albums", or
    /// "appears on 3 albums".
    pub fn set_artist(&mut self, name: &str, holds: String) {
        name.clone_into(&mut self.name);
        self.holds = holds;
    }

    pub fn set_favourite(&mut self, on: bool) {
        self.favourite = on;
    }

    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    pub fn url(&self) -> Option<String> {
        self.info.as_ref().and_then(|i| i.bio_url.clone())
    }

    /// The photograph's width: only when there is one, as the apps show it.
    fn art_w(&self, width: u16) -> u16 {
        let has_photo =
            self.photo.is_some() || self.info.as_ref().is_some_and(|i| i.image_url.is_some());
        if width >= 50 && has_photo {
            (width / 5).clamp(12, 20)
        } else {
            0
        }
    }

    /// Where the text column starts, from the left of the header.
    fn text_x(&self, width: u16) -> u16 {
        match self.art_w(width) {
            0 => 0,
            w => w + GAP,
        }
    }

    fn text_w(&self, width: u16) -> u16 {
        width.saturating_sub(self.text_x(width))
    }

    /// The bio in lines to fit `width`, broken again only when the width or
    /// the bio changes.
    fn bio_lines(&mut self, width: u16) -> &[String] {
        let w = (self.text_w(width) as usize).clamp(10, BIO_MAX);
        if self.bio.as_ref().is_none_or(|(at, _)| *at != w) {
            let lines = self
                .info
                .as_ref()
                .and_then(|i| i.bio.as_deref())
                .map(|b| wrap(b, w))
                .unwrap_or_default();
            self.bio = Some((w, lines));
        }
        self.bio.as_ref().map_or(&[], |(_, lines)| lines)
    }

    /// Rows the header takes in a page `avail` rows tall; the albums get the
    /// rest, and always at least a row of tiles.
    pub fn height(&mut self, width: u16, avail: u16, gfx: &Gfx) -> u16 {
        if avail < 12 {
            return 0;
        }
        let lines = self.bio_lines(width).len() as u16;
        let shown = if self.expanded {
            lines
        } else {
            lines.min(FOLDED)
        };
        let art_h = gfx.rows_for(self.art_w(width));
        // Then the bio and its credits, or the line saying it is being
        // looked up.
        let bio = if lines > 0 {
            shown + 1
        } else {
            u16::from(self.loading)
        };
        ((BIO_ROW + bio).max(art_h) + 1).min(avail * 3 / 5)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<HeaderHit> {
        self.hits
            .iter()
            .find(|(r, _)| inside(*r, col, row))
            .map(|(_, h)| *h)
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        hover: Option<HeaderHit>,
    ) {
        self.hits.clear();
        if area.height == 0 || area.width < 8 {
            return;
        }
        let art_w = self.art_w(area.width);
        if art_w > 0 {
            let art_h = gfx.rows_for(art_w).min(area.height);
            let art = Rect::new(area.x, area.y, art_w, art_h);
            match &self.photo {
                Some(img) => gfx.draw(album_key(-self.id - 1), img, art, buf, false),
                None => super::browse::placeholder(art, art.height, buf, theme, &self.name),
            }
        }
        let tx = area.x + self.text_x(area.width);
        let tw = self.text_w(area.width);
        let end = area.y + area.height;
        let mut hits = Vec::new();

        // The toolbar above names them: what the page holds, and the heart.
        let holds = rows::put(
            buf,
            tx,
            area.y,
            tw.saturating_sub(3),
            &self.holds,
            theme.hint_desc,
        );
        let (heart, heart_style) = if self.favourite {
            ("\u{2665}", theme.favourite)
        } else {
            ("\u{2661}", theme.hint_desc)
        };
        let hx = holds.x + holds.width + 2;
        if hx < tx + tw {
            let style = rows::link(heart_style, hover == Some(HeaderHit::Heart));
            buf.set_string(hx, area.y, heart, style);
            hits.push((Rect::new(hx, area.y, 1, 1), HeaderHit::Heart));
        }

        // The buttons. Play is the one prominent action of the page.
        let by = area.y + BUTTON_ROW;
        if by < end {
            // A button's padding sits outside the text column, so its label
            // lines up with the name.
            let mut x = tx.saturating_sub(1);
            for (label, hit, prominent) in [
                ("\u{25b6} play", HeaderHit::Play, true),
                ("shuffle", HeaderHit::Shuffle, false),
                ("+ add", HeaderHit::Add, false),
                ("play next", HeaderHit::Next, false),
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

        self.bio_lines(area.width);
        let lines: &[String] = self.bio.as_ref().map_or(&[], |(_, lines)| lines);
        let mut y = area.y + BIO_ROW;
        let bw = tw.min(BIO_MAX as u16);
        if lines.is_empty() {
            if self.loading && y < end {
                rows::put(
                    buf,
                    tx,
                    y,
                    bw,
                    "looking up biography\u{2026}",
                    theme.hint_desc,
                );
            }
            self.hits = hits;
            return;
        }
        // The credits line is kept below the bio.
        let room = end.saturating_sub(y + 1) as usize;
        let limit = if self.expanded {
            room
        } else {
            room.min(FOLDED as usize)
        };
        let truncated = lines.len() > limit;
        for (i, line) in lines.iter().take(limit).enumerate() {
            if i + 1 == limit && truncated {
                rows::put(buf, tx, y, bw, &format!("{line}\u{2026}"), theme.hint_desc);
            } else {
                rows::put(buf, tx, y, bw, line, theme.hint_desc);
            }
            y += 1;
        }
        if y < end {
            let link = |on: bool| rows::link(theme.track_playing, on);
            let mut x = tx;
            let mut credit = |buf: &mut Buffer, text: &str, style: Style| {
                let w = text.width() as u16;
                if x + w > tx + tw {
                    return None;
                }
                let at = rows::put(buf, x, y, w, text, style);
                x += w + 3;
                Some(at)
            };
            if truncated || self.expanded {
                let label = if self.expanded { "less" } else { "more" };
                if let Some(at) = credit(buf, label, link(hover == Some(HeaderHit::More))) {
                    hits.push((at, HeaderHit::More));
                }
            }
            if self.info.as_ref().is_some_and(|i| i.bio_url.is_some())
                && let Some(at) = credit(
                    buf,
                    "from wikipedia",
                    link(hover == Some(HeaderHit::Wikipedia)),
                )
            {
                hits.push((at, HeaderHit::Wikipedia));
            }
            if let Some(by) = self.info.as_ref().and_then(|i| i.image_credit.as_deref()) {
                credit(buf, &format!("photo: {by}"), theme.hint_desc);
            }
        }
        self.hits = hits;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_wraps_at_spaces() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("", 7), Vec::<String>::new());
    }

    #[test]
    fn the_buttons_and_heart_are_hits_beside_the_name() {
        koan_core::config::isolate_config_for_tests();
        let mut h = ArtistHeader::new(-1);
        h.set_artist("Someone", "3 albums".into());
        let area = Rect::new(0, 0, 100, 14);
        let mut buf = Buffer::empty(area);
        h.render(
            area,
            &mut buf,
            &Theme::default(),
            &mut Gfx::halfblocks(),
            None,
        );
        for hit in [
            HeaderHit::Play,
            HeaderHit::Shuffle,
            HeaderHit::Add,
            HeaderHit::Next,
            HeaderHit::Heart,
        ] {
            let (r, _) = h.hits.iter().find(|(_, k)| *k == hit).expect("drawn");
            assert_eq!(h.hit(r.x, r.y), Some(hit));
        }
        let play = h
            .hits
            .iter()
            .find(|(_, k)| *k == HeaderHit::Play)
            .unwrap()
            .0;
        assert_eq!(play.y, BUTTON_ROW, "the buttons sit under the count");
        let text: String = (0..100).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(text.contains("3 albums"));
        assert!(
            !text.contains("Someone"),
            "the name is the toolbar's, not repeated"
        );
    }
}

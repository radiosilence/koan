//! The top of an artist's page: photograph, name, play and shuffle, and the
//! opening of their Wikipedia article, as the apps lay it out.

use crossbeam_channel::{Receiver, unbounded};
use image::DynamicImage;
use koan_core::artist_info::{self, ArtistInfo};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::cover_art::{Gfx, album_key};
use crate::theme::Theme;

/// What the pointer can be over in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderHit {
    Play,
    Shuffle,
    More,
    Wikipedia,
}

enum Update {
    Info(Option<ArtistInfo>),
    Photo(DynamicImage),
    Done,
}

#[derive(Default, Clone, Copy)]
struct Geom {
    play: Rect,
    shuffle: Rect,
    more: Rect,
    wiki: Rect,
}

pub struct ArtistHeader {
    id: i64,
    info: Option<ArtistInfo>,
    photo: Option<DynamicImage>,
    loading: bool,
    expanded: bool,
    rx: Receiver<Update>,
    geom: Geom,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row == r.y
}

/// `text` broken at spaces into lines of at most `width` cells.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let need = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
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
            info: None,
            photo: None,
            loading: true,
            expanded: false,
            rx,
            geom: Geom::default(),
        }
    }

    /// Takes what the lookup has produced so far; true when something came.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.rx.try_recv() {
            changed = true;
            match update {
                Update::Info(info) => self.info = info.or(self.info.take()),
                Update::Photo(img) => self.photo = Some(img),
                Update::Done => self.loading = false,
            }
        }
        changed
    }

    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    pub fn url(&self) -> Option<String> {
        self.info.as_ref().and_then(|i| i.bio_url.clone())
    }

    fn art_w(&self, width: u16) -> u16 {
        let wants_photo = self.photo.is_some()
            || (self.loading || self.info.as_ref().is_some_and(|i| i.image_url.is_some()));
        if width >= 50 && wants_photo {
            (width / 4).clamp(12, 22)
        } else {
            0
        }
    }

    fn text_w(&self, width: u16) -> usize {
        let art_w = self.art_w(width);
        let tx = if art_w > 0 { art_w + 3 } else { 1 };
        width.saturating_sub(tx + 1) as usize
    }

    fn bio_lines(&self, width: u16) -> Vec<String> {
        self.info
            .as_ref()
            .and_then(|i| i.bio.as_deref())
            .map(|b| wrap(b, self.text_w(width).max(10)))
            .unwrap_or_default()
    }

    /// Rows the header takes in a page `avail` rows tall; the records get the
    /// rest, and always at least a row of tiles.
    pub fn height(&self, width: u16, avail: u16) -> u16 {
        if avail < 12 {
            return 0;
        }
        let lines = self.bio_lines(width).len() as u16;
        let shown = if self.expanded { lines } else { lines.min(4) };
        let art_h = self.art_w(width) / 2;
        // Name, count, buttons and a gap, then the bio and its credit line.
        let text = 4 + shown + u16::from(lines > 0 || self.loading);
        (text.max(art_h) + 1).min(avail * 3 / 5)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<HeaderHit> {
        [
            (self.geom.play, HeaderHit::Play),
            (self.geom.shuffle, HeaderHit::Shuffle),
            (self.geom.more, HeaderHit::More),
            (self.geom.wiki, HeaderHit::Wikipedia),
        ]
        .into_iter()
        .find(|(r, _)| inside(*r, col, row))
        .map(|(_, h)| h)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        name: &str,
        albums: usize,
        hover: Option<HeaderHit>,
    ) {
        self.geom = Geom::default();
        if area.height == 0 || area.width < 8 {
            return;
        }
        let art_w = self.art_w(area.width);
        let art_h = (art_w / 2).min(area.height);
        if art_w > 0 {
            let art = Rect::new(area.x + 1, area.y, art_w, art_h);
            match &self.photo {
                Some(img) => gfx.draw(album_key(-self.id - 1), img, art, buf, false),
                None => super::browse::placeholder(art, art.height, buf, theme, name),
            }
        }
        let tx = area.x + if art_w > 0 { art_w + 3 } else { 1 };
        let tw = self.text_w(area.width);
        let end = area.y + area.height;
        let link = |on: bool| {
            let base = theme.track_playing;
            if on {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            }
        };

        buf.set_stringn(
            tx,
            area.y,
            name,
            tw,
            theme.heading.add_modifier(Modifier::BOLD),
        );
        if area.y + 1 < end {
            let count = format!("{albums} {}", if albums == 1 { "album" } else { "albums" });
            buf.set_stringn(tx, area.y + 1, &count, tw, theme.hint_desc);
        }
        if area.y + 2 < end {
            let y = area.y + 2;
            let mut x = tx;
            for (label, hit, prominent) in [
                (" play ", HeaderHit::Play, true),
                (" shuffle ", HeaderHit::Shuffle, false),
            ] {
                let w = label.chars().count() as u16 + 2;
                if x + w > area.x + area.width {
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
                buf.set_string(x, y, format!("[{label}]"), style);
                let rect = Rect::new(x, y, w, 1);
                match hit {
                    HeaderHit::Play => self.geom.play = rect,
                    _ => self.geom.shuffle = rect,
                }
                x += w + 2;
            }
        }

        let lines = self.bio_lines(area.width);
        let mut y = area.y + 4;
        if lines.is_empty() {
            if self.loading && y < end {
                buf.set_stringn(tx, y, "looking up biography\u{2026}", tw, theme.hint_desc);
            }
            return;
        }
        let room = end.saturating_sub(y + 2) as usize;
        let limit = if self.expanded { room } else { room.min(4) };
        let truncated = lines.len() > limit;
        for (i, line) in lines.iter().take(limit).enumerate() {
            let last = i + 1 == limit && truncated;
            let text = if last {
                format!("{line}\u{2026}")
            } else {
                line.clone()
            };
            buf.set_stringn(tx, y, &text, tw, Style::new().patch(theme.track_normal));
            y += 1;
        }
        if y < end {
            let mut x = tx;
            let mut put = |buf: &mut Buffer, text: &str, style: Style| -> Rect {
                let w = (text.chars().count() as u16).min((area.x + area.width).saturating_sub(x));
                buf.set_stringn(x, y, text, w as usize, style);
                let at = Rect::new(x, y, w, 1);
                x += w + 2;
                at
            };
            if truncated || self.expanded {
                let label = if self.expanded { "less" } else { "more" };
                self.geom.more = put(buf, label, link(hover == Some(HeaderHit::More)));
            }
            if self.info.as_ref().is_some_and(|i| i.bio_url.is_some()) {
                self.geom.wiki = put(
                    buf,
                    "from wikipedia",
                    link(hover == Some(HeaderHit::Wikipedia)),
                );
            }
            if let Some(credit) = self.info.as_ref().and_then(|i| i.image_credit.as_deref()) {
                let text = format!("photo: {credit}");
                if x + text.chars().count() as u16 <= area.x + area.width {
                    put(buf, &text, theme.hint_desc);
                }
            }
        }
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
}

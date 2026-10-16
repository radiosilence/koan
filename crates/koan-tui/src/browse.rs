//! The browsing pages: Albums, Favourites and Recent. One page type with a
//! source, because they are the library's own listings with a filter on
//! (`koan_core::shelves`); what each holds and in what order is decided there.
//! Records are shown as tiles of cover art, or as rows.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use image::DynamicImage;
use koan_core::db::queries::{self, AlbumOrder, AlbumQuery, AlbumRow, TrackRow};
use koan_core::shelves::{self, Shelf};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::PickerAction;
use super::cover_art::{Gfx, album_key};
use super::theme::Theme;
use super::transport::format_time;

/// The widest a tile gets, in columns. Wider screens get more tiles.
const TILE_MAX_W: u16 = 26;
const TILE_MIN_W: u16 = 14;

/// Decoded covers kept for tiles before the lot is dropped.
const THUMB_LIMIT: usize = 320;

/// A second click this soon after the first is a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Albums,
    Favourites,
    Recent,
}

impl Source {
    fn shelf(self) -> Option<Shelf<'static>> {
        match self {
            Source::Albums => None,
            Source::Favourites => Some(Shelf::Favourites),
            Source::Recent => Some(Shelf::Recent),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Source::Albums => "albums",
            Source::Favourites => "nothing favourited yet",
            Source::Recent => "nothing played in the last 30 days",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Tiles,
    List,
}

const ORDERS: [(AlbumOrder, &str); 4] = [
    (AlbumOrder::ArtistThenDate, "artist"),
    (AlbumOrder::RecentlyAdded, "added"),
    (AlbumOrder::Title, "title"),
    (AlbumOrder::YearDesc, "year"),
];

/// What a click or key asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Play(Vec<i64>, PickerAction),
    Favourite(i64),
}

/// Where the pointer is on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Item(usize),
    Sort,
    View,
    Filter,
}

/// Covers, loaded off the UI thread. Asking for one that is not there starts
/// the load and answers nothing; the page asks again next frame.
pub struct Thumbs {
    images: HashMap<i64, Option<DynamicImage>>,
    asked: HashSet<i64>,
    tx: Sender<i64>,
    rx: Receiver<(i64, Option<DynamicImage>)>,
}

impl Default for Thumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Thumbs {
    pub fn new() -> Self {
        let (tx, req) = crossbeam_channel::unbounded::<i64>();
        let (done, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("koan-thumbs".into())
            .spawn(move || {
                while let Ok(album) = req.recv() {
                    if done.send((album, load(album))).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Self {
            images: HashMap::new(),
            asked: HashSet::new(),
            tx,
            rx,
        }
    }

    /// Takes in what the loader has finished. Returns whether anything came.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((id, image)) = self.rx.try_recv() {
            self.asked.remove(&id);
            if self.images.len() >= THUMB_LIMIT {
                self.images.clear();
            }
            self.images.insert(id, image);
            any = true;
        }
        any
    }

    /// The cover for `album`, loading it if it is not here. `None` is either
    /// not yet or no cover; `loaded` tells which.
    fn get(&mut self, album: i64) -> Option<&DynamicImage> {
        if !self.images.contains_key(&album) && self.asked.insert(album) {
            self.tx.send(album).ok();
        }
        self.images.get(&album).and_then(Option::as_ref)
    }
}

fn load(album: i64) -> Option<DynamicImage> {
    let db = koan_core::db::pool::shared().get().ok()?;
    let tracks = queries::tracks_for_album(&db.conn, album).ok()?;
    let path = tracks
        .iter()
        .find_map(|t| t.path.as_deref().or(t.cached_path.as_deref()))?;
    let bytes = koan_core::index::folder_art::cover_art(std::path::Path::new(path))?;
    Some(image::load_from_memory(&bytes).ok()?.thumbnail(192, 192))
}

/// What the last render laid out, for the keys and the mouse to agree with it.
#[derive(Default, Clone, Copy)]
struct Geom {
    area: Rect,
    cols: usize,
    tile_w: u16,
    tile_h: u16,
    rows: usize,
    sort: Rect,
    view: Rect,
    filter: Rect,
}

pub struct Browse {
    pub source: Source,
    pub view: View,
    albums: Vec<AlbumRow>,
    tracks: Vec<TrackRow>,
    /// What is shown, as indices into `albums` or `tracks`: all of them, or
    /// the ones the filter keeps.
    shown: Vec<usize>,
    pub cursor: usize,
    scroll: usize,
    order: usize,
    pub filter: String,
    pub filter_active: bool,
    loaded: bool,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

impl Browse {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            view: View::Tiles,
            albums: Vec::new(),
            tracks: Vec::new(),
            shown: Vec::new(),
            cursor: 0,
            scroll: 0,
            order: 0,
            filter: String::new(),
            filter_active: false,
            loaded: false,
            geom: Geom::default(),
            last_click: None,
        }
    }

    /// Whether the list view holds tracks (a shelf's) rather than records.
    fn tracks_in_list(&self) -> bool {
        self.view == View::List && self.source != Source::Albums
    }

    /// Reads the listing again. Cheap enough to do whenever the page opens.
    pub fn reload(&mut self) {
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return;
        };
        let now = shelves::now();
        let user = queries::LOCAL_USER;
        let query = match self.source.shelf() {
            Some(shelf) => shelf.albums(user, now),
            None => AlbumQuery {
                order: ORDERS[self.order].0,
                ..Default::default()
            },
        };
        self.albums = queries::list_albums(&db.conn, &query).unwrap_or_default();
        self.tracks = match self.source.shelf() {
            Some(shelf) => shelf
                .tracks(user, now)
                .page(&db.conn, 5000, 0)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        self.loaded = true;
        self.refilter();
    }

    pub fn ensure_loaded(&mut self) {
        if !self.loaded {
            self.reload();
        }
    }

    fn refilter(&mut self) {
        let q = self.filter.to_lowercase();
        let keep = |haystack: &[&str]| {
            q.is_empty() || haystack.iter().any(|h| h.to_lowercase().contains(&q))
        };
        self.shown = if self.tracks_in_list() {
            (0..self.tracks.len())
                .filter(|&i| {
                    let t = &self.tracks[i];
                    keep(&[&t.title, &t.artist_name, &t.album_title])
                })
                .collect()
        } else {
            (0..self.albums.len())
                .filter(|&i| {
                    let a = &self.albums[i];
                    keep(&[&a.title, &a.artist_name])
                })
                .collect()
        };
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
        self.scroll = 0;
        self.keep_cursor_visible();
    }

    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::Tiles => View::List,
            View::List => View::Tiles,
        };
        self.cursor = 0;
        self.refilter();
    }

    pub fn cycle_order(&mut self) {
        if self.source == Source::Albums {
            self.order = (self.order + 1) % ORDERS.len();
            self.reload();
        }
    }

    fn cols(&self) -> usize {
        if self.view == View::Tiles {
            self.geom.cols.max(1)
        } else {
            1
        }
    }

    fn page_rows(&self) -> usize {
        self.geom.rows.max(1)
    }

    fn keep_cursor_visible(&mut self) {
        let row = self.cursor / self.cols();
        let rows = self.page_rows();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + rows {
            self.scroll = row + 1 - rows;
        }
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.shown.len().saturating_sub(1));
        self.keep_cursor_visible();
    }

    pub fn scroll_by(&mut self, rows: isize) {
        let cols = self.cols();
        let total_rows = self.shown.len().div_ceil(cols);
        let max = total_rows.saturating_sub(self.page_rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    /// The ids a play of the item at `idx` would queue.
    fn ids_at(&self, idx: usize) -> Vec<i64> {
        let Some(&i) = self.shown.get(idx) else {
            return Vec::new();
        };
        if self.tracks_in_list() {
            return vec![self.tracks[i].id];
        }
        koan_core::db::pool::shared()
            .get()
            .ok()
            .and_then(|db| queries::tracks_for_album(&db.conn, self.albums[i].id).ok())
            .map(|ts| ts.iter().map(|t| t.id).collect())
            .unwrap_or_default()
    }

    pub fn append_at(&mut self, idx: usize) -> Action {
        self.move_to(idx);
        self.play(idx, PickerAction::Append)
    }

    fn play(&self, idx: usize, how: PickerAction) -> Action {
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, how)
        }
    }

    /// The track under the cursor when the page lists tracks, for hearting.
    pub fn track_at_cursor(&self) -> Option<i64> {
        if !self.tracks_in_list() {
            return None;
        }
        self.shown.get(self.cursor).map(|&i| self.tracks[i].id)
    }

    /// `None` when the key is not the page's, so the app can try it as a
    /// transport key.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.filter_active {
            match key.code {
                KeyCode::Esc => self.clear_filter(),
                KeyCode::Enter => self.filter_active = false,
                KeyCode::Backspace => {
                    if self.filter.pop().is_none() {
                        self.filter_active = false;
                    }
                    self.refilter();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.filter.push(c);
                    self.refilter();
                }
                KeyCode::Up | KeyCode::Down => self.filter_active = false,
                _ => {}
            }
            return Some(Action::None);
        }
        let cols = self.cols();
        let page = self.page_rows() * cols;
        let tiles = self.view == View::Tiles;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') if tiles => {
                self.move_to(self.cursor.saturating_sub(1));
            }
            KeyCode::Right | KeyCode::Char('l') if tiles => self.move_to(self.cursor + 1),
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(cols)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + cols),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => return Some(self.play(self.cursor, PickerAction::AppendAndPlay)),
            KeyCode::Char('r') if ctrl => {
                return Some(self.play(self.cursor, PickerAction::ReplaceQueue));
            }
            KeyCode::Char('a') => return Some(self.play(self.cursor, PickerAction::Append)),
            KeyCode::Char('f') if self.tracks_in_list() => {
                return Some(
                    self.track_at_cursor()
                        .map_or(Action::None, Action::Favourite),
                );
            }
            KeyCode::Char('/') | KeyCode::Char('f') => self.filter_active = true,
            KeyCode::Char('v') => self.toggle_view(),
            KeyCode::Char('o') => self.cycle_order(),
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            _ => return None,
        }
        Some(Action::None)
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_active = false;
        self.refilter();
    }

    /// What the pointer is over.
    pub fn hit(&self, col: u16, row: u16) -> Option<Hit> {
        let g = &self.geom;
        let inside = |r: Rect| col >= r.x && col < r.x + r.width && row == r.y;
        if inside(g.sort) {
            return Some(Hit::Sort);
        }
        if inside(g.view) {
            return Some(Hit::View);
        }
        if inside(g.filter) {
            return Some(Hit::Filter);
        }
        let body_y = g.area.y + 1;
        if col < g.area.x
            || col >= g.area.x + g.area.width
            || row < body_y
            || row >= g.area.y + g.area.height
        {
            return None;
        }
        let idx = match self.view {
            View::Tiles => {
                let tile_row = (row - body_y) / g.tile_h.max(1);
                let tile_col = (col - g.area.x) / g.tile_w.max(1);
                if tile_col as usize >= g.cols {
                    return None;
                }
                (self.scroll + tile_row as usize) * g.cols + tile_col as usize
            }
            View::List => self.scroll + (row - body_y) as usize,
        };
        (idx < self.shown.len()).then_some(Hit::Item(idx))
    }

    /// A left click. The first selects; the second, soon after on the same
    /// item, plays it.
    pub fn click(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Sort => {
                self.cycle_order();
                Action::None
            }
            Hit::View => {
                self.toggle_view();
                Action::None
            }
            Hit::Filter => {
                self.filter_active = true;
                Action::None
            }
            Hit::Item(idx) => {
                let double = self
                    .last_click
                    .is_some_and(|(i, at)| i == idx && at.elapsed() < DOUBLE_CLICK);
                self.move_to(idx);
                if double {
                    self.last_click = None;
                    self.play(idx, PickerAction::AppendAndPlay)
                } else {
                    self.last_click = Some((idx, Instant::now()));
                    Action::None
                }
            }
        }
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
        hover: Option<usize>,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.ensure_loaded();
        self.geom = Geom {
            area,
            ..Geom::default()
        };
        self.render_header(area, buf, theme);
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if body.height == 0 {
            return;
        }
        if body.width < 4 {
            return;
        }
        if self.shown.is_empty() {
            let msg = if self.filter.is_empty() {
                self.source.noun()
            } else {
                "no match"
            };
            let text = if self.source == Source::Albums && self.filter.is_empty() {
                "no albums in the library"
            } else {
                msg
            };
            buf.set_line(
                body.x + 1,
                body.y,
                &Line::from(Span::styled(text, theme.hint_desc)),
                body.width.saturating_sub(1),
            );
            return;
        }
        match self.view {
            View::Tiles => self.render_tiles(body, buf, theme, gfx, thumbs, hover),
            View::List => self.render_list(body, buf, theme, favourites, hover),
        }
    }

    fn render_header(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let count = format!(" {} ", self.shown.len());
        let mut x = area.x;
        let end = area.x + area.width;
        let put = |buf: &mut Buffer, x: &mut u16, text: &str, style: Style| -> Rect {
            let w = (text.chars().count() as u16).min(end.saturating_sub(*x));
            let at = Rect::new(*x, area.y, w, 1);
            buf.set_stringn(*x, area.y, text, w as usize, style);
            *x += w;
            at
        };
        put(buf, &mut x, "", theme.heading);
        let title = match self.source {
            Source::Albums => "albums",
            Source::Favourites => "favourites",
            Source::Recent => "recent",
        };
        put(buf, &mut x, title, theme.heading);
        put(buf, &mut x, &count, theme.hint_desc);
        if self.filter_active || !self.filter.is_empty() {
            let cursor = if self.filter_active { "\u{2588}" } else { "" };
            let text = format!(" /{}{} ", self.filter, cursor);
            self.geom.filter = put(buf, &mut x, &text, theme.hint_key);
        }
        // The rule fills the rest, with the page's own controls on its right.
        let mut controls = Vec::new();
        if self.source == Source::Albums && self.view == View::Tiles
            || self.source == Source::Albums && self.view == View::List
        {
            controls.push(format!(" sort: {} ", ORDERS[self.order].1));
        }
        controls.push(match (self.view, self.source) {
            (View::Tiles, _) => " view: tiles ".to_string(),
            (View::List, Source::Albums) => " view: rows ".to_string(),
            (View::List, _) => " view: tracks ".to_string(),
        });
        let total: u16 = controls.iter().map(|c| c.chars().count() as u16).sum();
        let rule_end = end.saturating_sub(total).max(x);
        for cx in x..rule_end {
            buf[(cx, area.y)].set_char('\u{2500}').set_style(theme.rule);
        }
        x = rule_end;
        for (i, text) in controls.iter().enumerate() {
            let is_sort = self.source == Source::Albums && i == 0;
            let at = put(buf, &mut x, text, theme.hint_desc);
            if is_sort {
                self.geom.sort = at;
            } else {
                self.geom.view = at;
            }
        }
    }

    fn render_tiles(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        hover: Option<usize>,
    ) {
        let cols = ((body.width / TILE_MIN_W) as usize).max(1);
        let tile_w = (body.width / cols as u16).min(TILE_MAX_W);
        // A cell is about twice as tall as wide, so art of `w` columns is
        // square at `w / 2` rows. Two lines of label under it, and a gap.
        let art_w = tile_w.saturating_sub(2).max(1);
        let art_h = (art_w / 2).max(1);
        let tile_h = art_h + 3;
        let rows = ((body.height / tile_h) as usize).max(1);
        self.geom.cols = cols;
        self.geom.tile_w = tile_w;
        self.geom.tile_h = tile_h;
        self.geom.rows = rows;
        // Left over width is split into a margin so the grid sits centred.
        let margin = (body.width - tile_w * cols as u16) / 2;
        self.keep_cursor_visible();
        let total_rows = self.shown.len().div_ceil(cols);
        self.scroll = self.scroll.min(total_rows.saturating_sub(rows));

        for r in 0..rows {
            for c in 0..cols {
                let idx = (self.scroll + r) * cols + c;
                let Some(&i) = self.shown.get(idx) else { break };
                let album = &self.albums[i];
                let x = body.x + margin + c as u16 * tile_w;
                let y = body.y + r as u16 * tile_h;
                if y + art_h + 2 > body.y + body.height {
                    continue;
                }
                let selected = idx == self.cursor;
                let hovered = hover == Some(idx);
                let art = Rect::new(x + 1, y, art_w, art_h);
                if let Some(img) = thumbs.get(album.id) {
                    gfx.draw(album_key(album.id), img, art, buf, true);
                } else {
                    placeholder(art, buf, theme, &album.title);
                }
                if selected {
                    for dy in 0..art_h {
                        buf[(x, y + dy)].set_char('\u{258c}').set_style(theme.bar);
                    }
                }
                let (title_style, sub_style) = if selected {
                    (theme.track_cursor, theme.track_playing)
                } else if hovered {
                    (theme.track_hover, theme.hint_desc)
                } else {
                    (theme.track_normal, theme.hint_desc)
                };
                let label = tile_w.saturating_sub(2) as usize;
                buf.set_stringn(x + 1, y + art_h, &album.title, label, title_style);
                let year = queries_year(album);
                let sub = if year.is_empty() {
                    album.artist_name.clone()
                } else {
                    format!("{} \u{b7} {}", album.artist_name, year)
                };
                buf.set_stringn(x + 1, y + art_h + 1, &sub, label, sub_style);
            }
        }
        self.scrollbar(body, buf, theme, total_rows, rows);
    }

    fn render_list(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        favourites: &HashSet<i64>,
        hover: Option<usize>,
    ) {
        let rows = body.height as usize;
        self.geom.cols = 1;
        self.geom.tile_w = body.width;
        self.geom.tile_h = 1;
        self.geom.rows = rows;
        self.keep_cursor_visible();
        self.scroll = self.scroll.min(self.shown.len().saturating_sub(rows));
        let tracks = self.tracks_in_list();
        for r in 0..rows {
            let idx = self.scroll + r;
            let Some(&i) = self.shown.get(idx) else { break };
            let y = body.y + r as u16;
            let selected = idx == self.cursor;
            let hovered = hover == Some(idx);
            let (name, detail, tail, fav) = if tracks {
                let t = &self.tracks[i];
                (
                    t.title.clone(),
                    format!("{} \u{b7} {}", t.artist_name, t.album_title),
                    t.duration_ms
                        .map(|d| format_time(d as u64))
                        .unwrap_or_default(),
                    favourites.contains(&t.id),
                )
            } else {
                let a = &self.albums[i];
                let year = queries_year(a);
                (
                    a.title.clone(),
                    a.artist_name.clone(),
                    [year, a.codec.clone().unwrap_or_default()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("  "),
                    false,
                )
            };
            let (name_style, rest_style) = if selected {
                (theme.library_cursor, theme.library_cursor)
            } else if hovered {
                (theme.library_hover, theme.hint_desc)
            } else {
                (theme.track_normal, theme.hint_desc)
            };
            if selected {
                for cx in body.x..body.x + body.width {
                    buf[(cx, y)].set_style(theme.library_cursor);
                }
                buf[(body.x, y)].set_char('\u{258c}').set_style(theme.bar);
            }
            let w = body.width as usize;
            let tail_w = tail.chars().count();
            let gutter = if fav { "\u{2665}" } else { " " };
            buf.set_stringn(
                body.x + 1,
                y,
                gutter,
                1,
                if selected {
                    theme.library_cursor
                } else {
                    theme.favourite
                },
            );
            let name_w = (w.saturating_sub(tail_w + 4) * 3 / 5)
                .max(8)
                .min(w.saturating_sub(4));
            buf.set_stringn(body.x + 3, y, &name, name_w, name_style);
            let detail_x = body.x + 3 + name_w as u16 + 2;
            let room = (body.x + body.width).saturating_sub(detail_x + tail_w as u16 + 2) as usize;
            if room > 0 {
                buf.set_stringn(detail_x, y, &detail, room, rest_style);
            }
            if tail_w > 0 && tail_w + 1 < w {
                buf.set_stringn(
                    body.x + body.width - tail_w as u16 - 1,
                    y,
                    &tail,
                    tail_w,
                    rest_style,
                );
            }
        }
        self.scrollbar(body, buf, theme, self.shown.len(), rows);
    }

    fn scrollbar(&self, body: Rect, buf: &mut Buffer, theme: &Theme, total: usize, visible: usize) {
        if total <= visible || body.height == 0 || body.width < 2 {
            return;
        }
        let x = body.x + body.width - 1;
        let h = body.height as usize;
        let thumb = (h * visible / total).max(1);
        let top = (h - thumb) * self.scroll / (total - visible).max(1);
        for row in 0..h {
            let on = row >= top && row < top + thumb;
            buf[(x, body.y + row as u16)]
                .set_char(if on { '\u{2588}' } else { ' ' })
                .set_style(if on {
                    Style::new().fg(theme.scrollbar_thumb)
                } else {
                    Style::new().bg(theme.scrollbar_bg)
                });
        }
    }
}

fn queries_year(album: &AlbumRow) -> String {
    super::library::album_year(album.date.as_deref()).unwrap_or_default()
}

/// A tile with no cover (yet): the record's first letter on the raised
/// ground, so the grid keeps its shape while covers arrive.
fn placeholder(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str) {
    let ground = Style::new().bg(theme.tokens.surface);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf[(x, y)].set_char(' ').set_style(ground);
        }
    }
    let letter = title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "\u{266a}".into());
    buf.set_string(
        area.x + area.width / 2,
        area.y + area.height / 2,
        letter,
        ground.fg(theme.tokens.muted).add_modifier(Modifier::BOLD),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: i64, title: &str, artist: &str) -> AlbumRow {
        AlbumRow {
            id,
            title: title.into(),
            artist_id: id,
            artist_name: artist.into(),
            date: Some("1998-04-20".into()),
            total_discs: None,
            total_tracks: None,
            codec: Some("FLAC".into()),
            label: None,
            remote_id: None,
            added_at: None,
            on_device: None,
        }
    }

    fn browse(n: usize) -> Browse {
        koan_core::config::isolate_config_for_tests();
        let mut b = Browse::new(Source::Albums);
        b.albums = (0..n)
            .map(|i| album(i as i64, &format!("record {i}"), "artist"))
            .collect();
        b.loaded = true;
        b.refilter();
        b
    }

    fn draw(b: &mut Browse, w: u16, h: u16) {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        b.render(
            area,
            &mut buf,
            &theme,
            &mut Gfx::halfblocks(),
            &mut Thumbs::new(),
            &HashSet::new(),
            None,
        );
    }

    #[test]
    fn tiles_fill_the_width_and_keys_move_by_row() {
        let mut b = browse(40);
        draw(&mut b, 80, 30);
        assert!(b.geom.cols >= 3);
        let cols = b.geom.cols;
        b.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Down,
        ));
        assert_eq!(b.cursor, cols);
        b.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Right,
        ));
        assert_eq!(b.cursor, cols + 1);
    }

    #[test]
    fn the_cursor_is_kept_on_screen() {
        let mut b = browse(200);
        draw(&mut b, 60, 20);
        b.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::End,
        ));
        draw(&mut b, 60, 20);
        let row = b.cursor / b.geom.cols;
        assert!(row >= b.scroll && row < b.scroll + b.geom.rows);
    }

    #[test]
    fn a_click_selects_and_the_second_plays() {
        let mut b = browse(10);
        draw(&mut b, 80, 30);
        let hit = b.hit(b.geom.area.x + 1, b.geom.area.y + 2).expect("a tile");
        assert_eq!(hit, Hit::Item(0));
        assert_eq!(b.click(hit), Action::None);
        // No database behind the test, so a play has no ids to ask for.
        assert_eq!(b.click(hit), Action::None);
        assert!(b.last_click.is_none());
    }

    #[test]
    fn the_filter_narrows_what_is_shown() {
        let mut b = browse(12);
        b.filter = "record 1".into();
        b.refilter();
        assert_eq!(b.len(), 3);
        b.filter = "zzz".into();
        b.refilter();
        assert!(b.is_empty());
    }

    #[test]
    fn it_draws_at_any_size() {
        let mut b = browse(30);
        for view in [View::Tiles, View::List] {
            b.view = view;
            for w in [1u16, 2, 5, 13, 14, 20, 60, 200] {
                for h in [1u16, 2, 3, 5, 12, 40] {
                    draw(&mut b, w, h);
                }
            }
        }
    }
}

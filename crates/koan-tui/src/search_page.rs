//! The Search page: one box, and what the library holds that matches it in
//! three sections (artists, records, tracks), as the apps' search results
//! lay them out. Choosing a result goes to where it lives or plays it.

use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use koan_core::db::queries::{self, AlbumRow, ArtistRow, TrackRow};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::app::PickerAction;
use super::browse::{Action, fit};
use super::theme::Theme;
use super::transport::format_time;

const ARTISTS: u32 = 8;
const ALBUMS: u32 = 12;
const TRACKS: u32 = 40;
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How long the box must rest before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchHit {
    Item(usize),
    Artist(usize),
    Album(usize),
    Input,
    Clear,
    Menu,
}

enum Item {
    Artist(ArtistRow),
    Album(AlbumRow),
    Track(TrackRow),
}

enum Line {
    Section(&'static str, usize),
    Item(usize),
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    bar: Rect,
    input: Rect,
    clear: Rect,
    menu: Rect,
    links: Vec<(Rect, SearchHit)>,
    total: usize,
}

pub struct SearchPage {
    pub query: String,
    /// Whether the keys go to the box rather than the results.
    pub typing: bool,
    items: Vec<Item>,
    lines: Vec<Line>,
    line_of: Vec<usize>,
    cursor: usize,
    scroll: usize,
    follow: bool,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
    /// Counts the queries asked for; results for any but the last are dropped.
    generation: u64,
    shown: u64,
    tx: Sender<(u64, String)>,
    rx: Receiver<(u64, Vec<Item>)>,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

impl Default for SearchPage {
    fn default() -> Self {
        Self::new()
    }
}

/// What the library holds that matches `q`.
fn find(q: &str) -> Vec<Item> {
    let Ok(db) = koan_core::db::pool::shared().get() else {
        return Vec::new();
    };
    let artists = queries::list_artists(
        &db.conn,
        &queries::ArtistQuery {
            search: Some(q),
            limit: Some(ARTISTS),
            without_track_counts: true,
            ..Default::default()
        },
    )
    .unwrap_or_default();
    let albums = queries::list_albums(
        &db.conn,
        &queries::AlbumQuery {
            search: Some(q),
            order: queries::AlbumOrder::Relevance,
            limit: Some(ALBUMS),
            ..Default::default()
        },
    )
    .unwrap_or_default();
    let tracks = queries::filter_tracks(
        &db.conn,
        &queries::TrackFilter {
            search: Some(q.to_string()),
            ..Default::default()
        },
        queries::TrackOrder::ArtistAlbumDiscTrack,
        false,
        TRACKS,
        0,
    )
    .unwrap_or_default();
    artists
        .into_iter()
        .map(Item::Artist)
        .chain(albums.into_iter().map(Item::Album))
        .chain(tracks.into_iter().map(Item::Track))
        .collect()
}

/// Runs the searches off the TUI thread, the last query of a burst only.
fn spawn_searcher() -> (Sender<(u64, String)>, Receiver<(u64, Vec<Item>)>) {
    let (tx, req) = crossbeam_channel::unbounded::<(u64, String)>();
    let (done, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("koan-search".into())
        .spawn(move || {
            while let Ok(mut next) = req.recv() {
                loop {
                    match req.recv_timeout(DEBOUNCE) {
                        Ok(newer) => next = newer,
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                let (generation, query) = next;
                if done.send((generation, find(&query))).is_err() {
                    return;
                }
            }
        })
        .ok();
    (tx, rx)
}

impl SearchPage {
    pub fn new() -> Self {
        let (tx, rx) = spawn_searcher();
        Self {
            query: String::new(),
            typing: true,
            items: Vec::new(),
            lines: Vec::new(),
            line_of: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
            last_click: None,
            generation: 0,
            shown: 0,
            tx,
            rx,
        }
    }

    /// Opens with the box ready for typing; what was searched is kept.
    pub fn open(&mut self) {
        self.typing = true;
    }

    /// Asks for the search over the library; an empty box clears at once.
    pub fn search(&mut self) {
        self.generation += 1;
        let q = self.query.trim();
        if q.is_empty() {
            self.show(self.generation, Vec::new());
        } else {
            self.tx.send((self.generation, q.to_string())).ok();
        }
    }

    /// Takes in what the searcher has finished. Returns whether anything came.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((generation, items)) = self.rx.try_recv() {
            any |= self.show(generation, items);
        }
        any
    }

    /// Shows the results of query `generation`, unless a newer one was asked.
    fn show(&mut self, generation: u64, items: Vec<Item>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.shown = generation;
        self.items = items;
        self.cursor = 0;
        self.rebuild();
        true
    }

    fn rebuild(&mut self) {
        self.lines.clear();
        self.line_of.clear();
        let mut section = None;
        for (i, item) in self.items.iter().enumerate() {
            let (name, kind) = match item {
                Item::Artist(_) => ("artists", 0),
                Item::Album(_) => ("records", 1),
                Item::Track(_) => ("tracks", 2),
            };
            if section != Some(kind) {
                section = Some(kind);
                let n = self
                    .items
                    .iter()
                    .filter(|it| {
                        matches!(
                            (it, kind),
                            (Item::Artist(_), 0) | (Item::Album(_), 1) | (Item::Track(_), 2)
                        )
                    })
                    .count();
                self.lines.push(Line::Section(name, n));
            }
            self.line_of.push(self.lines.len());
            self.lines.push(Line::Item(i));
        }
        self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
        self.scroll = 0;
        self.follow = true;
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn keep_cursor_visible(&mut self) {
        let Some(&line) = self.line_of.get(self.cursor) else {
            return;
        };
        let rows = self.rows();
        let top = if line > 0 && matches!(self.lines[line - 1], Line::Section(..)) {
            line - 1
        } else {
            line
        };
        if top < self.scroll {
            self.scroll = top;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.items.len().saturating_sub(1));
        self.follow = true;
        self.keep_cursor_visible();
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self.lines.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    /// The tracks of whatever is at `idx`: the track, the record's, the
    /// artist's.
    fn ids_at(&self, idx: usize) -> Vec<i64> {
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return Vec::new();
        };
        match self.items.get(idx) {
            Some(Item::Track(t)) => vec![t.id],
            Some(Item::Album(a)) => queries::tracks_for_album(&db.conn, a.id)
                .map(|ts| ts.into_iter().map(|t| t.id).collect())
                .unwrap_or_default(),
            Some(Item::Artist(a)) => queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    artist_id: Some(a.id),
                    order: queries::AlbumOrder::Date,
                    ..Default::default()
                },
            )
            .unwrap_or_default()
            .into_iter()
            .filter_map(|al| queries::tracks_for_album(&db.conn, al.id).ok())
            .flatten()
            .map(|t| t.id)
            .collect(),
            None => Vec::new(),
        }
    }

    fn play(&self, idx: usize, how: PickerAction) -> Action {
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, how)
        }
    }

    fn open_at(&self, idx: usize) -> Action {
        match self.items.get(idx) {
            Some(Item::Artist(a)) => Action::OpenArtist(a.id),
            Some(Item::Album(a)) => Action::OpenAlbum(a.id),
            Some(Item::Track(t)) => t.album_id.map_or(Action::None, Action::OpenAlbum),
            None => Action::None,
        }
    }

    fn artist_of(&self, idx: usize) -> Option<i64> {
        match self.items.get(idx)? {
            Item::Artist(a) => Some(a.id),
            Item::Album(a) => Some(a.artist_id),
            Item::Track(t) => t.artist_id,
        }
    }

    /// What can be done with the result at `idx`.
    fn menu(&self, idx: usize) -> Action {
        use PickerAction::{Append, AppendAndPlay, PlayNext};
        let (title, favourite, info) = match self.items.get(idx) {
            Some(Item::Artist(a)) => (a.name.clone(), Action::ToggleArtist(a.id), None),
            Some(Item::Album(a)) => (a.title.clone(), Action::ToggleAlbum(a.id), None),
            Some(Item::Track(t)) => (t.title.clone(), Action::Favourite(t.id), Some(t.id)),
            None => return Action::None,
        };
        let ids = self.ids_at(idx);
        let mut items = vec![
            ("Play".to_string(), Action::Play(ids.clone(), AppendAndPlay)),
            ("Add to the queue".into(), Action::Play(ids.clone(), Append)),
            ("Play next".into(), Action::Play(ids.clone(), PlayNext)),
            (
                "Add to a playlist\u{2026}".into(),
                Action::Choose(ids.clone()),
            ),
            ("Favourite or unfavourite".into(), favourite),
            ("Open".into(), self.open_at(idx)),
        ];
        if let Some(artist) = self.artist_of(idx)
            && !matches!(self.items.get(idx), Some(Item::Artist(_)))
        {
            items.push(("Go to the artist".into(), Action::OpenArtist(artist)));
        }
        if let Some(id) = info {
            items.push(("Track info".into(), Action::Info(id)));
        }
        items.push(("Download".into(), Action::Download(ids.clone())));
        items.push(("Remove the download".into(), Action::RemoveDownload(ids)));
        Action::Menu(title, items)
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.typing {
            match key.code {
                KeyCode::Esc if !self.query.is_empty() => {
                    self.query.clear();
                    self.search();
                }
                KeyCode::Esc | KeyCode::BackTab => return None,
                KeyCode::Enter | KeyCode::Down | KeyCode::Tab if !self.items.is_empty() => {
                    self.typing = false;
                }
                KeyCode::Tab => return None,
                KeyCode::Enter | KeyCode::Down => {}
                KeyCode::Backspace => {
                    self.query.pop();
                    self.search();
                }
                KeyCode::Char('u') if ctrl => {
                    self.query.clear();
                    self.search();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.search();
                }
                _ => {}
            }
            return Some(Action::None);
        }
        let page = self.rows();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.cursor == 0 => self.typing = true,
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => {
                return Some(match self.items.get(self.cursor) {
                    Some(Item::Track(_)) => self.play(self.cursor, PickerAction::AppendAndPlay),
                    _ => self.open_at(self.cursor),
                });
            }
            KeyCode::Char('P') => return Some(self.play(self.cursor, PickerAction::AppendAndPlay)),
            KeyCode::Char('a') => return Some(self.play(self.cursor, PickerAction::Append)),
            KeyCode::Char('N') => return Some(self.play(self.cursor, PickerAction::PlayNext)),
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('O') => return Some(self.open_at(self.cursor)),
            KeyCode::Char('A') => {
                return Some(
                    self.artist_of(self.cursor)
                        .map_or(Action::None, Action::OpenArtist),
                );
            }
            KeyCode::Char('w') => {
                let ids = self.ids_at(self.cursor);
                return Some(if ids.is_empty() {
                    Action::None
                } else {
                    Action::Choose(ids)
                });
            }
            KeyCode::Char('f') => {
                return Some(match self.items.get(self.cursor) {
                    Some(Item::Artist(a)) => Action::ToggleArtist(a.id),
                    Some(Item::Album(a)) => Action::ToggleAlbum(a.id),
                    Some(Item::Track(t)) => Action::Favourite(t.id),
                    None => Action::None,
                });
            }
            KeyCode::Char('i') => {
                return Some(match self.items.get(self.cursor) {
                    Some(Item::Track(t)) => Action::Info(t.id),
                    _ => Action::None,
                });
            }
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.typing = true;
                self.search();
            }
            _ => return None,
        }
        Some(Action::None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<SearchHit> {
        let g = &self.geom;
        if inside(g.clear, col, row) {
            return Some(SearchHit::Clear);
        }
        if inside(g.input, col, row) {
            return Some(SearchHit::Input);
        }
        if inside(g.menu, col, row) {
            return Some(SearchHit::Menu);
        }
        if let Some((_, hit)) = g.links.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if inside(g.list, col, row)
            && let Some(Line::Item(i)) = self.lines.get(self.scroll + (row - g.list.y) as usize)
        {
            return Some(SearchHit::Item(*i));
        }
        None
    }

    pub fn click(&mut self, hit: SearchHit) -> Action {
        match hit {
            SearchHit::Input => {
                self.typing = true;
                Action::None
            }
            SearchHit::Clear => {
                self.query.clear();
                self.typing = true;
                self.search();
                Action::None
            }
            SearchHit::Menu => self.menu(self.cursor),
            SearchHit::Artist(i) => {
                self.typing = false;
                self.move_to(i);
                self.artist_of(i).map_or(Action::None, Action::OpenArtist)
            }
            SearchHit::Album(i) => {
                self.typing = false;
                self.move_to(i);
                self.open_at(i)
            }
            SearchHit::Item(i) => {
                self.typing = false;
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                if double {
                    self.last_click = None;
                    match self.items.get(i) {
                        Some(Item::Track(_)) => self.play(i, PickerAction::AppendAndPlay),
                        _ => self.open_at(i),
                    }
                } else {
                    self.last_click = Some((i, Instant::now()));
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

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<SearchHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 8 || area.height < 4 {
            return;
        }
        let note = if self.items.is_empty() {
            String::new()
        } else {
            format!(" {} ", self.items.len())
        };
        super::ui::heading(buf, area, theme, "search", &note);
        let end = area.x + area.width;
        if !self.items.is_empty() {
            let text = " \u{22ef} actions ";
            let w = text.chars().count() as u16;
            let x = end.saturating_sub(w);
            let style = if hover == Some(SearchHit::Menu) {
                theme.hint_desc.add_modifier(Modifier::UNDERLINED)
            } else {
                theme.hint_desc
            };
            buf.set_stringn(x, area.y, text, w as usize, style);
            self.geom.menu = Rect::new(x, area.y, w.min(end - x), 1);
        }

        // The box.
        let by = area.y + 1;
        let cursor = if self.typing { "\u{2588}" } else { "" };
        let prompt = if self.query.is_empty() && !self.typing {
            "type to search the library".to_string()
        } else {
            format!("{}{cursor}", self.query)
        };
        let box_style = if self.query.is_empty() && !self.typing {
            theme.hint_desc
        } else {
            theme.track_normal
        };
        buf.set_stringn(area.x + 1, by, "/", 1, theme.hint_key);
        buf.set_stringn(
            area.x + 3,
            by,
            &prompt,
            area.width.saturating_sub(12) as usize,
            box_style,
        );
        self.geom.input = Rect::new(area.x, by, area.width.saturating_sub(9), 1);
        if !self.query.is_empty() {
            let text = " clear ";
            let w = text.chars().count() as u16;
            let x = end.saturating_sub(w + 1);
            let style = if hover == Some(SearchHit::Clear) {
                theme.hint_key.add_modifier(Modifier::UNDERLINED)
            } else {
                theme.hint_desc
            };
            buf.set_string(x, by, text, style);
            self.geom.clear = Rect::new(x, by, w, 1);
        }

        let list = Rect::new(
            area.x,
            area.y + 3,
            area.width,
            area.height.saturating_sub(3),
        );
        self.geom.list = list;
        if list.height == 0 {
            return;
        }
        if self.items.is_empty() {
            let msg = if self.query.trim().is_empty() {
                "artists, records and tracks that match appear here"
            } else if self.shown != self.generation {
                "searching\u{2026}"
            } else {
                "nothing matches"
            };
            buf.set_stringn(
                list.x + 1,
                list.y,
                msg,
                list.width as usize,
                theme.hint_desc,
            );
            return;
        }
        if self.follow {
            self.keep_cursor_visible();
        }
        let rows = list.height as usize;
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(rows));
        let bar = self.lines.len() > rows && list.width > 8;
        let text_end = list.x + list.width - if bar { 3 } else { 0 };

        for r in 0..rows {
            let Some(line) = self.lines.get(self.scroll + r) else {
                break;
            };
            let y = list.y + r as u16;
            match line {
                Line::Section(name, n) => {
                    let label = format!("{name} ({n})");
                    buf.set_stringn(
                        list.x + 1,
                        y,
                        &label,
                        list.width as usize,
                        theme.heading.add_modifier(Modifier::BOLD),
                    );
                    let from = list.x + 2 + label.chars().count() as u16;
                    for cx in from..text_end {
                        buf[(cx, y)].set_char('\u{2500}').set_style(theme.rule);
                    }
                }
                Line::Item(i) => {
                    let i = *i;
                    self.render_item(buf, theme, i, y, list.x, text_end, hover);
                }
            }
        }

        if bar {
            let x = list.x + list.width - 1;
            self.geom.bar = Rect::new(x - 2, list.y, 3, list.height);
            self.geom.total = self.lines.len();
            let h = list.height as usize;
            let total = self.lines.len();
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

    #[allow(clippy::too_many_arguments)]
    fn render_item(
        &mut self,
        buf: &mut Buffer,
        theme: &Theme,
        i: usize,
        y: u16,
        x0: u16,
        end: u16,
        hover: Option<SearchHit>,
    ) {
        let selected = i == self.cursor && !self.typing;
        let hovered = matches!(
            hover,
            Some(SearchHit::Item(n) | SearchHit::Artist(n) | SearchHit::Album(n)) if n == i
        );
        if selected {
            for cx in x0..end {
                buf[(cx, y)].set_style(theme.library_cursor);
            }
            buf[(x0, y)].set_char('\u{258c}').set_style(theme.bar);
        } else if hovered {
            for cx in x0..end {
                buf[(cx, y)].set_style(theme.library_hover);
            }
        }
        let base = if selected {
            theme.library_cursor
        } else {
            theme.track_normal
        };
        let dim = if selected {
            theme.library_cursor
        } else {
            theme.hint_desc
        };
        let link = |on: bool| {
            if on {
                dim.add_modifier(Modifier::UNDERLINED)
            } else {
                dim
            }
        };
        let left = x0 + 2;
        match &self.items[i] {
            Item::Artist(a) => {
                let counts = format!(
                    "{} record{}",
                    a.album_count,
                    if a.album_count == 1 { "" } else { "s" }
                );
                let cw = counts.chars().count() as u16;
                let cx = end.saturating_sub(cw + 1);
                buf.set_string(cx, y, &counts, dim);
                let w = cx.saturating_sub(left + 2) as usize;
                buf.set_stringn(left, y, fit(&a.name, w), w, base);
            }
            Item::Album(a) => {
                let year = a
                    .date
                    .as_deref()
                    .and_then(|d| d.get(..4))
                    .unwrap_or("")
                    .to_string();
                let yw = year.chars().count() as u16;
                let yx = end.saturating_sub(yw + 1);
                buf.set_string(yx, y, &year, dim);
                let avail = yx.saturating_sub(left + 2) as usize;
                let title_w = if avail >= 30 { avail * 3 / 5 } else { avail };
                buf.set_stringn(left, y, fit(&a.title, title_w), title_w, base);
                if avail >= 30 {
                    let ax = left + title_w as u16 + 2;
                    let aw = avail - title_w - 2;
                    let shown = fit(&a.artist_name, aw);
                    let width = shown.chars().count() as u16;
                    buf.set_string(ax, y, &shown, link(hover == Some(SearchHit::Artist(i))));
                    self.geom
                        .links
                        .push((Rect::new(ax, y, width, 1), SearchHit::Artist(i)));
                }
            }
            Item::Track(t) => {
                let len = t
                    .duration_ms
                    .map(|d| format_time(d as u64))
                    .unwrap_or_default();
                let lw = len.chars().count() as u16;
                let lx = end.saturating_sub(lw + 1);
                buf.set_string(lx, y, &len, dim);
                let avail = lx.saturating_sub(left + 2) as usize;
                if avail < 24 {
                    buf.set_stringn(left, y, fit(&t.title, avail), avail, base);
                    return;
                }
                let title_w = avail * 2 / 5;
                let artist_w = avail * 3 / 10;
                let album_w = avail - title_w - artist_w - 4;
                buf.set_stringn(left, y, fit(&t.title, title_w), title_w, base);
                let artist_x = left + title_w as u16 + 2;
                let album_x = artist_x + artist_w as u16 + 2;
                for (text, x, w, hit, linked) in [
                    (
                        &t.artist_name,
                        artist_x,
                        artist_w,
                        SearchHit::Artist(i),
                        t.artist_id.is_some(),
                    ),
                    (
                        &t.album_title,
                        album_x,
                        album_w,
                        SearchHit::Album(i),
                        t.album_id.is_some(),
                    ),
                ] {
                    let shown = fit(text, w);
                    let width = shown.chars().count() as u16;
                    buf.set_string(x, y, &shown, link(linked && hover == Some(hit)));
                    if linked {
                        self.geom.links.push((Rect::new(x, y, width, 1), hit));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn theme() -> Theme {
        Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_builds_the_query_and_escape_clears_then_leaves() {
        let mut p = SearchPage::new();
        for c in "abc".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.query, "abc");
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.query, "ab");
        assert!(matches!(
            p.handle_key(key(KeyCode::Esc)),
            Some(Action::None)
        ));
        assert_eq!(p.query, "");
        assert!(p.handle_key(key(KeyCode::Esc)).is_none(), "now it leaves");
    }

    #[test]
    fn numbers_and_commands_are_text_while_typing() {
        let mut p = SearchPage::new();
        for c in "1a/".chars() {
            assert!(p.handle_key(key(KeyCode::Char(c))).is_some());
        }
        assert_eq!(p.query, "1a/");
    }

    fn artist(id: i64) -> Item {
        Item::Artist(ArtistRow {
            id,
            name: format!("artist {id}"),
            sort_name: None,
            remote_id: None,
            album_count: 0,
            track_count: 0,
        })
    }

    #[test]
    fn only_the_latest_query_is_shown_and_it_starts_at_the_top() {
        let mut p = SearchPage::new();
        p.generation = 1;
        assert!(p.show(1, (0..40).map(artist).collect()));
        p.cursor = 30;
        p.generation = 3;
        assert!(!p.show(2, vec![artist(99)]), "a stale result is dropped");
        assert_eq!(p.items.len(), 40);
        assert!(p.show(3, (0..40).map(artist).collect()));
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn page_keys_pass_through_the_box() {
        let mut p = SearchPage::new();
        assert!(p.handle_key(key(KeyCode::BackTab)).is_none());
        assert!(
            p.handle_key(key(KeyCode::Tab)).is_none(),
            "no results to move to"
        );
        p.generation = 1;
        p.show(1, vec![artist(1)]);
        assert!(p.handle_key(key(KeyCode::Tab)).is_some());
        assert!(!p.typing, "Tab moves to the results");
    }

    #[test]
    fn it_draws_at_any_size_with_and_without_results() {
        let mut p = SearchPage::new();
        let theme = theme();
        for w in 0..70 {
            for h in 0..16 {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                p.render(area, &mut buf, &theme, None);
            }
        }
        p.query = "x".into();
        p.typing = false;
        let area = Rect::new(0, 0, 60, 12);
        p.render(area, &mut Buffer::empty(area), &theme, None);
        assert!(p.hit(1, 1).is_some(), "the box is a target");
    }
}

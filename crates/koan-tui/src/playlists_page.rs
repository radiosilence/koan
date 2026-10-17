//! The Playlists page, and a playlist's own page. Both are lists in the shape
//! of History's: rows with links and buttons, a scrollbar, and keys that do
//! what the mouse does.

use std::time::{Duration, Instant};

use koan_core::db::queries::{PlaylistEntry, PlaylistRow};
use koan_core::player::commands::PlayerCommand;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::app::PickerAction;
use super::browse::fit;
use super::playlist_ops as ops;
use super::theme::Theme;
use super::transport::format_time;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How long a delete waits for its second press.
const CONFIRM: Duration = Duration::from_secs(4);

/// What a key or a click on either page asks of the app.
pub enum PlAction {
    None,
    Open(i64),
    /// Replace the queue and play.
    Command(PlayerCommand),
    /// Put tracks in the queue as the pickers do.
    Tracks(Vec<i64>, PickerAction),
    OpenAlbum(i64),
    OpenArtist(i64),
    Info(i64),
    /// Ask for a name, then make a playlist.
    New,
    Rename(i64, String),
    /// Choose a playlist to put these tracks in.
    Choose(Vec<i64>),
    Say(String),
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn scrollbar(buf: &mut Buffer, theme: &Theme, list: Rect, total: usize, scroll: usize) -> Rect {
    let rows = list.height as usize;
    let x = list.x + list.width - 1;
    let h = list.height as usize;
    let thumb = (h * rows / total).max(1);
    let top = (h - thumb) * scroll / (total - rows).max(1);
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
    Rect::new(x - 2, list.y, 3, list.height)
}

fn scroll_to(bar: Rect, total: usize, rows: usize, row: u16) -> usize {
    if bar.height == 0 {
        return 0;
    }
    let max = total.saturating_sub(rows);
    let frac = row.saturating_sub(bar.y) as f64 / bar.height.saturating_sub(1).max(1) as f64;
    ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max)
}

fn duration(ms: i64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        format!("{}h {:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

// ---------------------------------------------------------------- the list

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListHit {
    Row(usize),
    Play(usize),
    Delete(usize),
    New,
    Filter,
}

#[derive(Default, Clone)]
struct ListGeom {
    list: Rect,
    bar: Rect,
    total: usize,
    new: Rect,
    filter: Rect,
    links: Vec<(Rect, ListHit)>,
}

pub struct PlaylistsPage {
    all: Vec<PlaylistRow>,
    shown: Vec<usize>,
    cursor: usize,
    scroll: usize,
    follow: bool,
    pub filter: String,
    pub filter_active: bool,
    armed: Option<(i64, Instant)>,
    geom: ListGeom,
    last_click: Option<(usize, Instant)>,
}

impl Default for PlaylistsPage {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaylistsPage {
    pub fn new() -> Self {
        Self {
            all: Vec::new(),
            shown: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            filter: String::new(),
            filter_active: false,
            armed: None,
            geom: ListGeom::default(),
            last_click: None,
        }
    }

    pub fn reload(&mut self) {
        self.all = ops::list();
        self.refilter();
    }

    fn refilter(&mut self) {
        let needle = self.filter.to_lowercase();
        self.shown = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, p)| needle.is_empty() || p.name.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect();
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
        self.follow = true;
    }

    fn at(&self, idx: usize) -> Option<&PlaylistRow> {
        self.shown.get(idx).and_then(|&i| self.all.get(i))
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.shown.len().saturating_sub(1));
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
        let max = self.shown.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    fn armed_for(&self, id: i64) -> bool {
        self.armed
            .is_some_and(|(armed, at)| armed == id && at.elapsed() < CONFIRM)
    }

    /// Plays the playlist at `idx`, shuffled or not.
    fn play(&self, idx: usize, shuffled: bool) -> PlAction {
        let Some(list) = self.at(idx) else {
            return PlAction::None;
        };
        let entries = ops::entries(list.id);
        match ops::queue_command(&entries, None, shuffled) {
            Some(cmd) => PlAction::Command(cmd),
            None => PlAction::Say(format!("'{}' has nothing to play", list.name)),
        }
    }

    fn track_ids(&self, idx: usize) -> Vec<i64> {
        self.at(idx)
            .map(|p| ops::entries(p.id).iter().map(|e| e.track.id).collect())
            .unwrap_or_default()
    }

    /// Deletes the playlist at `idx` on the second press.
    fn delete(&mut self, idx: usize) -> PlAction {
        let Some((id, name)) = self.at(idx).map(|p| (p.id, p.name.clone())) else {
            return PlAction::None;
        };
        if !self.armed_for(id) {
            self.armed = Some((id, Instant::now()));
            return PlAction::Say(format!("press again to delete '{name}'"));
        }
        self.armed = None;
        let said = match ops::delete(id) {
            Ok(()) => format!("deleted '{name}'"),
            Err(e) => e,
        };
        self.reload();
        PlAction::Say(said)
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<PlAction> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if self.filter_active {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_active = false;
                    self.refilter();
                }
                KeyCode::Enter => self.filter_active = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.refilter();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.refilter();
                }
                _ => {}
            }
            return Some(PlAction::None);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        if key.code != KeyCode::Char('x') && key.code != KeyCode::Delete {
            self.armed = None;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('O') => {
                return Some(
                    self.at(self.cursor)
                        .map_or(PlAction::None, |p| PlAction::Open(p.id)),
                );
            }
            KeyCode::Char('P') => return Some(self.play(self.cursor, false)),
            KeyCode::Char('s') => return Some(self.play(self.cursor, true)),
            KeyCode::Char('a') => {
                return Some(PlAction::Tracks(
                    self.track_ids(self.cursor),
                    PickerAction::Append,
                ));
            }
            KeyCode::Char('N') => {
                return Some(PlAction::Tracks(
                    self.track_ids(self.cursor),
                    PickerAction::PlayNext,
                ));
            }
            KeyCode::Char('c') | KeyCode::Char('+') => return Some(PlAction::New),
            KeyCode::Char('r') => {
                return Some(
                    self.at(self.cursor)
                        .map_or(PlAction::None, |p| PlAction::Rename(p.id, p.name.clone())),
                );
            }
            KeyCode::Char('x') | KeyCode::Delete => return Some(self.delete(self.cursor)),
            KeyCode::Char('/') => self.filter_active = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refilter();
            }
            _ => return None,
        }
        Some(PlAction::None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<ListHit> {
        let g = &self.geom;
        if inside(g.new, col, row) {
            return Some(ListHit::New);
        }
        if inside(g.filter, col, row) {
            return Some(ListHit::Filter);
        }
        if let Some((_, hit)) = g.links.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if inside(g.list, col, row) {
            let idx = self.scroll + (row - g.list.y) as usize;
            return (idx < self.shown.len()).then_some(ListHit::Row(idx));
        }
        None
    }

    pub fn click(&mut self, hit: ListHit) -> PlAction {
        self.armed = self.armed.filter(|_| matches!(hit, ListHit::Delete(_)));
        match hit {
            ListHit::New => PlAction::New,
            ListHit::Filter => {
                self.filter_active = true;
                PlAction::None
            }
            ListHit::Play(i) => {
                self.move_to(i);
                self.play(i, false)
            }
            ListHit::Delete(i) => {
                self.move_to(i);
                self.delete(i)
            }
            ListHit::Row(i) => {
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                if double {
                    self.last_click = None;
                    self.at(i).map_or(PlAction::None, |p| PlAction::Open(p.id))
                } else {
                    self.last_click = Some((i, Instant::now()));
                    PlAction::None
                }
            }
        }
    }

    pub fn on_scrollbar(&self, col: u16, row: u16) -> bool {
        inside(self.geom.bar, col, row)
    }

    pub fn scroll_to_bar(&mut self, row: u16) {
        self.follow = false;
        self.scroll = scroll_to(self.geom.bar, self.geom.total, self.rows(), row);
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, hover: Option<ListHit>) {
        self.geom = ListGeom::default();
        if area.width < 12 || area.height < 3 {
            return;
        }
        super::ui::heading(
            buf,
            area,
            theme,
            "playlists",
            &format!(" {} ", self.all.len()),
        );
        let mut x = area.x + area.width;
        let mut control = |buf: &mut Buffer, text: String, style: Style| -> Rect {
            let w = (text.chars().count() as u16).min(x.saturating_sub(area.x));
            x -= w;
            buf.set_stringn(x, area.y, &text, w as usize, style);
            Rect::new(x, area.y, w, 1)
        };
        let underline = |on: bool, base: Style| {
            if on {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            }
        };
        self.geom.new = control(
            buf,
            " + new ".into(),
            underline(hover == Some(ListHit::New), theme.hint_key),
        );
        let filter_text = if self.filter_active || !self.filter.is_empty() {
            let cursor = if self.filter_active { "\u{2588}" } else { "" };
            format!(" /{}{} ", self.filter, cursor)
        } else {
            " / filter ".to_string()
        };
        self.geom.filter = control(
            buf,
            filter_text,
            underline(
                hover == Some(ListHit::Filter) || self.filter_active,
                theme.hint_desc,
            ),
        );

        let list = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        self.geom.list = list;
        if self.shown.is_empty() {
            let msg = if !self.filter.is_empty() {
                "no match"
            } else {
                "no playlists yet: + new makes one, and + on a record or track adds to one"
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
            self.move_to(self.cursor);
        }
        let rows = list.height as usize;
        self.scroll = self.scroll.min(self.shown.len().saturating_sub(rows));
        let bar = self.shown.len() > rows && list.width > 8;
        let end = list.x + list.width - if bar { 3 } else { 0 };

        for r in 0..rows {
            let idx = self.scroll + r;
            let Some(p) = self.at(idx).cloned() else {
                break;
            };
            let y = list.y + r as u16;
            let selected = idx == self.cursor;
            let hovered = matches!(
                hover,
                Some(ListHit::Row(n) | ListHit::Play(n) | ListHit::Delete(n)) if n == idx
            );
            if selected {
                for cx in list.x..end {
                    buf[(cx, y)].set_style(theme.library_cursor);
                }
                buf[(list.x, y)].set_char('\u{258c}').set_style(theme.bar);
            } else if hovered {
                for cx in list.x..end {
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
            // The buttons are the last six cells; the facts sit before them.
            let play = Rect::new(end.saturating_sub(6), y, 3, 1);
            let delete = Rect::new(end.saturating_sub(3), y, 3, 1);
            let armed = self.armed_for(p.id);
            buf.set_string(
                play.x + 1,
                y,
                "\u{25b8}",
                underline(hover == Some(ListHit::Play(idx)), theme.track_playing),
            );
            buf.set_string(
                delete.x + 1,
                y,
                if armed { "?" } else { "\u{d7}" },
                if armed {
                    theme.heading.add_modifier(Modifier::BOLD)
                } else {
                    underline(hover == Some(ListHit::Delete(idx)), dim)
                },
            );
            self.geom.links.push((play, ListHit::Play(idx)));
            self.geom.links.push((delete, ListHit::Delete(idx)));

            let facts = format!(
                "{} track{} \u{b7} {}",
                p.track_count,
                if p.track_count == 1 { "" } else { "s" },
                duration(p.duration_ms)
            );
            let facts_w = facts.chars().count() as u16;
            let facts_x = play.x.saturating_sub(facts_w + 2);
            buf.set_string(facts_x, y, &facts, dim);
            let mut name = p.name.clone();
            if p.readonly {
                name.push_str("  (read-only)");
            }
            let room = facts_x.saturating_sub(list.x + 3) as usize;
            buf.set_stringn(list.x + 2, y, fit(&name, room), room, base);
        }
        if bar {
            self.geom.total = self.shown.len();
            self.geom.bar = scrollbar(buf, theme, list, self.shown.len(), self.scroll);
        }
    }
}

// ----------------------------------------------------------- one playlist

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistHit {
    Row(usize),
    Artist(usize),
    Album(usize),
    Remove(usize),
    Up(usize),
    Down(usize),
    Play,
    Shuffle,
    Add,
    Undo,
    Redo,
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    bar: Rect,
    total: usize,
    links: Vec<(Rect, PlaylistHit)>,
}

pub struct PlaylistPage {
    pub list: PlaylistRow,
    entries: Vec<PlaylistEntry>,
    cursor: usize,
    scroll: usize,
    follow: bool,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

impl PlaylistPage {
    pub fn load(id: i64) -> Option<Self> {
        let list = ops::get(id)?;
        Some(Self {
            list,
            entries: ops::entries(id),
            cursor: 0,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
            last_click: None,
        })
    }

    #[cfg(test)]
    pub fn for_test(list: PlaylistRow, entries: Vec<PlaylistEntry>) -> Self {
        Self {
            list,
            entries,
            cursor: 0,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
            last_click: None,
        }
    }

    pub fn reload(&mut self) {
        if let Some(list) = ops::get(self.list.id) {
            self.list = list;
        }
        self.entries = ops::entries(self.list.id);
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        self.follow = true;
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.entries.len().saturating_sub(1));
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
        let max = self.entries.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    fn all_tracks(&self) -> Vec<i64> {
        self.entries.iter().map(|e| e.track.id).collect()
    }

    fn command(&self, start: Option<i64>, shuffled: bool) -> PlAction {
        match ops::queue_command(&self.entries, start, shuffled) {
            Some(cmd) => PlAction::Command(cmd),
            None => PlAction::Say("nothing to play".into()),
        }
    }

    fn refuse_if_readonly(&self) -> Option<PlAction> {
        self.list.readonly.then(|| {
            PlAction::Say(format!(
                "'{}' is read-only: its rules, its file or its server decide what it holds",
                self.list.name
            ))
        })
    }

    fn remove(&mut self, idx: usize) -> PlAction {
        if let Some(refused) = self.refuse_if_readonly() {
            return refused;
        }
        let Some(id) = self.entries.get(idx).map(|e| e.id) else {
            return PlAction::None;
        };
        let said = match ops::remove(self.list.id, &[id]) {
            Ok(_) => "removed from the playlist".to_string(),
            Err(e) => e,
        };
        self.reload();
        PlAction::Say(said)
    }

    /// Takes back (or redoes) the last edit made to this playlist.
    fn step(&mut self, undo: bool) -> PlAction {
        if let Some(refused) = self.refuse_if_readonly() {
            return refused;
        }
        let said = match ops::step(self.list.id, undo) {
            Ok(true) => if undo { "undone" } else { "redone" }.to_string(),
            Ok(false) => format!("nothing to {}", if undo { "undo" } else { "redo" }),
            Err(e) => e,
        };
        self.reload();
        PlAction::Say(said)
    }

    /// Moves the entry at `idx` one place, and the cursor with it.
    fn shift(&mut self, idx: usize, down: bool) -> PlAction {
        if let Some(refused) = self.refuse_if_readonly() {
            return refused;
        }
        let to = if down {
            idx + 1
        } else {
            match idx.checked_sub(1) {
                Some(to) => to,
                None => return PlAction::None,
            }
        };
        if to >= self.entries.len() {
            return PlAction::None;
        }
        let order: Vec<i64> = self.entries.iter().map(|e| e.id).collect();
        let result = ops::reorder(self.list.id, &ops::moved(&order, idx, to));
        self.reload();
        self.move_to(to);
        match result {
            Ok(()) => PlAction::None,
            Err(e) => PlAction::Say(e),
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<PlAction> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        let here = self.entries.get(self.cursor).map(|e| e.id);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => return Some(self.command(here, false)),
            KeyCode::Char('P') => return Some(self.command(None, false)),
            KeyCode::Char('s') => return Some(self.command(None, true)),
            KeyCode::Char('a') => {
                return Some(PlAction::Tracks(self.all_tracks(), PickerAction::Append));
            }
            KeyCode::Char('N') => {
                return Some(PlAction::Tracks(self.all_tracks(), PickerAction::PlayNext));
            }
            KeyCode::Char('+') => {
                return Some(
                    self.entries
                        .get(self.cursor)
                        .map_or(PlAction::None, |e| PlAction::Choose(vec![e.track.id])),
                );
            }
            KeyCode::Char('r') => {
                return Some(
                    self.refuse_if_readonly()
                        .unwrap_or_else(|| PlAction::Rename(self.list.id, self.list.name.clone())),
                );
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                return Some(self.remove(self.cursor));
            }
            KeyCode::Char('z') if ctrl => return Some(self.step(true)),
            KeyCode::Char('y') if ctrl => return Some(self.step(false)),
            KeyCode::Char('K') => return Some(self.shift(self.cursor, false)),
            KeyCode::Char('J') => return Some(self.shift(self.cursor, true)),
            KeyCode::Char('i') => {
                return Some(
                    self.entries
                        .get(self.cursor)
                        .map_or(PlAction::None, |e| PlAction::Info(e.track.id)),
                );
            }
            KeyCode::Char('A') => {
                return Some(
                    self.entries
                        .get(self.cursor)
                        .and_then(|e| e.track.artist_id)
                        .map_or(PlAction::None, PlAction::OpenArtist),
                );
            }
            KeyCode::Char('O') => {
                return Some(
                    self.entries
                        .get(self.cursor)
                        .and_then(|e| e.track.album_id)
                        .map_or(PlAction::None, PlAction::OpenAlbum),
                );
            }
            _ => return None,
        }
        Some(PlAction::None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<PlaylistHit> {
        let g = &self.geom;
        if let Some((_, hit)) = g.links.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if inside(g.list, col, row) {
            let idx = self.scroll + (row - g.list.y) as usize;
            return (idx < self.entries.len()).then_some(PlaylistHit::Row(idx));
        }
        None
    }

    pub fn click(&mut self, hit: PlaylistHit) -> PlAction {
        match hit {
            PlaylistHit::Play => self.command(None, false),
            PlaylistHit::Shuffle => self.command(None, true),
            PlaylistHit::Add => PlAction::Tracks(self.all_tracks(), PickerAction::Append),
            PlaylistHit::Undo => self.step(true),
            PlaylistHit::Redo => self.step(false),
            PlaylistHit::Remove(i) => self.remove(i),
            PlaylistHit::Up(i) => self.shift(i, false),
            PlaylistHit::Down(i) => self.shift(i, true),
            PlaylistHit::Artist(i) => {
                self.move_to(i);
                self.entries
                    .get(i)
                    .and_then(|e| e.track.artist_id)
                    .map_or(PlAction::None, PlAction::OpenArtist)
            }
            PlaylistHit::Album(i) => {
                self.move_to(i);
                self.entries
                    .get(i)
                    .and_then(|e| e.track.album_id)
                    .map_or(PlAction::None, PlAction::OpenAlbum)
            }
            PlaylistHit::Row(i) => {
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                if double {
                    self.last_click = None;
                    self.command(self.entries.get(i).map(|e| e.id), false)
                } else {
                    self.last_click = Some((i, Instant::now()));
                    PlAction::None
                }
            }
        }
    }

    pub fn on_scrollbar(&self, col: u16, row: u16) -> bool {
        inside(self.geom.bar, col, row)
    }

    pub fn scroll_to_bar(&mut self, row: u16) {
        self.follow = false;
        self.scroll = scroll_to(self.geom.bar, self.geom.total, self.rows(), row);
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<PlaylistHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 12 || area.height < 4 {
            return;
        }
        super::ui::heading(buf, area, theme, "playlist", "");
        let underline = |on: bool, base: Style| {
            if on {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            }
        };
        let tw = area.width.saturating_sub(2) as usize;
        buf.set_stringn(
            area.x + 1,
            area.y + 1,
            &self.list.name,
            tw,
            theme.heading.add_modifier(Modifier::BOLD),
        );
        let facts = format!(
            "{} track{} \u{b7} {}{}",
            self.list.track_count,
            if self.list.track_count == 1 { "" } else { "s" },
            duration(self.list.duration_ms),
            if self.list.readonly {
                " \u{b7} read-only"
            } else {
                ""
            },
        );
        buf.set_stringn(area.x + 1, area.y + 2, &facts, tw, theme.hint_desc);

        // The buttons; play is the one prominent action.
        let by = area.y + 3;
        let mut x = area.x + 1;
        let mut buttons = vec![
            (" play ", PlaylistHit::Play, true),
            (" shuffle ", PlaylistHit::Shuffle, false),
            (" add to queue ", PlaylistHit::Add, false),
        ];
        if !self.list.readonly {
            buttons.push((" undo ", PlaylistHit::Undo, false));
            buttons.push((" redo ", PlaylistHit::Redo, false));
        }
        for (label, hit, prominent) in buttons {
            let w = label.chars().count() as u16 + 2;
            if x + w > area.x + area.width {
                break;
            }
            let base = if prominent {
                theme.track_cursor
            } else {
                theme.track_normal
            };
            buf.set_string(
                x,
                by,
                format!("[{label}]"),
                underline(hover == Some(hit), base),
            );
            self.geom.links.push((Rect::new(x, by, w, 1), hit));
            x += w + 2;
        }

        let list = Rect::new(
            area.x,
            area.y + 5,
            area.width,
            area.height.saturating_sub(5),
        );
        self.geom.list = list;
        if list.height == 0 {
            return;
        }
        if self.entries.is_empty() {
            buf.set_stringn(
                list.x + 1,
                list.y,
                "empty: + on a record or a track puts it here",
                list.width as usize,
                theme.hint_desc,
            );
            return;
        }
        if self.follow {
            self.move_to(self.cursor);
        }
        let rows = list.height as usize;
        self.scroll = self.scroll.min(self.entries.len().saturating_sub(rows));
        let bar = self.entries.len() > rows && list.width > 8;
        let end = list.x + list.width - if bar { 3 } else { 0 };

        for r in 0..rows {
            let idx = self.scroll + r;
            let Some(e) = self.entries.get(idx) else {
                break;
            };
            let t = &e.track;
            let y = list.y + r as u16;
            let selected = idx == self.cursor;
            let hovered = matches!(
                hover,
                Some(
                    PlaylistHit::Row(n)
                        | PlaylistHit::Artist(n)
                        | PlaylistHit::Album(n)
                        | PlaylistHit::Remove(n)
                        | PlaylistHit::Up(n)
                        | PlaylistHit::Down(n)
                ) if n == idx
            );
            if selected {
                for cx in list.x..end {
                    buf[(cx, y)].set_style(theme.library_cursor);
                }
                buf[(list.x, y)].set_char('\u{258c}').set_style(theme.bar);
            } else if hovered {
                for cx in list.x..end {
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
            buf.set_string(list.x + 2, y, format!("{:>3}", idx + 1), dim);

            // Buttons: move up, move down, remove: the last nine cells.
            let remove = Rect::new(end.saturating_sub(3), y, 3, 1);
            let down = Rect::new(end.saturating_sub(6), y, 3, 1);
            let up = Rect::new(end.saturating_sub(9), y, 3, 1);
            for (rect, glyph, hit) in [
                (up, "\u{25b4}", PlaylistHit::Up(idx)),
                (down, "\u{25be}", PlaylistHit::Down(idx)),
                (remove, "\u{d7}", PlaylistHit::Remove(idx)),
            ] {
                buf.set_string(rect.x + 1, y, glyph, underline(hover == Some(hit), dim));
                self.geom.links.push((rect, hit));
            }
            let len = t
                .duration_ms
                .map(|d| format_time(d as u64))
                .unwrap_or_default();
            let len_w = len.chars().count() as u16;
            let len_x = up.x.saturating_sub(len_w + 1);
            buf.set_string(len_x, y, &len, dim);

            let left = list.x + 7;
            let room = len_x.saturating_sub(left + 2) as usize;
            if room < 24 {
                buf.set_stringn(left, y, fit(&t.title, room), room, base);
                continue;
            }
            let title_w = room * 2 / 5;
            let artist_w = room * 3 / 10;
            let album_w = room - title_w - artist_w - 4;
            buf.set_stringn(left, y, fit(&t.title, title_w), title_w, base);
            let artist_x = left + title_w as u16 + 2;
            let album_x = artist_x + artist_w as u16 + 2;
            for (text, x, w, hit, linked) in [
                (
                    &t.artist_name,
                    artist_x,
                    artist_w,
                    PlaylistHit::Artist(idx),
                    t.artist_id.is_some(),
                ),
                (
                    &t.album_title,
                    album_x,
                    album_w,
                    PlaylistHit::Album(idx),
                    t.album_id.is_some(),
                ),
            ] {
                let shown = fit(text, w);
                let width = shown.chars().count() as u16;
                buf.set_string(x, y, &shown, underline(linked && hover == Some(hit), dim));
                if linked {
                    self.geom.links.push((Rect::new(x, y, width, 1), hit));
                }
            }
        }
        if bar {
            self.geom.total = self.entries.len();
            self.geom.bar = scrollbar(buf, theme, list, self.entries.len(), self.scroll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};
    use koan_core::db::queries::TrackRow;

    fn theme() -> Theme {
        Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167))
    }

    fn track(id: i64, title: &str) -> TrackRow {
        TrackRow {
            id,
            album_id: Some(1),
            artist_id: Some(2),
            artist_name: "artist".into(),
            album_artist_name: "artist".into(),
            album_title: "record".into(),
            disc: None,
            track_number: Some(1),
            title: title.into(),
            duration_ms: Some(200_000),
            path: None,
            codec: None,
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

    fn row(id: i64, name: &str, readonly: bool) -> PlaylistRow {
        PlaylistRow {
            id,
            uid: id.to_string(),
            name: name.into(),
            comment: None,
            public: false,
            owner: None,
            user_id: 1,
            remote_id: None,
            created_at: String::new(),
            changed_at: String::new(),
            sort_order: 0,
            grouped: None,
            track_count: 3,
            duration_ms: 600_000,
            revision: 0,
            synced_revision: None,
            remote_changed: None,
            rules: None,
            source_path: None,
            readonly,
        }
    }

    fn page(readonly: bool) -> PlaylistPage {
        PlaylistPage::for_test(
            row(7, "mix", readonly),
            (0..3)
                .map(|i| PlaylistEntry {
                    id: 100 + i,
                    position: i,
                    track: track(i + 1, &format!("song {i}")),
                })
                .collect(),
        )
    }

    fn draw(p: &mut PlaylistPage, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf, &theme(), None);
        buf
    }

    fn list_page() -> PlaylistsPage {
        let mut p = PlaylistsPage::new();
        p.all = vec![row(1, "road trip", false), row(2, "smart one", true)];
        p.refilter();
        p
    }

    fn draw_list(p: &mut PlaylistsPage, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf, &theme(), None);
        buf
    }

    #[test]
    fn the_list_shows_each_playlist_with_its_size() {
        let mut p = list_page();
        let buf = draw_list(&mut p, 90, 10);
        let shown: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(shown.contains("road trip"));
        assert!(shown.contains("3 tracks"));
        assert!(shown.contains("(read-only)"));
    }

    #[test]
    fn a_second_click_opens_a_playlist() {
        let mut p = list_page();
        draw_list(&mut p, 90, 10);
        let hit = p.hit(10, 2).expect("a row");
        assert_eq!(hit, ListHit::Row(1));
        assert!(matches!(p.click(hit), PlAction::None));
        assert!(matches!(p.click(hit), PlAction::Open(2)));
    }

    #[test]
    fn the_list_has_buttons_to_play_and_delete() {
        let mut p = list_page();
        draw_list(&mut p, 90, 10);
        let kinds: Vec<_> = p.geom.links.iter().map(|(_, h)| *h).collect();
        assert!(kinds.contains(&ListHit::Play(0)));
        assert!(kinds.contains(&ListHit::Delete(0)));
        assert!(inside(p.geom.new, p.geom.new.x, p.geom.new.y));
    }

    #[test]
    fn a_delete_needs_a_second_press() {
        let mut p = list_page();
        draw_list(&mut p, 90, 10);
        let said = p.handle_key(KeyEvent::from(KeyCode::Char('x')));
        assert!(matches!(said, Some(PlAction::Say(s)) if s.contains("press again")));
        assert!(p.armed_for(1));
        p.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert!(!p.armed_for(1), "any other key disarms it");
    }

    #[test]
    fn c_asks_for_a_new_playlist_and_r_for_a_new_name() {
        let mut p = list_page();
        draw_list(&mut p, 90, 10);
        assert!(matches!(
            p.handle_key(KeyEvent::from(KeyCode::Char('c'))),
            Some(PlAction::New)
        ));
        assert!(matches!(
            p.handle_key(KeyEvent::from(KeyCode::Char('r'))),
            Some(PlAction::Rename(1, name)) if name == "road trip"
        ));
    }

    #[test]
    fn the_filter_narrows_the_list() {
        let mut p = list_page();
        p.handle_key(KeyEvent::from(KeyCode::Char('/')));
        for c in "smart".chars() {
            p.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(p.shown.len(), 1);
        p.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(p.shown.len(), 2);
    }

    #[test]
    fn a_playlist_page_lists_its_tracks_with_links() {
        let mut p = page(false);
        let buf = draw(&mut p, 100, 14);
        let shown: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(shown.contains("mix") && shown.contains("song 0") && shown.contains("song 2"));
        let (rect, hit) = p
            .geom
            .links
            .iter()
            .find(|(_, h)| matches!(h, PlaylistHit::Artist(0)))
            .cloned()
            .expect("an artist link");
        assert_eq!(p.hit(rect.x, rect.y), Some(hit));
        assert!(matches!(p.click(hit), PlAction::OpenArtist(2)));
    }

    #[test]
    fn a_read_only_playlist_refuses_edits() {
        let mut p = page(true);
        draw(&mut p, 100, 14);
        for c in ['x', 'J', 'K', 'r'] {
            let said = p.handle_key(KeyEvent::from(KeyCode::Char(c)));
            assert!(
                matches!(said, Some(PlAction::Say(s)) if s.contains("read-only")),
                "{c}"
            );
        }
    }

    #[test]
    fn plus_chooses_a_playlist_for_the_track_under_the_cursor() {
        let mut p = page(false);
        draw(&mut p, 100, 14);
        p.handle_key(KeyEvent::from(KeyCode::Down));
        assert!(matches!(
            p.handle_key(KeyEvent::from(KeyCode::Char('+'))),
            Some(PlAction::Choose(ids)) if ids == vec![2]
        ));
    }

    #[test]
    fn both_pages_draw_at_any_size() {
        let mut p = page(false);
        let mut l = list_page();
        for w in 0..70 {
            for h in 0..14 {
                draw(&mut p, w, h);
                draw_list(&mut l, w, h);
            }
        }
    }
}

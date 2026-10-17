//! The Playlists page, and a playlist's own page. Both are lists in the shape
//! of History's: rows with links and buttons, a scrollbar, and keys that do
//! what the mouse does.

use std::time::{Duration, Instant};

use koan_core::db::queries::{PlaylistEntry, PlaylistRow};
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueEntry, QueueEntryStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::actions_menu::Item;
use super::app::PickerAction;
use super::playlist_ops as ops;
use super::shell::{Link, Links, ToolItem, Toolbar, fit};
use super::theme::Theme;
use super::track_rows as rows;

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
    Favourite(i64),
    /// A menu for the row, with its heading.
    Menu(String, Vec<(String, Item)>),
    /// Ask where to write the playlist with this id and name.
    Export(i64, String),
    /// The playlist on screen, with this name, was deleted.
    Deleted(String),
    /// Something for the playlist's own page to do.
    Page(PageAct),
    /// Play the playlist with this id, shuffled or not.
    Play(i64, bool),
    /// Delete the playlist with this id, as asked for from its menu.
    Delete(i64),
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

/// Asks for a new name for the playlist, or says why it keeps its own.
fn rename(list: &PlaylistRow) -> PlAction {
    match ops::renamable(list) {
        Ok(()) => PlAction::Rename(list.id, list.name.clone()),
        Err(e) => PlAction::Say(e),
    }
}

// ---------------------------------------------------------------- the list

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListHit {
    Row(usize),
    Play(usize),
    Delete(usize),
}

#[derive(Default, Clone)]
struct ListGeom {
    list: Rect,
    bar: Rect,
    total: usize,
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

    /// Plays the playlist at `idx`.
    fn play(&self, idx: usize) -> PlAction {
        self.at(idx)
            .map_or(PlAction::None, |list| PlAction::Play(list.id, false))
    }

    /// The menu for the playlist at `idx`, as the apps' sidebar row has it.
    fn menu(&self, idx: usize) -> PlAction {
        let Some(list) = self.at(idx) else {
            return PlAction::None;
        };
        let item = |label: &str, action| (label.to_string(), Item::Playlist(action));
        let mut items = vec![
            item("Play", PlAction::Play(list.id, false)),
            item("Shuffle", PlAction::Play(list.id, true)),
            (String::new(), Item::Playlist(PlAction::None)),
        ];
        if list.source_path.is_none() {
            items.push(item(
                "Rename\u{2026}",
                PlAction::Rename(list.id, list.name.clone()),
            ));
        }
        items.extend([
            item(
                "Export as M3U8\u{2026}",
                PlAction::Export(list.id, list.name.clone()),
            ),
            (String::new(), Item::Playlist(PlAction::None)),
            item("Delete", PlAction::Delete(list.id)),
        ]);
        PlAction::Menu(list.name.clone(), items)
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
            KeyCode::Char('P') => return Some(self.play(self.cursor)),
            KeyCode::Char('w') => return Some(PlAction::Choose(self.track_ids(self.cursor))),
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
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('r') => return Some(self.at(self.cursor).map_or(PlAction::None, rename)),
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
            ListHit::Play(i) => {
                self.move_to(i);
                self.play(i)
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

    /// The page's toolbar: how many playlists, a new one, the filter and the
    /// menu.
    pub fn toolbar(&self) -> Toolbar {
        use crossterm::event::{KeyCode, KeyEvent};
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let filter = if self.filter_active || !self.filter.is_empty() {
            format!("/{}", self.filter)
        } else {
            "/ filter".into()
        };
        Toolbar::new("playlists")
            .count(rows::count(self.all.len(), "playlist"))
            .item(ToolItem::new("+ new", key('c')).priority(3))
            .item(
                ToolItem::new(filter, key('/'))
                    .on(self.filter_active)
                    .priority(2),
            )
            .item(ToolItem::new("actions", key('m')).priority(1))
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, hover: Option<ListHit>) {
        self.geom = ListGeom::default();
        if area.width < 12 || area.height == 0 {
            return;
        }
        let underline = |on: bool, base: Style| {
            if on {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            }
        };
        let list = area;
        self.geom.list = list;
        if self.shown.is_empty() {
            let msg = if !self.filter.is_empty() {
                "no match"
            } else {
                "no playlists yet: c makes one, and w on an album or a track adds to one"
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

/// What the page's own controls and menu ask of it, handed back to it by the
/// app so a menu item does exactly what its key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageAct {
    /// Play from this entry, or from the top; shuffled or not.
    Play(Option<i64>, bool),
    Remove(Vec<i64>),
    /// Move the entry one place, down or up.
    Shift(i64, bool),
    /// Shuffle the playlist itself, for good.
    Reorder,
    Grouped(bool),
    Undo,
    Redo,
    Rename,
    Export,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistHit {
    Row(usize),
    Heading(usize),
    Heart(usize),
    Menu(usize),
}

/// One line of a playlist's page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Heading { first: usize, last: usize },
    Entry(usize),
}

/// The entries in runs of one album, as they stand: by the album's id where
/// both have one, else by its title and artist. A track without an album
/// stands alone.
fn lines_of(entries: &[PlaylistEntry], grouped: bool) -> Vec<Line> {
    let same = |a: &PlaylistEntry, b: &PlaylistEntry| match (a.track.album_id, b.track.album_id) {
        (Some(x), Some(y)) => x == y,
        _ => {
            a.track.album_title == b.track.album_title
                && a.track.album_artist_name == b.track.album_artist_name
        }
    };
    let mut out = Vec::with_capacity(entries.len());
    let mut i = 0;
    while i < entries.len() {
        if !grouped || entries[i].track.album_title.is_empty() {
            out.push(Line::Entry(i));
            i += 1;
            continue;
        }
        let last = (i..entries.len())
            .take_while(|&j| same(&entries[i], &entries[j]))
            .last()
            .unwrap_or(i);
        out.push(Line::Heading { first: i, last });
        out.extend((i..=last).map(Line::Entry));
        i = last + 1;
    }
    out
}

/// What the queue says of the playlist's entries while it plays them: the one
/// playing, and those still arriving or that could not be fetched.
#[derive(Default)]
pub struct Live<'a> {
    pub queue: &'a [QueueEntry],
    pub links: Option<&'a Links>,
    pub favourites: Option<&'a std::collections::HashSet<i64>>,
    pub offline: bool,
    pub spinner: usize,
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
    lines: Vec<Line>,
    /// The line each entry is on.
    line_of: Vec<usize>,
    /// The entry under the cursor.
    cursor: usize,
    /// The first line shown.
    scroll: usize,
    follow: bool,
    armed: Option<Instant>,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

impl PlaylistPage {
    pub fn load(id: i64) -> Option<Self> {
        Some(Self::with(ops::get(id)?, ops::entries(id)))
    }

    fn with(list: PlaylistRow, entries: Vec<PlaylistEntry>) -> Self {
        let mut page = Self {
            list,
            entries,
            lines: Vec::new(),
            line_of: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            armed: None,
            geom: Geom::default(),
            last_click: None,
        };
        page.relayout();
        page
    }

    #[cfg(test)]
    pub fn for_test(list: PlaylistRow, entries: Vec<PlaylistEntry>) -> Self {
        Self::with(list, entries)
    }

    pub fn reload(&mut self) {
        if let Some(list) = ops::get(self.list.id) {
            self.list = list;
        }
        self.entries = ops::entries(self.list.id);
        self.relayout();
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        self.follow = true;
    }

    /// A playlist is a sequence someone chose, not a shelf of albums, so it is
    /// a row per track unless it was asked to group.
    fn grouped(&self) -> bool {
        self.list.grouped.unwrap_or(false)
    }

    fn relayout(&mut self) {
        self.lines = lines_of(&self.entries, self.grouped());
        self.line_of = vec![0; self.entries.len()];
        for (n, line) in self.lines.iter().enumerate() {
            if let Line::Entry(i) = line {
                self.line_of[*i] = n;
            }
        }
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.entries.len().saturating_sub(1));
        self.follow = true;
        let rows = self.rows();
        let line = self.line_of.get(self.cursor).copied().unwrap_or(0);
        // A heading stays in view over its first track.
        let top = match line.checked_sub(1).map(|l| self.lines[l]) {
            Some(Line::Heading { .. }) => line - 1,
            _ => line,
        };
        if top < self.scroll {
            self.scroll = top;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self.lines.len().saturating_sub(self.rows());
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
        self.list
            .readonly
            .then(|| PlAction::Say(ops::read_only(&self.list.name)))
    }

    fn here(&self) -> Option<&PlaylistEntry> {
        self.entries.get(self.cursor)
    }

    /// Does what a control or a menu item on this page asks.
    pub fn apply(&mut self, act: PageAct) -> PlAction {
        let fillable = matches!(
            act,
            PageAct::Remove(_)
                | PageAct::Shift(..)
                | PageAct::Reorder
                | PageAct::Undo
                | PageAct::Redo
        );
        if fillable && let Some(refused) = self.refuse_if_readonly() {
            return refused;
        }
        if act != PageAct::Delete {
            self.armed = None;
        }
        match act {
            PageAct::Play(start, shuffled) => self.command(start, shuffled),
            PageAct::Remove(ids) => {
                let said = match ops::remove(self.list.id, &ids) {
                    Ok(n) => format!("removed {} from the playlist", rows::count(n, "track")),
                    Err(e) => e,
                };
                self.reload();
                PlAction::Say(said)
            }
            PageAct::Shift(entry, down) => self.shift(entry, down),
            PageAct::Reorder => {
                if self.entries.len() < 2 {
                    return PlAction::None;
                }
                let result = ops::shuffle(self.list.id);
                self.reload();
                PlAction::Say(result.map_or_else(|e| e, |()| "shuffled the playlist".into()))
            }
            PageAct::Grouped(grouped) => {
                let result = ops::set_grouped(self.list.id, grouped);
                self.reload();
                result.map_or_else(PlAction::Say, |()| PlAction::None)
            }
            PageAct::Undo | PageAct::Redo => {
                let undo = act == PageAct::Undo;
                let said = match ops::step(self.list.id, undo) {
                    Ok(true) => if undo { "undone" } else { "redone" }.to_string(),
                    Ok(false) => format!("nothing to {}", if undo { "undo" } else { "redo" }),
                    Err(e) => e,
                };
                self.reload();
                PlAction::Say(said)
            }
            PageAct::Rename => rename(&self.list),
            PageAct::Export => PlAction::Export(self.list.id, self.list.name.clone()),
            PageAct::Delete => {
                if !self.armed.is_some_and(|at| at.elapsed() < CONFIRM) {
                    self.armed = Some(Instant::now());
                    return PlAction::Say(format!("press again to delete '{}'", self.list.name));
                }
                self.armed = None;
                match ops::delete(self.list.id) {
                    Ok(()) => PlAction::Deleted(self.list.name.clone()),
                    Err(e) => PlAction::Say(e),
                }
            }
        }
    }

    /// Moves `entry` one place, and the cursor with it.
    fn shift(&mut self, entry: i64, down: bool) -> PlAction {
        let Some(idx) = self.entries.iter().position(|e| e.id == entry) else {
            return PlAction::None;
        };
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
        match result {
            Ok(()) => {
                self.move_to(to);
                PlAction::None
            }
            Err(e) => PlAction::Say(e),
        }
    }

    /// The menu for the entry at `idx`, as the apps' playlist menu has it.
    fn menu(&self, idx: usize) -> PlAction {
        use super::actions_menu::Item::{Browse, Playlist};
        use super::browse::Action;
        let Some(e) = self.entries.get(idx) else {
            return PlAction::None;
        };
        let one = vec![e.track.id];
        let page = |act| Playlist(PlAction::Page(act));
        let mut items = vec![
            ("Play".to_string(), page(PageAct::Play(Some(e.id), false))),
            (
                "Play next".into(),
                Browse(Action::Play(one.clone(), PickerAction::PlayNext)),
            ),
            (
                "Add to queue".into(),
                Browse(Action::Play(one.clone(), PickerAction::Append)),
            ),
            (String::new(), Browse(Action::None)),
            (
                "Add to playlist\u{2026}".into(),
                Browse(Action::Choose(one)),
            ),
        ];
        if !self.list.readonly {
            items.extend([
                (String::new(), Browse(Action::None)),
                ("Move up".into(), page(PageAct::Shift(e.id, false))),
                ("Move down".into(), page(PageAct::Shift(e.id, true))),
                (
                    "Remove from playlist".into(),
                    page(PageAct::Remove(vec![e.id])),
                ),
            ]);
        }
        items.extend([
            (String::new(), Browse(Action::None)),
            (
                "Favourite or unfavourite".into(),
                Browse(Action::Favourite(e.track.id)),
            ),
            ("Track info".into(), Browse(Action::Info(e.track.id))),
        ]);
        if let Some(album) = e.track.album_id {
            items.push(("Go to album".into(), Browse(Action::OpenAlbum(album))));
        }
        if let Some(artist) = e.track.artist_id {
            items.push(("Go to artist".into(), Browse(Action::OpenArtist(artist))));
        }
        PlAction::Menu(e.track.title.clone(), items)
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<PlAction> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        let here = self.here().map(|e| (e.id, e.track.clone()));
        let act = |a| Some(PlAction::Page(a));
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => return act(PageAct::Play(here.map(|(id, _)| id), false)),
            KeyCode::Char('P') => return act(PageAct::Play(None, false)),
            KeyCode::Char('S') => return act(PageAct::Play(None, true)),
            KeyCode::Char('Y') => return act(PageAct::Reorder),
            KeyCode::Char('e') => return act(PageAct::Export),
            KeyCode::Char('X') => return act(PageAct::Delete),
            KeyCode::Char('a') => {
                return Some(PlAction::Tracks(self.all_tracks(), PickerAction::Append));
            }
            KeyCode::Char('N') => {
                return Some(PlAction::Tracks(self.all_tracks(), PickerAction::PlayNext));
            }
            KeyCode::Char('t') => return act(PageAct::Grouped(!self.grouped())),
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('w') => {
                return Some(here.map_or(PlAction::None, |(_, t)| PlAction::Choose(vec![t.id])));
            }
            KeyCode::Char('r') => return act(PageAct::Rename),
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                return Some(here.map_or(PlAction::None, |(id, _)| {
                    PlAction::Page(PageAct::Remove(vec![id]))
                }));
            }
            KeyCode::Char('z') if ctrl => return act(PageAct::Undo),
            KeyCode::Char('y') if ctrl => return act(PageAct::Redo),
            KeyCode::Char('K') | KeyCode::Char('J') => {
                let down = key.code == KeyCode::Char('J');
                return Some(here.map_or(PlAction::None, |(id, _)| {
                    PlAction::Page(PageAct::Shift(id, down))
                }));
            }
            KeyCode::Char('f') => {
                return Some(here.map_or(PlAction::None, |(_, t)| PlAction::Favourite(t.id)));
            }
            KeyCode::Char('i') => {
                return Some(here.map_or(PlAction::None, |(_, t)| PlAction::Info(t.id)));
            }
            KeyCode::Char('A') => {
                return Some(
                    here.and_then(|(_, t)| t.artist_id)
                        .map_or(PlAction::None, PlAction::OpenArtist),
                );
            }
            KeyCode::Char('O') => {
                return Some(
                    here.and_then(|(_, t)| t.album_id)
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
            return match self.lines.get(self.scroll + (row - g.list.y) as usize)? {
                Line::Entry(i) => Some(PlaylistHit::Row(*i)),
                Line::Heading { first, .. } => Some(PlaylistHit::Heading(*first)),
            };
        }
        None
    }

    pub fn click(&mut self, hit: PlaylistHit) -> PlAction {
        match hit {
            PlaylistHit::Menu(i) => {
                self.move_to(i);
                self.menu(i)
            }
            PlaylistHit::Heart(i) => {
                self.move_to(i);
                self.entries
                    .get(i)
                    .map_or(PlAction::None, |e| PlAction::Favourite(e.track.id))
            }
            PlaylistHit::Heading(first) => {
                self.move_to(first);
                PlAction::None
            }
            PlaylistHit::Row(i) => {
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                if double {
                    self.last_click = None;
                    self.apply(PageAct::Play(self.entries.get(i).map(|e| e.id), false))
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

    /// The toolbar: the name, what it holds, and the controls that apply to
    /// this playlist, each a key of the page's.
    pub fn toolbar(&self) -> Toolbar {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let mut facts = vec![rows::count(self.entries.len(), "track")];
        let ms: u64 = self
            .entries
            .iter()
            .filter_map(|e| e.track.duration_ms)
            .map(|d| d as u64)
            .sum();
        if ms > 0 {
            facts.push(rows::total(ms));
        }
        if self.list.rules.is_some() {
            facts.push("smart".into());
        } else if self.list.readonly {
            facts.push("read-only".into());
        }
        if self.list.remote_id.is_some() {
            facts.push("synced".into());
        }
        let fillable = !self.list.readonly;
        let armed = self.armed.is_some_and(|at| at.elapsed() < CONFIRM);
        let mut bar = Toolbar::new(self.list.name.clone())
            .count(facts.join(" \u{b7} "))
            .item(ToolItem::new("play", key('P')).on(true).priority(9))
            .item(ToolItem::new("shuffle", key('S')).priority(8))
            .item(ToolItem::new("next", key('N')).priority(6))
            .item(ToolItem::new("queue", key('a')).priority(7));
        if fillable && self.entries.len() > 1 {
            bar = bar.item(ToolItem::new("reorder", key('Y')).priority(1));
        }
        bar = bar.item(
            ToolItem::new(
                if self.grouped() {
                    "by album"
                } else {
                    "by track"
                },
                key('t'),
            )
            .priority(5),
        );
        if fillable {
            bar = bar
                .item(ToolItem::new("undo", ctrl('z')).priority(4))
                .item(ToolItem::new("redo", ctrl('y')).priority(3));
        }
        if self.list.source_path.is_none() {
            bar = bar.item(ToolItem::new("rename", key('r')).priority(2));
        }
        bar.item(ToolItem::new("export", key('e')).priority(0))
            .item(
                ToolItem::new(if armed { "delete?" } else { "delete" }, key('X'))
                    .on(armed)
                    .priority(0),
            )
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<PlaylistHit>,
        live: &Live,
    ) {
        self.geom = Geom::default();
        if area.width < 12 || area.height == 0 {
            return;
        }
        let list = area;
        self.geom.list = list;
        if list.height == 0 {
            return;
        }
        if self.entries.is_empty() {
            let why = if self.list.rules.is_some() {
                "nothing in the library matches its rules yet"
            } else if self.list.readonly {
                "none of its tracks are in the library"
            } else {
                "nothing in here yet: w on an album or a track puts it here"
            };
            buf.set_stringn(
                list.x + 1,
                list.y,
                why,
                list.width.saturating_sub(1) as usize,
                theme.hint_desc,
            );
            return;
        }
        if self.follow {
            self.move_to(self.cursor);
        }
        let rows_shown = list.height as usize;
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(rows_shown));
        let bar = self.lines.len() > rows_shown && list.width > 8;
        // The row's cells, less the menu button and the scrollbar.
        let menu_x = list.x + list.width.saturating_sub(4);
        let row_w = menu_x.saturating_sub(list.x + 1);

        // What the queue says of these entries while it plays them.
        let mut live_status = std::collections::HashMap::new();
        for q in live.queue {
            if let Some(entry) = q.playlist_entry_id {
                live_status.insert(entry, q);
            }
        }
        for r in 0..rows_shown {
            let n = self.scroll + r;
            let Some(&line) = self.lines.get(n) else {
                break;
            };
            let y = list.y + r as u16;
            let rect = Rect::new(list.x, y, row_w, 1);
            match line {
                Line::Heading { first, .. } => {
                    let t = &self.entries[first].track;
                    let artist = if t.album_artist_name.is_empty() {
                        "Unknown Artist"
                    } else {
                        &t.album_artist_name
                    };
                    rows::heading(
                        buf,
                        rect,
                        theme,
                        &rows::Heading {
                            album: (&t.album_title, t.album_id.map(Link::Album)),
                            artist: (artist, Some(Link::ArtistOfTrack(t.id))),
                            facts: "",
                            selected: false,
                        },
                        live.links,
                    );
                }
                Line::Entry(i) => {
                    let e = &self.entries[i];
                    let t = &e.track;
                    let queued = live_status.get(&e.id);
                    let status = match queued.map(|q| q.status) {
                        Some(QueueEntryStatus::Playing) => rows::Status::Playing,
                        Some(QueueEntryStatus::PriorityPending) => rows::Status::Pending,
                        Some(QueueEntryStatus::Failed) => rows::Status::Failed,
                        _ => rows::Status::None,
                    };
                    let on_disk = t.path.is_some() || t.cached_path.is_some();
                    let avail = match queued {
                        Some(q) if q.status == QueueEntryStatus::Downloading => {
                            rows::Avail::Transferring(q.download_progress.and_then(
                                |(done, total)| (total > 0).then(|| done as f64 / total as f64),
                            ))
                        }
                        _ => rows::Avail::of(t.remote_id.is_some(), on_disk, live.offline),
                    };
                    let selected = i == self.cursor;
                    let hovered = matches!(
                        hover,
                        Some(PlaylistHit::Row(n) | PlaylistHit::Heart(n) | PlaylistHit::Menu(n)) if n == i
                    );
                    let grouped = self.grouped();
                    let row = rows::Track {
                        marker: if selected {
                            rows::Marker::Cursor
                        } else {
                            rows::Marker::None
                        },
                        status,
                        number: Some(format!("{}", i + 1)),
                        title: &t.title,
                        artist: Some((&t.artist_name, t.artist_id.map(Link::Artist))),
                        album: (!grouped)
                            .then_some((t.album_title.as_str(), t.album_id.map(Link::Album))),
                        avail,
                        favourite: Some(live.favourites.is_some_and(|f| f.contains(&t.id))),
                        codec: t.codec.as_deref(),
                        duration_ms: t.duration_ms.map(|d| d as u64),
                        dim: avail == rows::Avail::NotHere,
                        selected: false,
                        hovered: hovered && !selected,
                        current: status == rows::Status::Playing,
                        spinner: live.spinner,
                    };
                    rows::track(buf, rect, theme, &row, live.links);
                    if let Some(hx) = rows::Columns::new(rect.x, rect.width).heart {
                        self.geom
                            .links
                            .push((Rect::new(hx, y, 1, 1), PlaylistHit::Heart(i)));
                    }
                    if selected || hovered {
                        buf[(menu_x, y)]
                            .set_char('\u{22ef}')
                            .set_style(theme.hint_key);
                        self.geom
                            .links
                            .push((Rect::new(menu_x, y, 1, 1), PlaylistHit::Menu(i)));
                    }
                }
            }
        }
        if bar {
            self.geom.total = self.lines.len();
            self.geom.bar = scrollbar(buf, theme, list, self.lines.len(), self.scroll);
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
        p.render(area, &mut buf, &theme(), None, &Live::default());
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
        let hit = p.hit(10, 1).expect("a row");
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
        let labels: Vec<String> = p.toolbar().items.into_iter().map(|i| i.label).collect();
        assert_eq!(labels, ["+ new", "/ filter", "actions"]);
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
        let links = Links::default();
        links.begin((0, 0));
        let area = Rect::new(0, 0, 100, 14);
        let mut buf = Buffer::empty(area);
        let live = Live {
            links: Some(&links),
            ..Default::default()
        };
        p.render(area, &mut buf, &theme(), None, &live);
        let shown: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(shown.contains("song 0") && shown.contains("song 2"));
        assert_eq!(p.toolbar().title, "mix");
        let row: String = (0..100).map(|x| buf[(x, 0)].symbol()).collect();
        let x = row.find("artist").expect("the artist is drawn") as u16;
        assert_eq!(links.at(x, 0), Some(Link::Artist(2)));
    }

    #[test]
    fn a_read_only_playlist_refuses_edits() {
        let mut p = page(true);
        draw(&mut p, 100, 14);
        for c in ['x', 'J', 'K'] {
            let Some(PlAction::Page(act)) = p.handle_key(KeyEvent::from(KeyCode::Char(c))) else {
                panic!("{c} is the page's");
            };
            let said = p.apply(act);
            assert!(
                matches!(said, PlAction::Say(s) if s.contains("read-only")),
                "{c}"
            );
        }
        let labels: Vec<String> = p.toolbar().items.into_iter().map(|i| i.label).collect();
        for refused in ["reorder", "undo", "redo"] {
            assert!(
                !labels.iter().any(|l| l == refused),
                "no {refused} for edits it refuses"
            );
        }
        let Some(PlAction::Page(act)) = p.handle_key(KeyEvent::from(KeyCode::Char('r'))) else {
            panic!("r is the page's");
        };
        assert!(
            matches!(p.apply(act), PlAction::Rename(7, _)),
            "a smart playlist can still be renamed"
        );
    }

    #[test]
    fn a_playlist_named_by_its_file_is_not_offered_a_new_name() {
        let mut p = page(true);
        p.list.source_path = Some("/music/mix.m3u8".into());
        draw(&mut p, 100, 14);
        assert!(matches!(
            p.apply(PageAct::Rename),
            PlAction::Say(s) if s.contains("named by its file")
        ));
        assert!(!p.toolbar().items.iter().any(|i| i.label == "rename"));
    }

    #[test]
    fn w_chooses_a_playlist_for_the_track_under_the_cursor() {
        let mut p = page(false);
        draw(&mut p, 100, 14);
        p.handle_key(KeyEvent::from(KeyCode::Down));
        assert!(matches!(
            p.handle_key(KeyEvent::from(KeyCode::Char('w'))),
            Some(PlAction::Choose(ids)) if ids == vec![2]
        ));
    }

    #[test]
    fn s_is_left_to_the_app() {
        let mut p = page(false);
        draw(&mut p, 100, 14);
        let mut l = list_page();
        draw_list(&mut l, 100, 14);
        let s = KeyEvent::from(KeyCode::Char('s'));
        assert!(p.handle_key(s).is_none(), "shuffle stays the app's");
        assert!(l.handle_key(s).is_none(), "shuffle stays the app's");
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

    fn album_entries() -> Vec<PlaylistEntry> {
        (0..5)
            .map(|i| {
                let mut t = track(i + 1, &format!("song {i}"));
                t.album_id = Some(if i < 3 { 1 } else { 2 });
                t.album_title = if i < 3 {
                    "first".into()
                } else {
                    "second".into()
                };
                PlaylistEntry {
                    id: 100 + i,
                    position: i,
                    track: t,
                }
            })
            .collect()
    }

    #[test]
    fn a_playlist_is_a_row_per_track_until_it_is_grouped() {
        let mut list = row(7, "mix", false);
        let mut p = PlaylistPage::for_test(list.clone(), album_entries());
        assert!(p.lines.iter().all(|l| matches!(l, Line::Entry(_))));
        list.grouped = Some(true);
        p = PlaylistPage::for_test(list, album_entries());
        assert_eq!(
            p.lines,
            [
                Line::Heading { first: 0, last: 2 },
                Line::Entry(0),
                Line::Entry(1),
                Line::Entry(2),
                Line::Heading { first: 3, last: 4 },
                Line::Entry(3),
                Line::Entry(4),
            ]
        );
        let buf = draw(&mut p, 100, 14);
        // The list starts under the name and the buttons; a heading's row is
        // not a track's.
        assert_eq!(p.hit(60, 0), Some(PlaylistHit::Heading(0)));
        assert_eq!(p.hit(10, 1), Some(PlaylistHit::Row(0)));
        assert_eq!(p.hit(10, 5), Some(PlaylistHit::Row(3)));
        let heading: String = (0..100).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(heading.contains("first"), "{heading:?}");
    }

    #[test]
    fn the_menu_offers_what_the_playlist_takes() {
        let labels = |readonly| -> Vec<String> {
            let p = page(readonly);
            let PlAction::Menu(_, items) = p.menu(0) else {
                panic!("a menu");
            };
            items.into_iter().map(|(l, _)| l).collect()
        };
        assert!(labels(false).contains(&"Remove from playlist".to_string()));
        assert!(!labels(true).contains(&"Remove from playlist".to_string()));
        assert!(labels(true).contains(&"Play next".to_string()));
    }

    #[test]
    fn a_delete_needs_a_second_press_on_the_page_too() {
        let mut p = page(false);
        assert!(matches!(p.apply(PageAct::Delete), PlAction::Say(s) if s.contains("press again")));
        p.apply(PageAct::Rename);
        assert!(
            matches!(p.apply(PageAct::Delete), PlAction::Say(s) if s.contains("press again")),
            "anything else in between disarms it"
        );
    }

    #[test]
    fn a_playlist_in_the_list_has_the_apps_menu() {
        let mut p = list_page();
        p.all[0].source_path = Some("/music/a.m3u8".into());
        let PlAction::Menu(_, items) = p.menu(0) else {
            panic!("a menu");
        };
        let labels: Vec<&str> = items.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Play",
                "Shuffle",
                "",
                "Export as M3U8\u{2026}",
                "",
                "Delete"
            ],
            "a playlist named by its file is not offered a rename"
        );
    }
}

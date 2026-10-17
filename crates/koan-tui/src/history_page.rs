//! The History page: every play, most recent first, under the day it was
//! played, as the apps' history page lists them. Plays are events, not
//! tracks: an album played three times is three runs of rows. Plays can be
//! picked out together, played, queued and forgotten, here and, signed in to
//! a koan server, on the account's other devices.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeZone};
use crossbeam_channel::{Receiver, Sender};
use koan_core::db::queries::{self, PlayHistoryRow};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::actions_menu::{Playable, playable};
use super::app::PickerAction;
use super::browse::Action;
use super::shell::{Link, Links, ToolItem, Toolbar};
use super::text::{fit, width};
use super::theme::Theme;
use super::transport::format_time;

/// The most plays held at once; the page filters, so older ones stay a search away.
const LIMIT: u32 = 5000;
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How long "forget all" waits for the second press.
const CONFIRM: Duration = Duration::from_secs(4);
/// The length column, room for an hour and a space.
const LENGTH_W: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryHit {
    Play(usize),
    Forget(usize),
}

enum Line {
    Day(String),
    Play(usize),
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    bar: Rect,
    links: Vec<(Rect, HistoryHit)>,
    total: usize,
}

enum Forget {
    Plays(Vec<i64>),
    Everything,
}

/// A read of the plays under `filter`, after forgetting what `forget` names.
struct Request {
    generation: u64,
    forget: Option<Forget>,
    filter: String,
}

struct Reply {
    generation: u64,
    /// The history's version the plays were read at.
    version: u64,
    entries: Option<Vec<PlayHistoryRow>>,
    error: Option<String>,
}

/// Reads and forgets plays off the TUI thread. A burst of requests is
/// answered with one read, after every forgetting in it.
fn spawn_reader() -> (Sender<Request>, Receiver<Reply>) {
    let (tx, req) = crossbeam_channel::unbounded::<Request>();
    let (done, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("koan-history".into())
        .spawn(move || {
            while let Ok(mut next) = req.recv() {
                let mut error = None;
                loop {
                    if let Some(forget) = next.forget.take()
                        && let Err(e) = forget_plays(forget)
                    {
                        error = Some(e);
                    }
                    match req.try_recv() {
                        Ok(newer) => next = newer,
                        Err(_) => break,
                    }
                }
                let version = koan_core::player::history::version();
                let entries = match read(&next.filter) {
                    Ok(entries) => Some(entries),
                    Err(e) => {
                        error = Some(e);
                        None
                    }
                };
                let reply = Reply {
                    generation: next.generation,
                    version,
                    entries,
                    error,
                };
                if done.send(reply).is_err() {
                    return;
                }
            }
        })
        .ok();
    (tx, rx)
}

fn read(filter: &str) -> Result<Vec<PlayHistoryRow>, String> {
    let db = koan_core::db::pool::shared()
        .get()
        .map_err(|e| format!("history could not be read: {e}"))?;
    let search = (!filter.is_empty()).then_some(filter);
    queries::play_history_with_tracks(&db.conn, queries::LOCAL_USER, search, Some(LIMIT), 0)
        .map_err(|e| format!("history could not be read: {e}"))
}

fn forget_plays(forget: Forget) -> Result<(), String> {
    let failed = |e: String| format!("could not forget: {e}");
    let db = koan_core::db::pool::shared()
        .get()
        .map_err(|e| failed(e.to_string()))?;
    match forget {
        Forget::Plays(ids) => koan_core::remote::history::forget(&db, &ids),
        Forget::Everything => koan_core::remote::history::clear(&db),
    }
    .map_err(|e| failed(e.to_string()))?;
    koan_core::player::history::changed();
    Ok(())
}

pub struct HistoryPage {
    entries: Vec<PlayHistoryRow>,
    lines: Vec<Line>,
    /// The line each entry is on.
    line_of: Vec<usize>,
    /// The day the headings were named against.
    today: NaiveDate,
    cursor: usize,
    scroll: usize,
    follow: bool,
    pub filter: String,
    pub filter_active: bool,
    armed: Option<Instant>,
    /// The plays picked out, by their ids, and where a range starts.
    picked: HashSet<i64>,
    anchor: Option<usize>,
    /// The history's version when it was read; a play moves it on.
    seen: u64,
    /// The last read asked for, and the last one shown.
    generation: u64,
    shown: u64,
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn day_of(secs: i64) -> Option<NaiveDate> {
    Some(Local.timestamp_opt(secs, 0).single()?.date_naive())
}

fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    match (today - day).num_days() {
        0 => "today".into(),
        1 => "yesterday".into(),
        _ => day.format("%A %-d %B %Y").to_string().to_lowercase(),
    }
}

impl Default for HistoryPage {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryPage {
    pub fn new() -> Self {
        let (tx, rx) = spawn_reader();
        Self {
            entries: Vec::new(),
            lines: Vec::new(),
            line_of: Vec::new(),
            today: Local::now().date_naive(),
            cursor: 0,
            scroll: 0,
            follow: true,
            filter: String::new(),
            filter_active: false,
            armed: None,
            picked: HashSet::new(),
            anchor: None,
            seen: 0,
            generation: 0,
            shown: 0,
            tx,
            rx,
            geom: Geom::default(),
            last_click: None,
        }
    }

    /// Takes in what the reader has finished, asks again if a play was
    /// recorded or forgotten since the last read, and renames the days once
    /// midnight passes, so the page stays current while it is open. Returns
    /// what failed, to be said.
    pub fn poll(&mut self) -> Option<String> {
        let mut error = None;
        while let Ok(reply) = self.rx.try_recv() {
            error = reply.error.or(error);
            if reply.generation != self.generation {
                continue;
            }
            // A failed read still counts as shown, so the next play asks again.
            self.shown = reply.generation;
            self.seen = reply.version;
            if let Some(entries) = reply.entries {
                self.take(entries);
            }
        }
        if self.shown == self.generation && koan_core::player::history::version() != self.seen {
            self.reload();
        }
        let today = Local::now().date_naive();
        if today != self.today {
            self.today = today;
            self.rebuild();
        }
        error
    }

    /// Reads the plays again, narrowed by the filter.
    pub fn reload(&mut self) {
        self.ask(None);
    }

    fn ask(&mut self, forget: Option<Forget>) {
        self.generation += 1;
        let request = Request {
            generation: self.generation,
            forget,
            filter: self.filter.clone(),
        };
        self.tx.send(request).ok();
    }

    /// Shows `entries`, keeping the selected play selected and where it was
    /// on screen when it is still among them.
    fn take(&mut self, entries: Vec<PlayHistoryRow>) {
        let kept = self
            .entries
            .get(self.cursor)
            .map(|e| (e.id, self.line_of[self.cursor]));
        self.entries = entries;
        let ids: HashSet<i64> = self.entries.iter().map(|e| e.id).collect();
        self.picked.retain(|id| ids.contains(id));
        self.today = Local::now().date_naive();
        self.rebuild();
        let Some((id, was)) = kept else { return };
        if let Some(idx) = self.entries.iter().position(|e| e.id == id) {
            self.cursor = idx;
            if self.scroll > 0 {
                let now = self.line_of[idx];
                self.scroll = (self.scroll + now).saturating_sub(was);
            }
        }
    }

    fn rebuild(&mut self) {
        self.lines.clear();
        self.line_of.clear();
        let mut current = None;
        for (i, e) in self.entries.iter().enumerate() {
            let day = day_of(e.played_at);
            if day != current || self.lines.is_empty() {
                current = day;
                let label = day.map_or_else(|| "earlier".into(), |d| day_label(d, self.today));
                self.lines.push(Line::Day(label));
            }
            self.line_of.push(self.lines.len());
            self.lines.push(Line::Play(i));
        }
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn keep_cursor_visible(&mut self) {
        let Some(&line) = self.line_of.get(self.cursor) else {
            return;
        };
        let rows = self.rows();
        // A heading above the first play of a day stays with it.
        let top = if line > 0 && matches!(self.lines[line - 1], Line::Day(_)) {
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
        self.cursor = idx.min(self.entries.len().saturating_sub(1));
        self.follow = true;
        self.keep_cursor_visible();
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self.lines.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    fn track_at(&self, idx: usize) -> Option<&queries::TrackRow> {
        self.entries.get(idx).map(|e| &e.track)
    }

    /// The plays an action is for: those picked out, in the page's order, or
    /// the one at `idx`.
    fn chosen(&self, idx: usize) -> Vec<&PlayHistoryRow> {
        if self.picked.is_empty() {
            return self.entries.get(idx).into_iter().collect();
        }
        self.entries
            .iter()
            .filter(|e| self.picked.contains(&e.id))
            .collect()
    }

    fn chosen_tracks(&self, idx: usize) -> Vec<i64> {
        self.chosen(idx).iter().map(|e| e.track.id).collect()
    }

    fn play(&self, idx: usize, how: PickerAction) -> Action {
        let ids = self.chosen_tracks(idx);
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, how)
        }
    }

    /// Forgets the plays picked out, or the one at `idx`. The rows go at
    /// once, so the next press acts on the play below them; the read that
    /// follows the forgetting puts them back if that failed.
    fn forget(&mut self, idx: usize) {
        let ids: Vec<i64> = self.chosen(idx).iter().map(|e| e.id).collect();
        if ids.is_empty() {
            return;
        }
        self.picked.clear();
        self.anchor = None;
        let above = self.entries[..self.cursor.min(self.entries.len())]
            .iter()
            .filter(|e| ids.contains(&e.id))
            .count();
        self.entries.retain(|e| !ids.contains(&e.id));
        self.cursor = (self.cursor - above).min(self.entries.len().saturating_sub(1));
        self.rebuild();
        self.keep_cursor_visible();
        self.ask(Some(Forget::Plays(ids)));
    }

    /// Picks out the plays from the anchor to `idx`.
    fn pick_to(&mut self, idx: usize) {
        let anchor = *self.anchor.get_or_insert(self.cursor);
        let (lo, hi) = (anchor.min(idx), anchor.max(idx));
        self.picked = self.entries[lo..=hi.min(self.entries.len() - 1)]
            .iter()
            .map(|e| e.id)
            .collect();
    }

    fn toggle_pick(&mut self, idx: usize) {
        if let Some(id) = self.entries.get(idx).map(|e| e.id)
            && !self.picked.remove(&id)
        {
            self.picked.insert(id);
        }
        self.anchor = Some(idx);
    }

    /// Forgets every play, on a second press within a few seconds of the first.
    fn forget_all(&mut self) {
        if self.armed() {
            self.armed = None;
            self.cursor = 0;
            self.scroll = 0;
            self.ask(Some(Forget::Everything));
        } else {
            self.armed = Some(Instant::now());
        }
    }

    fn armed(&self) -> bool {
        self.armed.is_some_and(|at| at.elapsed() < CONFIRM)
    }

    /// A key. `favourites` is what the menu says the track is.
    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        favourites: &HashSet<i64>,
    ) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if self.filter_active {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_active = false;
                    self.reload();
                }
                KeyCode::Enter => self.filter_active = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.reload();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.reload();
                }
                _ => {}
            }
            self.follow = true;
            return Some(Action::None);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.rows();
        if self.entries.is_empty() {
            return match key.code {
                KeyCode::Char('/') => {
                    self.filter_active = true;
                    Some(Action::None)
                }
                KeyCode::Esc if !self.filter.is_empty() => {
                    self.filter.clear();
                    self.reload();
                    Some(Action::None)
                }
                _ => None,
            };
        }
        if key.code != KeyCode::Char('D') {
            let asked = self.armed();
            self.armed = None;
            if asked && key.code == KeyCode::Esc {
                return Some(Action::None);
            }
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('K') if shift => {
                self.anchor.get_or_insert(self.cursor);
                self.move_to(self.cursor.saturating_sub(1));
                self.pick_to(self.cursor);
            }
            KeyCode::Down | KeyCode::Char('J') if shift => {
                self.anchor.get_or_insert(self.cursor);
                self.move_to(self.cursor + 1);
                self.pick_to(self.cursor);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.anchor = None;
                self.move_to(self.cursor.saturating_sub(1))
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.anchor = None;
                self.move_to(self.cursor + 1)
            }
            KeyCode::Char('v') => self.toggle_pick(self.cursor),
            KeyCode::Char('a') if ctrl => {
                self.picked = self.entries.iter().map(|e| e.id).collect();
            }
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter => return Some(self.play(self.cursor, PickerAction::AppendAndPlay)),
            KeyCode::Char('a') => return Some(self.play(self.cursor, PickerAction::Append)),
            KeyCode::Char('N') => return Some(self.play(self.cursor, PickerAction::PlayNext)),
            KeyCode::Char('m') => return Some(self.menu(self.cursor, favourites)),
            KeyCode::Char('w') => return Some(Action::Choose(self.chosen_tracks(self.cursor))),
            KeyCode::Char('d') => return Some(Action::Download(self.chosen_tracks(self.cursor))),
            KeyCode::Char('u') => {
                return Some(Action::RemoveDownload(self.chosen_tracks(self.cursor)));
            }
            KeyCode::Char('i') => {
                return Some(
                    self.track_at(self.cursor)
                        .map_or(Action::None, |t| Action::Info(t.id)),
                );
            }
            KeyCode::Char('A') => {
                return Some(
                    self.track_at(self.cursor)
                        .and_then(|t| t.artist_id)
                        .map_or(Action::None, Action::OpenArtist),
                );
            }
            KeyCode::Char('O') => {
                return Some(
                    self.track_at(self.cursor)
                        .and_then(|t| t.album_id)
                        .map_or(Action::None, Action::OpenAlbum),
                );
            }
            KeyCode::Char('/') => self.filter_active = true,
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => self.forget(self.cursor),
            KeyCode::Char('D') => self.forget_all(),
            KeyCode::Esc if !self.picked.is_empty() => {
                self.picked.clear();
                self.anchor = None;
            }
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.reload();
            }
            _ => return None,
        }
        Some(Action::None)
    }

    /// What can be done with the plays picked out, or the one at `idx`: the
    /// apps' menu for a track or for several, then forgetting them.
    fn menu(&self, idx: usize, favourites: &HashSet<i64>) -> Action {
        let chosen = self.chosen(idx);
        let (title, thing) = match chosen.as_slice() {
            [] => return Action::None,
            [one] => {
                let t = &one.track;
                let thing = Playable::Track {
                    id: t.id,
                    favourite: favourites.contains(&t.id),
                    on_disk: t.path.is_some() || t.cached_path.is_some(),
                    album: t.album_id,
                    artist: t.artist_id,
                };
                (t.title.clone(), thing)
            }
            many => (
                format!("{} plays", many.len()),
                Playable::Tracks(many.iter().map(|e| e.track.id).collect()),
            ),
        };
        let mut items = playable(&thing);
        items.push((String::new(), Action::None));
        items.push(("Remove from History".into(), Action::ForgetPlays));
        Action::Menu(title, items)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<HistoryHit> {
        let g = &self.geom;
        if let Some((_, hit)) = g.links.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if inside(g.list, col, row)
            && let Some(Line::Play(i)) = self.lines.get(self.scroll + (row - g.list.y) as usize)
        {
            return Some(HistoryHit::Play(*i));
        }
        None
    }

    /// Forgets what the actions menu was opened on.
    pub fn forget_chosen(&mut self) {
        self.forget(self.cursor);
    }

    /// A click on `hit`. On a play, Shift or Ctrl picks out the run from
    /// the last click, and Alt picks it out alone or puts it back: most
    /// terminals pass no Shift with the mouse, and none pass Cmd.
    pub fn click(&mut self, hit: HistoryHit, modifiers: crossterm::event::KeyModifiers) -> Action {
        use crossterm::event::KeyModifiers;
        self.armed = None;
        match hit {
            HistoryHit::Forget(i) => {
                if let Some(e) = self.entries.get(i) {
                    self.picked.remove(&e.id);
                    self.ask(Some(Forget::Plays(vec![e.id])));
                }
                Action::None
            }
            HistoryHit::Play(i)
                if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                self.pick_to(i);
                self.move_to(i);
                Action::None
            }
            HistoryHit::Play(i) if modifiers.contains(KeyModifiers::ALT) => {
                self.toggle_pick(i);
                self.move_to(i);
                Action::None
            }
            HistoryHit::Play(i) => {
                self.picked.clear();
                self.anchor = Some(i);
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                if double {
                    self.last_click = None;
                    self.play(i, PickerAction::AppendAndPlay)
                } else {
                    self.last_click = Some((i, Instant::now()));
                    Action::None
                }
            }
        }
    }

    /// The page's toolbar: its count, the filter, the actions menu and
    /// clearing, each the same as its key.
    pub fn toolbar(&self) -> Toolbar {
        use crossterm::event::{KeyCode, KeyEvent};
        let count = match (self.entries.len(), self.picked.len()) {
            (1, _) => "1 play".to_string(),
            (n, 0) => format!("{n} plays"),
            (n, k) => format!("{n} plays \u{b7} {k} picked"),
        };
        let filter = if self.filter_active || !self.filter.is_empty() {
            let cursor = if self.filter_active { "\u{2588}" } else { "" };
            format!("/{}{cursor}", self.filter)
        } else {
            "/ filter".to_string()
        };
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let mut bar = Toolbar::new("history").count(count).item(
            ToolItem::new(filter, key('/'))
                .on(self.filter_active)
                .priority(2),
        );
        if !self.entries.is_empty() {
            let clear = if self.armed() {
                "press D again to clear every play"
            } else {
                "clear\u{2026}"
            };
            bar = bar
                .item(ToolItem::new("\u{22ef} actions", key('m')).priority(1))
                .item(ToolItem::new(clear, key('D')).on(self.armed()));
        }
        bar
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
        hover: Option<HistoryHit>,
        links: &Links,
    ) {
        self.geom = Geom::default();
        if area.width < 8 || area.height < 2 {
            return;
        }
        let list = area;
        self.geom.list = list;
        if self.entries.is_empty() {
            let (msg, detail) = if self.filter.is_empty() {
                (
                    "nothing played yet",
                    "everything you play lands here, most recent first.",
                )
            } else {
                ("no matches", "")
            };
            let w = list.width.saturating_sub(2) as usize;
            buf.set_stringn(list.x + 1, list.y + 1, msg, w, theme.track_normal);
            if list.height > 2 {
                buf.set_stringn(list.x + 1, list.y + 2, detail, w, theme.hint_desc);
            }
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
                Line::Day(label) => {
                    let style = theme.heading.add_modifier(Modifier::BOLD);
                    buf.set_stringn(list.x + 1, y, label, list.width as usize, style);
                    let from = list.x + 2 + width(label) as u16;
                    for cx in from..text_end {
                        buf[(cx, y)].set_char('\u{2500}').set_style(theme.rule);
                    }
                }
                Line::Play(i) => {
                    let row = Rect::new(list.x, y, text_end - list.x, 1);
                    self.render_play(buf, theme, *i, row, hover, links);
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

    /// Draws the play at `i` across `row`.
    fn render_play(
        &mut self,
        buf: &mut Buffer,
        theme: &Theme,
        i: usize,
        row: Rect,
        hover: Option<HistoryHit>,
        links: &Links,
    ) {
        let (x0, y, end) = (row.x, row.y, row.x + row.width);
        let e = &self.entries[i];
        let t = &e.track;
        let selected = i == self.cursor;
        let hovered = matches!(
            hover,
            Some(HistoryHit::Play(n) | HistoryHit::Forget(n)) if n == i
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
        let row_style = if selected {
            theme.library_cursor
        } else {
            theme.track_normal
        };
        let dim = if selected {
            theme.library_cursor
        } else {
            theme.hint_desc
        };
        let link = || {
            if selected {
                theme.library_cursor
            } else {
                theme.hint_desc
            }
        };

        let time = Local
            .timestamp_opt(e.played_at, 0)
            .single()
            .map(|d| d.format("%H:%M").to_string())
            .unwrap_or_default();
        buf.set_stringn(x0 + 2, y, &time, 5, dim);

        let len = t
            .duration_ms
            .map(|d| format_time(d as u64))
            .unwrap_or_default();
        // The forget button is the last three cells; the length sits before
        // it, right-aligned in a column of its own so every row's columns
        // start where the others' do.
        let forget = Rect::new(end.saturating_sub(3), y, 3, 1);
        let forget_on = hover == Some(HistoryHit::Forget(i));
        buf.set_string(
            forget.x + 1,
            y,
            "\u{d7}",
            if forget_on {
                theme.heading.add_modifier(Modifier::BOLD)
            } else {
                dim
            },
        );
        self.geom.links.push((forget, HistoryHit::Forget(i)));
        let len_x = forget.x.saturating_sub(LENGTH_W);
        let len_w = width(&len) as u16;
        buf.set_string(forget.x.saturating_sub(len_w + 1), y, &len, dim);
        // A play another client scrobbled in did not happen here.
        if e.source != "local" {
            buf.set_string(len_x.saturating_sub(2), y, "\u{21aa}", dim);
        }
        let len_x = len_x.saturating_sub(2);

        let left = x0 + 9;
        let room = len_x.saturating_sub(left + 2) as usize;
        if room < 24 {
            buf.set_stringn(left, y, fit(&t.title, room), room, row_style);
            return;
        }
        let title_w = room * 2 / 5;
        let artist_w = room * 3 / 10;
        let album_w = room - title_w - artist_w - 4;
        buf.set_stringn(left, y, fit(&t.title, title_w), title_w, row_style);

        let artist_x = left + title_w as u16 + 2;
        let album_x = artist_x + artist_w as u16 + 2;
        for (text, x, w, to) in [
            (
                &t.artist_name,
                artist_x,
                artist_w,
                t.artist_id.map(Link::Artist),
            ),
            (
                &t.album_title,
                album_x,
                album_w,
                t.album_id.map(Link::Album),
            ),
        ] {
            match to {
                Some(to) => {
                    links.draw(buf, x, y, text, w as u16, link(), to);
                }
                None => {
                    buf.set_stringn(x, y, fit(text, w), w, link());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn entry(id: i64, played_at: i64, title: &str) -> PlayHistoryRow {
        PlayHistoryRow {
            id,
            track: queries::TrackRow {
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
            },
            played_at,
            listened_ms: None,
            source: "local".into(),
        }
    }

    /// A page whose reads go nowhere: tests never touch the library.
    fn page() -> HistoryPage {
        let now = Local::now().timestamp();
        let mut p = HistoryPage::new();
        p.tx = crossbeam_channel::unbounded().0;
        p.entries = vec![
            entry(1, now, "one"),
            entry(2, now - 60, "two"),
            entry(3, now - 3 * 86_400, "three"),
        ];
        p.rebuild();
        p
    }

    fn draw(p: &mut HistoryPage, w: u16, h: u16) -> Buffer {
        draw_with(p, w, h, &Links::default())
    }

    fn draw_with(p: &mut HistoryPage, w: u16, h: u16, links: &Links) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        p.render(area, &mut buf, &theme, None, links);
        buf
    }

    /// Where the artist link of each line starts.
    fn artist_links(links: &Links, w: u16, h: u16) -> Vec<u16> {
        (0..h)
            .filter_map(|y| (0..w).find(|&x| matches!(links.at(x, y), Some(Link::Artist(_)))))
            .collect()
    }

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn plays_sit_under_the_day_they_were_played() {
        let mut p = page();
        let shown = text(&draw(&mut p, 100, 12));
        assert!(shown.contains("today"));
        assert!(shown.contains("one") && shown.contains("two") && shown.contains("three"));
        assert_eq!(p.lines.len(), 5, "two day headings and three plays");
    }

    #[test]
    fn the_cursor_skips_the_day_headings() {
        let mut p = page();
        draw(&mut p, 100, 12);
        for _ in 0..2 {
            p.handle_key(KeyEvent::from(KeyCode::Down), &HashSet::new());
        }
        assert_eq!(p.cursor, 2);
        assert!(matches!(p.lines[p.line_of[2] - 1], Line::Day(_)));
    }

    #[test]
    fn a_click_selects_and_a_second_plays() {
        let mut p = page();
        draw(&mut p, 100, 12);
        let hit = p.hit(40, 2).expect("a play under the pointer");
        assert_eq!(hit, HistoryHit::Play(1));
        assert_eq!(p.click(hit, KeyModifiers::NONE), Action::None);
        assert!(matches!(p.click(hit, KeyModifiers::NONE), Action::Play(ids, _) if ids == vec![2]));
    }

    #[test]
    fn the_artist_and_record_are_links() {
        let mut p = page();
        let links = Links::default();
        draw_with(&mut p, 100, 12, &links);
        let x = artist_links(&links, 100, 12)[0];
        assert_eq!(links.at(x, 1), Some(Link::Artist(2)));
        let album = (x..100).find(|&cx| links.at(cx, 1) == Some(Link::Album(1)));
        assert!(album.is_some(), "the album is a link too");
    }

    #[test]
    fn forgetting_everything_takes_two_presses() {
        let mut p = page();
        draw(&mut p, 100, 12);
        p.handle_key(KeyEvent::from(KeyCode::Char('D')), &HashSet::new());
        assert!(p.armed());
        assert_eq!(
            p.handle_key(KeyEvent::from(KeyCode::Esc), &HashSet::new()),
            Some(Action::None),
            "esc lets it be and stays on the page"
        );
        assert!(!p.armed());
        assert_eq!(
            p.handle_key(KeyEvent::from(KeyCode::Esc), &HashSet::new()),
            None
        );
    }

    #[test]
    fn n_puts_the_track_next_and_i_asks_for_its_info() {
        let mut p = page();
        draw(&mut p, 100, 12);
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        assert_eq!(
            p.handle_key(key('N'), &HashSet::new()),
            Some(Action::Play(vec![1], PickerAction::PlayNext))
        );
        assert_eq!(
            p.handle_key(key('i'), &HashSet::new()),
            Some(Action::Info(1))
        );
        assert_eq!(
            p.handle_key(key('d'), &HashSet::new()),
            Some(Action::Download(vec![1]))
        );
        assert_eq!(
            p.handle_key(key('u'), &HashSet::new()),
            Some(Action::RemoveDownload(vec![1]))
        );
    }

    #[test]
    fn the_menu_acts_on_the_play_under_the_cursor() {
        let mut p = page();
        draw(&mut p, 100, 12);
        let Action::Menu(title, items) = p.menu(1, &HashSet::from([2])) else {
            panic!("a menu");
        };
        assert_eq!(title, "two");
        assert!(
            items
                .iter()
                .any(|(l, a)| l == "Track info" && *a == Action::Info(2))
        );
        assert!(
            items.iter().any(|(l, _)| l == "Remove favourite"),
            "the menu knows it is a favourite"
        );
        let n = items.len();
        assert_eq!(items[n - 2], (String::new(), Action::None), "a rule");
        assert_eq!(items[n - 1].1, Action::ForgetPlays);
        let bar = p.toolbar();
        assert!(
            bar.items
                .iter()
                .any(|i| i.key == KeyEvent::from(KeyCode::Char('m')))
        );
    }

    #[test]
    fn a_new_play_leaves_the_selection_where_it_was() {
        let mut p = page();
        draw(&mut p, 100, 3);
        p.move_to(2);
        draw(&mut p, 100, 3);
        let (scroll, line) = (p.scroll, p.line_of[2]);
        assert!(scroll > 0);
        let now = Local::now().timestamp();
        let mut entries = vec![entry(4, now + 1, "four")];
        entries.extend(p.entries.iter().cloned());
        p.take(entries);
        assert_eq!(p.entries[p.cursor].id, 3, "the same play stays selected");
        assert_eq!(p.scroll + line, scroll + p.line_of[p.cursor]);
        draw(&mut p, 100, 3);
        assert_eq!(
            p.line_of[p.cursor] - p.scroll,
            line - scroll,
            "and stays put on screen"
        );
    }

    #[test]
    fn each_forget_acts_on_the_next_play() {
        let mut p = page();
        let (tx, sent) = crossbeam_channel::unbounded();
        p.tx = tx;
        draw(&mut p, 100, 12);
        p.handle_key(KeyEvent::from(KeyCode::Char('x')), &HashSet::new());
        p.handle_key(KeyEvent::from(KeyCode::Char('x')), &HashSet::new());
        let forgot: Vec<_> = sent
            .try_iter()
            .map(|r| match r.forget {
                Some(Forget::Plays(ids)) => ids,
                _ => panic!("a forget of plays"),
            })
            .collect();
        assert_eq!(forgot, vec![vec![1], vec![2]]);
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[p.cursor].id, 3);
    }

    #[test]
    fn forgetting_the_last_play_selects_the_one_above() {
        let mut p = page();
        draw(&mut p, 100, 12);
        p.move_to(2);
        p.handle_key(KeyEvent::from(KeyCode::Char('x')), &HashSet::new());
        assert_eq!(p.entries[p.cursor].id, 2);
        p.handle_key(KeyEvent::from(KeyCode::Char('x')), &HashSet::new());
        assert_eq!(p.entries[p.cursor].id, 1);
    }

    #[test]
    fn a_failed_read_still_follows_new_plays() {
        let mut p = page();
        let (done, rx) = crossbeam_channel::unbounded();
        p.rx = rx;
        p.reload();
        done.send(Reply {
            generation: p.generation,
            version: koan_core::player::history::version(),
            entries: None,
            error: Some("history could not be read".into()),
        })
        .unwrap();
        assert!(p.poll().is_some());
        assert_eq!(p.shown, p.generation, "the failed read is the one shown");
    }

    #[test]
    fn a_superseded_read_is_not_shown_but_its_failure_is() {
        let mut p = page();
        let (done, rx) = crossbeam_channel::unbounded();
        p.rx = rx;
        p.reload();
        p.reload();
        let reply = |generation, title: &str, error: Option<&str>| Reply {
            generation,
            version: koan_core::player::history::version(),
            entries: Some(vec![entry(9, Local::now().timestamp(), title)]),
            error: error.map(Into::into),
        };
        done.send(reply(p.generation - 1, "old", Some("could not forget")))
            .unwrap();
        assert_eq!(p.poll().as_deref(), Some("could not forget"));
        assert_eq!(p.entries.len(), 3, "the older read is dropped");
        done.send(reply(p.generation, "new", None)).unwrap();
        assert_eq!(p.poll(), None);
        assert_eq!(p.entries[0].track.title, "new");
    }

    #[test]
    fn the_days_are_renamed_after_midnight() {
        let mut p = page();
        p.today = p.today.pred_opt().unwrap();
        p.rebuild();
        assert!(matches!(&p.lines[0], Line::Day(l) if l != "today"));
        p.poll();
        assert!(matches!(&p.lines[0], Line::Day(l) if l == "today"));
    }

    #[test]
    fn it_draws_at_any_size() {
        let mut p = page();
        for w in 0..60 {
            for h in 0..12 {
                draw(&mut p, w, h);
            }
        }
    }

    #[test]
    fn shift_picks_out_a_run_that_plays_and_is_forgotten_together() {
        let mut p = page();
        draw(&mut p, 100, 12);
        p.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            &HashSet::new(),
        );
        p.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            &HashSet::new(),
        );
        assert_eq!(p.picked.len(), 3);
        assert_eq!(
            p.handle_key(KeyEvent::from(KeyCode::Enter), &HashSet::new()),
            Some(Action::Play(vec![1, 2, 3], PickerAction::AppendAndPlay))
        );
        let Action::Menu(title, items) = p.menu(p.cursor, &HashSet::new()) else {
            panic!("a menu");
        };
        assert_eq!(title, "3 plays");
        assert!(items.iter().any(|(_, a)| *a == Action::ForgetPlays));
        let (tx, rx) = crossbeam_channel::unbounded();
        p.tx = tx;
        p.handle_key(KeyEvent::from(KeyCode::Char('x')), &HashSet::new());
        let asked = rx.try_recv().unwrap();
        assert!(matches!(asked.forget, Some(Forget::Plays(ids)) if ids == vec![1, 2, 3]));
        assert!(p.picked.is_empty());
    }

    #[test]
    fn alt_click_picks_one_out_and_esc_lets_them_go() {
        let mut p = page();
        draw(&mut p, 100, 12);
        p.click(HistoryHit::Play(0), KeyModifiers::ALT);
        p.click(HistoryHit::Play(2), KeyModifiers::ALT);
        assert_eq!(p.chosen_tracks(0), vec![1, 3]);
        p.handle_key(KeyEvent::from(KeyCode::Esc), &HashSet::new());
        assert!(p.picked.is_empty());
    }

    #[test]
    fn columns_start_in_the_same_place_whatever_the_length() {
        let mut p = page();
        p.entries[0].track.duration_ms = Some(3_723_000);
        p.entries[1].track.duration_ms = Some(9_000);
        p.rebuild();
        let links = Links::default();
        draw_with(&mut p, 100, 12, &links);
        let artist_x = artist_links(&links, 100, 12);
        assert_eq!(artist_x.len(), 3);
        assert!(artist_x.windows(2).all(|w| w[0] == w[1]), "{artist_x:?}");
    }

    /// Writes the page at the sizes the guide is checked at, with long and
    /// wide names, to read by eye:
    /// `cargo test -p koan-tui dump_history -- --ignored`.
    #[test]
    #[ignore]
    fn dump_history() {
        let mut p = page();
        p.entries[0].track.title = "千と千尋の神隠し — あの夏へ (piano version, remastered)".into();
        p.entries[0].track.artist_name = "久石譲".into();
        p.entries[1].source = "listenbrainz".into();
        p.entries[2].track.album_title = "A Record Whose Title Goes On Well Past Any Column".into();
        p.rebuild();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-dumps");
        std::fs::create_dir_all(&dir).unwrap();
        for (w, h) in [(50, 20), (80, 24), (120, 40), (200, 60)] {
            let buf = draw(&mut p, w, h);
            let mut out = String::new();
            for y in 0..h {
                let mut skip = 0;
                for x in 0..w {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let sym = buf[(x, y)].symbol();
                    skip = width(sym).saturating_sub(1);
                    out.push_str(sym);
                }
                out.push('\n');
            }
            std::fs::write(dir.join(format!("history-{w}x{h}.txt")), out).unwrap();
        }
    }

    #[test]
    fn days_are_named_for_how_far_back_they_are() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        assert_eq!(day_label(today, today), "today");
        assert_eq!(day_label(today.pred_opt().unwrap(), today), "yesterday");
        assert_eq!(
            day_label(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(), today),
            "monday 12 october 2026"
        );
    }
}

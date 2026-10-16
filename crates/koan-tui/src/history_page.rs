//! The History page: every play, most recent first, under the day it was
//! played, as the apps' history page lists them. A play can be forgotten,
//! here and, signed in to a koan server, on the account's other devices.

use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeZone};
use koan_core::db::queries::{self, PlayHistoryRow};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::app::PickerAction;
use super::browse::{Action, fit};
use super::theme::Theme;
use super::transport::format_time;

/// The most plays held at once; the page filters, so older ones stay a search away.
const LIMIT: u32 = 5000;
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How long "forget all" waits for the second press.
const CONFIRM: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryHit {
    Play(usize),
    Artist(usize),
    Album(usize),
    Forget(usize),
    Filter,
    ForgetAll,
}

enum Line {
    Day(String),
    Play(usize),
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    bar: Rect,
    filter: Rect,
    clear: Rect,
    links: Vec<(Rect, HistoryHit)>,
    total: usize,
}

pub struct HistoryPage {
    entries: Vec<PlayHistoryRow>,
    lines: Vec<Line>,
    /// The line each entry is on.
    line_of: Vec<usize>,
    cursor: usize,
    scroll: usize,
    follow: bool,
    pub filter: String,
    pub filter_active: bool,
    armed: Option<Instant>,
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
        _ => day.format("%a %-d %b %Y").to_string().to_lowercase(),
    }
}

impl Default for HistoryPage {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryPage {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            lines: Vec::new(),
            line_of: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            filter: String::new(),
            filter_active: false,
            armed: None,
            geom: Geom::default(),
            last_click: None,
        }
    }

    /// Reads the plays again, narrowed by the filter.
    pub fn reload(&mut self) {
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return;
        };
        let search = (!self.filter.is_empty()).then_some(self.filter.as_str());
        self.entries = queries::play_history_with_tracks(
            &db.conn,
            queries::LOCAL_USER,
            search,
            Some(LIMIT),
            0,
        )
        .unwrap_or_default();
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let today = Local::now().date_naive();
        self.lines.clear();
        self.line_of.clear();
        let mut current = None;
        for (i, e) in self.entries.iter().enumerate() {
            let day = day_of(e.played_at);
            if day != current || self.lines.is_empty() {
                current = day;
                let label = day.map_or_else(|| "earlier".into(), |d| day_label(d, today));
                self.lines.push(Line::Day(label));
            }
            self.line_of.push(self.lines.len());
            self.lines.push(Line::Play(i));
        }
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
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

    fn play(&self, idx: usize, how: PickerAction) -> Action {
        self.track_at(idx)
            .map_or(Action::None, |t| Action::Play(vec![t.id], how))
    }

    /// Forgets the play at `idx`.
    fn forget(&mut self, idx: usize) {
        let Some(id) = self.entries.get(idx).map(|e| e.id) else {
            return;
        };
        if let Ok(db) = koan_core::db::pool::shared().get() {
            koan_core::remote::history::forget(&db, &[id]).ok();
            koan_core::player::history::changed();
        }
        self.reload();
    }

    /// Forgets every play, on a second press within a few seconds of the first.
    fn forget_all(&mut self) {
        if self.armed.is_some_and(|at| at.elapsed() < CONFIRM) {
            self.armed = None;
            if let Ok(db) = koan_core::db::pool::shared().get() {
                koan_core::remote::history::clear(&db).ok();
                koan_core::player::history::changed();
            }
            self.cursor = 0;
            self.scroll = 0;
            self.reload();
        } else {
            self.armed = Some(Instant::now());
        }
    }

    fn armed(&self) -> bool {
        self.armed.is_some_and(|at| at.elapsed() < CONFIRM)
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
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
            return Some(Action::None);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        if key.code != KeyCode::Char('D') {
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
            KeyCode::Enter => return Some(self.play(self.cursor, PickerAction::AppendAndPlay)),
            KeyCode::Char('a') => return Some(self.play(self.cursor, PickerAction::Append)),
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
            KeyCode::Esc if self.armed.take().is_some() => {}
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.reload();
            }
            _ => return None,
        }
        Some(Action::None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<HistoryHit> {
        let g = &self.geom;
        if inside(g.filter, col, row) {
            return Some(HistoryHit::Filter);
        }
        if inside(g.clear, col, row) {
            return Some(HistoryHit::ForgetAll);
        }
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

    pub fn click(&mut self, hit: HistoryHit) -> Action {
        self.armed = self.armed.filter(|_| hit == HistoryHit::ForgetAll);
        match hit {
            HistoryHit::Filter => {
                self.filter_active = true;
                Action::None
            }
            HistoryHit::ForgetAll => {
                self.forget_all();
                Action::None
            }
            HistoryHit::Forget(i) => {
                self.forget(i);
                Action::None
            }
            HistoryHit::Artist(i) => {
                self.move_to(i);
                self.track_at(i)
                    .and_then(|t| t.artist_id)
                    .map_or(Action::None, Action::OpenArtist)
            }
            HistoryHit::Album(i) => {
                self.move_to(i);
                self.track_at(i)
                    .and_then(|t| t.album_id)
                    .map_or(Action::None, Action::OpenAlbum)
            }
            HistoryHit::Play(i) => {
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
    ) {
        self.geom = Geom::default();
        if area.width < 8 || area.height < 3 {
            return;
        }
        let note = format!(" {} ", self.entries.len());
        super::ui::heading(buf, area, theme, "history", &note);

        // The page's controls sit on the heading's rule, at its right.
        let end = area.x + area.width;
        let mut x = end;
        let mut control = |buf: &mut Buffer, text: String, style: Style| -> Rect {
            let w = (text.chars().count() as u16).min(x.saturating_sub(area.x));
            x -= w;
            buf.set_stringn(x, area.y, &text, w as usize, style);
            Rect::new(x, area.y, w, 1)
        };
        let clear_text = if self.armed() {
            " press again to forget every play ".to_string()
        } else {
            " forget all ".to_string()
        };
        let clear_style = if self.armed() {
            theme.heading.add_modifier(Modifier::BOLD)
        } else if hover == Some(HistoryHit::ForgetAll) {
            theme.hint_desc.add_modifier(Modifier::UNDERLINED)
        } else {
            theme.hint_desc
        };
        if !self.entries.is_empty() {
            self.geom.clear = control(buf, clear_text, clear_style);
        }
        let filter_text = if self.filter_active || !self.filter.is_empty() {
            let cursor = if self.filter_active { "\u{2588}" } else { "" };
            format!(" /{}{} ", self.filter, cursor)
        } else {
            " / filter ".to_string()
        };
        let filter_style = if hover == Some(HistoryHit::Filter) || self.filter_active {
            theme.hint_key.add_modifier(Modifier::UNDERLINED)
        } else {
            theme.hint_desc
        };
        self.geom.filter = control(buf, filter_text, filter_style);

        let list = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        self.geom.list = list;
        if self.entries.is_empty() {
            let msg = if self.filter.is_empty() {
                "nothing played yet"
            } else {
                "no match"
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
                Line::Day(label) => {
                    let style = theme.heading.add_modifier(Modifier::BOLD);
                    buf.set_stringn(list.x + 1, y, label, list.width as usize, style);
                    let from = list.x + 2 + label.chars().count() as u16;
                    for cx in from..text_end {
                        buf[(cx, y)].set_char('\u{2500}').set_style(theme.rule);
                    }
                }
                Line::Play(i) => {
                    let i = *i;
                    self.render_play(buf, theme, i, y, list.x, text_end, hover);
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
    fn render_play(
        &mut self,
        buf: &mut Buffer,
        theme: &Theme,
        i: usize,
        y: u16,
        x0: u16,
        end: u16,
        hover: Option<HistoryHit>,
    ) {
        let e = &self.entries[i];
        let t = &e.track;
        let selected = i == self.cursor;
        let hovered = matches!(
            hover,
            Some(HistoryHit::Play(n) | HistoryHit::Artist(n) | HistoryHit::Album(n) | HistoryHit::Forget(n)) if n == i
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
        let link = |on: bool| {
            let base = if selected {
                theme.library_cursor
            } else {
                theme.hint_desc
            };
            if on {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
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
        let len_w = len.chars().count() as u16;
        // The forget button is the last three cells; the length sits before it.
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
        let len_x = forget.x.saturating_sub(len_w + 1);
        buf.set_string(len_x, y, &len, dim);

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
        for (text, x, w, hit, linked) in [
            (
                &t.artist_name,
                artist_x,
                artist_w,
                HistoryHit::Artist(i),
                t.artist_id.is_some(),
            ),
            (
                &t.album_title,
                album_x,
                album_w,
                HistoryHit::Album(i),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

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

    fn page() -> HistoryPage {
        let now = Local::now().timestamp();
        let mut p = HistoryPage::new();
        p.entries = vec![
            entry(1, now, "one"),
            entry(2, now - 60, "two"),
            entry(3, now - 3 * 86_400, "three"),
        ];
        p.rebuild();
        p
    }

    fn draw(p: &mut HistoryPage, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(
            area,
            &mut buf,
            &Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167)),
            None,
        );
        buf
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
            p.handle_key(KeyEvent::from(KeyCode::Down));
        }
        assert_eq!(p.cursor, 2);
        assert!(matches!(p.lines[p.line_of[2] - 1], Line::Day(_)));
    }

    #[test]
    fn a_click_selects_and_a_second_plays() {
        let mut p = page();
        draw(&mut p, 100, 12);
        let hit = p.hit(40, 3).expect("a play under the pointer");
        assert_eq!(hit, HistoryHit::Play(1));
        assert_eq!(p.click(hit), Action::None);
        assert!(matches!(p.click(hit), Action::Play(ids, _) if ids == vec![2]));
    }

    #[test]
    fn the_artist_and_record_are_links() {
        let mut p = page();
        draw(&mut p, 100, 12);
        let (rect, hit) = p
            .geom
            .links
            .iter()
            .find(|(_, h)| matches!(h, HistoryHit::Artist(0)))
            .cloned()
            .expect("an artist link");
        assert_eq!(p.hit(rect.x, rect.y), Some(hit));
        assert_eq!(p.click(hit), Action::OpenArtist(2));
    }

    #[test]
    fn forgetting_everything_takes_two_presses() {
        let mut p = page();
        draw(&mut p, 100, 12);
        p.handle_key(KeyEvent::from(KeyCode::Char('D')));
        assert!(p.armed());
        p.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(!p.armed());
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
    fn days_are_named_for_how_far_back_they_are() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        assert_eq!(day_label(today, today), "today");
        assert_eq!(day_label(today.pred_opt().unwrap(), today), "yesterday");
        assert_eq!(
            day_label(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(), today),
            "mon 12 oct 2026"
        );
    }
}

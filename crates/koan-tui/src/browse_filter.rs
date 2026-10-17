//! What narrows the browsers beyond the name filter: the apps' browse filters
//! (favourites, recently played, downloaded, lossless, codec, genre, years),
//! as a panel of rows that a key or a click changes. One set applies to the
//! album, artist and track browsers alike, as in the apps.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use koan_core::browse::BrowseFilter;
use koan_core::db::queries;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Widget};

use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Favourites,
    Recent,
    Downloaded,
    Lossless,
    Codec,
    Genre,
    From,
    To,
    Reset,
}

/// The genres to pick from, narrowed by what is typed.
struct Pick {
    options: Vec<String>,
    typed: String,
    cursor: usize,
}

impl Pick {
    fn shown(&self) -> Vec<&String> {
        let q = self.typed.to_lowercase();
        self.options
            .iter()
            .filter(|o| q.is_empty() || o.to_lowercase().contains(&q))
            .collect()
    }
}

pub struct FilterPanel {
    pub filters: BrowseFilter,
    pub cursor: usize,
    rows: Vec<Row>,
    codecs: Vec<String>,
    genres: Vec<String>,
    pick: Option<Pick>,
    /// Digits typed into a year row, applied once there are four.
    year: String,
}

/// The year a decade step starts from, when a bound is not set yet.
fn this_decade() -> i32 {
    let year = chrono::Datelike::year(&chrono::Local::now());
    year - year.rem_euclid(10)
}

impl FilterPanel {
    /// The panel for a browser; `lossless` when it offers that filter.
    pub fn new(filters: BrowseFilter, lossless: bool) -> Self {
        let (codecs, genres) = match koan_core::db::pool::shared().get() {
            Ok(db) => (
                queries::album_codecs(&db.conn).unwrap_or_default(),
                queries::genres(&db.conn, GENRES_OFFERED).unwrap_or_default(),
            ),
            Err(_) => (Vec::new(), Vec::new()),
        };
        Self::with_choices(filters, lossless, codecs, genres)
    }

    fn with_choices(
        filters: BrowseFilter,
        lossless: bool,
        codecs: Vec<String>,
        genres: Vec<String>,
    ) -> Self {
        let rows = [
            Row::Favourites,
            Row::Recent,
            Row::Downloaded,
            Row::Lossless,
            Row::Codec,
            Row::Genre,
            Row::From,
            Row::To,
            Row::Reset,
        ]
        .into_iter()
        .filter(|r| lossless || *r != Row::Lossless)
        .collect();
        Self {
            filters,
            cursor: 0,
            rows,
            codecs,
            genres,
            pick: None,
            year: String::new(),
        }
    }

    pub fn picking(&self) -> bool {
        self.pick.is_some()
    }

    pub fn move_by(&mut self, delta: isize) {
        self.year.clear();
        match self.pick.as_mut() {
            Some(pick) => {
                let n = pick.shown().len() + 1;
                pick.cursor = (pick.cursor as isize + delta).rem_euclid(n as isize) as usize;
            }
            None => {
                let n = self.rows.len() as isize;
                self.cursor = (self.cursor as isize + delta).rem_euclid(n) as usize;
            }
        }
    }

    /// Leaves the list being picked from; false when there was none.
    pub fn back(&mut self) -> bool {
        self.pick.take().is_some()
    }

    fn year_bound(&mut self, row: Row) -> &mut Option<i32> {
        match row {
            Row::From => &mut self.filters.year_from,
            _ => &mut self.filters.year_to,
        }
    }

    /// Does what the row asks, one step `forward` or back for a row that
    /// cycles. True when a filter changed.
    pub fn activate(&mut self, row: usize, forward: bool) -> bool {
        self.cursor = row.min(self.rows.len() - 1);
        self.year.clear();
        let f = &mut self.filters;
        match self.rows[self.cursor] {
            Row::Favourites => f.favourites = !f.favourites,
            Row::Recent => f.recent = !f.recent,
            Row::Downloaded => f.downloaded = !f.downloaded,
            Row::Lossless => f.lossless = !f.lossless,
            Row::Codec => f.codec = step(f.codec.as_ref(), &self.codecs, forward),
            Row::Genre => {
                self.pick = Some(Pick {
                    options: self.genres.clone(),
                    typed: String::new(),
                    cursor: 0,
                });
                return false;
            }
            row @ (Row::From | Row::To) => {
                // A decade at a time: its first year for From, its last for
                // To. Back from nothing starts at this decade; forward past it
                // is no bound.
                let end = if row == Row::To { 9 } else { 0 };
                let now = this_decade();
                let bound = self.year_bound(row);
                let decade = match (*bound, forward) {
                    (None, false) => Some(now),
                    (None, true) => None,
                    (Some(y), _) => {
                        let d = y - y.rem_euclid(10) + if forward { 10 } else { -10 };
                        (d <= now).then_some(d)
                    }
                };
                *bound = decade.map(|d| d + end);
            }
            Row::Reset => {
                let changed = *f != BrowseFilter::default();
                *f = BrowseFilter::default();
                return changed;
            }
        }
        true
    }

    /// A digit typed on a year row: four make the year. True when a filter
    /// changed.
    fn type_year(&mut self, c: char) -> bool {
        let row = self.rows[self.cursor];
        if !matches!(row, Row::From | Row::To) || !c.is_ascii_digit() {
            return false;
        }
        self.year.push(c);
        if self.year.len() < 4 {
            return false;
        }
        let year = self.year.parse().ok();
        self.year.clear();
        *self.year_bound(row) = year;
        true
    }

    /// Takes the highlighted value of the list being picked from. True when
    /// a filter changed.
    pub fn choose(&mut self, idx: usize) -> bool {
        let Some(pick) = self.pick.take() else {
            return false;
        };
        let value = match idx {
            0 => None,
            n => pick.shown().get(n - 1).map(|s| (*s).clone()),
        };
        if idx > 0 && value.is_none() {
            self.pick = Some(pick);
            return false;
        }
        self.filters.genre = value;
        true
    }

    pub fn pick_cursor(&self) -> usize {
        self.pick.as_ref().map_or(0, |p| p.cursor)
    }

    /// A key, as the panel takes it: `Some(changed)`, or `None` when it asks
    /// for the panel to close.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<bool> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(pick) = self.pick.as_mut() {
            let at = pick.cursor;
            return Some(match key.code {
                KeyCode::Esc => {
                    self.pick = None;
                    false
                }
                KeyCode::Enter => self.choose(at),
                KeyCode::Up => {
                    self.move_by(-1);
                    false
                }
                KeyCode::Down => {
                    self.move_by(1);
                    false
                }
                KeyCode::Backspace => {
                    pick.typed.pop();
                    pick.cursor = 0;
                    false
                }
                KeyCode::Char(c) if !ctrl => {
                    pick.typed.push(c);
                    pick.cursor = 0;
                    false
                }
                _ => false,
            });
        }
        let row = self.rows[self.cursor];
        Some(match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return None,
            KeyCode::Char('f') if ctrl => return None,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_by(-1);
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_by(1);
                false
            }
            KeyCode::Char(c) if c.is_ascii_digit() => self.type_year(c),
            KeyCode::Backspace if matches!(row, Row::From | Row::To) => {
                if self.year.pop().is_some() {
                    false
                } else {
                    self.year_bound(row).take().is_some()
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.activate(self.cursor, false),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate(self.cursor, true)
            }
            _ => false,
        })
    }

    /// How many lines the panel is tall for.
    pub fn lines(&self) -> usize {
        match &self.pick {
            Some(pick) => (pick.shown().len() + 1).min(14),
            None => self.rows.len(),
        }
    }

    /// The first value shown of the list being picked from, for `rows` lines.
    pub fn pick_top(&self, rows: usize) -> usize {
        (self.pick_cursor() + 1).saturating_sub(rows.max(1))
    }
}

/// How many genres the filter offers, most common first.
const GENRES_OFFERED: u32 = 80;

/// The value after (or before) `now` in `options`, with `None` at either end
/// of the loop.
fn step<T: PartialEq + Clone>(now: Option<&T>, options: &[T], forward: bool) -> Option<T> {
    if options.is_empty() {
        return None;
    }
    let at = now.and_then(|n| options.iter().position(|o| o == n));
    let next = match (at, forward) {
        (None, true) => Some(0),
        (None, false) => Some(options.len() - 1),
        (Some(i), true) => (i + 1 < options.len()).then_some(i + 1),
        (Some(i), false) => i.checked_sub(1),
    };
    next.map(|i| options[i].clone())
}

pub fn panel_rect(area: Rect, rows: usize) -> Rect {
    let w = 44u16
        .min(area.width.saturating_sub(2))
        .max(10.min(area.width));
    let h = (rows as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3.min(area.height));
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

pub struct FilterPanelOverlay<'a> {
    panel: &'a FilterPanel,
    theme: &'a Theme,
}

impl<'a> FilterPanelOverlay<'a> {
    pub fn new(panel: &'a FilterPanel, theme: &'a Theme) -> Self {
        Self { panel, theme }
    }
}

impl Widget for FilterPanelOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let panel = self.panel;
        let popup = panel_rect(area, panel.lines());
        crate::shell::clear(buf, popup);
        let title = match &panel.pick {
            Some(pick) => format!(" genre /{} ", pick.typed),
            None => match panel.filters.active_count() {
                0 => " filters ".to_string(),
                n => format!(" filters ({n}) "),
            },
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(title, self.theme.heading));
        let inner = block.inner(popup);
        block.render(popup, buf);
        let line = |buf: &mut Buffer, y: u16, text: &str, selected: bool| {
            let style = if selected {
                for x in inner.x..inner.x + inner.width {
                    buf[(x, y)].set_style(self.theme.picker_cursor);
                }
                self.theme.picker_cursor
            } else {
                self.theme.track_normal
            };
            super::rows::put(
                buf,
                inner.x + 1,
                y,
                inner.width.saturating_sub(2),
                text,
                style,
            );
        };

        if let Some(pick) = &panel.pick {
            let shown = pick.shown();
            let rows = inner.height as usize;
            let top = panel.pick_top(rows);
            for (r, i) in (top..=shown.len()).take(rows).enumerate() {
                let text = match i {
                    0 => "any",
                    n => shown[n - 1].as_str(),
                };
                line(buf, inner.y + r as u16, text, i == pick.cursor);
            }
            return;
        }

        let f = &panel.filters;
        let mark = |on: bool| if on { "[x]" } else { "[ ]" };
        let choice = |label: &str, value: &str| format!("{label:<8}\u{2039} {value} \u{203a}");
        let year = |row: Row, value: Option<i32>| {
            let typing = panel.rows[panel.cursor] == row && !panel.year.is_empty();
            let value = match (typing, value) {
                (true, _) => format!("{}\u{2588}", panel.year),
                (false, Some(y)) => y.to_string(),
                (false, None) => "any".into(),
            };
            choice(if row == Row::From { "from" } else { "to" }, &value)
        };
        for (i, row) in panel.rows.iter().enumerate().take(inner.height as usize) {
            let text = match row {
                Row::Favourites => format!("{} favourites", mark(f.favourites)),
                Row::Recent => format!("{} recently played", mark(f.recent)),
                Row::Downloaded => format!("{} downloaded", mark(f.downloaded)),
                Row::Lossless => format!("{} lossless", mark(f.lossless)),
                Row::Codec => choice("codec", f.codec.as_deref().unwrap_or("any")),
                Row::Genre => choice("genre", f.genre.as_deref().unwrap_or("any")),
                Row::From => year(Row::From, f.year_from),
                Row::To => year(Row::To, f.year_to),
                Row::Reset => "reset".to_string(),
            };
            line(buf, inner.y + i as u16, &text, i == panel.cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(lossless: bool) -> FilterPanel {
        FilterPanel::with_choices(
            BrowseFilter::default(),
            lossless,
            vec!["FLAC".into(), "MP3".into()],
            vec!["Jazz".into(), "Rock".into(), "Soul".into()],
        )
    }

    fn row(p: &FilterPanel, r: Row) -> usize {
        p.rows.iter().position(|x| *x == r).unwrap()
    }

    #[test]
    fn the_rows_toggle_and_cycle() {
        let mut p = panel(true);
        assert!(p.activate(row(&p, Row::Favourites), true));
        assert!(p.filters.favourites);
        assert!(p.activate(row(&p, Row::Codec), true));
        assert_eq!(p.filters.codec.as_deref(), Some("FLAC"));
        p.activate(row(&p, Row::Codec), true);
        assert_eq!(p.filters.codec.as_deref(), Some("MP3"));
        p.activate(row(&p, Row::Codec), true);
        assert_eq!(p.filters.codec, None, "past the last value is off");
        assert_eq!(p.filters.active_count(), 1);
        assert!(p.activate(row(&p, Row::Reset), true));
        assert_eq!(p.filters, BrowseFilter::default());
        assert!(!p.activate(row(&p, Row::Reset), true), "nothing to reset");
    }

    #[test]
    fn lossless_is_left_off_where_it_does_not_apply() {
        assert!(!panel(false).rows.contains(&Row::Lossless));
        assert!(panel(true).rows.contains(&Row::Lossless));
    }

    #[test]
    fn years_step_by_decade_and_are_typed() {
        let mut p = panel(true);
        let from = row(&p, Row::From);
        p.activate(from, false);
        assert_eq!(p.filters.year_from, Some(this_decade()));
        p.activate(from, false);
        assert_eq!(p.filters.year_from, Some(this_decade() - 10));
        p.activate(from, true);
        p.activate(from, true);
        assert_eq!(p.filters.year_from, None, "forward past now is no bound");
        let to = row(&p, Row::To);
        p.activate(to, false);
        assert_eq!(p.filters.year_to, Some(this_decade() + 9));
        p.cursor = to;
        for c in "199".chars() {
            assert!(!p.type_year(c), "three digits are not a year yet");
        }
        assert!(p.type_year('7'));
        assert_eq!(p.filters.year_to, Some(1997));
        assert_eq!(
            p.handle_key(KeyEvent::from(KeyCode::Backspace)),
            Some(true),
            "backspace on a set year clears it"
        );
        assert_eq!(p.filters.year_to, None);
    }

    #[test]
    fn genres_are_picked_from_a_list_narrowed_by_typing() {
        let mut p = panel(true);
        let genre = row(&p, Row::Genre);
        assert!(!p.activate(genre, true), "opening the list changes nothing");
        assert!(p.picking());
        p.handle_key(KeyEvent::from(KeyCode::Char('o')));
        p.handle_key(KeyEvent::from(KeyCode::Char('u')));
        // "any" then the one genre containing "ou".
        assert!(p.choose(1));
        assert_eq!(p.filters.genre.as_deref(), Some("Soul"));
        p.activate(genre, true);
        assert!(p.choose(0));
        assert_eq!(p.filters.genre, None);
    }

    #[test]
    fn escape_asks_to_close_unless_picking() {
        let mut p = panel(true);
        p.activate(row(&p, Row::Genre), true);
        assert_eq!(p.handle_key(KeyEvent::from(KeyCode::Esc)), Some(false));
        assert!(!p.picking());
        assert_eq!(p.handle_key(KeyEvent::from(KeyCode::Esc)), None);
    }

    #[test]
    fn it_draws_at_any_size() {
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        let mut p = panel(true);
        for pick in [false, true] {
            if pick {
                p.activate(row(&p, Row::Genre), true);
            }
            for w in 0..60 {
                for h in 0..20 {
                    let area = Rect::new(0, 0, w, h);
                    let mut buf = Buffer::empty(area);
                    FilterPanelOverlay::new(&p, &theme).render(area, &mut buf);
                }
            }
        }
    }
}

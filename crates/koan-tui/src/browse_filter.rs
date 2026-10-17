//! What narrows the record pages beyond the name filter: the same codec,
//! genre, decade, lossless and downloaded filters the apps' browser has, as a
//! panel of rows that a key or a click changes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use koan_core::db::queries::{self, AlbumFilter};

use super::theme::Theme;

/// Decades offered, newest first; `0` is everything before 1960.
const DECADES: [i32; 8] = [2020, 2010, 2000, 1990, 1980, 1970, 1960, 0];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub lossless: bool,
    pub downloaded: bool,
    pub codec: Option<String>,
    pub genre: Option<String>,
    /// The first year of a decade in `DECADES`.
    pub decade: Option<i32>,
}

impl Filters {
    /// How many are on, as the heading says.
    pub fn count(&self) -> usize {
        [
            self.lossless,
            self.downloaded,
            self.codec.is_some(),
            self.genre.is_some(),
            self.decade.is_some(),
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
    }

    pub fn album_filter(&self) -> AlbumFilter<'_> {
        let (year_from, year_to) = match self.decade {
            Some(0) => (None, Some(1959)),
            Some(d) => (Some(d), Some(d + 9)),
            None => (None, None),
        };
        AlbumFilter {
            lossless: self.lossless,
            codec: self.codec.as_deref(),
            year_from,
            year_to,
            genre: self.genre.as_deref(),
            on_device: self.downloaded,
        }
    }
}

fn decade_label(decade: Option<i32>) -> String {
    match decade {
        None => "any".into(),
        Some(0) => "before 1960".into(),
        Some(d) => format!("{d}s"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Lossless,
    Downloaded,
    Codec,
    Genre,
    Decade,
    Reset,
}

const ROWS: [Row; 6] = [
    Row::Lossless,
    Row::Downloaded,
    Row::Codec,
    Row::Genre,
    Row::Decade,
    Row::Reset,
];

/// A list of values for one row to take, narrowed by what is typed.
struct Pick {
    row: Row,
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
    pub filters: Filters,
    pub cursor: usize,
    codecs: Vec<String>,
    genres: Vec<String>,
    pick: Option<Pick>,
}

impl FilterPanel {
    pub fn new(filters: Filters) -> Self {
        let (codecs, genres) = match koan_core::db::pool::shared().get() {
            Ok(db) => (
                queries::album_codecs(&db.conn).unwrap_or_default(),
                queries::genres(&db.conn, 400).unwrap_or_default(),
            ),
            Err(_) => (Vec::new(), Vec::new()),
        };
        Self::with_choices(filters, codecs, genres)
    }

    fn with_choices(filters: Filters, codecs: Vec<String>, genres: Vec<String>) -> Self {
        Self {
            filters,
            cursor: 0,
            codecs,
            genres,
            pick: None,
        }
    }

    pub fn picking(&self) -> bool {
        self.pick.is_some()
    }

    pub fn move_by(&mut self, delta: isize) {
        match self.pick.as_mut() {
            Some(pick) => {
                let n = pick.shown().len() + 1;
                pick.cursor = (pick.cursor as isize + delta).rem_euclid(n as isize) as usize;
            }
            None => {
                self.cursor =
                    (self.cursor as isize + delta).rem_euclid(ROWS.len() as isize) as usize;
            }
        }
    }

    /// Types into the list being picked from.
    pub fn type_char(&mut self, c: char) {
        if let Some(pick) = self.pick.as_mut() {
            pick.typed.push(c);
            pick.cursor = 0;
        }
    }

    pub fn backspace(&mut self) {
        if let Some(pick) = self.pick.as_mut() {
            pick.typed.pop();
            pick.cursor = 0;
        }
    }

    /// Leaves the list being picked from; false when there was none.
    pub fn back(&mut self) -> bool {
        self.pick.take().is_some()
    }

    /// Does what the row asks, one step `forward` or back for a row that
    /// cycles. True when a filter changed.
    pub fn activate(&mut self, row: usize, forward: bool) -> bool {
        self.cursor = row.min(ROWS.len() - 1);
        let f = &mut self.filters;
        match ROWS[self.cursor] {
            Row::Lossless => f.lossless = !f.lossless,
            Row::Downloaded => f.downloaded = !f.downloaded,
            Row::Codec => f.codec = step(f.codec.as_deref(), &self.codecs, forward),
            Row::Genre => {
                self.pick = Some(Pick {
                    row: Row::Genre,
                    options: self.genres.clone(),
                    typed: String::new(),
                    cursor: 0,
                });
                return false;
            }
            Row::Decade => {
                let labels: Vec<String> = DECADES.iter().map(|d| d.to_string()).collect();
                let now = f.decade.map(|d| d.to_string());
                f.decade = step(now.as_deref(), &labels, forward).and_then(|d| d.parse().ok());
            }
            Row::Reset => {
                let changed = *f != Filters::default();
                *f = Filters::default();
                return changed;
            }
        }
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
        if pick.row == Row::Genre {
            self.filters.genre = value;
        }
        true
    }

    pub fn pick_cursor(&self) -> usize {
        self.pick.as_ref().map_or(0, |p| p.cursor)
    }
}

/// The value after (or before) `now` in `options`, with `None` at either end
/// of the loop.
fn step(now: Option<&str>, options: &[String], forward: bool) -> Option<String> {
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

impl FilterPanel {
    /// How many lines the panel is tall for.
    pub fn lines(&self) -> usize {
        match &self.pick {
            Some(pick) => (pick.shown().len() + 2).min(14),
            None => ROWS.len(),
        }
    }

    /// The first value shown of the list being picked from, for `rows` lines.
    pub fn pick_top(&self, rows: usize) -> usize {
        (self.pick_cursor() + 1).saturating_sub(rows.max(1))
    }
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
        Clear.render(popup, buf);
        let title = match &panel.pick {
            Some(pick) => format!(" genre /{} ", pick.typed),
            None => format!(" filters ({}) ", panel.filters.count()),
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
            buf.set_stringn(
                inner.x + 1,
                y,
                text,
                inner.width.saturating_sub(2) as usize,
                style,
            );
        };

        if let Some(pick) = &panel.pick {
            let shown = pick.shown();
            let rows = inner.height as usize;
            let top = panel.pick_top(rows);
            for (r, i) in (top..=shown.len()).take(rows).enumerate() {
                let text = match i {
                    0 => "any".to_string(),
                    n => shown[n - 1].clone(),
                };
                line(buf, inner.y + r as u16, &text, i == pick.cursor);
            }
            return;
        }

        let f = &panel.filters;
        let mark = |on: bool| if on { "[x]" } else { "[ ]" };
        for (i, row) in ROWS.iter().enumerate().take(inner.height as usize) {
            let text = match row {
                Row::Lossless => format!("{} lossless", mark(f.lossless)),
                Row::Downloaded => format!("{} downloaded", mark(f.downloaded)),
                Row::Codec => format!(
                    "codec    \u{2039} {} \u{203a}",
                    f.codec.as_deref().unwrap_or("any")
                ),
                Row::Genre => format!(
                    "genre    \u{2039} {} \u{203a}",
                    f.genre.as_deref().unwrap_or("any")
                ),
                Row::Decade => format!("decade   \u{2039} {} \u{203a}", decade_label(f.decade)),
                Row::Reset => "reset".to_string(),
            };
            line(buf, inner.y + i as u16, &text, i == panel.cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> FilterPanel {
        FilterPanel::with_choices(
            Filters::default(),
            vec!["FLAC".into(), "MP3".into()],
            vec!["Jazz".into(), "Rock".into(), "Soul".into()],
        )
    }

    #[test]
    fn a_decade_becomes_a_year_range() {
        let f = Filters {
            decade: Some(1990),
            ..Default::default()
        };
        let a = f.album_filter();
        assert_eq!((a.year_from, a.year_to), (Some(1990), Some(1999)));
        let old = Filters {
            decade: Some(0),
            ..Default::default()
        };
        assert_eq!(old.album_filter().year_to, Some(1959));
        assert_eq!(old.album_filter().year_from, None);
    }

    #[test]
    fn the_rows_toggle_and_cycle() {
        let mut p = panel();
        assert!(p.activate(0, true));
        assert!(p.filters.lossless);
        assert!(p.activate(2, true));
        assert_eq!(p.filters.codec.as_deref(), Some("FLAC"));
        p.activate(2, true);
        assert_eq!(p.filters.codec.as_deref(), Some("MP3"));
        p.activate(2, true);
        assert_eq!(p.filters.codec, None, "past the last value is off");
        p.activate(4, true);
        assert_eq!(p.filters.decade, Some(2020));
        assert_eq!(p.filters.count(), 2);
        assert!(p.activate(5, true));
        assert_eq!(p.filters, Filters::default());
        assert!(!p.activate(5, true), "nothing to reset");
    }

    #[test]
    fn genres_are_picked_from_a_list_narrowed_by_typing() {
        let mut p = panel();
        assert!(!p.activate(3, true), "opening the list changes nothing");
        assert!(p.picking());
        p.type_char('o');
        p.type_char('u');
        // "any" then the one genre containing "ou".
        assert!(p.choose(1));
        assert_eq!(p.filters.genre.as_deref(), Some("Soul"));
        p.activate(3, true);
        assert!(p.choose(0));
        assert_eq!(p.filters.genre, None);
    }

    #[test]
    fn it_draws_at_any_size() {
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        let mut p = panel();
        for pick in [false, true] {
            if pick {
                p.activate(3, true);
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

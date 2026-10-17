//! The picker: the library searched as you type, as the apps' picker sheet
//! does it. One sheet with tracks, albums and artists as tabs; picks are
//! kept across the tabs in the order they were made, and the whole pick goes
//! to the queue at once: added, added and played, or in place of it. Jumping
//! to a track in the queue uses the same sheet with one list and no tabs.

use crossbeam_channel::Receiver;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};

use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};

use super::shell::{self, width};
use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Track,
    Album,
    Artist,
    QueueJump,
}

impl PickerKind {
    fn label(self) -> &'static str {
        match self {
            PickerKind::Track => "tracks",
            PickerKind::Album => "albums",
            PickerKind::Artist => "artists",
            PickerKind::QueueJump => "queue",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerPartKind {
    Artist,
    Album,
    Title,
    Date,
    TrackNum,
    Duration,
    Separator,
    Codec,
    Plain,
}

#[derive(Clone)]
pub struct PickerItem {
    pub id: i64,
    pub display: String,
    pub match_text: String,
    pub parts: Vec<(String, PickerPartKind)>,
}

/// One list of the sheet, and the matcher over it.
struct Tab {
    kind: PickerKind,
    items: Vec<PickerItem>,
    nucleo: Nucleo<u32>,
    /// The items, while they are still being read off the UI thread.
    loading: Option<Receiver<Vec<PickerItem>>>,
}

impl Tab {
    fn new(kind: PickerKind, items: Vec<PickerItem>) -> Self {
        let mut tab = Self {
            kind,
            items: Vec::new(),
            nucleo: Nucleo::new(Config::DEFAULT, std::sync::Arc::new(|| {}), None, 1),
            loading: None,
        };
        tab.fill(items);
        tab
    }

    /// A tab whose items are read from the library on a thread of their own.
    fn loading(kind: PickerKind) -> Self {
        let mut tab = Self::new(kind, Vec::new());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("koan-picker".into())
            .spawn(move || {
                tx.send(super::picker_items::load_picker_items(kind)).ok();
            });
        if spawned.is_ok() {
            tab.loading = Some(rx);
        }
        tab
    }

    fn fill(&mut self, items: Vec<PickerItem>) {
        let injector = self.nucleo.injector();
        for (i, item) in items.iter().enumerate() {
            let text = item.match_text.clone();
            injector.push(i as u32, |_, cols| {
                cols[0] = text.into();
            });
        }
        self.items = items;
        self.nucleo.tick(10);
    }

    fn reparse(&mut self, query: &str, append: bool) {
        self.nucleo
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.nucleo.tick(10);
    }

    fn matched(&self) -> usize {
        self.nucleo.snapshot().matched_item_count() as usize
    }

    /// The item at place `n` among the matches.
    fn matched_at(&self, n: usize) -> Option<usize> {
        self.nucleo
            .snapshot()
            .get_matched_item(n as u32)
            .map(|item| *item.data as usize)
    }
}

pub struct PickerState {
    tabs: Vec<Tab>,
    pub tab: usize,
    pub query: String,
    pub cursor: usize,
    /// What has been picked, in the order it was: a tab and an item in it.
    picked: Vec<(usize, usize)>,
    pub multi: bool,
}

impl PickerState {
    /// A sheet of one list, already read.
    pub fn new(kind: PickerKind, items: Vec<PickerItem>, multi: bool) -> Self {
        Self {
            tabs: vec![Tab::new(kind, items)],
            tab: 0,
            query: String::new(),
            cursor: 0,
            picked: Vec::new(),
            multi,
        }
    }

    /// The library's sheet, open on `kind`'s tab. Each tab is read on a
    /// thread of its own and fills in when it arrives.
    pub fn library(kind: PickerKind) -> Self {
        let kinds = [PickerKind::Track, PickerKind::Album, PickerKind::Artist];
        Self {
            tabs: kinds.into_iter().map(Tab::loading).collect(),
            tab: kinds.iter().position(|k| *k == kind).unwrap_or(0),
            query: String::new(),
            cursor: 0,
            picked: Vec::new(),
            multi: true,
        }
    }

    pub fn kind(&self) -> PickerKind {
        self.tabs[self.tab].kind
    }

    /// The tabs' names and how many each matches, when there is more than one.
    pub fn tab_labels(&self) -> Vec<String> {
        if self.tabs.len() < 2 {
            return Vec::new();
        }
        self.tabs
            .iter()
            .map(|t| match t.loading {
                Some(_) => format!("{} \u{2026}", t.kind.label()),
                None => format!("{} {}", t.kind.label(), t.matched()),
            })
            .collect()
    }

    /// Shows tab `i`, matched against what has been typed.
    pub fn set_tab(&mut self, i: usize) {
        if i < self.tabs.len() && i != self.tab {
            self.tab = i;
            let query = self.query.clone();
            self.tabs[i].reparse(&query, false);
            self.cursor = 0;
        }
    }

    /// The tab `delta` along, wrapping.
    pub fn move_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        self.set_tab((self.tab as isize + delta).rem_euclid(n) as usize);
    }

    pub fn prompt(&self) -> &str {
        match self.kind() {
            PickerKind::QueueJump => "jump to a track in the queue",
            _ => "find in the library",
        }
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        let query = self.query.clone();
        self.tabs[self.tab].reparse(&query, true);
        self.cursor = 0;
    }

    pub fn backspace(&mut self) {
        if self.query.pop().is_some() {
            let query = self.query.clone();
            self.tabs[self.tab].reparse(&query, false);
            self.cursor = 0;
        }
    }

    pub fn move_up(&mut self) {
        let count = self.matched_count();
        if count > 0 {
            self.cursor = self.cursor.checked_sub(1).unwrap_or(count - 1);
        }
    }

    pub fn move_down(&mut self) {
        let count = self.matched_count();
        if count > 0 {
            self.cursor = (self.cursor + 1) % count;
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.cursor = self.cursor.saturating_sub(page_size);
    }

    pub fn page_down(&mut self, page_size: usize) {
        let count = self.matched_count();
        if count > 0 {
            self.cursor = (self.cursor + page_size).min(count - 1);
        }
    }

    pub fn jump_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn jump_to_end(&mut self) {
        self.cursor = self.matched_count().saturating_sub(1);
    }

    fn at_cursor(&self) -> Option<(usize, usize)> {
        self.tabs[self.tab]
            .matched_at(self.cursor)
            .map(|i| (self.tab, i))
    }

    fn is_picked(&self, tab: usize, item: usize) -> bool {
        self.picked.contains(&(tab, item))
    }

    /// Picks the row under the cursor, or lets it go.
    fn toggle_cursor(&mut self) {
        if let Some(at) = self.at_cursor() {
            match self.picked.iter().position(|p| *p == at) {
                Some(i) => {
                    self.picked.remove(i);
                }
                None => self.picked.push(at),
            }
        }
    }

    /// Picks the row under the cursor, or lets it go, and moves on.
    pub fn toggle_select(&mut self) {
        if self.multi {
            self.toggle_cursor();
            self.move_down();
        }
    }

    /// Picks the row at `n` among the matches, or lets it go, as a click does.
    pub fn toggle_at(&mut self, n: usize) {
        self.cursor = n;
        if self.multi {
            self.toggle_cursor();
        }
    }

    /// Picks only the row at `n`, as a double click does.
    pub fn pick_only(&mut self, n: usize) {
        self.cursor = n;
        self.picked = self.at_cursor().into_iter().collect();
    }

    pub fn picked_count(&self) -> usize {
        self.picked.len()
    }

    /// What the sheet commits, in the order picked: everything picked, or
    /// with nothing picked, the row under the cursor.
    pub fn confirm(&self) -> Vec<(PickerKind, i64)> {
        let picks = if self.picked.is_empty() {
            self.at_cursor().into_iter().collect()
        } else {
            self.picked.clone()
        };
        picks
            .into_iter()
            .map(|(tab, i)| (self.tabs[tab].kind, self.tabs[tab].items[i].id))
            .collect()
    }

    pub fn matched_count(&self) -> usize {
        self.tabs[self.tab].matched()
    }

    /// Takes in a tab read since the last frame, and lets the matcher catch
    /// up with what was typed.
    pub fn tick(&mut self) {
        let query = self.query.clone();
        for tab in &mut self.tabs {
            if let Some(items) = tab.loading.as_ref().and_then(|rx| rx.try_recv().ok()) {
                tab.loading = None;
                tab.fill(items);
                tab.reparse(&query, false);
            }
        }
        self.tabs[self.tab].nucleo.tick(0);
    }
}

/// The sheet's place in `area`: most of it, never more.
pub fn picker_popup_rect(area: Rect) -> Rect {
    let w = ((area.width as f32 * 0.6) as u16).max(40);
    let h = ((area.height as f32 * 0.7) as u16).max(10);
    shell::centred(area, w, h)
}

/// The rows inside the sheet's frame, with the query, the tabs when there
/// are any, and the keys at the foot taken off.
pub fn picker_results_rect(popup: Rect, tabs: bool) -> Rect {
    let inner = shell::frame("", &Theme::default()).inner(popup);
    let top = 1 + u16::from(tabs);
    if inner.height < top + 2 {
        return Rect::default();
    }
    Rect::new(inner.x, inner.y + top, inner.width, inner.height - top - 1)
}

/// Where each tab's name sits, for a click.
pub fn picker_tab_rects(popup: Rect, labels: &[String]) -> Vec<Rect> {
    let inner = shell::frame("", &Theme::default()).inner(popup);
    let mut x = inner.x + 1;
    let end = inner.x + inner.width;
    labels
        .iter()
        .map(|l| {
            let w = (width(l) as u16).min(end.saturating_sub(x));
            let r = Rect::new(x, inner.y + 1, w, 1);
            x = (x + w + 3).min(end);
            r
        })
        .collect()
}

pub struct PickerOverlay<'a> {
    state: &'a PickerState,
    theme: &'a Theme,
}

impl<'a> PickerOverlay<'a> {
    pub fn new(state: &'a PickerState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for PickerOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = picker_popup_rect(area);
        Clear.render(popup, buf);
        let block = shell::frame(self.state.prompt(), self.theme);
        let inner = block.inner(popup);
        block.render(popup, buf);
        let labels = self.state.tab_labels();
        let results = picker_results_rect(popup, !labels.is_empty());
        if results.height == 0 {
            return;
        }
        let theme = self.theme;
        let tab = &self.state.tabs[self.state.tab];
        let matched = tab.matched();

        // What has been typed.
        let query = Line::from(vec![
            Span::styled(" > ", theme.track_playing),
            Span::raw(self.state.query.as_str()),
            Span::styled("\u{2588}", theme.library_cursor),
        ]);
        buf.set_line(inner.x, inner.y, &query, inner.width);

        // The tabs, the one shown lit.
        for (i, (r, label)) in picker_tab_rects(popup, &labels)
            .into_iter()
            .zip(&labels)
            .enumerate()
        {
            let style = if i == self.state.tab {
                theme
                    .heading
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                theme.hint_desc
            };
            buf.set_stringn(r.x, r.y, label, r.width as usize, style);
        }

        // The matches, the cursor's row kept in view.
        let rows = results.height as usize;
        let cursor = self.state.cursor.min(matched.saturating_sub(1));
        let start = (cursor + 1).saturating_sub(rows);
        let snap = tab.nucleo.snapshot();
        for (row, n) in (start..matched.min(start + rows)).enumerate() {
            let Some(item) = snap.get_matched_item(n as u32) else {
                continue;
            };
            let idx = *item.data as usize;
            let is_cursor = n == cursor;
            let picked = self.state.is_picked(self.state.tab, idx);
            let row_style = if is_cursor {
                theme.picker_cursor
            } else if picked {
                theme.album_header_album
            } else {
                theme.track_normal
            };
            let y = results.y + row as u16;
            for x in results.x..results.x + results.width {
                buf[(x, y)].set_style(row_style);
            }
            let marker = match (picked, is_cursor) {
                (true, _) => "\u{25c6} ",
                (false, true) => "\u{25b8} ",
                (false, false) => "  ",
            };
            let item = &tab.items[idx];
            let mut spans = vec![
                Span::styled(" ", row_style),
                Span::styled(marker, row_style),
            ];
            if item.parts.is_empty() {
                spans.push(Span::styled(item.display.as_str(), row_style));
            } else {
                for (text, kind) in &item.parts {
                    let style = theme.picker_part_style(*kind).patch(row_style);
                    spans.push(Span::styled(text.as_str(), style));
                }
            }
            buf.set_line(results.x, y, &Line::from(spans), results.width);
        }
        if matched == 0 {
            let said = if tab.loading.is_some() {
                "reading the library\u{2026}"
            } else if self.state.query.is_empty() {
                "nothing here"
            } else {
                "no match"
            };
            buf.set_stringn(
                results.x + 1,
                results.y,
                said,
                results.width as usize,
                theme.hint_desc,
            );
        }

        // What the keys do with what is picked.
        let key = |k: &'static str| Span::styled(k, theme.hint_key);
        let desc = |d: &'static str| Span::styled(d, theme.hint_desc);
        let mut hints = vec![Span::raw(" ")];
        let picked = self.state.picked_count();
        if picked > 0 {
            hints.push(Span::styled(
                format!("{picked} picked  "),
                theme.album_header_album,
            ));
        }
        if self.state.multi {
            hints.extend([key("tab"), desc(" pick  ")]);
        }
        if !labels.is_empty() {
            hints.extend([key("\u{2190}\u{2192}"), desc(" kind  ")]);
        }
        if self.state.kind() == PickerKind::QueueJump {
            hints.extend([key("ret"), desc(" play  ")]);
        } else {
            hints.extend([
                key("ret"),
                desc(" add  "),
                key("C-ret"),
                desc(" add & play  "),
                key("C-r"),
                desc(" replace queue  "),
            ]);
        }
        hints.extend([key("esc"), desc(" cancel")]);
        let foot = results.y + results.height;
        buf.set_line(inner.x, foot, &Line::from(hints), inner.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<PickerItem> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| PickerItem {
                id: i as i64 + 1,
                display: n.to_string(),
                match_text: n.to_string(),
                parts: Vec::new(),
            })
            .collect()
    }

    fn sheet() -> PickerState {
        PickerState {
            tabs: vec![
                Tab::new(PickerKind::Track, items(&["alpha", "beta"])),
                Tab::new(PickerKind::Album, items(&["gamma", "delta"])),
                Tab::new(PickerKind::Artist, items(&["epsilon"])),
            ],
            tab: 0,
            query: String::new(),
            cursor: 0,
            picked: Vec::new(),
            multi: true,
        }
    }

    #[test]
    fn picks_span_the_tabs_in_the_order_made() {
        let mut s = sheet();
        s.cursor = 1;
        s.toggle_select();
        s.move_tab(1);
        assert_eq!(s.kind(), PickerKind::Album);
        s.toggle_select();
        s.move_tab(-2);
        assert_eq!(s.kind(), PickerKind::Artist, "the tabs wrap");
        s.toggle_select();
        assert_eq!(
            s.confirm(),
            [
                (PickerKind::Track, 2),
                (PickerKind::Album, 1),
                (PickerKind::Artist, 1)
            ]
        );
    }

    #[test]
    fn with_nothing_picked_the_cursor_row_is_committed() {
        let mut s = sheet();
        s.move_down();
        assert_eq!(s.confirm(), [(PickerKind::Track, 2)]);
        s.toggle_at(0);
        s.toggle_at(0);
        assert_eq!(s.picked_count(), 0, "a second click lets it go");
        s.pick_only(1);
        assert_eq!(s.confirm(), [(PickerKind::Track, 2)]);
    }

    #[test]
    fn typing_narrows_the_tab_shown_and_a_new_tab_follows_it() {
        let mut s = sheet();
        for c in "ta".chars() {
            s.type_char(c);
        }
        s.tick();
        assert_eq!(s.matched_count(), 1, "beta");
        s.set_tab(1);
        s.tick();
        assert_eq!(s.matched_count(), 1, "delta");
    }

    #[test]
    fn the_tabs_are_where_they_are_drawn() {
        let s = sheet();
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        PickerOverlay::new(&s, &Theme::default()).render(area, &mut buf);
        let popup = picker_popup_rect(area);
        let labels = s.tab_labels();
        assert_eq!(labels, ["tracks 2", "albums 2", "artists 1"]);
        for (r, label) in picker_tab_rects(popup, &labels).iter().zip(&labels) {
            let drawn: String = (r.x..r.x + r.width)
                .map(|x| buf[(x, r.y)].symbol())
                .collect();
            assert_eq!(&drawn, label);
        }
    }
}

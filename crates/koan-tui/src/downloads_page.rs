//! The Downloads page: every transfer from the server, those still arriving
//! first, then the ones that settled most recently, as the apps list them.
//! The list is read again when the store's shape changes; the byte counts are
//! read as each frame is drawn, which only happens while something moves.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use koan_core::player::state::SharedPlayerState;
use koan_core::remote::downloads::{Download, DownloadState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::browse::Action;
use super::rows::fit;
use super::theme::Theme;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// A transfer takes two lines: what it is, then how it is going.
const ROW: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadsHit {
    Row(usize),
    Show(usize),
    Clear,
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    clear: Rect,
    links: Vec<(Rect, DownloadsHit)>,
}

#[derive(Default)]
pub struct DownloadsPage {
    transfers: Vec<Download>,
    /// The album each transfer's track is on, for "show in library".
    albums: HashMap<i64, i64>,
    /// The store's version when the list was read.
    seen: Option<u64>,
    cursor: usize,
    scroll: usize,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// A byte count as people read one: "451 MB", "4.2 MB", "1.2 GB", "820 KB",
/// with a decimal only while the number is small.
pub fn bytes(n: u64) -> String {
    let (value, unit) = match n as f64 {
        n if n >= 1e9 => (n / 1e9, "GB"),
        n if n >= 1e6 => (n / 1e6, "MB"),
        n => (n / 1e3, "KB"),
    };
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

/// The figure at the end of a transfer's first line.
fn figure(d: &Download) -> String {
    match &d.state {
        DownloadState::Done => "done".into(),
        DownloadState::Failed(_) => "failed".into(),
        DownloadState::Queued => "queued".into(),
        DownloadState::Running => d
            .fraction()
            .map(|f| format!("{}%", (f * 100.0) as u32))
            .unwrap_or_default(),
    }
}

/// How a transfer is going, in words.
fn detail(d: &Download) -> String {
    let who = |what: &str| {
        if d.artist.is_empty() {
            what.to_string()
        } else {
            format!("{} \u{2014} {what}", d.artist)
        }
    };
    match &d.state {
        DownloadState::Failed(reason) if reason.is_empty() => "couldn't be fetched".into(),
        DownloadState::Failed(reason) => reason.clone(),
        DownloadState::Queued => who("waiting"),
        DownloadState::Done => who("downloaded"),
        DownloadState::Running => {
            let rate = match d.bytes_per_second {
                0 => "stalled".to_string(),
                r => format!("{}/s", bytes(r)),
            };
            let so_far = match d.total {
                0 => bytes(d.bytes_written()),
                total => format!("{} of {}", bytes(d.bytes_written()), bytes(total)),
            };
            let mut parts = vec![rate, so_far];
            if !d.artist.is_empty() {
                parts.insert(0, d.artist.clone());
            }
            parts.join(" \u{b7} ")
        }
    }
}

impl DownloadsPage {
    /// Reads the list again if a transfer was listed, settled or forgotten
    /// since the last read.
    pub fn refresh(&mut self, state: &SharedPlayerState) {
        let store = state.downloads();
        if self.seen == Some(store.version()) {
            return;
        }
        self.seen = Some(store.version());
        self.transfers = store.all();
        let ids: Vec<i64> = self.transfers.iter().map(|d| d.track_id).collect();
        self.albums = koan_core::db::pool::shared()
            .get()
            .ok()
            .and_then(|db| koan_core::db::queries::album_ids_for_tracks(&db.conn, &ids).ok())
            .unwrap_or_default();
        self.cursor = self.cursor.min(self.transfers.len().saturating_sub(1));
    }

    /// How many transfers are still to finish, for the sidebar.
    pub fn unsettled(&self) -> usize {
        self.transfers
            .iter()
            .filter(|d| !d.state.is_settled())
            .count()
    }

    fn settled(&self) -> bool {
        self.transfers.iter().any(|d| d.state.is_settled())
    }

    fn rows(&self) -> usize {
        (self.geom.list.height / ROW).max(1) as usize
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.transfers.len().saturating_sub(1));
        let rows = self.rows();
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + rows {
            self.scroll = self.cursor + 1 - rows;
        }
    }

    pub fn scroll_by(&mut self, rows: isize) {
        let max = self.transfers.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    /// The album page for the transfer at `idx`, with nothing to say when
    /// the track is no longer in the library.
    fn show(&self, idx: usize) -> Action {
        self.transfers
            .get(idx)
            .and_then(|d| self.albums.get(&d.track_id))
            .map_or(Action::None, |&album| Action::OpenAlbum(album))
    }

    fn clear(&mut self, state: &SharedPlayerState) {
        state.downloads().clear_settled();
        self.refresh(state);
    }

    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        state: &SharedPlayerState,
    ) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.rows();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + 1),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter | KeyCode::Char('O') => return Some(self.show(self.cursor)),
            KeyCode::Char('c') => self.clear(state),
            KeyCode::Char('u') => {
                return Some(
                    self.transfers
                        .get(self.cursor)
                        .filter(|d| d.state == DownloadState::Done)
                        .map_or(Action::None, |d| Action::RemoveDownload(vec![d.track_id])),
                );
            }
            _ => return None,
        }
        Some(Action::None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<DownloadsHit> {
        let g = &self.geom;
        if inside(g.clear, col, row) {
            return Some(DownloadsHit::Clear);
        }
        if let Some((_, hit)) = g.links.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if inside(g.list, col, row) {
            let idx = self.scroll + ((row - g.list.y) / ROW) as usize;
            return (idx < self.transfers.len()).then_some(DownloadsHit::Row(idx));
        }
        None
    }

    pub fn click(&mut self, hit: DownloadsHit, state: &SharedPlayerState) -> Action {
        match hit {
            DownloadsHit::Clear => {
                self.clear(state);
                Action::None
            }
            DownloadsHit::Show(i) => {
                self.move_to(i);
                self.show(i)
            }
            DownloadsHit::Row(i) => {
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                if double {
                    self.last_click = None;
                    self.show(i)
                } else {
                    self.last_click = Some((i, Instant::now()));
                    Action::None
                }
            }
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<DownloadsHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 12 || area.height < 3 {
            return;
        }
        let unsettled = self.unsettled();
        let note = if unsettled > 0 {
            format!(" {unsettled} to go ")
        } else {
            " ".to_string()
        };
        super::ui::heading(buf, area, theme, "downloads", &note);
        if self.settled() {
            let label = " clear finished ";
            let w = label.chars().count() as u16;
            let x = (area.x + area.width).saturating_sub(w);
            let style = if hover == Some(DownloadsHit::Clear) {
                theme.hint_key.add_modifier(Modifier::UNDERLINED)
            } else {
                theme.hint_key
            };
            buf.set_stringn(x, area.y, label, w as usize, style);
            self.geom.clear = Rect::new(x, area.y, w, 1);
        }

        let list = Rect::new(
            area.x + 1,
            area.y + 2,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        self.geom.list = list;
        if self.transfers.is_empty() {
            buf.set_stringn(
                list.x,
                list.y,
                "nothing downloading",
                list.width as usize,
                theme.heading,
            );
            if list.height > 1 {
                buf.set_stringn(
                    list.x,
                    list.y + 1,
                    "tracks fetched from your server appear here while they arrive",
                    list.width as usize,
                    theme.hint_desc,
                );
            }
            return;
        }
        if list.height < ROW {
            return;
        }
        self.scroll = self
            .scroll
            .min(self.transfers.len().saturating_sub(self.rows()));
        let width = list.width as usize;
        for (r, idx) in (self.scroll..self.transfers.len())
            .take(self.rows())
            .enumerate()
        {
            let d = &self.transfers[idx];
            let y = list.y + r as u16 * ROW;
            let selected = idx == self.cursor;
            let hovered =
                matches!(hover, Some(DownloadsHit::Row(n) | DownloadsHit::Show(n)) if n == idx);
            let ground = if selected {
                Some(theme.library_cursor)
            } else if hovered {
                Some(theme.library_hover)
            } else {
                None
            };
            if let Some(ground) = ground {
                for line in 0..ROW {
                    for x in list.x..list.x + list.width {
                        buf[(x, y + line)].set_style(ground);
                    }
                }
            }
            let base = ground.unwrap_or(theme.track_normal);
            let dim = ground.unwrap_or(theme.hint_desc);

            let fig = figure(d);
            let fig_w = fig.chars().count();
            let fig_style = match d.state {
                DownloadState::Failed(_) => ground.unwrap_or(theme.failed),
                DownloadState::Running => ground.unwrap_or(theme.spinner),
                _ => dim,
            };
            buf.set_string(
                list.x + (width - fig_w.min(width)) as u16,
                y,
                &fig,
                fig_style,
            );
            let title_w = width.saturating_sub(fig_w + 2);
            buf.set_stringn(list.x, y, fit(&d.title, title_w), title_w, base);

            // The second line: a bar for a transfer whose size is known and
            // still arriving, then how it is going.
            let show = "show in library";
            let link_w = if self.albums.contains_key(&d.track_id)
                && (selected || hovered)
                && width >= show.chars().count() + 24
            {
                show.chars().count() + 2
            } else {
                0
            };
            let mut x = list.x;
            if d.state == DownloadState::Running && width >= 24 {
                let bar_w = (width / 4).min(24);
                let filled = d
                    .fraction()
                    .map_or(0, |f| (f * bar_w as f64).round() as usize);
                for i in 0..bar_w {
                    let (glyph, style) = if i < filled {
                        (
                            '\u{2501}',
                            ground.unwrap_or(Style::new().fg(theme.scrollbar_thumb)),
                        )
                    } else {
                        ('\u{2500}', ground.unwrap_or(theme.rule))
                    };
                    buf[(x + i as u16, y + 1)].set_char(glyph).set_style(style);
                }
                x += bar_w as u16 + 1;
            }
            let detail_style = match d.state {
                DownloadState::Failed(_) => ground.unwrap_or(theme.failed),
                _ => dim,
            };
            let room = (list.x as usize + width).saturating_sub(x as usize + link_w);
            buf.set_stringn(x, y + 1, fit(&detail(d), room), room, detail_style);
            if link_w > 0 {
                let lx = list.x + (width - link_w + 2) as u16;
                let on = hover == Some(DownloadsHit::Show(idx));
                let style = if on {
                    theme.hint_key.add_modifier(Modifier::UNDERLINED)
                } else {
                    theme.hint_key
                };
                buf.set_string(lx, y + 1, show, style);
                self.geom.links.push((
                    Rect::new(lx, y + 1, show.chars().count() as u16, 1),
                    DownloadsHit::Show(idx),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::remote::downloads::ByteFeed;

    fn transfer(id: i64, state: DownloadState, total: u64, written: u64) -> Download {
        let feed = ByteFeed::new();
        feed.set(written);
        Download {
            track_id: id,
            title: format!("track {id}"),
            artist: "artist".into(),
            source: "/tmp/a.part".into(),
            dest: "/tmp/a".into(),
            total,
            written: feed,
            state,
            bytes_per_second: 4_200_000,
        }
    }

    fn page() -> DownloadsPage {
        DownloadsPage {
            transfers: vec![
                transfer(1, DownloadState::Running, 451_000_000, 210_000_000),
                transfer(2, DownloadState::Queued, 0, 0),
                transfer(3, DownloadState::Done, 10, 10),
                transfer(4, DownloadState::Failed("server said no".into()), 0, 0),
            ],
            albums: [(1, 10), (3, 30)].into_iter().collect(),
            seen: Some(0),
            ..Default::default()
        }
    }

    fn text(buf: &Buffer) -> String {
        let area = buf.area;
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn draw(p: &mut DownloadsPage, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        p.render(area, &mut buf, &theme, None);
        buf
    }

    #[test]
    fn each_transfer_says_how_it_is_going() {
        let mut p = page();
        let shown = text(&draw(&mut p, 90, 14));
        assert!(shown.contains("46%"), "{shown}");
        assert!(
            shown.contains("artist \u{b7} 4.2 MB/s \u{b7} 210 MB of 451 MB"),
            "{shown}"
        );
        assert!(shown.contains("artist \u{2014} waiting"));
        assert!(shown.contains("artist \u{2014} downloaded"));
        assert!(shown.contains("server said no"));
        assert!(shown.contains("clear finished"));
        assert!(shown.contains("2 to go"));
    }

    #[test]
    fn a_figure_sits_at_the_end_of_its_line() {
        let mut p = page();
        let buf = draw(&mut p, 60, 14);
        let first: String = (0..60).map(|x| buf[(x, 2)].symbol()).collect();
        assert!(
            first.ends_with("46% "),
            "against the side gutter: {first:?}"
        );
    }

    #[test]
    fn enter_shows_the_track_in_the_library() {
        let mut p = page();
        draw(&mut p, 90, 14);
        let state = SharedPlayerState::new();
        assert_eq!(
            p.handle_key(crossterm::event::KeyCode::Enter.into(), &state),
            Some(Action::OpenAlbum(10))
        );
        p.handle_key(crossterm::event::KeyCode::Char('j').into(), &state);
        assert_eq!(
            p.handle_key(crossterm::event::KeyCode::Enter.into(), &state),
            Some(Action::None),
            "a track the library no longer has goes nowhere"
        );
    }

    #[test]
    fn only_a_finished_download_can_be_removed() {
        let mut p = page();
        draw(&mut p, 90, 14);
        let state = SharedPlayerState::new();
        let u = crossterm::event::KeyCode::Char('u').into();
        assert_eq!(p.handle_key(u, &state), Some(Action::None));
        p.move_to(2);
        assert_eq!(
            p.handle_key(u, &state),
            Some(Action::RemoveDownload(vec![3]))
        );
    }

    #[test]
    fn a_click_lands_on_the_transfer_drawn_there() {
        let mut p = page();
        draw(&mut p, 90, 14);
        assert_eq!(p.hit(5, 2), Some(DownloadsHit::Row(0)));
        assert_eq!(p.hit(5, 3), Some(DownloadsHit::Row(0)));
        assert_eq!(p.hit(5, 4), Some(DownloadsHit::Row(1)));
    }

    #[test]
    fn it_draws_at_any_size() {
        let mut p = page();
        for w in 0..70 {
            for h in 0..14 {
                draw(&mut p, w, h);
            }
        }
        let mut empty = DownloadsPage::default();
        let shown = text(&draw(&mut empty, 80, 6));
        assert!(shown.contains("nothing downloading"));
    }
}

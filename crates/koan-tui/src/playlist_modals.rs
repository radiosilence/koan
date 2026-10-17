//! The two small modals playlists need: a line to type a name into, and a
//! list to choose the playlist something goes in.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use koan_core::db::queries::PlaylistRow;

use super::theme::Theme;

/// What the name typed is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A new playlist holding these tracks.
    New(Vec<i64>),
    Rename(i64),
    /// A folder to add to the library.
    Folder,
}

pub struct NamePrompt {
    pub purpose: Purpose,
    pub text: String,
}

impl NamePrompt {
    pub fn new(purpose: Purpose, text: String) -> Self {
        Self { purpose, text }
    }

    fn title(&self) -> &'static str {
        match self.purpose {
            Purpose::New(_) => " new playlist ",
            Purpose::Rename(_) => " rename playlist ",
            Purpose::Folder => " add library folder ",
        }
    }
}

/// Where the prompt sits in `area`.
pub fn prompt_rect(area: Rect) -> Rect {
    let w = 54u16
        .min(area.width.saturating_sub(4))
        .max(20.min(area.width));
    let h = 5u16.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

/// The `ok` on the prompt's last line, for a click.
pub fn prompt_ok_rect(popup: Rect) -> Rect {
    Rect::new(popup.x + 2, popup.y + popup.height.saturating_sub(2), 6, 1)
}

pub struct NamePromptOverlay<'a> {
    prompt: &'a NamePrompt,
    theme: &'a Theme,
}

impl<'a> NamePromptOverlay<'a> {
    pub fn new(prompt: &'a NamePrompt, theme: &'a Theme) -> Self {
        Self { prompt, theme }
    }
}

impl Widget for NamePromptOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = prompt_rect(area);
        Clear.render(popup, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(self.prompt.title(), self.theme.heading));
        let inner = block.inner(popup);
        block.render(popup, buf);
        if inner.height < 3 || inner.width < 8 {
            return;
        }
        let room = inner.width.saturating_sub(3) as usize;
        let chars: Vec<char> = self.prompt.text.chars().collect();
        // The end of the name stays in view as it is typed.
        let shown: String = chars[chars.len().saturating_sub(room)..].iter().collect();
        buf.set_string(inner.x + 1, inner.y, &shown, self.theme.track_normal);
        let cursor_x = inner.x + 1 + shown.chars().count() as u16;
        if cursor_x < inner.x + inner.width {
            buf[(cursor_x, inner.y)]
                .set_char('\u{2588}')
                .set_style(self.theme.library_cursor);
        }
        let hint_y = popup.y + popup.height.saturating_sub(2);
        let ok = prompt_ok_rect(popup);
        buf.set_string(
            ok.x,
            hint_y,
            "[ ok ]",
            self.theme.track_cursor.add_modifier(Modifier::BOLD),
        );
        buf.set_stringn(
            ok.x + 8,
            hint_y,
            "enter to keep  esc to cancel",
            (inner.x + inner.width).saturating_sub(ok.x + 8) as usize,
            self.theme.hint_desc,
        );
    }
}

/// A list of playlists for something to go in, with "new playlist" first.
pub struct Chooser {
    pub ids: Vec<i64>,
    /// The playlists that can be filled; `None` is one yet to be made.
    pub entries: Vec<Option<PlaylistRow>>,
    pub cursor: usize,
}

impl Chooser {
    pub fn new(ids: Vec<i64>, lists: Vec<PlaylistRow>) -> Self {
        let mut entries = vec![None];
        entries.extend(lists.into_iter().filter(|l| !l.readonly).map(Some));
        Self {
            ids,
            entries,
            cursor: 0,
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.entries.len() as isize;
        if n > 0 {
            self.cursor = (self.cursor as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// The first entry shown when `rows` fit.
    pub fn top(&self, rows: usize) -> usize {
        (self.cursor + 1).saturating_sub(rows.max(1))
    }
}

pub fn chooser_rect(area: Rect, count: usize) -> Rect {
    let w = 56u16.min(area.width.saturating_sub(4));
    let h = (2 + count as u16).min(area.height.saturating_sub(2));
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

pub struct ChooserOverlay<'a> {
    chooser: &'a Chooser,
    theme: &'a Theme,
}

impl<'a> ChooserOverlay<'a> {
    pub fn new(chooser: &'a Chooser, theme: &'a Theme) -> Self {
        Self { chooser, theme }
    }
}

impl Widget for ChooserOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = chooser_rect(area, self.chooser.entries.len());
        Clear.render(popup, buf);
        let n = self.chooser.ids.len();
        let title = format!(
            " add {n} track{} to a playlist ",
            if n == 1 { "" } else { "s" }
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(title, self.theme.heading));
        let inner = block.inner(popup);
        block.render(popup, buf);
        let top = self.chooser.top(inner.height as usize);
        for (row, (i, entry)) in self
            .chooser
            .entries
            .iter()
            .enumerate()
            .skip(top)
            .take(inner.height as usize)
            .enumerate()
        {
            let y = inner.y + row as u16;
            let label = match entry {
                Some(l) => format!(
                    "{} \u{b7} {} track{}",
                    l.name,
                    l.track_count,
                    if l.track_count == 1 { "" } else { "s" }
                ),
                None => "+ new playlist\u{2026}".into(),
            };
            let selected = i == self.chooser.cursor;
            let style = if selected {
                for x in inner.x..inner.x + inner.width {
                    buf[(x, y)].set_style(self.theme.picker_cursor);
                }
                self.theme.picker_cursor
            } else if i == 0 {
                self.theme.track_playing
            } else {
                self.theme.track_normal
            };
            buf.set_stringn(
                inner.x + 1,
                y,
                &label,
                inner.width.saturating_sub(2) as usize,
                style,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: i64, name: &str, readonly: bool) -> PlaylistRow {
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
            track_count: 2,
            duration_ms: 0,
            revision: 0,
            synced_revision: None,
            remote_changed: None,
            rules: None,
            source_path: None,
            readonly,
        }
    }

    #[test]
    fn the_chooser_leaves_out_what_cannot_be_filled() {
        let c = Chooser::new(
            vec![1, 2],
            vec![list(1, "mine", false), list(2, "smart", true)],
        );
        let ids: Vec<_> = c.entries.iter().map(|e| e.as_ref().map(|l| l.id)).collect();
        assert_eq!(
            ids,
            [None, Some(1)],
            "new playlist, and the one that can be filled"
        );
    }

    #[test]
    fn the_cursor_wraps() {
        let mut c = Chooser::new(vec![1], vec![list(1, "a", false)]);
        c.move_by(-1);
        assert_eq!(c.cursor, 1);
        c.move_by(1);
        assert_eq!(c.cursor, 0);
    }

    #[test]
    fn both_draw_at_any_size() {
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        let prompt = NamePrompt::new(Purpose::New(vec![1]), "a long name to type".into());
        let chooser = Chooser::new(vec![1], vec![list(1, "a", false), list(2, "b", false)]);
        for w in 0..70 {
            for h in 0..14 {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                NamePromptOverlay::new(&prompt, &theme).render(area, &mut buf);
                ChooserOverlay::new(&chooser, &theme).render(area, &mut buf);
            }
        }
    }
}

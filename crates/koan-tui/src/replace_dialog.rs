//! Asking before a row replaces the queue, as the apps' replace dialog does:
//! a queue built up over an evening should not go to a click or a key meant
//! to play one track. With anything queued, the person chooses: play the list
//! in place of the queue, add it, or leave things as they are.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::rows;
use super::shell;
use super::text::{count, width};
use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Play,
    Add,
    Cancel,
}

const CHOICES: [(&str, Choice); 3] = [
    ("play", Choice::Play),
    ("add to queue", Choice::Add),
    ("cancel", Choice::Cancel),
];

pub struct ReplaceDialog {
    /// The list that would be played, and the track it starts at.
    pub ids: Vec<i64>,
    pub start: usize,
    /// How many tracks are queued now.
    queued: usize,
    cursor: usize,
    hits: Vec<(Rect, Choice)>,
}

impl ReplaceDialog {
    pub fn new(ids: Vec<i64>, start: usize, queued: usize) -> Self {
        Self {
            ids,
            start,
            queued,
            cursor: 0,
            hits: Vec::new(),
        }
    }

    /// A key: `Some` once a choice is made, `None` while the dialog stays.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Choice> {
        let n = CHOICES.len();
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => return Some(CHOICES[self.cursor].1),
            KeyCode::Char('p') => return Some(Choice::Play),
            KeyCode::Char('a') => return Some(Choice::Add),
            KeyCode::Esc | KeyCode::Char('q' | 'c') => return Some(Choice::Cancel),
            KeyCode::Left | KeyCode::Up | KeyCode::BackTab | KeyCode::Char('h' | 'k') => {
                self.cursor = (self.cursor + n - 1) % n;
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab | KeyCode::Char('l' | 'j') => {
                self.cursor = (self.cursor + 1) % n;
            }
            _ => {}
        }
        None
    }

    /// The choice under the pointer.
    pub fn hit(&self, col: u16, row: u16) -> Option<Choice> {
        self.hits
            .iter()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row == r.y)
            .map(|(_, c)| *c)
    }

    /// Where the dialog sits in `area`.
    pub fn rect(area: Rect) -> Rect {
        shell::centred(area, 50, 7)
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, hover: Option<Choice>) {
        self.hits.clear();
        let popup = Self::rect(area);
        if popup.width < 12 || popup.height < 5 {
            return;
        }
        shell::clear(buf, popup);
        let block = shell::frame("replace the queue?", theme);
        let inner = block.inner(popup);
        block.render(popup, buf);
        let message = format!(
            "Playing this takes the {} queued off.",
            count(self.queued as u64, "track")
        );
        rows::put(
            buf,
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            &message,
            theme.track_normal,
        );
        let y = inner.y + inner.height.saturating_sub(1);
        let mut x = inner.x + 1;
        let end = inner.x + inner.width;
        for (i, (label, choice)) in CHOICES.iter().enumerate() {
            let text = format!(" {label} ");
            let w = width(&text) as u16;
            if x + w > end {
                break;
            }
            let style = if i == self.cursor {
                theme.track_cursor
            } else {
                theme.track_normal
            };
            let at = rows::put(
                buf,
                x,
                y,
                w,
                &text,
                rows::link(style, hover == Some(*choice)),
            );
            self.hits.push((at, *choice));
            x += w + 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_choose_and_move() {
        let mut d = ReplaceDialog::new(vec![1, 2], 1, 5);
        assert_eq!(
            d.handle_key(KeyEvent::from(KeyCode::Enter)),
            Some(Choice::Play)
        );
        assert_eq!(d.handle_key(KeyEvent::from(KeyCode::Right)), None);
        assert_eq!(
            d.handle_key(KeyEvent::from(KeyCode::Enter)),
            Some(Choice::Add)
        );
        assert_eq!(
            d.handle_key(KeyEvent::from(KeyCode::Esc)),
            Some(Choice::Cancel)
        );
        assert_eq!(
            d.handle_key(KeyEvent::from(KeyCode::Char('a'))),
            Some(Choice::Add)
        );
    }

    #[test]
    fn the_choices_are_clickable_and_it_draws_at_any_size() {
        let theme = Theme::default();
        let mut d = ReplaceDialog::new(vec![1], 0, 12);
        for (w, h) in [(0, 0), (10, 4), (30, 8), (80, 24), (200, 60)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            d.render(area, &mut buf, &theme, None);
        }
        let (r, _) = d.hits[1];
        assert_eq!(d.hit(r.x, r.y), Some(Choice::Add));
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        d.render(area, &mut buf, &theme, None);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("12 tracks queued off"));
    }
}

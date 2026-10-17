//! The list of things that can be done with what the cursor is on: the keys
//! that have no room in a hint bar, as a menu for a pointer. A page builds the
//! items; choosing one runs the same action its key would.

use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use super::browse::{Action, scroll_to};
use super::theme::Theme;

pub struct ActionsMenu {
    pub title: String,
    pub items: Vec<(String, Action)>,
    pub cursor: usize,
    /// The first item shown.
    scroll: Cell<usize>,
}

impl ActionsMenu {
    pub fn new(title: String, items: Vec<(String, Action)>) -> Self {
        Self {
            title,
            items,
            cursor: 0,
            scroll: Cell::default(),
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        if n > 0 {
            self.cursor = (self.cursor as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// The first item shown when `rows` fit.
    pub fn top(&self, rows: usize) -> usize {
        scroll_to(&self.scroll, self.cursor, rows)
    }

    /// The action of the item at `idx`, taken out of the menu.
    pub fn take(&mut self, idx: usize) -> Option<Action> {
        (idx < self.items.len()).then(|| self.items.swap_remove(idx).1)
    }
}

pub fn menu_rect(area: Rect, items: &[(String, Action)], title: &str) -> Rect {
    let widest = items
        .iter()
        .map(|(l, _)| l.chars().count())
        .chain([title.chars().count() + 2])
        .max()
        .unwrap_or(10) as u16;
    let w = (widest + 4)
        .min(area.width.saturating_sub(2))
        .max(10.min(area.width));
    let h = (items.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3.min(area.height));
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

pub struct ActionsMenuOverlay<'a> {
    menu: &'a ActionsMenu,
    theme: &'a Theme,
}

impl<'a> ActionsMenuOverlay<'a> {
    pub fn new(menu: &'a ActionsMenu, theme: &'a Theme) -> Self {
        Self { menu, theme }
    }
}

impl Widget for ActionsMenuOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = menu_rect(area, &self.menu.items, &self.menu.title);
        Clear.render(popup, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(
                format!(" {} ", self.menu.title),
                self.theme.heading,
            ));
        let inner = block.inner(popup);
        block.render(popup, buf);
        let top = self.menu.top(inner.height as usize);
        for (row, (i, (label, _))) in self
            .menu
            .items
            .iter()
            .enumerate()
            .skip(top)
            .take(inner.height as usize)
            .enumerate()
        {
            let y = inner.y + row as u16;
            let style = if i == self.menu.cursor {
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
                label,
                inner.width.saturating_sub(2) as usize,
                style,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> ActionsMenu {
        ActionsMenu::new(
            "record".into(),
            vec![
                (
                    "Play".into(),
                    Action::Play(vec![1], super::super::app::PickerAction::AppendAndPlay),
                ),
                ("Favourite".into(), Action::ToggleAlbum(1)),
                ("Info".into(), Action::Info(1)),
            ],
        )
    }

    #[test]
    fn the_cursor_wraps_and_an_item_is_taken_whole() {
        let mut m = menu();
        m.move_by(-1);
        assert_eq!(m.cursor, 2);
        assert_eq!(m.take(1), Some(Action::ToggleAlbum(1)));
        assert_eq!(m.take(9), None);
    }

    #[test]
    fn it_draws_at_any_size() {
        let m = menu();
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        for w in 0..60 {
            for h in 0..12 {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                ActionsMenuOverlay::new(&m, &theme).render(area, &mut buf);
            }
        }
    }
}

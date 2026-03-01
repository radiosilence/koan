use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::app::ContextMenuState;
use super::theme::Theme;

pub struct ContextMenuOverlay<'a> {
    state: &'a ContextMenuState,
    theme: &'a Theme,
}

impl<'a> ContextMenuOverlay<'a> {
    pub fn new(state: &'a ContextMenuState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

/// Compute the popup rect for the context menu — small, roughly centered.
pub fn context_menu_rect(area: Rect, action_count: usize) -> Rect {
    let w = 32u16.min(area.width.saturating_sub(4));
    // 2 for border top/bottom + 1 row per action
    let h = (2 + action_count as u16).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

impl Widget for ContextMenuOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = context_menu_rect(area, self.state.actions.len());
        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Actions ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, (_action, label)) in self.state.actions.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let style = if i == self.state.cursor {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };
            let line = Line::from(Span::styled(format!(" {} ", label), style));
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

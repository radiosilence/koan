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
    click_col: Option<u16>,
    click_row: Option<u16>,
}

impl<'a> ContextMenuOverlay<'a> {
    pub fn new(state: &'a ContextMenuState, theme: &'a Theme) -> Self {
        Self {
            state,
            theme,
            click_col: None,
            click_row: None,
        }
    }

    /// Position the context menu at the mouse click location.
    pub fn at_position(mut self, col: u16, row: u16) -> Self {
        self.click_col = Some(col);
        self.click_row = Some(row);
        self
    }
}

/// Compute the popup rect at a specific position, clamped to terminal bounds.
/// Pass None for both click_col/click_row to center the menu.
pub fn context_menu_rect_at(
    area: Rect,
    action_count: usize,
    click_col: Option<u16>,
    click_row: Option<u16>,
) -> Rect {
    let w = 32u16.min(area.width.saturating_sub(4));
    // 2 for border top/bottom + 1 row per action
    let h = (2 + action_count as u16).min(area.height);

    let (x, y) = match (click_col, click_row) {
        (Some(cx), Some(cy)) => {
            // Position at click, clamped to terminal bounds.
            let x = if cx + w > area.x + area.width {
                (area.x + area.width).saturating_sub(w)
            } else {
                cx
            };
            let y = if cy + h > area.y + area.height {
                cy.saturating_sub(h)
            } else {
                cy
            };
            (x, y)
        }
        _ => {
            // Centered fallback.
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            (x, y)
        }
    };

    Rect::new(x, y, w, h)
}

impl Widget for ContextMenuOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = context_menu_rect_at(
            area,
            self.state.actions.len(),
            self.click_col,
            self.click_row,
        );
        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Actions ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, (_action, label, hotkey)) in self.state.actions.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let is_selected = i == self.state.cursor;
            let base_style = if is_selected {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };
            let key_style = if is_selected {
                base_style
            } else {
                self.theme.hint_key
            };
            let line = Line::from(vec![
                Span::styled(" [", base_style),
                Span::styled(hotkey.to_string(), key_style),
                Span::styled("] ", base_style),
                Span::styled(*label, base_style),
                Span::styled(" ", base_style),
            ]);
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

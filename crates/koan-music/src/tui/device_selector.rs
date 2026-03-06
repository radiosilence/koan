use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::app::DeviceSelectorState;
use super::theme::Theme;

pub struct DeviceSelectorOverlay<'a> {
    state: &'a DeviceSelectorState,
    theme: &'a Theme,
}

impl<'a> DeviceSelectorOverlay<'a> {
    pub fn new(state: &'a DeviceSelectorState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

/// Compute centered popup rect for the device selector.
pub fn device_selector_rect(area: Rect, device_count: usize) -> Rect {
    let w = 50u16.min(area.width.saturating_sub(4));
    // 2 for border + 1 row per device.
    let h = (2 + device_count as u16).min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

impl Widget for DeviceSelectorOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = device_selector_rect(area, self.state.devices.len());
        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Output Device ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, name) in self.state.devices.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let is_selected = i == self.state.cursor;
            let is_current = self
                .state
                .current_device
                .as_ref()
                .is_some_and(|c| c == name);

            let base_style = if is_selected {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };

            let marker = if is_current { "\u{25CF} " } else { "  " };
            let marker_style = if is_selected {
                base_style
            } else if is_current {
                Style::default()
                    .fg(ratatui::style::Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };

            // Truncate name to fit.
            let max_len = inner.width.saturating_sub(3) as usize;
            let display_name = if name.len() > max_len {
                format!("{}…", &name[..max_len.saturating_sub(1)])
            } else {
                name.clone()
            };

            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(display_name, base_style),
            ]);
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

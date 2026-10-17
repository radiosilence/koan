use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::app::DeviceSelectorState;
use super::browse::{fit, scroll_to};
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

impl DeviceSelectorState {
    /// The first device shown when `rows` fit: the cursor stays on screen.
    pub fn top(&self, rows: usize) -> usize {
        scroll_to(&self.scroll, self.cursor, rows)
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

        let rows = inner.height as usize;
        let top = self.state.top(rows);
        for (row, (i, name)) in self
            .state
            .devices
            .iter()
            .enumerate()
            .skip(top)
            .take(rows)
            .enumerate()
        {
            let is_selected = i == self.state.cursor;
            // Index 0 is "System Default" — current when no device is configured.
            let is_current = if i == 0 {
                self.state.current_device.is_none()
            } else {
                self.state
                    .current_device
                    .as_ref()
                    .is_some_and(|c| c == name)
            };

            let base_style = if is_selected {
                self.theme.picker_cursor
            } else {
                self.theme.hint_desc
            };

            let marker = if is_current { "\u{25CF} " } else { "  " };
            let marker_style = if is_selected {
                base_style
            } else if is_current {
                self.theme.status_playing.add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };

            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(
                    fit(name, inner.width.saturating_sub(3) as usize),
                    base_style,
                ),
            ]);
            let row = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

/// The Play on chooser: which kōan the terminal shows and commands.
pub struct PlayOnOverlay<'a> {
    state: &'a super::control::PlayOnState,
    theme: &'a Theme,
}

impl<'a> PlayOnOverlay<'a> {
    pub fn new(state: &'a super::control::PlayOnState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

/// The chooser's rect: wider than the outputs', for what each device is doing.
pub fn play_on_rect(area: Rect, rows: usize) -> Rect {
    let w = 72u16.min(area.width.saturating_sub(4));
    // A row per device, and one saying what is found when only this one is.
    let h = (2 + rows.max(2) as u16).min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

impl Widget for PlayOnOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = play_on_rect(area, self.state.choices.len());
        Clear.render(popup, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Play on ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, choice) in self.state.choices.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let selected = i == self.state.cursor;
            let base = if selected {
                self.theme.picker_cursor
            } else if choice.reachable {
                self.theme.track_normal
            } else {
                self.theme.hint_desc
            };
            let dim = if selected {
                self.theme.picker_cursor
            } else {
                self.theme.hint_desc
            };
            let marker = if choice.controlled { "\u{25CF} " } else { "  " };
            let marker_style = if selected {
                base
            } else if choice.controlled {
                self.theme.status_playing.add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            if selected {
                for x in row.x..row.x + row.width {
                    buf[(x, row.y)].set_style(base);
                }
            }
            let name_w = 24usize.min(inner.width.saturating_sub(2) as usize);
            let name: String = choice.name.chars().take(name_w).collect();
            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(format!("{name:<name_w$}"), base),
                Span::styled(format!(" {}", choice.detail), dim),
            ]);
            Paragraph::new(line).render(row, buf);
        }
        if self.state.choices.len() == 1 && inner.height > 1 {
            let row = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            Paragraph::new(Line::from(Span::styled(
                "  no other kōan devices on this network or your server",
                self.theme.hint_desc,
            )))
            .render(row, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(names: &[&str], cursor: usize) -> DeviceSelectorState {
        DeviceSelectorState {
            devices: names.iter().map(|n| n.to_string()).collect(),
            cursor,
            current_device: None,
            renderers: Vec::new(),
            scroll: Default::default(),
        }
    }

    #[test]
    fn a_long_name_with_accents_is_cut_without_panicking() {
        let s = state(
            &[
                "System Default",
                "Kopfhörer – Wohnzimmer – sehr langer Name",
            ],
            1,
        );
        let theme = Theme::default();
        for w in [1u16, 10, 16, 24, 30, 50] {
            let area = Rect::new(0, 0, w, 10);
            let mut buf = Buffer::empty(area);
            DeviceSelectorOverlay::new(&s, &theme).render(area, &mut buf);
        }
    }

    #[test]
    fn the_cursor_stays_on_screen_in_a_long_list() {
        let names: Vec<String> = (0..30).map(|i| format!("device {i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut s = state(&names, 25);
        assert_eq!(s.top(8), 18);
        s.cursor = 20;
        assert_eq!(s.top(8), 18, "moving within the view leaves it still");
        s.cursor = 10;
        assert_eq!(s.top(8), 10);
        s.cursor = 25;
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        DeviceSelectorOverlay::new(&s, &Theme::default()).render(area, &mut buf);
        let shown: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(shown.contains("device 25"));
    }
}

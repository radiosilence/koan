//! Choosing the EQ the output plays through: a list of the profiles, with the
//! one in use marked. It does what the apps' EQ page and `koan dsp assign` do,
//! through the same calls, so the three never disagree about what is chosen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use koan_core::audio::dsp::profiles;

use super::theme::Theme;

pub struct DspEntry {
    /// `None` is the output untouched.
    pub name: Option<String>,
    pub label: String,
}

pub struct DspSelectorState {
    pub entries: Vec<DspEntry>,
    pub cursor: usize,
    /// The profile the output plays through now.
    pub active: Option<String>,
}

impl DspSelectorState {
    pub fn load(device: Option<String>) -> Self {
        let overview = profiles::overview_for(device);
        let mut entries = vec![DspEntry {
            name: None,
            label: "off \u{b7} the output untouched".into(),
        }];
        for p in overview.profiles.iter().filter(|p| p.problem.is_none()) {
            let kind = if p.preset {
                " \u{b7} preset"
            } else if !p.members.is_empty() {
                " \u{b7} group"
            } else {
                ""
            };
            entries.push(DspEntry {
                name: Some(p.name.clone()),
                label: format!("{}{kind}", p.name),
            });
        }
        let active = overview.active;
        let cursor = entries.iter().position(|e| e.name == active).unwrap_or(0);
        Self {
            entries,
            cursor,
            active,
        }
    }

    pub fn is_active(&self, idx: usize) -> bool {
        self.entries.get(idx).is_some_and(|e| e.name == self.active)
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.entries.len();
        if n > 0 {
            self.cursor = (self.cursor as isize + delta).rem_euclid(n as isize) as usize;
        }
    }

    /// The first entry shown when `rows` fit: the cursor stays on screen.
    pub fn top(&self, rows: usize) -> usize {
        (self.cursor + 1).saturating_sub(rows.max(1))
    }
}

pub fn dsp_selector_rect(area: Rect, count: usize) -> Rect {
    let w = 60u16.min(area.width.saturating_sub(4));
    let h = (2 + count as u16).min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

pub struct DspSelectorOverlay<'a> {
    state: &'a DspSelectorState,
    theme: &'a Theme,
}

impl<'a> DspSelectorOverlay<'a> {
    pub fn new(state: &'a DspSelectorState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for DspSelectorOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = dsp_selector_rect(area, self.state.entries.len());
        Clear.render(popup, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(" eq ", self.theme.heading));
        let inner = block.inner(popup);
        block.render(popup, buf);
        let top = self.state.top(inner.height as usize);
        for (row, (i, entry)) in self
            .state
            .entries
            .iter()
            .enumerate()
            .skip(top)
            .take(inner.height as usize)
            .enumerate()
        {
            let selected = i == self.state.cursor;
            let style = if selected {
                self.theme.picker_cursor
            } else {
                self.theme.track_normal
            };
            let marker = if self.state.is_active(i) {
                "\u{25cf} "
            } else {
                "  "
            };
            let marker_style = if selected {
                style
            } else {
                self.theme.status_playing.add_modifier(Modifier::BOLD)
            };
            if selected {
                for x in inner.x..inner.x + inner.width {
                    buf[(x, inner.y + row as u16)].set_style(style);
                }
            }
            let max = inner.width.saturating_sub(3) as usize;
            let label: String = if entry.label.chars().count() > max {
                let mut cut: String = entry.label.chars().take(max.saturating_sub(1)).collect();
                cut.push('\u{2026}');
                cut
            } else {
                entry.label.clone()
            };
            Paragraph::new(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(label, style),
            ]))
            .render(
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
                buf,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(n: usize) -> DspSelectorState {
        DspSelectorState {
            entries: (0..n)
                .map(|i| DspEntry {
                    name: Some(format!("p{i}")),
                    label: format!("profile {i}"),
                })
                .collect(),
            cursor: 0,
            active: Some("p1".into()),
        }
    }

    #[test]
    fn the_cursor_wraps_and_stays_on_screen() {
        let mut s = state(10);
        s.move_by(-1);
        assert_eq!(s.cursor, 9);
        assert_eq!(s.top(4), 6);
        s.move_by(1);
        assert_eq!(s.cursor, 0);
        assert_eq!(s.top(4), 0);
    }

    #[test]
    fn the_active_profile_is_marked() {
        let s = state(3);
        assert!(s.is_active(1));
        assert!(!s.is_active(0));
    }

    #[test]
    fn it_draws_in_any_area() {
        let s = state(40);
        let theme = Theme::default();
        for w in [1u16, 5, 20, 80] {
            for h in [1u16, 3, 8, 50] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                DspSelectorOverlay::new(&s, &theme).render(area, &mut buf);
            }
        }
    }
}

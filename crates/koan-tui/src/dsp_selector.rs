//! The preset menu, as the apps' transport has it: what the output in use
//! was set from, Flat and each preset to set it from instead, and the EQ
//! page. A preset changed since it was set is listed twice: as edited,
//! marked, and as saved, which goes back to it.

use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use super::browse::scroll_to;
use super::eq_worker::Snapshot;
use super::text::fit;
use super::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choose {
    /// Set the output from a preset, or flat with `None`.
    Preset(Option<String>),
    /// What it is set to now, unsaved or edited: nothing to do.
    Nothing,
    Edit,
}

pub struct DspEntry {
    pub choose: Choose,
    pub label: String,
}

pub struct DspSelectorState {
    pub title: String,
    pub entries: Vec<DspEntry>,
    pub cursor: usize,
    /// The entry the output is set to.
    pub active: Option<usize>,
    /// The first entry shown.
    scroll: Cell<usize>,
}

impl DspSelectorState {
    pub fn load(snap: &Snapshot) -> Self {
        let mut entries = vec![DspEntry {
            choose: Choose::Preset(None),
            label: "flat \u{b7} plays untouched".into(),
        }];
        let mut active = None;
        let current = snap.in_use_preset.clone();
        if current.is_none() {
            if snap.in_use_flat {
                active = Some(0);
            } else {
                active = Some(entries.len());
                entries.push(DspEntry {
                    choose: Choose::Nothing,
                    label: "unsaved".into(),
                });
            }
        }
        for p in snap.overview.profiles.iter().filter(|p| p.preset) {
            if let Some((name, edited)) = &current
                && *name == p.name
            {
                if *edited {
                    active = Some(entries.len());
                    entries.push(DspEntry {
                        choose: Choose::Nothing,
                        label: format!("{} (edited)", p.name),
                    });
                } else {
                    active = Some(entries.len());
                }
            }
            entries.push(DspEntry {
                choose: Choose::Preset(Some(p.name.clone())),
                label: p.name.clone(),
            });
        }
        entries.push(DspEntry {
            choose: Choose::Edit,
            label: "edit eq\u{2026}".into(),
        });
        let title = snap.in_use.as_deref().map_or_else(
            || "preset".into(),
            |d| format!("preset \u{b7} {}", snap.label(d)),
        );
        Self {
            title,
            cursor: active.unwrap_or(0),
            entries,
            active,
            scroll: Cell::default(),
        }
    }

    pub fn is_active(&self, idx: usize) -> bool {
        self.active == Some(idx)
    }

    /// The entry chosen, marked ahead of the read that will say so.
    pub fn mark(&mut self, idx: usize) {
        self.active = Some(idx);
        self.cursor = idx;
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.entries.len();
        if n > 0 {
            self.cursor = (self.cursor as isize + delta).rem_euclid(n as isize) as usize;
        }
    }

    /// The first entry shown when `rows` fit: the cursor stays on screen.
    pub fn top(&self, rows: usize) -> usize {
        scroll_to(&self.scroll, self.cursor, rows)
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
        crate::shell::clear(buf, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.rule)
            .title(Span::styled(
                format!(" {} ", self.state.title),
                self.theme.heading,
            ));
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
            let label = fit(&entry.label, inner.width.saturating_sub(3) as usize);
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
            title: "preset".into(),
            entries: (0..n)
                .map(|i| DspEntry {
                    choose: Choose::Preset(Some(format!("p{i}"))),
                    label: format!("preset {i}"),
                })
                .collect(),
            cursor: 0,
            active: Some(1),
            scroll: Cell::default(),
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
    fn the_preset_in_use_is_marked() {
        let mut s = state(3);
        assert!(s.is_active(1));
        s.mark(2);
        assert!(s.is_active(2));
        assert!(!s.is_active(1));
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

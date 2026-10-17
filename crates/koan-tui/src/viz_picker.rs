use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::browse::scroll_to;
use super::theme::Theme;
use super::visualizer::VisualizerMode;

/// All visualizer modes in rotation order.
pub const ALL_MODES: &[VisualizerMode] = &[
    VisualizerMode::Bars,
    VisualizerMode::Oscilloscope,
    VisualizerMode::Radial,
    VisualizerMode::Particles,
    VisualizerMode::Lissajous,
    VisualizerMode::Spectrogram,
    VisualizerMode::StereoWaveform,
    VisualizerMode::VuMeter,
    VisualizerMode::Flame,
    VisualizerMode::Plasma,
    VisualizerMode::Tunnel,
    VisualizerMode::Wireframe,
    VisualizerMode::Metaballs,
    VisualizerMode::Starfield,
    VisualizerMode::Terrain,
    VisualizerMode::Moire,
    VisualizerMode::Kaleidoscope,
    VisualizerMode::Julia,
    VisualizerMode::Spiral,
    VisualizerMode::Interference,
    VisualizerMode::Wormhole,
    VisualizerMode::Matrix,
];

/// State for the visualizer picker modal.
pub struct VizPickerState {
    pub cursor: usize,
    pub current_mode: VisualizerMode,
    /// The first mode shown, in a terminal too short for them all.
    scroll: Cell<usize>,
}

impl VizPickerState {
    pub fn new(current: VisualizerMode) -> Self {
        let cursor = ALL_MODES.iter().position(|&m| m == current).unwrap_or(0);
        Self {
            cursor,
            current_mode: current,
            scroll: Cell::default(),
        }
    }

    pub fn selected_mode(&self) -> VisualizerMode {
        ALL_MODES[self.cursor]
    }
}

pub struct VizPickerOverlay<'a> {
    state: &'a VizPickerState,
    theme: &'a Theme,
}

impl<'a> VizPickerOverlay<'a> {
    pub fn new(state: &'a VizPickerState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for VizPickerOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = 36u16.min(area.width.saturating_sub(4));
        let h = (2 + ALL_MODES.len() as u16).min(area.height.saturating_sub(2));

        // Right-aligned to stay out of the visualizer's way.
        let x = area.x + area.width.saturating_sub(w + 2);
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect::new(x, y, w, h);

        crate::shell::clear(buf, popup);

        let block = crate::shell::frame("visualiser", self.theme);
        let inner = block.inner(popup);
        block.render(popup, buf);

        // A terminal too short for the whole list scrolls it to the cursor.
        let rows = inner.height as usize;
        let first = scroll_to(&self.state.scroll, self.state.cursor, rows);
        for (row, (i, mode)) in ALL_MODES
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .enumerate()
        {
            let is_cursor = i == self.state.cursor;
            let is_current = *mode == self.state.current_mode;

            let base_style = if is_cursor {
                self.theme.picker_cursor
            } else {
                self.theme.hint_desc
            };

            let marker = if is_current { "\u{25CF} " } else { "  " };
            let marker_style = if is_cursor {
                base_style
            } else if is_current {
                self.theme.status_playing.add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };

            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(mode.label(), base_style),
            ]);
            let row = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_stays_on_screen_in_a_short_terminal() {
        let theme = Theme::default();
        let mut state = VizPickerState::new(VisualizerMode::Bars);
        state.cursor = ALL_MODES.len() - 1;
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        VizPickerOverlay::new(&state, &theme).render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains(ALL_MODES[state.cursor].label()), "{text}");
        let top = state.scroll.get();
        state.cursor -= 1;
        VizPickerOverlay::new(&state, &theme).render(area, &mut buf);
        assert_eq!(
            state.scroll.get(),
            top,
            "moving within the view leaves it still"
        );
    }
}

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

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
];

/// State for the visualizer picker modal.
pub struct VizPickerState {
    pub cursor: usize,
    pub current_mode: VisualizerMode,
}

impl VizPickerState {
    pub fn new(current: VisualizerMode) -> Self {
        let cursor = ALL_MODES.iter().position(|&m| m == current).unwrap_or(0);
        Self {
            cursor,
            current_mode: current,
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

        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Visualiser ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, mode) in ALL_MODES.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let is_cursor = i == self.state.cursor;
            let is_current = *mode == self.state.current_mode;

            let base_style = if is_cursor {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };

            let marker = if is_current { "\u{25CF} " } else { "  " };
            let marker_style = if is_cursor {
                base_style
            } else if is_current {
                Style::default()
                    .fg(ratatui::style::Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.theme.hint_desc
            };

            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(mode.label(), base_style),
            ]);
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

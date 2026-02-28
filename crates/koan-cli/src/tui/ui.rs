use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use super::app::{App, LibraryFocus, Mode};
use super::keys::HintBar;
use super::library::LibraryView;
use super::picker::{PickerOverlay, picker_popup_rect};
use super::queue::QueueView;
use super::track_info::TrackInfoOverlay;
use super::transport::TransportBar;

pub fn render(frame: &mut Frame, app: &mut App) {
    // Refresh the visible queue cache once per frame so all reads
    // within this render cycle see a consistent snapshot.
    app.refresh_visible_queue();

    let area = frame.area();

    // Main layout: transport (3) | content (flex) | hints (1)
    let chunks = Layout::vertical([
        Constraint::Length(3), // transport
        Constraint::Min(3),    // content area
        Constraint::Length(1), // hint bar
    ])
    .split(area);

    // Store areas for mouse interaction.
    app.transport_area = chunks[0];

    // Transport.
    let track_info = app.state.track_info();
    let transport = TransportBar::new(
        track_info.as_ref(),
        app.state.playback_state(),
        app.state.position_ms(),
        &app.theme,
    );
    frame.render_widget(transport, chunks[0]);

    // Content area: library + queue side-by-side, or just queue.
    let content_area = chunks[1];
    let show_library = app.mode == Mode::LibraryBrowse && app.library.is_some();

    if show_library {
        let panes = Layout::horizontal([
            Constraint::Percentage(40), // library
            Constraint::Percentage(60), // queue
        ])
        .split(content_area);

        app.library_area = panes[0];
        app.queue_area = panes[1];

        // Library pane.
        if let Some(ref mut lib) = app.library {
            let visible_height = panes[0].height.saturating_sub(2) as usize;
            lib.update_scroll(visible_height);
            let focused = app.library_focus == LibraryFocus::Library;
            let lib_view = LibraryView::new(lib, &app.theme, focused);
            frame.render_widget(lib_view, panes[0]);
        }

        // Queue pane.
        render_queue(frame, app, panes[1]);
    } else {
        app.queue_area = content_area;
        render_queue(frame, app, content_area);
    }

    // Key hints.
    let hint_bar = HintBar::new(&app.mode, &app.theme);
    frame.render_widget(hint_bar, chunks[2]);

    // Picker overlay (on top of everything).
    if let Mode::Picker(_) = &app.mode
        && let Some(ref picker) = app.picker
    {
        app.picker_area = picker_popup_rect(area);
        let overlay = PickerOverlay::new(picker, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Track info overlay.
    if let Mode::TrackInfo(idx) = app.mode {
        let visible = app.visible_queue();
        if let Some(entry) = visible.get(idx) {
            let current_track_info = app.state.track_info();
            let is_playing = entry.status == koan_core::player::state::QueueEntryStatus::Playing;
            let ti_ref = if is_playing {
                current_track_info.as_ref()
            } else {
                None
            };

            // Calculate popup rect for mouse hit-testing.
            let popup_width = (area.width as f32 * 0.7).max(40.0).min(area.width as f32) as u16;
            let popup_height = (area.height as f32 * 0.7).max(14.0).min(area.height as f32) as u16;
            let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
            let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
            app.track_info_area = Rect::new(x, y, popup_width, popup_height);

            let overlay = TrackInfoOverlay::new(entry, ti_ref, app.cover_art.cached(), &app.theme);
            frame.render_widget(overlay, area);
        }
    }

    // Loading overlay with braille spinner.
    if let Some(ref msg) = app.loading_message {
        const SPINNER: &[char] = &[
            '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}',
        ];
        let frame_char = SPINNER[app.spinner_tick % SPINNER.len()];
        let display = format!("{} {}", frame_char, msg);
        let text_len = display.len() as u16 + 4;
        let w = text_len.max(20).min(area.width);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + area.height / 2;
        let popup = Rect::new(x, y, w, 1);
        Clear.render(popup, frame.buffer_mut());
        let line = Line::from(vec![
            Span::styled("  ", app.theme.hint_desc),
            Span::styled(display, app.theme.spinner),
            Span::styled("  ", app.theme.hint_desc),
        ]);
        frame.render_widget(Paragraph::new(line), popup);
    }
}

fn render_queue(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let visible = app.visible_queue();

    // Clamp cursor.
    if !visible.is_empty() && app.queue_cursor >= visible.len() {
        app.queue_cursor = visible.len() - 1;
    }

    let drag_target = app.drag_target_index();
    let queue_view = QueueView::new(
        &visible,
        &app.mode,
        app.queue_cursor,
        app.queue_scroll_offset,
        &app.theme,
        &app.selected_indices,
        app.spinner_tick,
    )
    .with_drag_target(drag_target);
    frame.render_widget(queue_view, area);
}

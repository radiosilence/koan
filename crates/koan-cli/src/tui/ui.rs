use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use super::app::{App, LibraryFocus, Mode};
use super::keys::HintBar;
use super::library::LibraryView;
use super::picker::PickerOverlay;
use super::queue::QueueView;
use super::transport::TransportBar;

pub fn render(frame: &mut Frame, app: &mut App) {
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
        let overlay = PickerOverlay::new(picker, &app.theme);
        frame.render_widget(overlay, area);
    }
}

fn render_queue(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let queue = app.state.full_queue();

    // Clamp cursor.
    if !queue.is_empty() && app.queue_cursor >= queue.len() {
        app.queue_cursor = queue.len() - 1;
    }

    let drag_target = app.drag_target_index();
    let queue_view = QueueView::new(
        &queue,
        &app.mode,
        app.queue_cursor,
        app.queue_scroll_offset,
        app.spinner_tick,
        &app.theme,
        &app.selected_indices,
    )
    .with_drag_target(drag_target);
    frame.render_widget(queue_view, area);
}

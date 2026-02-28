use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use super::app::{App, Mode};
use super::keys::HintBar;
use super::picker::PickerOverlay;
use super::queue::QueueView;
use super::transport::TransportBar;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Main layout: transport (3) | queue (flex) | hints (1)
    let chunks = Layout::vertical([
        Constraint::Length(3), // transport
        Constraint::Min(3),    // queue
        Constraint::Length(1), // hint bar
    ])
    .split(area);

    // Store areas for mouse interaction.
    app.transport_area = chunks[0];
    app.queue_area = chunks[1];

    // Transport.
    let track_info = app.state.track_info();
    let transport = TransportBar::new(
        track_info.as_ref(),
        app.state.playback_state(),
        app.state.position_ms(),
        &app.theme,
    );
    frame.render_widget(transport, chunks[0]);

    // Queue.
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
    frame.render_widget(queue_view, chunks[1]);

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

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use super::app::{App, LibraryFocus, Mode};
use super::context_menu::{ContextMenuOverlay, context_menu_rect_at};
use super::device_selector::DeviceSelectorOverlay;
use super::help_modal::HelpModalOverlay;
use super::keys::HintBar;
use super::library::LibraryView;
use super::lyrics::LyricsPanel;
use super::organize::{OrganizeOverlay, organize_popup_rect};
use super::picker::{PickerOverlay, picker_popup_rect};
use super::queue::QueueView;
use super::track_info::TrackInfoOverlay;
use super::transport::TransportBar;
use super::visualizer::VisualizerWidget;
use super::viz_picker::VizPickerOverlay;

/// Height of the transport bar without album art.
const TRANSPORT_HEIGHT_DEFAULT: u16 = 3;

pub fn render(frame: &mut Frame, app: &mut App) {
    // Refresh the visible queue cache once per frame so all reads
    // within this render cycle see a consistent snapshot.
    app.refresh_visible_queue();

    let area = frame.area();

    // Below this the transport/queue geometry has no room to lay out.
    if area.height < 5 || area.width < 20 {
        frame.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }

    // Fullscreen visualizer — take the entire frame, skip everything else.
    if app.viz_fullscreen && app.viz_config.enabled {
        let viz = VisualizerWidget::new(&mut app.visualizer, &app.theme);
        viz.render(area, frame.buffer_mut());

        // FPS overlay in fullscreen too.
        if app.show_fps {
            render_fps(frame, app, area);
        }

        // Visualizer picker overlay renders on top of fullscreen viz.
        if app.mode == Mode::VizPicker
            && let Some(ref picker) = app.viz_picker
        {
            let overlay = VizPickerOverlay::new(picker, &app.theme);
            frame.render_widget(overlay, area);
        }

        return;
    }

    let has_art = app.art.now_playing_art.cached().is_some();
    let art_height = app.art_size / 2; // square via halfblock rendering
    let transport_height = art_height.max(TRANSPORT_HEIGHT_DEFAULT);

    // Main layout: transport | content (flex) | hints (1)
    let chunks = Layout::vertical([
        Constraint::Length(transport_height),
        Constraint::Min(3),    // content area
        Constraint::Length(1), // hint bar
    ])
    .split(area);

    // The solver shrinks the transport when the terminal is short — every
    // rect below is derived from what it actually granted, not the request.
    let transport_rect = chunks[0];
    app.layout.transport_area = transport_rect;

    // Transport — with optional album art on the left.
    let track_info = app.state.track_info();

    // Find the currently playing QueueEntry for rich metadata.
    let playing_entry = app
        .queue
        .vq_cache
        .entries
        .iter()
        .find(|e| e.status == koan_core::player::state::QueueEntryStatus::Playing)
        .cloned();

    // Always reserve art space — placeholder keeps layout stable.
    let art_width = app.art_size.min(transport_rect.width.saturating_sub(2));
    let art_area = Rect::new(
        transport_rect.x,
        transport_rect.y,
        art_width,
        transport_rect.height,
    );
    let text_area = {
        // Bottom-align the transport text (3 lines) within the granted height.
        let text_height = 3u16.min(transport_rect.height);
        let text_y = transport_rect.y + transport_rect.height - text_height;
        Rect::new(
            transport_rect.x + art_width + 1,
            text_y,
            transport_rect.width.saturating_sub(art_width + 1),
            text_height,
        )
    };

    if has_art {
        app.layout.now_playing_art_area = art_area;
        app.art
            .now_playing_art
            .render_to(art_area, frame.buffer_mut());
    } else {
        // No art — area is reserved but empty (placeholder space).
        app.layout.now_playing_art_area = art_area;
    }
    app.layout.transport_text_area = text_area;

    // Seek bar metrics + transport widget — rendered once.
    // Prefer the queue entry's DB-sourced duration over the probed duration so
    // bar_metrics and the rendered time string agree (probing a partial streaming
    // file returns a truncated duration).
    let pos_ms = app.state.position_ms();
    let dur_ms = playing_entry
        .as_ref()
        .and_then(|e| e.duration_ms)
        .or_else(|| track_info.as_ref().map(|t| t.duration_ms))
        .unwrap_or(0);
    let (bs, bw) = TransportBar::bar_metrics(text_area, pos_ms, dur_ms);
    app.layout.seek_bar_start = bs;
    app.layout.seek_bar_width = bw;

    let dl_fraction = app.state.current_download_fraction();
    let transport = TransportBar::new(
        track_info.as_ref(),
        playing_entry.as_ref(),
        app.state.playback_state(),
        pos_ms,
        &app.theme,
    )
    .with_ticker_offset(app.ticker_offset)
    .with_download_fraction(dl_fraction);
    frame.render_widget(transport, text_area);

    // Visualizer — renders in the space above the transport text.
    // Dispatches to the active mode (bars, oscilloscope, radial, particles, lissajous).
    let spectrum_height = transport_rect
        .height
        .saturating_sub(TRANSPORT_HEIGHT_DEFAULT);
    if spectrum_height > 0 {
        let spectrum_area = Rect::new(
            transport_rect.x + art_width + 1,
            transport_rect.y,
            transport_rect.width.saturating_sub(art_width + 1),
            spectrum_height,
        );
        let viz = VisualizerWidget::new(&mut app.visualizer, &app.theme);
        viz.render(spectrum_area, frame.buffer_mut());
    }

    // Content area: library + queue side-by-side, or just queue, with optional lyrics panel.
    let content_area = chunks[1];
    let show_library = app.mode == Mode::LibraryBrowse && app.library.is_some();
    let show_lyrics = app.lyrics_panel;

    if show_library {
        let panes = Layout::horizontal([
            Constraint::Percentage(40), // library
            Constraint::Percentage(60), // queue
        ])
        .split(content_area);

        app.layout.library_area = panes[0];
        app.layout.queue_area = panes[1];

        // Library pane.
        let visible_height = app.library_content_height();
        if let Some(ref mut lib) = app.library {
            lib.update_scroll(visible_height);
            let focused = app.library_focus == LibraryFocus::Library;
            let hover_idx = match &app.hover.zone {
                super::app::HoverZone::LibraryItem(idx) => Some(*idx),
                _ => None,
            };
            let lib_view = LibraryView::new(lib, &app.theme, focused).with_hover(hover_idx);
            frame.render_widget(lib_view, panes[0]);
        }

        // Queue pane.
        render_queue(frame, app, panes[1]);
    } else if show_lyrics {
        let panes = Layout::horizontal([
            Constraint::Percentage(60), // queue
            Constraint::Percentage(40), // lyrics
        ])
        .split(content_area);

        app.layout.queue_area = panes[0];
        render_queue(frame, app, panes[0]);

        // Lyrics panel.
        let pos_ms = app.state.position_ms();
        let lyrics_panel = LyricsPanel::new(&app.lyrics, pos_ms, &app.theme, app.spinner_tick);
        frame.render_widget(lyrics_panel, panes[1]);
    } else {
        app.layout.queue_area = content_area;
        render_queue(frame, app, content_area);
    }

    // Key hints / status message.
    let radio_badge = if app.state.radio_mode() {
        Some(ratatui::text::Span::styled(
            " RADIO ",
            ratatui::style::Style::new()
                .fg(ratatui::style::Color::Black)
                .bg(ratatui::style::Color::Magenta)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ))
    } else {
        None
    };

    if let Some((ref msg, _)) = app.status_message {
        let style = app.theme.hint_key;
        let mut spans = vec![
            ratatui::text::Span::styled(msg.as_str(), style),
            ratatui::text::Span::styled("  [Esc] dismiss", app.theme.hint_desc),
        ];
        if let Some(badge) = radio_badge {
            spans.push(ratatui::text::Span::raw("  "));
            spans.push(badge);
        }
        let line = ratatui::text::Line::from(spans);
        frame.render_widget(ratatui::widgets::Paragraph::new(line), chunks[2]);
    } else {
        let hint_bar = HintBar::new(&app.mode, &app.theme);
        frame.render_widget(hint_bar, chunks[2]);
        // Overlay radio badge on the right edge of the hint bar.
        if let Some(badge) = radio_badge {
            let badge_width = 7u16;
            if chunks[2].width > badge_width + 1 {
                let badge_area = ratatui::layout::Rect {
                    x: chunks[2].x + chunks[2].width - badge_width - 1,
                    y: chunks[2].y,
                    width: badge_width,
                    height: 1,
                };
                frame.render_widget(
                    ratatui::widgets::Paragraph::new(ratatui::text::Line::from(badge)),
                    badge_area,
                );
            }
        }
    }

    // Picker overlay (on top of everything).
    if let Mode::Picker(_) = &app.mode
        && let Some(ref picker) = app.picker
    {
        app.layout.picker_area = picker_popup_rect(area);
        let overlay = PickerOverlay::new(picker, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Context menu overlay — positioned at click location if available.
    if app.mode == Mode::ContextMenu
        && let Some(ref menu) = app.context_menu
    {
        let click_col = if app.hover.column > 0 {
            Some(app.hover.column)
        } else {
            None
        };
        let click_row = if app.hover.row > 0 {
            Some(app.hover.row)
        } else {
            None
        };
        app.layout.context_menu_area =
            context_menu_rect_at(area, menu.actions.len(), click_col, click_row);
        let mut overlay = ContextMenuOverlay::new(menu, &app.theme);
        if let (Some(c), Some(r)) = (click_col, click_row) {
            overlay = overlay.at_position(c, r);
        }
        frame.render_widget(overlay, area);
    }

    // Organize modal overlay.
    if app.mode == Mode::Organize
        && let Some(ref org) = app.organize
    {
        app.layout.organize_area = organize_popup_rect(area);
        let overlay = OrganizeOverlay::new(org, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Device selector overlay.
    if app.mode == Mode::DeviceSelector
        && let Some(ref selector) = app.device_selector
    {
        let overlay = DeviceSelectorOverlay::new(selector, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Help modal overlay.
    if app.mode == Mode::HelpModal {
        let overlay = HelpModalOverlay::new(&app.theme);
        frame.render_widget(overlay, area);
    }

    // Visualizer picker overlay.
    if app.mode == Mode::VizPicker
        && let Some(ref picker) = app.viz_picker
    {
        let overlay = VizPickerOverlay::new(picker, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Track info overlay.
    if let Mode::TrackInfo(id) = app.mode
        && let Some(entry) = app
            .queue
            .vq_cache
            .entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
    {
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
        app.layout.track_info_area = Rect::new(x, y, popup_width, popup_height);

        let overlay = TrackInfoOverlay::new(&entry, ti_ref, app.art.cover_art.cached(), &app.theme);
        frame.render_widget(overlay, area);
    }

    // Cover art zoom overlay — fullscreen, 1:1 aspect ratio.
    if app.mode == Mode::CoverArtZoom && app.art.now_playing_art.cached().is_some() {
        Clear.render(area, frame.buffer_mut());

        // Use the full area minus 1 row for hint.
        let art_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        // Use cached render to avoid Lanczos3 resize every frame.
        app.art
            .now_playing_art
            .render_to_centered(art_area, frame.buffer_mut());

        // Hint at bottom.
        let hint_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        );
        let hint = Line::from(vec![
            Span::styled(" [esc]", app.theme.hint_key),
            Span::styled(" close  ", app.theme.hint_desc),
            Span::styled("[z]", app.theme.hint_key),
            Span::styled(" close", app.theme.hint_desc),
        ]);
        frame.render_widget(Paragraph::new(hint), hint_area);
    }

    // Drop/paste import progress bar.
    if let Some(ref progress) = app.drop_progress {
        let done = progress.0.load(std::sync::atomic::Ordering::Relaxed);
        let total = progress.1.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(pct) = (done * 100).checked_div(total).map(|p| p.min(100)) {
            let label = format!(" scanning {}/{} ({}%) ", done, total, pct);
            let w = (label.len() as u16 + 2).max(30).min(area.width);
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + area.height / 2;
            let popup = Rect::new(x, y, w, 1);
            Clear.render(popup, frame.buffer_mut());

            // Progress bar: filled portion. `total > 0` is guaranteed by the
            // checked_div above, so this division cannot trap.
            let bar_width = w.saturating_sub(2) as usize;
            let filled = (bar_width * done).checked_div(total).unwrap_or(0);
            let bar: String =
                "\u{2588}".repeat(filled) + &"\u{2591}".repeat(bar_width.saturating_sub(filled));
            let line = Line::from(vec![
                Span::styled(" ", app.theme.hint_desc),
                Span::styled(bar, app.theme.spinner),
                Span::styled(" ", app.theme.hint_desc),
            ]);
            frame.render_widget(Paragraph::new(line), popup);

            // Label below.
            let label_area = Rect::new(x, y.saturating_sub(1), w, 1);
            Clear.render(label_area, frame.buffer_mut());
            let label_line = Line::from(Span::styled(label, app.theme.spinner));
            frame.render_widget(Paragraph::new(label_line), label_area);
        }
    }

    // FPS counter overlay (top-right corner).
    if app.show_fps {
        render_fps(frame, app, area);
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

/// FPS + beat-energy counter, right-aligned on the top row.
fn render_fps(frame: &mut Frame, app: &App, area: Rect) {
    let beat = app.visualizer.beat_energy;
    let beat_tag = if beat > 0.3 { " BEAT" } else { "" };
    let fps_text = format!(" {}fps b:{:.2}{} ", app.display_fps, beat, beat_tag);
    let w = fps_text.chars().count() as u16;
    if area.width < w {
        return;
    }
    let fps_area = Rect::new(area.x + area.width - w, area.y, w, 1);
    let fps_line = Line::from(Span::styled(fps_text, app.theme.hint_desc));
    frame.render_widget(Paragraph::new(fps_line), fps_area);
}

fn render_queue(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    // Clamp cursor before borrowing visible queue.
    let vq_len = app.queue.vq_cache.entries.len();
    if vq_len > 0 && app.queue.cursor >= vq_len {
        app.queue.cursor = vq_len - 1;
    }

    let visible = app.visible_queue();
    let drop_indicator = app.drop_indicator_index();
    let selected_indices = app.selected_indices();
    let queue_view = QueueView::new(
        &visible,
        &app.mode,
        app.queue.cursor,
        app.queue.scroll_offset,
        &app.theme,
        &selected_indices,
        app.spinner_tick,
    )
    .with_drop_indicator(drop_indicator)
    .with_hover(&app.hover.zone)
    .with_favourites(&app.favourites);
    frame.render_widget(queue_view, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::player::state::{
        QueueEntry, QueueEntryStatus, QueueItemId, SharedPlayerState, TrackInfo,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn entry(n: usize) -> QueueEntry {
        QueueEntry {
            id: QueueItemId::new(),
            db_id: None,
            path: PathBuf::from(format!("/music/{}.flac", n)),
            title: format!("track {}", n),
            artist: "artist".into(),
            album_artist: "artist".into(),
            album: "album".into(),
            year: Some("2024".into()),
            codec: Some("FLAC".into()),
            track_number: Some(n as i64 + 1),
            disc: None,
            duration_ms: Some(200_000),
            status: QueueEntryStatus::Queued,
            download_progress: None,
        }
    }

    /// An `App` with a queue and a track playing, detached from any real player.
    fn app_with_queue(len: usize) -> App {
        // First, before anything reads configuration: spawning the download
        // queue resolves the remote server, and a render test has no business
        // reaching for anyone's credentials.
        koan_core::config::isolate_config_for_tests();

        let state = SharedPlayerState::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let log_buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let download_queue = koan_core::remote::queue::DownloadQueue::spawn(
            tx.clone(),
            state.clone(),
            log_buffer.clone(),
        );

        let mut entries: Vec<QueueEntry> = (0..len).map(entry).collect();
        if let Some(first) = entries.first_mut() {
            first.status = QueueEntryStatus::Playing;
        }
        if let Some(first) = entries.first() {
            state.set_track_info(Some(TrackInfo {
                id: first.id,
                path: first.path.clone(),
                codec: "FLAC".into(),
                sample_rate: 44100,
                bit_depth: Some(16),
                bitrate_kbps: None,
                channels: 2,
                duration_ms: 200_000,
            }));
        }

        let mut app = App::new(
            state,
            koan_core::audio::viz::VizSnapshot::new(),
            tx,
            log_buffer,
            PathBuf::from("/nonexistent/koan-test.db"),
            60,
            download_queue,
        );
        app.queue.vq_cache.entries = entries;
        app.queue.vq_version = app.state.playlist_version();
        // Pinned so the sweep does not inherit the developer's own config.
        app.art_size = 24;
        app.show_fps = false;
        app.viz_fullscreen = false;
        app.viz_config.enabled = true;
        app
    }

    fn draw(app: &mut App, w: u16, h: u16) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
    }

    /// Transport geometry used to be derived from the requested height rather
    /// than the height the layout solver granted, writing below the buffer.
    #[test]
    fn renders_at_any_terminal_size() {
        let mut app = app_with_queue(40);
        for art_size in [4u16, 24, 56, 80] {
            app.art_size = art_size;
            for w in [1u16, 2, 3, 19, 20, 26, 40, 80, 200] {
                for h in [1u16, 2, 3, 4, 5, 8, 9, 10, 24, 60] {
                    draw(&mut app, w, h);
                }
            }
        }
    }

    /// Dragging the transport divider large on a big screen used to poison
    /// config.toml and panic on the first frame of every smaller terminal.
    #[test]
    fn oversized_art_renders_on_a_short_terminal() {
        let mut app = app_with_queue(40);
        app.art_size = 80;
        draw(&mut app, 80, 24);
        draw(&mut app, 26, 8);
        draw(&mut app, 200, 6);
    }

    /// The counter is 14 cells wide (19 with the beat tag) — narrower areas
    /// underflowed the right-aligned x.
    #[test]
    fn fps_counter_renders_at_narrow_widths() {
        let mut app = app_with_queue(4);
        app.show_fps = true;
        for beat in [0.0f32, 0.9] {
            app.visualizer.beat_energy = beat;
            for w in 1u16..=25 {
                let mut terminal = Terminal::new(TestBackend::new(w, 3)).unwrap();
                terminal
                    .draw(|f| {
                        let area = f.area();
                        render_fps(f, &app, area)
                    })
                    .unwrap();
            }
        }
        // And through both render paths that reach it.
        for fullscreen in [false, true] {
            app.viz_fullscreen = fullscreen;
            for w in [20u16, 26, 80] {
                draw(&mut app, w, 24);
            }
        }
    }

    /// Indices shift when a track is removed; the modal keyed off one and
    /// silently showed a different track, or rendered nothing while swallowing
    /// every keystroke.
    #[test]
    fn track_info_modal_closes_when_its_track_is_removed() {
        let mut app = app_with_queue(5);
        let id = app.queue.vq_cache.entries[3].id;
        app.mode = Mode::TrackInfo(id);
        draw(&mut app, 80, 24);

        app.queue.vq_cache.entries.retain(|e| e.id != id);
        app.close_stale_track_info();

        assert_eq!(app.mode, Mode::Normal);
        // Still renders, and keys are no longer swallowed by a dead modal.
        draw(&mut app, 80, 24);
    }

    #[test]
    fn track_info_modal_follows_its_track_across_a_removal() {
        let mut app = app_with_queue(5);
        let id = app.queue.vq_cache.entries[3].id;
        app.mode = Mode::TrackInfo(id);

        app.queue.vq_cache.entries.remove(0);
        app.close_stale_track_info();

        assert_eq!(app.mode, Mode::TrackInfo(id));
    }
}

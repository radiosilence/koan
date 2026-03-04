use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use super::app::{App, LibraryFocus, Mode};
use super::context_menu::{ContextMenuOverlay, context_menu_rect_at};
use super::keys::HintBar;
use super::library::LibraryView;
use super::lyrics::LyricsPanel;
use super::organize::{OrganizeOverlay, organize_popup_rect};
use super::picker::{PickerOverlay, picker_popup_rect};
use super::queue::QueueView;
use super::track_info::TrackInfoOverlay;
use super::transport::TransportBar;
use super::visualizer::SpectrumWidget;

/// Height of the transport bar without album art.
const TRANSPORT_HEIGHT_DEFAULT: u16 = 3;
/// Desired art width in columns. Art is square-ish so height ~ width/2 cells.
const ART_WIDTH: u16 = 24;

pub fn render(frame: &mut Frame, app: &mut App) {
    // Refresh the visible queue cache once per frame so all reads
    // within this render cycle see a consistent snapshot.
    app.refresh_visible_queue();

    let area = frame.area();

    let has_art = app.art.now_playing_art.cached().is_some();
    // Derive height from actual image aspect ratio at desired width.
    // Always reserve art-sized space once we've had art, so the UI
    // doesn't jump when switching between tracks with/without art.
    let art_h = app.art.now_playing_art.cell_height_for_width(ART_WIDTH);
    if art_h > 0 {
        app.art.last_art_height = art_h;
    }
    let transport_height = if app.art.last_art_height > 0 {
        app.art.last_art_height.max(TRANSPORT_HEIGHT_DEFAULT)
    } else {
        TRANSPORT_HEIGHT_DEFAULT
    };

    // Main layout: transport | content (flex) | hints (1)
    let chunks = Layout::vertical([
        Constraint::Length(transport_height),
        Constraint::Min(3),    // content area
        Constraint::Length(1), // hint bar
    ])
    .split(area);

    // Store areas for mouse interaction.
    app.layout.transport_area = chunks[0];

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

    // Determine art area and text area based on art presence.
    let reserve_art_space = has_art || app.art.last_art_height > 0;
    let text_area = if reserve_art_space {
        // Bottom-align the transport text (3 lines) within the full height.
        let text_height = 3u16.min(transport_height);
        let text_y = chunks[0].y + transport_height - text_height;
        Rect::new(
            chunks[0].x + ART_WIDTH + 1,
            text_y,
            chunks[0].width.saturating_sub(ART_WIDTH + 1),
            text_height,
        )
    } else {
        chunks[0]
    };

    if has_art {
        let art_area = Rect::new(chunks[0].x, chunks[0].y, ART_WIDTH, transport_height);
        app.layout.now_playing_art_area = art_area;
        app.art
            .now_playing_art
            .render_to(art_area, frame.buffer_mut());
    } else {
        app.layout.now_playing_art_area = Rect::default();
    }
    app.layout.transport_text_area = text_area;

    // Seek bar metrics + transport widget — rendered once.
    let pos_ms = app.state.position_ms();
    let dur_ms = track_info.as_ref().map_or(0, |t| t.duration_ms);
    let (bs, bw) = TransportBar::bar_metrics(text_area, pos_ms, dur_ms);
    app.layout.seek_bar_start = bs;
    app.layout.seek_bar_width = bw;

    let transport = TransportBar::new(
        track_info.as_ref(),
        playing_entry.as_ref(),
        app.state.playback_state(),
        pos_ms,
        &app.theme,
    )
    .with_ticker_offset(app.ticker_offset);
    frame.render_widget(transport, text_area);

    // Spectrum visualizer — renders in the space above the transport text.
    if transport_height > TRANSPORT_HEIGHT_DEFAULT {
        let spectrum_height = transport_height - TRANSPORT_HEIGHT_DEFAULT;
        let spectrum_area = if reserve_art_space {
            Rect::new(
                chunks[0].x + ART_WIDTH + 1,
                chunks[0].y,
                chunks[0].width.saturating_sub(ART_WIDTH + 1),
                spectrum_height,
            )
        } else {
            Rect::new(chunks[0].x, chunks[0].y, chunks[0].width, spectrum_height)
        };
        let spectrum = SpectrumWidget::new(&app.visualizer, &app.theme);
        frame.render_widget(spectrum, spectrum_area);
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
        if let Some(ref mut lib) = app.library {
            let visible_height = panes[0].height.saturating_sub(2) as usize;
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

    // Key hints.
    let hint_bar = HintBar::new(&app.mode, &app.theme);
    frame.render_widget(hint_bar, chunks[2]);

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

    // Track info overlay.
    if let Mode::TrackInfo(idx) = app.mode
        && let Some(entry) = app.queue.vq_cache.entries.get(idx).cloned()
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
        if total > 0 {
            let pct = (done * 100 / total).min(100);
            let label = format!(" scanning {}/{} ({}%) ", done, total, pct);
            let w = (label.len() as u16 + 2).max(30).min(area.width);
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + area.height / 2;
            let popup = Rect::new(x, y, w, 1);
            Clear.render(popup, frame.buffer_mut());

            // Progress bar: filled portion.
            let bar_width = w.saturating_sub(2) as usize;
            let filled = bar_width * done / total;
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
    if app.show_fps && area.width >= 8 {
        let fps_text = format!(" {}fps ", app.display_fps);
        let w = fps_text.len() as u16;
        let fps_area = Rect::new(area.x + area.width - w, area.y, w, 1);
        let fps_line = Line::from(Span::styled(fps_text, app.theme.hint_desc));
        frame.render_widget(Paragraph::new(fps_line), fps_area);
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

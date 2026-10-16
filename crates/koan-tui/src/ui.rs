use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use super::app::{App, Mode, NavTarget};
use super::context_menu::{ContextMenuOverlay, context_menu_rect_at};
use super::device_selector::DeviceSelectorOverlay;
use super::help_modal::HelpModalOverlay;
use super::keys::HintBar;
use super::library::LibraryView;
use super::lyrics::LyricsPanel;
use super::nav::Page;
use super::organize::{OrganizeOverlay, organize_popup_rect};
use super::picker::{PickerOverlay, picker_popup_rect};
use super::queue::QueueView;
use super::theme::Theme;
use super::track_info::TrackInfoOverlay;
use super::transport::{MINI_BARS, TransportBar};
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

    app.layout.queue_area = Rect::default();
    app.layout.library_area = Rect::default();
    app.layout.browse_area = Rect::default();
    app.layout.now_playing_art_area = Rect::default();
    app.layout.nav_hits.clear();

    let has_art = app.art.now_playing_art.cached().is_some();
    // Now playing carries the cover and the spectrum itself, so the transport
    // is just its three lines there; on a narrow terminal the cover takes no
    // more than a third of the width.
    let art_cells = if app.page() == Page::NowPlaying {
        0
    } else if area.width < 80 {
        app.art_size.min((area.width / 3).max(4))
    } else {
        app.art_size
    };
    let art_height = art_cells / 2; // square via halfblock rendering
    let transport_height = art_height.max(TRANSPORT_HEIGHT_DEFAULT);

    // Main layout: stage (flex) | hairline | transport | hints (1)
    let chunks = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(transport_height),
        Constraint::Length(1),
    ])
    .split(area);

    let stage_all = chunks[0];
    let rule_row = chunks[1];
    for x in rule_row.x..rule_row.x + rule_row.width {
        frame.buffer_mut()[(x, rule_row.y)]
            .set_char('\u{2500}')
            .set_style(app.theme.rule);
    }
    app.layout.transport_rule_y = Some(rule_row.y);

    // The solver shrinks the transport when the terminal is short — every
    // rect below is derived from what it actually granted, not the request.
    let transport_rect = chunks[2];
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
    let art_width = art_cells.min(transport_rect.width.saturating_sub(2));
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

    if has_art && art_width > 0 {
        app.layout.now_playing_art_area = art_area;
        app.art
            .now_playing_art
            .render_to(&mut app.art.gfx, art_area, frame.buffer_mut());
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

    let seekable_ms = app.state.seek_ceiling_ms();
    // The little spectrum stands in for the big one: only where the transport
    // has none above it and the page is not drawing its own.
    let levels = (app.viz_config.enabled
        && transport_rect.height <= TRANSPORT_HEIGHT_DEFAULT
        && app.page() != Page::NowPlaying)
        .then(|| mini_levels(&app.visualizer.spectrum));
    let transport = TransportBar::new(
        track_info.as_ref(),
        playing_entry.as_ref(),
        app.state.playback_state(),
        pos_ms,
        &app.theme,
    )
    .with_ticker_offset(app.ticker_offset)
    .with_seekable_ms(seekable_ms)
    .with_output_rate(app.state.output_sample_rate())
    .with_dsp(app.state.dsp())
    .with_sleep(app.state.sleep(), app.state.sleep_fading())
    .with_levels(levels);
    frame.render_widget(transport, text_area);

    // Visualizer — renders in the space above the transport text.

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

    // The stage: a sidebar or a tab bar, and the page beside or under it.
    let stage = render_nav(frame, app, stage_all);
    app.layout.stage_area = stage;
    match app.page() {
        Page::Queue => {
            if app.lyrics_panel {
                let panes = Layout::horizontal([
                    Constraint::Percentage(60), // queue
                    Constraint::Percentage(40), // lyrics
                ])
                .split(stage);

                app.layout.queue_area = panes[0];
                render_queue(frame, app, panes[0]);

                let pos_ms = app.state.position_ms();
                let lyrics_panel =
                    LyricsPanel::new(&app.lyrics, pos_ms, &app.theme, app.spinner_tick);
                frame.render_widget(lyrics_panel, panes[1]);
            } else {
                app.layout.queue_area = stage;
                render_queue(frame, app, stage);
            }
        }
        Page::NowPlaying => {
            heading(frame.buffer_mut(), stage, &app.theme, "now playing", "");
            super::now_playing::render(frame.buffer_mut(), app, stage);
        }
        Page::Playlists => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Playlists(h) => Some(h),
                _ => None,
            };
            let App {
                playlists_page,
                theme,
                ..
            } = &mut *app;
            playlists_page.render(stage, frame.buffer_mut(), theme, hover);
        }
        Page::Playlist(_) => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Playlist(h) => Some(h),
                _ => None,
            };
            let App {
                playlist_page,
                theme,
                ..
            } = &mut *app;
            match playlist_page {
                Some(p) => p.render(stage, frame.buffer_mut(), theme, hover),
                None => {
                    heading(frame.buffer_mut(), stage, theme, "playlist", "");
                    frame.buffer_mut().set_string(
                        stage.x + 1,
                        stage.y + 1,
                        "this playlist is gone",
                        theme.hint_desc,
                    );
                }
            }
        }
        Page::History => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::History(h) => Some(h),
                _ => None,
            };
            let App {
                history_page,
                theme,
                ..
            } = &mut *app;
            history_page.render(stage, frame.buffer_mut(), theme, hover);
        }
        Page::Settings => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Settings(h) => Some(h),
                _ => None,
            };
            let App {
                settings_page,
                theme,
                ..
            } = &mut *app;
            settings_page.render(stage, frame.buffer_mut(), theme, hover);
        }
        Page::Album(_) => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Album(h) => Some(h),
                _ => None,
            };
            let App {
                album_page,
                thumbs,
                art,
                theme,
                favourites,
                ..
            } = &mut *app;
            match album_page {
                Some(p) => p.render(
                    stage,
                    frame.buffer_mut(),
                    theme,
                    &mut art.gfx,
                    thumbs,
                    favourites,
                    hover,
                ),
                None => {
                    heading(frame.buffer_mut(), stage, theme, "album", "");
                    frame.buffer_mut().set_string(
                        stage.x + 1,
                        stage.y + 1,
                        "this record is not in the library",
                        theme.hint_desc,
                    );
                }
            }
        }
        Page::Albums | Page::Favourites | Page::Recent | Page::Artist(_) => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Browse(h) => Some(h),
                _ => None,
            };
            let App {
                albums_page,
                favourites_page,
                recent_page,
                artist_page,
                thumbs,
                art,
                theme,
                favourites,
                nav,
                ..
            } = &mut *app;
            let page = match nav.current() {
                Page::Albums => albums_page,
                Page::Favourites => favourites_page,
                Page::Recent => recent_page,
                _ => artist_page,
            };
            page.render(
                stage,
                frame.buffer_mut(),
                theme,
                &mut art.gfx,
                thumbs,
                favourites,
                hover,
            );
        }
        Page::Artists => {
            app.layout.library_area = stage;
            let visible_height = app.library_content_height();
            if let Some(ref mut lib) = app.library {
                lib.update_scroll(visible_height);
                let hover_idx = match &app.hover.zone {
                    super::app::HoverZone::LibraryItem(idx) => Some(*idx),
                    _ => None,
                };
                let lib_view = LibraryView::new(lib, &app.theme, true).with_hover(hover_idx);
                frame.render_widget(lib_view, stage);
            }
        }
    }

    // Key hints / status message.
    app.layout.hint_hits.clear();
    if let Some((ref msg, _)) = app.status_message {
        let style = app.theme.hint_key;
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(msg.as_str(), style),
            ratatui::text::Span::styled("  [Esc] dismiss", app.theme.hint_desc),
        ]);
        frame.render_widget(ratatui::widgets::Paragraph::new(line), chunks[3]);
    } else {
        app.layout.hint_hits = super::keys::hits(&app.mode, app.page(), chunks[3]);
        let hint_bar = HintBar::new(&app.mode, app.page(), &app.theme);
        frame.render_widget(hint_bar, chunks[3]);
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
        app.layout.selector_area =
            super::device_selector::device_selector_rect(area, selector.devices.len());
        let overlay = DeviceSelectorOverlay::new(selector, &app.theme);
        frame.render_widget(overlay, area);
    }

    // EQ chooser overlay.
    if app.mode == Mode::DspSelector
        && let Some(ref selector) = app.dsp_selector
    {
        app.layout.selector_area =
            super::dsp_selector::dsp_selector_rect(area, selector.entries.len());
        let overlay = super::dsp_selector::DspSelectorOverlay::new(selector, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Playlist name prompt and chooser.
    if app.mode == Mode::NamePrompt
        && let Some(ref prompt) = app.name_prompt
    {
        app.layout.modal_area = super::playlist_modals::prompt_rect(area);
        let overlay = super::playlist_modals::NamePromptOverlay::new(prompt, &app.theme);
        frame.render_widget(overlay, area);
    }
    if app.mode == Mode::PlaylistChooser
        && let Some(ref chooser) = app.chooser
    {
        app.layout.modal_area = super::playlist_modals::chooser_rect(area, chooser.entries.len());
        let overlay = super::playlist_modals::ChooserOverlay::new(chooser, &app.theme);
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

    // Track details overlay, for a track picked from any list.
    if app.mode == Mode::TrackDetails
        && let Some(track) = app.track_details.as_deref()
    {
        app.layout.track_info_area = super::track_details::popup_rect(area);
        let overlay = super::track_details::TrackDetailsOverlay::new(track, &app.theme);
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
            .render_to_centered(&mut app.art.gfx, art_area, frame.buffer_mut());

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

/// A page's heading: its title, a note, and the hairline to the edge.
pub fn heading(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    theme: &Theme,
    title: &str,
    note: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let end = area.x + area.width;
    let mut x = area.x;
    for (text, style) in [(title, theme.heading), (note, theme.hint_desc)] {
        let w = (text.chars().count() as u16).min(end - x);
        buf.set_stringn(x, area.y, text, w as usize, style);
        x += w;
    }
    for cx in x..end {
        buf[(cx, area.y)].set_char('\u{2500}').set_style(theme.rule);
    }
}

/// The spectrum as the dozen bars the transport has room for.
fn mini_levels(spectrum: &[f32]) -> [f32; MINI_BARS] {
    let per = (spectrum.len() / MINI_BARS).max(1);
    std::array::from_fn(|i| {
        let group = &spectrum[(i * per).min(spectrum.len())..((i + 1) * per).min(spectrum.len())];
        if group.is_empty() {
            0.0
        } else {
            group.iter().sum::<f32>() / group.len() as f32
        }
    })
}

/// How wide the screen is, as the layout sees it: a sidebar beside the page
/// when there is room for one, a tab bar over it when there is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Wide,
    Regular,
    Compact,
}

const SIDEBAR_WIDTH: u16 = 18;

impl Width {
    pub fn of(area: Rect) -> Self {
        if area.width >= 100 && area.height >= 9 {
            Width::Wide
        } else if area.width >= 56 {
            Width::Regular
        } else {
            Width::Compact
        }
    }
}

/// Draws the sidebar or the tab bar into `area`, registers what each part
/// opens, and returns the rest of `area` for the page.
fn render_nav(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let width = Width::of(area);
    let theme = &app.theme;
    let current = app.nav.current().tab();
    let can_back = app.nav.can_back();
    let can_forward = app.nav.can_forward();
    let buf = frame.buffer_mut();
    let mut hits = Vec::new();

    if width == Width::Wide {
        let side = Rect::new(area.x, area.y, SIDEBAR_WIDTH, area.height);
        for y in side.y..side.y + side.height {
            buf[(side.x + SIDEBAR_WIDTH - 1, y)]
                .set_char('\u{2502}')
                .set_style(theme.rule);
        }
        buf.set_stringn(side.x + 1, side.y, "koan", 8, theme.heading);
        for (i, page) in Page::ALL.into_iter().enumerate() {
            let y = side.y + 2 + i as u16;
            if y >= side.y + side.height {
                break;
            }
            let active = page == current;
            let hovered = app.hover.zone == super::app::HoverZone::Nav(NavTarget::Page(page));
            let label_style = if active {
                theme.heading
            } else if hovered {
                theme.track_normal
            } else {
                theme.hint_desc
            };
            if active {
                buf[(side.x, y)].set_char('\u{258c}').set_style(theme.bar);
            }
            buf.set_stringn(side.x + 1, y, page.title(), 13, label_style);
            buf.set_string(
                side.x + SIDEBAR_WIDTH - 3,
                y,
                page.key().to_string(),
                theme.hint_desc,
            );
            hits.push((
                Rect::new(side.x, y, SIDEBAR_WIDTH - 1, 1),
                NavTarget::Page(page),
            ));
        }
        let y = side.y + side.height - 1;
        let back_style = if can_back {
            theme.track_normal
        } else {
            theme.rule
        };
        let fwd_style = if can_forward {
            theme.track_normal
        } else {
            theme.rule
        };
        buf.set_string(side.x + 1, y, "\u{2039} back", back_style);
        buf.set_string(side.x + 9, y, "fwd \u{203a}", fwd_style);
        hits.push((Rect::new(side.x, y, 8, 1), NavTarget::Back));
        hits.push((Rect::new(side.x + 8, y, 8, 1), NavTarget::Forward));
        app.layout.nav_hits = hits;
        return Rect::new(
            area.x + SIDEBAR_WIDTH,
            area.y,
            area.width - SIDEBAR_WIDTH,
            area.height,
        );
    }

    // Tab bar: back and forward, then a tab a page. Titles when they fit,
    // the short names when they do not, and numbers when not even those.
    let bar = Rect::new(area.x, area.y, area.width, 1.min(area.height));
    let usable = bar.width.saturating_sub(5) as usize;
    let cost = |label: fn(Page) -> &'static str| {
        Page::ALL
            .iter()
            .map(|p| label(*p).chars().count() + 2)
            .sum::<usize>()
    };
    let label: fn(Page) -> &'static str = if cost(Page::title) <= usable {
        Page::title
    } else if cost(Page::short) <= usable {
        Page::short
    } else {
        |_| ""
    };
    let numbers_only = label(Page::Queue).is_empty();
    let back_style = if can_back {
        theme.track_normal
    } else {
        theme.rule
    };
    let fwd_style = if can_forward {
        theme.track_normal
    } else {
        theme.rule
    };
    buf.set_string(bar.x + 1, bar.y, "\u{2039}", back_style);
    buf.set_string(bar.x + 3, bar.y, "\u{203a}", fwd_style);
    hits.push((Rect::new(bar.x, bar.y, 3, 1), NavTarget::Back));
    hits.push((Rect::new(bar.x + 3, bar.y, 2, 1), NavTarget::Forward));
    let mut x = bar.x + 5;
    let end = bar.x + bar.width;
    for page in Page::ALL {
        let text = if numbers_only {
            if page == current {
                format!(" {} {} ", page.key(), page.title())
            } else {
                format!(" {} ", page.key())
            }
        } else {
            format!(" {} ", label(page))
        };
        let w = (text.chars().count() as u16).min(end.saturating_sub(x));
        if w == 0 {
            break;
        }
        let active = page == current;
        let hovered = app.hover.zone == super::app::HoverZone::Nav(NavTarget::Page(page));
        let style = if active {
            theme.track_cursor
        } else if hovered {
            theme.track_normal
        } else {
            theme.hint_desc
        };
        buf.set_stringn(x, bar.y, &text, w as usize, style);
        hits.push((Rect::new(x, bar.y, w, 1), NavTarget::Page(page)));
        x += w;
    }
    app.layout.nav_hits = hits;
    if area.height <= 1 {
        return Rect::new(area.x, area.y + area.height, area.width, 0);
    }
    Rect::new(area.x, area.y + 1, area.width, area.height - 1)
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
    .with_pointer(Some((app.hover.column, app.hover.row)))
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
            playlist_entry_id: None,
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
            error: None,
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
            60,
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

    /// Transport geometry must come from the height the layout solver granted,
    /// not the height requested, or it writes below the buffer.
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

    /// An art size dragged large on a big screen must not panic the first
    /// frame of a smaller terminal.
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

    #[test]
    fn every_page_renders_at_any_size() {
        let mut app = app_with_queue(40);
        for page in Page::ALL {
            app.go_page(page);
            for w in [1u16, 20, 30, 55, 56, 80, 99, 100, 160] {
                for h in [1u16, 4, 5, 8, 9, 12, 24, 60] {
                    draw(&mut app, w, h);
                }
            }
        }
    }

    #[test]
    fn the_layout_follows_the_width() {
        assert_eq!(Width::of(Rect::new(0, 0, 160, 40)), Width::Wide);
        assert_eq!(Width::of(Rect::new(0, 0, 100, 40)), Width::Wide);
        assert_eq!(Width::of(Rect::new(0, 0, 99, 40)), Width::Regular);
        assert_eq!(Width::of(Rect::new(0, 0, 56, 40)), Width::Regular);
        assert_eq!(Width::of(Rect::new(0, 0, 55, 40)), Width::Compact);
        // Too short for a sidebar's six pages and its foot.
        assert_eq!(Width::of(Rect::new(0, 0, 160, 8)), Width::Regular);
    }

    #[test]
    fn the_sidebar_and_the_tab_bar_open_pages_when_clicked() {
        use crate::app::HoverZone;
        for width in [160u16, 80, 40] {
            let mut app = app_with_queue(5);
            draw(&mut app, width, 40);
            let pages: Vec<_> = app
                .layout
                .nav_hits
                .iter()
                .filter_map(|(r, t)| match t {
                    NavTarget::Page(p) => Some((*r, *p)),
                    _ => None,
                })
                .collect();
            assert!(!pages.is_empty(), "width {width}");
            let (rect, page) = *pages
                .iter()
                .find(|(_, p)| *p == Page::Albums)
                .unwrap_or(&pages[0]);
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            assert_eq!(app.page(), page, "width {width}");
            assert!(app.nav.can_back());
            assert_ne!(app.hover.zone, HoverZone::Nav(NavTarget::Back));
        }
    }

    #[test]
    fn keys_move_between_pages_and_back() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = app_with_queue(5);
        app.handle_key(KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(app.page(), Page::Albums);
        app.handle_key(KeyEvent::from(KeyCode::Char('5')));
        assert_eq!(app.page(), Page::Favourites);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.page(), Page::Albums);
        app.handle_key(KeyEvent::from(KeyCode::Char(']')));
        assert_eq!(app.page(), Page::Favourites);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.page(), Page::Playlists);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.page(), Page::Recent);
        app.handle_key(KeyEvent::from(KeyCode::Char('1')));
        assert_eq!(app.page(), Page::Queue);
    }

    #[test]
    fn the_playlist_chooser_leads_to_a_name_prompt_and_back_out() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = app_with_queue(5);
        app.open_chooser(vec![1, 2]);
        assert_eq!(app.mode, Mode::PlaylistChooser);
        draw(&mut app, 100, 30);
        // The first row is always "new playlist", whatever else is held.
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::NamePrompt);
        for c in "road".chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        draw(&mut app, 100, 30);
        assert_eq!(
            app.name_prompt.as_ref().map(|p| p.text.as_str()),
            Some("road")
        );
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.name_prompt.is_none());
    }

    #[test]
    fn a_click_outside_a_playlist_modal_cancels_it() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with_queue(5);
        app.open_chooser(vec![1]);
        draw(&mut app, 100, 30);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.chooser.is_none());
    }

    #[test]
    fn nothing_to_add_is_said_rather_than_opening_an_empty_chooser() {
        let mut app = app_with_queue(5);
        app.open_chooser(Vec::new());
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status_message.is_some());
    }

    #[test]
    fn typing_in_a_filter_does_not_change_page() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = app_with_queue(5);
        app.handle_key(KeyEvent::from(KeyCode::Char('3')));
        app.handle_key(KeyEvent::from(KeyCode::Char('/')));
        app.handle_key(KeyEvent::from(KeyCode::Char('5')));
        assert_eq!(app.page(), Page::Albums);
        assert_eq!(app.albums_page.filter, "5");
    }

    #[test]
    fn the_hairline_above_the_transport_resizes_it() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with_queue(5);
        draw(&mut app, 100, 40);
        let y = app.layout.transport_rule_y.expect("a hairline");
        let before = app.art_size;
        let ev = |kind, row| MouseEvent {
            kind,
            column: 10,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(ev(MouseEventKind::Down(MouseButton::Left), y));
        app.handle_mouse(ev(MouseEventKind::Drag(MouseButton::Left), y - 3));
        assert_eq!(app.art_size, before + 6);
    }

    #[test]
    fn the_cover_gives_the_interface_its_accent() {
        let mut app = app_with_queue(2);
        assert_eq!(app.accent.target(), crate::theme::MINT);
        let red = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            32,
            32,
            image::Rgb([200, 20, 30]),
        ));
        let rgb = crate::accent::from_image(&red).expect("a red");
        app.accent.set(rgb, std::time::Instant::now());
        assert_eq!(app.accent.target(), rgb);
    }

    #[test]
    fn a_hint_is_a_button() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with_queue(5);
        draw(&mut app, 120, 40);
        // `[1-6] pages` is not one key, but `[e] edit` is.
        let (rect, key) = app
            .layout
            .hint_hits
            .iter()
            .find(|(_, k)| k.code == crossterm::event::KeyCode::Char('e'))
            .cloned()
            .expect("an edit hint");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(key.code, crossterm::event::KeyCode::Char('e'));
        assert_eq!(app.mode, Mode::QueueEdit);
    }

    #[test]
    fn the_queue_headings_name_links_and_a_row_has_a_menu_button() {
        use crate::queue::{HeaderLink, QueueView, menu_button_x};
        let app = app_with_queue(3);
        let entries = &app.queue.vq_cache.entries;
        let area = Rect::new(0, 0, 80, 20);
        // Row 1 is the heading: " artist \u{2014} (2024) album [FLAC]".
        assert_eq!(
            QueueView::header_link_at(entries, area, 0, 2, 1),
            Some((0, HeaderLink::Artist))
        );
        let album_col = 1 + "artist".len() as u16 + 3 + "(2024) ".len() as u16;
        assert_eq!(
            QueueView::header_link_at(entries, area, 0, album_col + 1, 1),
            Some((0, HeaderLink::Album))
        );
        // Off the text, and on a track row, there is no link.
        assert_eq!(QueueView::header_link_at(entries, area, 0, 70, 1), None);
        assert_eq!(QueueView::header_link_at(entries, area, 0, 2, 2), None);
        assert_eq!(menu_button_x(area), 77);
    }

    #[test]
    fn the_eq_chooser_opens_and_closes_by_key_and_by_click() {
        use crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };
        let mut app = app_with_queue(3);
        app.handle_key(KeyEvent::from(KeyCode::Char('E')));
        assert_eq!(app.mode, Mode::DspSelector);
        draw(&mut app, 100, 30);
        assert!(app.layout.selector_area.width > 0);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        app.handle_key(KeyEvent::from(KeyCode::Char('E')));
        draw(&mut app, 100, 30);
        // A click outside the popup closes it.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_record_and_an_artist_page_render_without_the_library() {
        let mut app = app_with_queue(3);
        for page in [Page::Album(999), Page::Artist(999)] {
            app.go_page(page);
            for (w, h) in [(120u16, 40u16), (60, 20), (30, 10), (5, 3)] {
                draw(&mut app, w, h);
            }
        }
        // Back from either lands on the page before it, and the tab it sits
        // under is the one lit.
        assert_eq!(app.page().tab(), Page::Artists);
        app.go_back();
        assert_eq!(app.page(), Page::Album(999));
    }
}

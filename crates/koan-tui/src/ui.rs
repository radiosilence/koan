use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use super::app::{App, Mode, NavTarget};
use super::context_menu::{ContextMenuOverlay, context_menu_rect_at};
use super::help_modal::HelpModalOverlay;
use super::keys::HintBar;
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
    app.links.begin((app.hover.column, app.hover.row));

    // Below this the transport/queue geometry has no room to lay out.
    if area.height < 5 || area.width < 20 {
        frame.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }

    // Fullscreen visualizer — take the entire frame, skip everything else.
    if app.viz_fullscreen && app.viz_config.enabled {
        // Nothing else is on screen to be clicked.
        app.layout = Default::default();
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
    app.layout.browse_area = Rect::default();
    app.layout.now_playing_art_area = Rect::default();
    app.layout.nav_hits.clear();
    app.layout.eq_hits.clear();
    app.layout.toolbar_hits.clear();

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
    // On the hairline: a readout, nowhere near a page's controls.
    if app.show_fps {
        render_fps(frame, app, rule_row);
    }

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
    let eq_hit = std::cell::Cell::new(Rect::default());
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
    .with_levels(levels)
    .with_eq_hit(&eq_hit)
    .with_controlled(app.view.controlled.as_ref());
    frame.render_widget(transport, text_area);
    if eq_hit.get().width > 0 {
        app.layout.eq_hits.push(eq_hit.get());
    }

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
    // The toolbar row and the page under it, inside the gutters.
    let content = super::shell::content(stage);
    let bar_row = Rect::new(content.x, content.y, content.width, content.height.min(1));
    let toolbar = app.toolbar();
    app.layout.toolbar_hits = super::shell::draw_toolbar(
        frame.buffer_mut(),
        bar_row,
        &app.theme,
        &toolbar,
        app.nav.can_back(),
        app.nav.can_forward(),
        (app.hover.column, app.hover.row),
    );
    let stage = Rect::new(
        content.x,
        content.y + bar_row.height,
        content.width,
        content.height - bar_row.height,
    );
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
                queue,
                favourites,
                spinner_tick,
                links,
                ..
            } = &mut *app;
            let live = super::playlists_page::Live {
                queue: &queue.vq_cache.entries,
                links: Some(links),
                favourites: Some(favourites),
                offline: koan_core::remote::offline::active(),
                spinner: *spinner_tick,
            };
            match playlist_page {
                Some(p) => p.render(stage, frame.buffer_mut(), theme, hover, &live),
                None => {
                    frame.buffer_mut().set_string(
                        stage.x,
                        super::shell::body(stage).y,
                        "this playlist is gone",
                        theme.hint_desc,
                    );
                }
            }
        }
        Page::Search | Page::Favourites | Page::Recent | Page::Downloaded => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Shelf(h) => Some(h),
                _ => None,
            };
            let App {
                search_page,
                favourites_page,
                recent_page,
                downloaded_page,
                thumbs,
                art,
                theme,
                favourites,
                nav,
                ..
            } = &mut *app;
            let page = match nav.current() {
                Page::Favourites => favourites_page,
                Page::Recent => recent_page,
                Page::Downloaded => downloaded_page,
                _ => search_page,
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
        Page::History => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::History(h) => Some(h),
                _ => None,
            };
            let App {
                history_page,
                theme,
                links,
                ..
            } = &mut *app;
            history_page.render(stage, frame.buffer_mut(), theme, hover, links);
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
                    frame.buffer_mut().set_string(
                        stage.x,
                        super::shell::body(stage).y,
                        "this album is not in the library",
                        theme.hint_desc,
                    );
                }
            }
        }
        Page::Albums | Page::Artists | Page::Tracks | Page::Artist(_) => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Browse(h) => Some(h),
                _ => None,
            };
            let App {
                albums_page,
                artists_page,
                tracks_page,
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
                Page::Artists => artists_page,
                Page::Tracks => tracks_page,
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
        Page::Downloads => {
            app.layout.browse_area = stage;
            let hover = match app.hover.zone {
                super::app::HoverZone::Downloads(h) => Some(h),
                _ => None,
            };
            let App {
                downloads_page,
                state,
                theme,
                ..
            } = &mut *app;
            downloads_page.refresh(state);
            downloads_page.render(stage, frame.buffer_mut(), theme, hover);
        }
    }

    // Key hints / status message, inside the side gutters.
    app.layout.hint_hits.clear();
    let foot = super::shell::inset(chunks[3]);
    if let Some((ref msg, _)) = app.status_message {
        // The message cut to leave room for how to dismiss it.
        const DISMISS: &str = "  [esc] dismiss";
        let room = (foot.width as usize).saturating_sub(DISMISS.len());
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(super::text::fit(msg, room), app.theme.hint_key),
            ratatui::text::Span::styled(DISMISS, app.theme.hint_desc),
        ]);
        frame.render_widget(ratatui::widgets::Paragraph::new(line), foot);
    } else {
        let filterable = app.browse().is_some_and(|b| b.filterable());
        app.layout.hint_hits = super::keys::hits(&app.mode, app.page(), filterable, foot);
        let hint_bar = HintBar::new(&app.mode, app.page(), filterable, &app.theme);
        frame.render_widget(hint_bar, foot);
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
        let (click_col, click_row) = menu.at.unzip();
        app.layout.context_menu_area =
            context_menu_rect_at(area, menu.actions.len(), click_col, click_row);
        let mut overlay = ContextMenuOverlay::new(menu, &app.theme);
        if let Some((c, r)) = menu.at {
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
        && let Some(picker) = app.device_selector.as_mut()
    {
        app.layout.selector_area = picker.rect(area);
        let problem = app.state.renderer().and_then(|r| r.problem);
        picker.render(area, frame.buffer_mut(), &app.theme, problem.as_deref());
    }

    // Play on overlay.
    if app.mode == Mode::PlayOn
        && let Some(ref play_on) = app.play_on
    {
        app.layout.selector_area =
            super::device_selector::play_on_rect(area, play_on.choices.len());
        let overlay = super::device_selector::PlayOnOverlay::new(play_on, &app.theme);
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

    if app.mode == Mode::FilterPanel
        && let Some(ref panel) = app.filter_panel
    {
        app.layout.modal_area = super::browse_filter::panel_rect(area, panel.lines());
        let overlay = super::browse_filter::FilterPanelOverlay::new(panel, &app.theme);
        frame.render_widget(overlay, area);
    }

    if app.mode == Mode::ReplaceQueue
        && let Some(ref mut dialog) = app.replace_dialog
    {
        app.layout.modal_area = super::replace_dialog::ReplaceDialog::rect(area);
        let hover = dialog.hit(app.hover.column, app.hover.row);
        dialog.render(area, frame.buffer_mut(), &app.theme, hover);
    }

    if app.mode == Mode::ActionsMenu
        && let Some(ref menu) = app.actions_menu
    {
        app.layout.modal_area = super::actions_menu::menu_rect(area, &menu.items, &menu.title);
        let overlay = super::actions_menu::ActionsMenuOverlay::new(menu, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Help modal overlay.
    if app.mode == Mode::HelpModal {
        // Keys and the wheel scroll it without knowing how far it goes.
        app.help_scroll = app.help_scroll.min(super::help_modal::max_scroll(area));
        let overlay = HelpModalOverlay::new(&app.theme, app.help_scroll);
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
        let playing = (entry.status == koan_core::player::state::QueueEntryStatus::Playing)
            .then(|| app.state.track_info())
            .flatten();
        app.layout.track_info_area = super::track_info::popup_rect(area);
        let cover = app
            .art
            .cover_art
            .cover_for(&entry.path)
            .map(|c| (c, &mut app.art.gfx));
        let details = app.track_details.as_deref().cloned().unwrap_or_default();
        let groups = details.with_entry(&entry, playing.as_ref());
        let overlay =
            TrackInfoOverlay::new(&entry.title, groups, details.scroll, cover, &app.theme);
        frame.render_widget(overlay, area);
    }

    // Track info for a track picked from any list.
    if app.mode == Mode::TrackDetails
        && let Some(details) = app.track_details.as_deref()
    {
        app.layout.track_info_area = super::track_info::popup_rect(area);
        let overlay = TrackInfoOverlay::new(
            &details.title,
            details.groups(),
            details.scroll,
            None,
            &app.theme,
        );
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
            super::shell::clear(frame.buffer_mut(), popup);

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
            super::shell::clear(frame.buffer_mut(), label_area);
            let label_line = Line::from(Span::styled(label, app.theme.spinner));
            frame.render_widget(Paragraph::new(label_line), label_area);
        }
    }

    // Loading overlay with braille spinner: tracks being read into the
    // queue, or a picker's items.
    if app.loading() || matches!(app.mode, Mode::Picker(_)) && app.picker.is_none() {
        const SPINNER: &[char] = &[
            '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}',
        ];
        let frame_char = SPINNER[app.spinner_tick % SPINNER.len()];
        let display = format!("{} loading...", frame_char);
        let text_len = display.len() as u16 + 4;
        let w = text_len.max(20).min(area.width);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + area.height / 2;
        let popup = Rect::new(x, y, w, 1);
        super::shell::clear(frame.buffer_mut(), popup);
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

const SIDEBAR_WIDTH: u16 = 20;

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
    if Width::of(area) == Width::Wide {
        app.layout.nav_hits = render_sidebar(frame.buffer_mut(), app, area);
        return Rect::new(
            area.x + SIDEBAR_WIDTH,
            area.y,
            area.width - SIDEBAR_WIDTH,
            area.height,
        );
    }

    // Tab bar: a tab for each page with a digit. Titles when they fit, the
    // short names when they do not, and numbers when not even those.
    let theme = &app.theme;
    let current = app.nav.current().tab();
    let buf = frame.buffer_mut();
    let mut hits = Vec::new();
    let bar = Rect::new(area.x, area.y, area.width, 1.min(area.height));
    let mut tabs: Vec<(Page, char)> = Page::ALL
        .into_iter()
        .filter_map(|p| p.key().map(|k| (p, k)))
        .collect();
    // In the keyboard's order: 1 to 9, then 0.
    tabs.sort_by_key(|(_, k)| (*k == '0', *k));
    let usable = bar.width.saturating_sub(1) as usize;
    let cost = |label: fn(Page) -> &'static str| {
        tabs.iter()
            .map(|(p, _)| label(*p).chars().count() + 2)
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
    let mut x = bar.x + 1;
    let end = bar.x + bar.width;
    for (page, key) in tabs {
        let text = if numbers_only {
            if page == current {
                format!(" {key} {} ", page.short())
            } else {
                format!(" {key} ")
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

/// One row of the sidebar.
enum SideRow<'a> {
    Heading(&'static str),
    Gap,
    Target(&'a str, Option<char>, NavTarget),
}

/// The sidebar, as the apps lay it out: search at the top; the queue and
/// what is playing; the library's pages; the playlists, with a row for a new
/// one; then what is running, and settings at the foot. The digits run down
/// it in order. Returns what each row opens.
fn render_sidebar(
    buf: &mut ratatui::buffer::Buffer,
    app: &App,
    area: Rect,
) -> Vec<(Rect, NavTarget)> {
    let theme = &app.theme;
    let current = app.nav.current();
    let side = Rect::new(area.x, area.y, SIDEBAR_WIDTH, area.height);
    for y in side.y..side.y + side.height {
        buf[(side.x + SIDEBAR_WIDTH - 1, y)]
            .set_char('\u{2502}')
            .set_style(theme.rule);
    }
    let page = |p: Page| SideRow::Target(p.title(), p.key(), NavTarget::Page(p));
    let mut top = vec![
        // The search field, as the apps head their sidebar with it.
        SideRow::Target(
            "/ search",
            Page::Search.key(),
            NavTarget::Page(Page::Search),
        ),
        SideRow::Gap,
        page(Page::Queue),
        page(Page::NowPlaying),
        SideRow::Gap,
        SideRow::Heading("library"),
        page(Page::Albums),
        page(Page::Artists),
        page(Page::Tracks),
        page(Page::Favourites),
        page(Page::Recent),
        page(Page::Downloaded),
        page(Page::History),
        page(Page::Downloads),
        SideRow::Gap,
        SideRow::Target(
            "playlists",
            Page::Playlists.key(),
            NavTarget::Page(Page::Playlists),
        ),
        SideRow::Target("+ new playlist", None, NavTarget::NewPlaylist),
    ];
    let foot = [page(Page::Settings)];
    let busy = busy(app);
    // Each running job takes its label and its bar, and a gap above them.
    let busy_rows = if busy.is_empty() { 0 } else { 3 * busy.len() };
    // The playlists fill what is left between the two, as many as fit.
    let room = (side.height as usize).saturating_sub(top.len() + 1 + foot.len() + busy_rows);
    top.extend(app.sidebar_playlists.iter().take(room).map(|(id, name)| {
        SideRow::Target(name.as_str(), None, NavTarget::Page(Page::Playlist(*id)))
    }));

    let mut hits = Vec::new();
    let label_w = (SIDEBAR_WIDTH - 5) as usize;
    let mut draw = |row: &SideRow, y: u16, hits: &mut Vec<(Rect, NavTarget)>| match row {
        SideRow::Gap => {}
        SideRow::Heading(text) => {
            buf.set_stringn(side.x + 1, y, text, label_w, theme.rule);
        }
        SideRow::Target(text, key, target) => {
            let active = match target {
                NavTarget::Page(Page::Playlist(id)) => current == Page::Playlist(*id),
                NavTarget::Page(p) => *p == current.tab() && !matches!(current, Page::Playlist(_)),
                _ => false,
            } || matches!(
                (target, current),
                (NavTarget::Page(Page::Playlists), Page::Playlist(_))
            );
            let hovered = app.hover.zone == super::app::HoverZone::Nav(*target);
            let style = if active {
                theme.heading
            } else if hovered {
                theme.track_normal
            } else {
                theme.hint_desc
            };
            if active {
                buf[(side.x, y)].set_char('\u{258c}').set_style(theme.bar);
            }
            buf.set_stringn(
                side.x + 1,
                y,
                super::text::fit(text, label_w),
                label_w,
                style,
            );
            if let Some(k) = key {
                buf.set_string(
                    side.x + SIDEBAR_WIDTH - 3,
                    y,
                    k.to_string(),
                    theme.hint_desc,
                );
            }
            hits.push((Rect::new(side.x, y, SIDEBAR_WIDTH - 1, 1), *target));
        }
    };
    let bottom = side.y + side.height;
    let below = (foot.len() + busy_rows) as u16;
    for (i, row) in top.iter().enumerate() {
        let y = side.y + super::shell::TOP_GUTTER + i as u16;
        if y + below >= bottom {
            break;
        }
        draw(row, y, &mut hits);
    }
    for (i, row) in foot.iter().enumerate() {
        let y = bottom - foot.len() as u16 + i as u16;
        if y > side.y {
            draw(row, y, &mut hits);
        }
    }
    let mut y = bottom.saturating_sub(below);
    for job in &busy {
        y += 1;
        if y <= side.y || y + 1 >= bottom {
            break;
        }
        let w = SIDEBAR_WIDTH - 2;
        let counts = job.counts.as_deref().unwrap_or("");
        let counts_w = super::text::width(counts) as u16;
        let label_room = w.saturating_sub(counts_w + 1) as usize;
        buf.set_stringn(
            side.x + 1,
            y,
            super::text::fit(&job.label, label_room),
            label_room,
            theme.hint_desc,
        );
        buf.set_stringn(
            side.x + 1 + w - counts_w.min(w),
            y,
            counts,
            w as usize,
            theme.rule,
        );
        hits.push((Rect::new(side.x, y, SIDEBAR_WIDTH - 1, 1), job.open));
        // The bar: as far as it has got, or dashed while there is no total.
        let bar_w = if job.cancellable { w - 2 } else { w };
        let filled = job.fraction.map(|f| (f * bar_w as f64).round() as u16);
        for i in 0..bar_w {
            let (c, style) = match filled {
                Some(n) if i < n => ('\u{2501}', theme.bar),
                Some(_) => ('\u{2500}', theme.rule),
                None => ('\u{254c}', theme.rule),
            };
            buf[(side.x + 1 + i, y + 1)].set_char(c).set_style(style);
        }
        if job.cancellable {
            let x = side.x + 1 + w - 1;
            buf.set_string(x, y + 1, "\u{d7}", theme.hint_desc);
            hits.push((Rect::new(x, y + 1, 1, 1), NavTarget::CancelTask));
        }
        y += 2;
    }
    hits
}

/// A job the sidebar shows running.
struct Busy {
    label: String,
    counts: Option<String>,
    fraction: Option<f64>,
    cancellable: bool,
    /// Where a click on it goes.
    open: NavTarget,
}

/// What this device is busy with: the scan or sync the TUI started, and the
/// downloads in flight. Nothing when idle.
fn busy(app: &App) -> Vec<Busy> {
    let mut out = Vec::new();
    if let Some(a) = app.activity.as_ref() {
        out.push(Busy {
            label: a.label(),
            counts: a.counts(),
            fraction: a.fraction(),
            cancellable: a.cancel.is_some(),
            open: NavTarget::Page(Page::Settings),
        });
    }
    let readings = app.view.local.downloads().readings();
    if !readings.is_empty() {
        let written: u64 = readings.iter().map(|r| r.written).sum();
        let total: u64 = readings.iter().map(|r| r.total).sum();
        out.push(Busy {
            label: format!("downloading {}", readings.len()),
            counts: (total > 0).then(|| format!("{} / {} MB", written >> 20, total >> 20)),
            fraction: (total > 0).then(|| (written as f64 / total as f64).min(1.0)),
            cancellable: false,
            open: NavTarget::Page(Page::Downloads),
        });
    }
    out
}

/// FPS + beat-energy counter, right-aligned on the top row of `area`.
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

fn render_queue(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let selected_indices = app.selected_indices();
    let queue_view = QueueView::new(
        app.visible_queue(),
        &app.queue.layout,
        &app.mode,
        app.queue.cursor,
        app.queue.scroll_offset,
        &app.theme,
    )
    .with_facts(&app.queue.facts, koan_core::remote::offline::active())
    .with_selected(&selected_indices)
    .with_spinner(app.spinner_tick)
    .with_drop_indicator(app.drop_indicator_index())
    .with_links(&app.links)
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
        app_and_player(len).0
    }

    /// The same, and what it sends the player.
    fn app_and_player(
        len: usize,
    ) -> (
        App,
        crossbeam_channel::Receiver<koan_core::player::commands::PlayerCommand>,
    ) {
        // First, before anything reads configuration: spawning the download
        // queue resolves the remote server, and a render test has no business
        // reaching for anyone's credentials.
        koan_core::config::isolate_config_for_tests();

        let state = SharedPlayerState::new();
        let (tx, rx) = crossbeam_channel::unbounded();

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

        let mut app = App::new(state, koan_core::audio::viz::VizSnapshot::new(), tx, 60);
        app.queue.vq_cache.entries = entries;
        app.queue.vq_version = app.state.playlist_version();
        app.relayout_queue();
        // Pinned so the sweep does not inherit the developer's own config.
        app.art_size = 24;
        app.show_fps = false;
        app.viz_fullscreen = false;
        app.viz_config.enabled = true;
        (app, rx)
    }

    fn draw(app: &mut App, w: u16, h: u16) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
    }

    /// A row's list replaces the queue at once when nothing is queued, asks
    /// first when something is, and an explicit play button never asks.
    #[test]
    fn replacing_the_queue_from_a_row_asks_only_when_it_has_something() {
        use crate::app::PickerAction;
        use crate::browse::Action;
        let from_row = || Action::Play(vec![1, 2, 3], PickerAction::ReplaceFrom(1));

        let mut empty = app_with_queue(0);
        empty.apply_browse(from_row());
        assert_eq!(
            empty.mode,
            Mode::Normal,
            "nothing queued: plays straight away"
        );
        assert!(empty.replace_dialog.is_none());

        let mut queued = app_with_queue(4);
        queued.apply_browse(from_row());
        assert_eq!(queued.mode, Mode::ReplaceQueue, "something queued: asks");
        let dialog = queued.replace_dialog.as_ref().expect("the dialog");
        assert_eq!((dialog.ids.clone(), dialog.start), (vec![1, 2, 3], 1));
        draw(&mut queued, 80, 24);
        queued.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Esc,
        ));
        assert_eq!(queued.mode, Mode::Normal, "cancelled");
        assert!(queued.replace_dialog.is_none());

        let mut button = app_with_queue(4);
        button.apply_browse(Action::Play(vec![1, 2, 3], PickerAction::ReplaceQueue));
        assert_eq!(
            button.mode,
            Mode::Normal,
            "a play button replaces without asking"
        );
        assert!(button.replace_dialog.is_none());
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
        assert_eq!(app.page(), Page::Recent);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.page(), Page::Downloaded);
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
        use crate::queue::{QueueView, menu_button_x};
        use crate::shell::Link;
        let mut app = app_with_queue(3);
        for (i, e) in app.queue.vq_cache.entries.iter_mut().enumerate() {
            e.db_id = Some(i as i64 + 1);
        }
        app.relayout_queue();
        draw(&mut app, 120, 30);
        let area = app.layout.queue_area;
        // The first line is the heading: " album  artist ... 2024 · 3 tracks".
        assert_eq!(
            app.links.at(area.x + 1, area.y),
            Some(Link::AlbumOfTrack(1))
        );
        assert_eq!(
            app.links.at(area.x + 1 + "album  ".len() as u16, area.y),
            Some(Link::ArtistOfTrack(1))
        );
        assert_eq!(app.links.at(area.x + 40, area.y), None);
        // A track's heart is its own hit.
        let entries = &app.queue.vq_cache.entries;
        let heart = (area.x..area.x + area.width)
            .find(|&x| {
                QueueView::heart_at(entries, &app.queue.layout, area, 0, x, area.y + 1).is_some()
            })
            .expect("a heart column");
        assert!(
            heart > area.x + 40,
            "the heart sits at the right, among the figures"
        );
        assert_eq!(menu_button_x(area), area.x + area.width - 3);
    }

    #[test]
    fn the_queue_draws_no_heading_of_its_own_under_the_toolbar() {
        let mut app = app_with_queue(3);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let q = app.layout.queue_area;
        let row =
            |y: u16| -> String { (q.x..q.x + q.width).map(|x| buf[(x, y)].symbol()).collect() };
        let toolbar = row(q.y - 1);
        assert!(
            toolbar.contains("queue") && toolbar.contains("3 tracks"),
            "{toolbar:?}"
        );
        let first = row(q.y);
        assert!(
            first.trim_start().starts_with("album"),
            "the album's heading: {first:?}"
        );
        assert!(
            !first.contains('\u{2500}'),
            "no rule of a heading row: {first:?}"
        );
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

    fn click(app: &mut App, column: u16, row: u16) {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    fn key(app: &mut App, code: crossterm::event::KeyCode) {
        app.handle_key(crossterm::event::KeyEvent::from(code));
    }

    #[test]
    fn leaving_the_queue_leaves_editing_it() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(5);
        draw(&mut app, 160, 40);
        key(&mut app, KeyCode::Char('e'));
        assert_eq!(app.mode, Mode::QueueEdit);
        let (rect, _) = *app
            .layout
            .nav_hits
            .iter()
            .find(|(_, t)| *t == NavTarget::Page(Page::Albums))
            .expect("an albums tab");
        click(&mut app, rect.x, rect.y);
        assert_eq!(app.page(), Page::Albums);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.mode_stack.is_empty());
        // Keys are the page's again, not the queue editor's.
        key(&mut app, KeyCode::Char('/'));
        key(&mut app, KeyCode::Char('x'));
        assert_eq!(app.albums_page.filter, "x");
    }

    #[test]
    fn closing_track_info_returns_to_editing() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(5);
        key(&mut app, KeyCode::Char('e'));
        key(&mut app, KeyCode::Char('i'));
        assert!(matches!(app.mode, Mode::TrackInfo(_)));
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::QueueEdit);
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.mode_stack.is_empty());
    }

    #[test]
    fn the_fullscreen_visualiser_hides_what_was_clickable() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(5);
        draw(&mut app, 160, 40);
        let (rect, _) = *app
            .layout
            .nav_hits
            .iter()
            .find(|(_, t)| *t == NavTarget::Page(Page::Albums))
            .expect("an albums tab");
        key(&mut app, KeyCode::Char('F'));
        draw(&mut app, 160, 40);
        click(&mut app, rect.x, rect.y);
        assert_eq!(app.page(), Page::Queue);
    }

    #[test]
    fn a_click_off_the_queue_with_no_queue_on_screen_is_harmless() {
        let mut app = app_with_queue(5);
        app.go_page(Page::NowPlaying);
        for (w, h) in [(160u16, 40u16), (60, 20)] {
            draw(&mut app, w, h);
            for row in 0..h {
                for column in [0, w / 2, w - 1] {
                    click(&mut app, column, row);
                    if app.mode != Mode::Normal {
                        app.mode_stack.clear();
                        app.mode = Mode::Normal;
                    }
                    app.go_page(Page::NowPlaying);
                }
            }
        }
    }

    #[test]
    fn the_context_menu_border_is_not_its_first_row() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with_queue(5);
        draw(&mut app, 100, 30);
        let q = app.layout.queue_area;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: q.x + 10,
            row: q.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.mode, Mode::ContextMenu);
        draw(&mut app, 100, 30);
        let area = app.layout.context_menu_area;
        click(&mut app, area.x + 2, area.y);
        assert_eq!(app.mode, Mode::ContextMenu);
    }

    #[test]
    fn a_click_closes_the_help_rather_than_reaching_under_it() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(5);
        draw(&mut app, 160, 40);
        let (rect, _) = *app
            .layout
            .nav_hits
            .iter()
            .find(|(_, t)| *t == NavTarget::Page(Page::Albums))
            .expect("an albums tab");
        key(&mut app, KeyCode::Char('?'));
        draw(&mut app, 160, 40);
        click(&mut app, rect.x, rect.y);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.page(), Page::Queue);
    }

    #[test]
    fn a_heading_then_its_first_row_is_not_a_double_click() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (mut app, player) = app_and_player(3);
        app.queue.vq_cache.entries[0].status = QueueEntryStatus::Queued;
        draw(&mut app, 100, 30);
        // The queue's first row is the album's heading, the next its first track.
        let q = app.layout.queue_area;
        click(&mut app, q.x + 40, q.y);
        assert_eq!(app.queue.selected_ids.len(), 3);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: q.x + 40,
            row: q.y,
            modifiers: KeyModifiers::NONE,
        });
        click(&mut app, q.x + 40, q.y + 1);
        // Commands reach the player through the router's thread.
        let wait = std::time::Duration::from_millis(150);
        assert!(player.recv_timeout(wait).is_err(), "nothing was played");
        // The same row twice is.
        click(&mut app, q.x + 40, q.y + 1);
        assert!(matches!(
            player.recv_timeout(wait * 3),
            Ok(koan_core::player::commands::PlayerCommand::Play(_))
        ));
    }

    #[test]
    fn the_picker_closes_before_its_items_arrive() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(3);
        key(&mut app, KeyCode::Char('p'));
        assert!(matches!(app.mode, Mode::Picker(_)));
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_paste_goes_into_the_field_that_has_the_keys() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(3);
        assert!(!app.takes_text());
        key(&mut app, KeyCode::Char('3'));
        key(&mut app, KeyCode::Char('/'));
        assert!(app.takes_text());
        app.type_text("boards of\ncanada");
        assert_eq!(app.albums_page.filter, "boards of canada");
        assert_eq!(app.page(), Page::Albums);
    }

    #[test]
    fn the_help_scrolls_no_further_than_its_end() {
        use crossterm::event::KeyCode;
        let mut app = app_with_queue(3);
        key(&mut app, KeyCode::Char('?'));
        key(&mut app, KeyCode::End);
        draw(&mut app, 100, 20);
        let end = app.help_scroll;
        assert!(end > 0);
        key(&mut app, KeyCode::Up);
        draw(&mut app, 100, 20);
        assert_eq!(app.help_scroll, end - 1);
    }

    #[test]
    fn editing_the_queue_g_moves_and_ctrl_d_pages() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut app, player) = app_and_player(5);
        draw(&mut app, 100, 30);
        key(&mut app, KeyCode::Char('e'));
        // As terminals send it: an uppercase letter, with Shift.
        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(app.queue.cursor, 4);
        assert_eq!(app.queue.selected_ids.len(), 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        // Commands reach the player through the router's thread.
        assert!(
            player
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "nothing was removed"
        );
    }

    #[test]
    fn the_toolbar_sits_below_the_top_gutter_and_goes_back() {
        use crate::shell::{TOP_GUTTER, ToolHit};
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with_queue(3);
        app.go_page(Page::Albums);
        for (w, top) in [(160u16, TOP_GUTTER), (80, 1 + TOP_GUTTER)] {
            draw(&mut app, w, 40);
            assert!(!app.layout.toolbar_hits.is_empty(), "width {w}");
            for (r, _) in &app.layout.toolbar_hits {
                assert_eq!(r.y, top, "width {w}");
                assert!(r.x + r.width <= w, "width {w}");
            }
        }
        let back = app
            .layout
            .toolbar_hits
            .iter()
            .find(|(_, h)| *h == ToolHit::Nav(NavTarget::Back))
            .expect("a back button")
            .0;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: back.x,
            row: back.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.page(), Page::Queue);
    }

    #[test]
    fn a_link_drawn_anywhere_opens_its_page() {
        use crate::shell::Link;
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with_queue(3);
        draw(&mut app, 120, 40);
        let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 120, 40));
        app.links.draw(
            &mut buf,
            60,
            20,
            "Selected Ambient Works",
            30,
            ratatui::style::Style::default(),
            Link::Album(999),
        );
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 62,
            row: 20,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.page(), Page::Album(999));
    }

    #[test]
    fn the_sidebar_lists_every_page_and_the_playlists() {
        let mut app = app_with_queue(3);
        app.sidebar_playlists = vec![(41, "road trip".into()), (42, "東京事変 best".into())];
        draw(&mut app, 160, 40);
        let targets: Vec<_> = app.layout.nav_hits.iter().map(|(_, t)| *t).collect();
        for page in Page::ALL {
            assert!(targets.contains(&NavTarget::Page(page)), "{page:?}");
        }
        assert!(targets.contains(&NavTarget::NewPlaylist));
        assert!(targets.contains(&NavTarget::Page(Page::Playlist(42))));
        // Short: the playlists give way, the pages do not.
        draw(&mut app, 160, 26);
        let targets: Vec<_> = app.layout.nav_hits.iter().map(|(_, t)| *t).collect();
        assert!(targets.contains(&NavTarget::Page(Page::Settings)));
    }

    /// The buffer as text, a line a row, the second cell of a wide
    /// character left out.
    fn dump(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            let mut skip = 0;
            for x in 0..w {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let sym = buf[(x, y)].symbol();
                skip = crate::text::width(sym).saturating_sub(1);
                out.push_str(sym);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn a_running_scan_has_a_row_and_a_stop_button_in_the_sidebar() {
        let mut app = app_with_queue(3);
        let activity = crate::tasks::Activity::new("scanning the library", true);
        activity
            .total
            .store(4000, std::sync::atomic::Ordering::Relaxed);
        activity
            .done
            .store(1204, std::sync::atomic::Ordering::Relaxed);
        app.activity = Some(activity.clone());
        let text = dump(&mut app, 160, 40);
        assert!(text.contains("1,204 / 4,000"), "{text}");
        let stop = app
            .layout
            .nav_hits
            .iter()
            .find(|(_, t)| *t == NavTarget::CancelTask)
            .expect("a stop button")
            .0;
        click(&mut app, stop.x, stop.y);
        assert!(
            activity
                .cancel
                .as_ref()
                .unwrap()
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        // Idle, nothing shows.
        app.activity = None;
        draw(&mut app, 160, 40);
        assert!(
            !app.layout
                .nav_hits
                .iter()
                .any(|(_, t)| *t == NavTarget::CancelTask)
        );
    }

    /// Writes every page at four sizes to `target/tui-dumps/shell`, against
    /// the awkward library, with the playing track's cover absent and
    /// present. Fills this process's library: run it on its own.
    #[test]
    #[ignore = "fills the library; run on its own with --ignored"]
    fn dump_the_shell() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-dumps/shell");
        let mut app = app_with_queue(12);
        let (double, _) = crate::dumps::seed();
        let artist = koan_core::db::pool::shared()
            .get()
            .ok()
            .and_then(|db| {
                koan_core::db::queries::get_album(&db.conn, double)
                    .ok()
                    .flatten()
            })
            .map_or(0, |a| a.artist_id);
        app.load_favourites();
        app.refresh_sidebar_playlists();
        app.sidebar_playlists
            .push((42, "東京事変 best of everything".into()));
        // A cover beside the playing track, for the second pass.
        let covers = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(64, 64, image::Rgb([200, 60, 40]))
            .save(covers.path().join("cover.png"))
            .unwrap();
        let settle = |app: &mut App| {
            for _ in 0..15 {
                app.handle_tick();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let mut pages: Vec<Page> = Page::ALL.to_vec();
        pages.extend([Page::Album(double), Page::Artist(artist)]);
        for pass in ["nocover", "cover"] {
            let dir = root.join(pass);
            std::fs::create_dir_all(&dir).unwrap();
            if pass == "cover" {
                let path = covers.path().join("track.flac");
                app.queue.vq_cache.entries[0].path = path.clone();
                if let Some(mut info) = app.state.track_info() {
                    info.path = path;
                    app.state.set_track_info(Some(info));
                }
            }
            for &page in &pages {
                app.go_page(page);
                draw(&mut app, 120, 40);
                settle(&mut app);
                for (w, h) in [(50u16, 20u16), (80, 24), (120, 40), (200, 60)] {
                    let name = format!("{}-{w}x{h}.txt", page.title().replace(' ', "-"));
                    std::fs::write(dir.join(name), dump(&mut app, w, h)).unwrap();
                }
            }
        }
        let dir = root.join("modals");
        std::fs::create_dir_all(&dir).unwrap();
        app.go_page(Page::Queue);
        app.status_message = Some((
            "a status line long enough to be cut at the edge of a narrow terminal".into(),
            std::time::Instant::now(),
        ));
        type Open = fn(&mut App);
        let modals: [(&str, Open); 5] = [
            ("status", |_| {}),
            ("help", |a| {
                a.handle_key(crossterm::event::KeyEvent::from(
                    crossterm::event::KeyCode::Char('?'),
                ))
            }),
            ("chooser", |a| a.open_chooser(vec![1, 2])),
            ("prompt", |a| {
                a.open_chooser(vec![1]);
                a.handle_key(crossterm::event::KeyEvent::from(
                    crossterm::event::KeyCode::Enter,
                ));
            }),
            ("eq", |a| {
                a.handle_key(crossterm::event::KeyEvent::from(
                    crossterm::event::KeyCode::Char('E'),
                ))
            }),
        ];
        for (name, open) in modals {
            open(&mut app);
            for (w, h) in [(50u16, 20u16), (120, 40)] {
                std::fs::write(
                    dir.join(format!("modal-{name}-{w}x{h}.txt")),
                    dump(&mut app, w, h),
                )
                .unwrap();
            }
            app.mode_stack.clear();
            app.mode = Mode::Normal;
        }
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

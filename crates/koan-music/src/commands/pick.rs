use std::io;
use std::time::Duration;

use koan_core::db::queries;

use super::{
    cmd_play, install_terminal_panic_hook, make_album_picker_items, make_artist_picker_items,
    make_track_picker_items, open_db,
};
use crate::tui;
use crate::tui::picker::{
    PickerItem, PickerKind, PickerPartKind, PickerState, all_tracks_sentinel,
    is_all_tracks_sentinel,
};

pub fn cmd_pick(_query: Option<&str>, album_mode: bool, artist_mode: bool) {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::execute;
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    let db = open_db();
    let theme = tui::theme::Theme::default();

    let (items, kind) = if album_mode {
        let albums = queries::all_albums(&db.conn).unwrap_or_default();
        if albums.is_empty() {
            eprintln!("no albums found");
            std::process::exit(1);
        }
        (make_album_picker_items(&albums), PickerKind::Album)
    } else if artist_mode {
        let artists = queries::all_artists(&db.conn).unwrap_or_default();
        if artists.is_empty() {
            eprintln!("no artists found");
            std::process::exit(1);
        }
        (make_artist_picker_items(&artists), PickerKind::Artist)
    } else {
        let tracks = queries::all_tracks(&db.conn).unwrap_or_default();
        if tracks.is_empty() {
            eprintln!("no tracks found");
            std::process::exit(1);
        }
        (make_track_picker_items(&tracks), PickerKind::Track)
    };

    let multi = matches!(kind, PickerKind::Track);
    let mut picker = PickerState::new(kind, items, multi);

    // Setup terminal for picker.
    install_terminal_panic_hook();
    enable_raw_mode().expect("failed to enable raw mode");
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).expect("failed to enter alt screen");
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");

    let result = loop {
        terminal
            .draw(|f| {
                let overlay = tui::picker::PickerOverlay::new(&picker, &theme);
                f.render_widget(overlay, f.area());
            })
            .ok();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(Event::Key(key)) = event::read()
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                break None;
            }
            match key.code {
                KeyCode::Esc => break None,
                KeyCode::Enter => {
                    let ids = picker.confirm();
                    break if ids.is_empty() { None } else { Some(ids) };
                }
                KeyCode::Up => picker.move_up(),
                KeyCode::Down => picker.move_down(),
                KeyCode::Tab => picker.toggle_select(),
                KeyCode::Backspace => picker.backspace(),
                KeyCode::Char(c) => picker.type_char(c),
                _ => {}
            }
        }
        picker.tick();
    };

    // Restore terminal.
    disable_raw_mode().expect("failed to disable raw mode");
    execute!(terminal.backend_mut(), LeaveAlternateScreen).expect("failed to leave alt screen");
    terminal.show_cursor().ok();

    // Process result.
    if let Some(ids) = result {
        match kind {
            PickerKind::Track => {
                cmd_play(&[], &ids, None, None, false);
            }
            PickerKind::Album => {
                if let Some(&album_id) = ids.first() {
                    cmd_play(&[], &[], Some(album_id), None, false);
                }
            }
            PickerKind::Artist => {
                if let Some(&artist_id) = ids.first() {
                    // Drill down: pick album for this artist.
                    let albums =
                        queries::albums_for_artist(&db.conn, artist_id).unwrap_or_default();
                    if albums.is_empty() {
                        cmd_play(&[], &[], None, Some(artist_id), false);
                    } else {
                        // Show album picker for this artist with an "all tracks" entry.
                        let mut items = vec![PickerItem {
                            id: all_tracks_sentinel(artist_id),
                            display: "all tracks".to_string(),
                            match_text: "all tracks".into(),
                            parts: vec![("all tracks".into(), PickerPartKind::Plain)],
                        }];
                        items.extend(make_album_picker_items(&albums));

                        let mut picker2 = PickerState::new(PickerKind::Album, items, false);

                        enable_raw_mode().expect("failed to enable raw mode");
                        let mut stdout2 = io::stdout();
                        execute!(stdout2, EnterAlternateScreen)
                            .expect("failed to enter alt screen");
                        let backend2 = CrosstermBackend::new(stdout2);
                        let mut terminal2 =
                            Terminal::new(backend2).expect("failed to create terminal");

                        let album_result = loop {
                            terminal2
                                .draw(|f| {
                                    let overlay = tui::picker::PickerOverlay::new(&picker2, &theme);
                                    f.render_widget(overlay, f.area());
                                })
                                .ok();

                            if event::poll(Duration::from_millis(50)).unwrap_or(false)
                                && let Ok(Event::Key(key)) = event::read()
                            {
                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.code == KeyCode::Char('c')
                                {
                                    break None;
                                }
                                match key.code {
                                    KeyCode::Esc => break None,
                                    KeyCode::Enter => {
                                        let ids = picker2.confirm();
                                        break if ids.is_empty() { None } else { Some(ids) };
                                    }
                                    KeyCode::Up => picker2.move_up(),
                                    KeyCode::Down => picker2.move_down(),
                                    KeyCode::Backspace => picker2.backspace(),
                                    KeyCode::Char(c) => picker2.type_char(c),
                                    _ => {}
                                }
                            }
                            picker2.tick();
                        };

                        disable_raw_mode().expect("failed to disable raw mode");
                        execute!(terminal2.backend_mut(), LeaveAlternateScreen)
                            .expect("failed to leave alt screen");
                        terminal2.show_cursor().ok();

                        if let Some(album_ids) = album_result {
                            if is_all_tracks_sentinel(album_ids[0]) {
                                cmd_play(&[], &[], None, Some(artist_id), false);
                            } else {
                                cmd_play(&[], &[], Some(album_ids[0]), None, false);
                            }
                        }
                    }
                }
            }
            // QueueJump is only used in the TUI playback loop, not standalone picker.
            PickerKind::QueueJump => {}
        }
    }
}

use super::*;
use crossterm::event::{KeyCode, KeyEvent};

fn theme() -> Theme {
    Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167))
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn page() -> SettingsPage {
    koan_core::config::isolate_config_for_tests();
    let mut p = SettingsPage::new();
    p.reload();
    p
}

fn draw(p: &mut SettingsPage, w: u16, h: u16) -> Buffer {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    p.render(area, &mut buf, &theme(), None);
    buf
}

/// The buffer as lines of text, a wide character's trailing cell skipped.
fn lines(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut skip = 0;
            for x in 0..area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let sym = buf[(x, y)].symbol();
                skip = sym.width().saturating_sub(1);
                line.push_str(sym);
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn go_to(p: &mut SettingsPage, section: Section) {
    p.select_section(section);
    p.focus = Focus::Pane;
}

#[test]
fn signed_out_there_is_no_account_people_or_integrations() {
    let p = page();
    if p.signed_in() {
        return;
    }
    assert_eq!(
        p.sections(),
        vec![
            Section::Library,
            Section::Server,
            Section::Playback,
            Section::Eq,
            Section::Devices,
            Section::Terminal
        ]
    );
}

#[test]
fn the_sections_move_with_j_and_k_and_enter_goes_into_the_pane() {
    let mut p = page();
    assert_eq!(p.handle_key(key(KeyCode::Char('j'))), Some(None));
    assert_eq!(p.section, Section::Server);
    p.handle_key(key(KeyCode::Enter));
    assert_eq!(p.focus, Focus::Pane);
    p.handle_key(key(KeyCode::Esc));
    assert_eq!(p.focus, Focus::Sections);
    assert_eq!(p.handle_key(key(KeyCode::Esc)), None, "esc leaves the page");
}

#[test]
fn a_field_takes_every_key_until_enter_or_esc() {
    let mut p = page();
    go_to(&mut p, Section::Library);
    draw(&mut p, 100, 30);
    let at = p
        .rows()
        .iter()
        .position(|r| {
            matches!(
                r.kind,
                Kind::Field {
                    field: Field::Folder,
                    ..
                }
            )
        })
        .unwrap();
    p.cursor = p.rows()[at].key.clone();
    p.handle_key(key(KeyCode::Enter));
    assert!(p.typing());
    for c in "1j/".chars() {
        assert_eq!(p.handle_key(key(KeyCode::Char(c))), Some(None));
    }
    assert_eq!(p.field(Field::Folder), "1j/");
    p.handle_key(key(KeyCode::Esc));
    assert!(!p.typing());
    assert_eq!(p.field(Field::Folder), "1j/", "kept as typed");
}

#[test]
fn a_folder_typed_in_is_added_and_scanned() {
    let mut p = page();
    let dir = tempfile::tempdir().unwrap();
    go_to(&mut p, Section::Library);
    p.fields
        .insert(Field::Folder, dir.path().display().to_string());
    assert_eq!(
        p.run(Op::AddFolder),
        Some(Outcome::Run(Task::Scan { force: false }))
    );
    assert!(p.cfg.library.folders.iter().any(|f| f == dir.path()));
    assert_eq!(p.field(Field::Folder), "");
    work::remove_folder(dir.path()).unwrap();
}

#[test]
fn removing_a_folder_asks_whether_to_forget_its_tracks() {
    let mut p = page();
    let dir = tempfile::tempdir().unwrap();
    work::add_folder(dir.path().to_str().unwrap()).unwrap();
    p.reload();
    go_to(&mut p, Section::Library);
    let row = p
        .rows()
        .into_iter()
        .find(|r| r.key == format!("folder:{}", dir.path().display()))
        .unwrap();
    p.cursor = row.key;
    assert_eq!(p.handle_key(key(KeyCode::Char('x'))), Some(None));
    let dialog = p.dialog.clone().expect("a question");
    assert_eq!(dialog.choices.len(), 3);
    assert!(p.typing());
    p.handle_key(key(KeyCode::Esc));
    assert!(p.dialog.is_none(), "esc lets it be");
    assert!(p.cfg.library.folders.iter().any(|f| f == dir.path()));

    p.handle_key(key(KeyCode::Char('x')));
    p.handle_key(key(KeyCode::Right));
    assert_eq!(
        p.handle_key(key(KeyCode::Enter)),
        Some(Some(Outcome::Say(format!(
            "stopped scanning {}",
            dir.path().display()
        ))))
    );
    assert!(!p.cfg.library.folders.iter().any(|f| f == dir.path()));
}

#[test]
fn the_cursor_stays_on_its_row_as_rows_above_come_and_go() {
    let mut p = page();
    go_to(&mut p, Section::Library);
    p.cursor = "button:scan".into();
    let dir = tempfile::tempdir().unwrap();
    work::add_folder(dir.path().to_str().unwrap()).unwrap();
    p.reload();
    let rows = p.rows();
    assert_eq!(rows[p.cursor_at(&rows).unwrap()].key, "button:scan");
    work::remove_folder(dir.path()).unwrap();
}

#[test]
fn a_choice_steps_and_is_written() {
    let mut p = page();
    go_to(&mut p, Section::Terminal);
    p.cursor = "setting:ShowFps".into();
    let before = p.cfg.playback.show_fps;
    assert_eq!(
        p.handle_key(key(KeyCode::Right)),
        Some(Some(Outcome::Changed(Setting::ShowFps)))
    );
    assert_eq!(p.cfg.playback.show_fps, !before);
    p.handle_key(key(KeyCode::Left));
    assert_eq!(p.cfg.playback.show_fps, before);
}

#[test]
fn values_line_up_in_one_column() {
    let mut p = page();
    for section in p.sections() {
        go_to(&mut p, section);
        let buf = draw(&mut p, 120, 60);
        let arrows: Vec<u16> = (0..60)
            .filter_map(|y| (0..120).find(|&x| buf[(x, y)].symbol() == "\u{2039}"))
            .collect();
        assert!(
            arrows.windows(2).all(|w| w[0] == w[1]),
            "{section:?}: choices start at {arrows:?}"
        );
    }
}

#[test]
fn a_narrow_pane_keeps_every_value() {
    let mut p = page();
    go_to(&mut p, Section::Terminal);
    let shown = lines(&draw(&mut p, 50, 20)).join("\n");
    for s in [Setting::Theme, Setting::FrameRate, Setting::Palette] {
        let value = s.value(&p.cfg);
        assert!(shown.contains(&value), "{s:?} shows {value}:\n{shown}");
    }
}

#[test]
fn every_section_draws_at_any_size() {
    let mut p = page();
    for section in p.sections() {
        go_to(&mut p, section);
        for (w, h) in [
            (0, 0),
            (39, 10),
            (40, 4),
            (50, 20),
            (80, 24),
            (120, 40),
            (200, 60),
        ] {
            let buf = draw(&mut p, w, h);
            if w >= 50 {
                let shown = lines(&buf).join("\n");
                assert!(shown.contains(section.label()), "{section:?} at {w}x{h}");
            }
        }
    }
}

#[test]
fn a_dialog_is_drawn_whole_and_its_choices_are_clickable() {
    let mut p = page();
    go_to(&mut p, Section::Library);
    p.run(Op::AskRebuild);
    let buf = draw(&mut p, 80, 24);
    let shown = lines(&buf).join("\n");
    assert!(shown.contains("clear the library index?"));
    assert!(shown.contains("clear index") && shown.contains("cancel"));
    let (rect, hit) = *p
        .geom
        .links
        .iter()
        .find(|(_, h)| *h == SettingsHit::DialogChoice(1))
        .unwrap();
    assert_eq!(p.hit(rect.x, rect.y), Some(hit));
    assert_eq!(p.click(hit), None);
    assert!(p.dialog.is_none());
}

#[test]
fn clicking_a_section_shows_it() {
    let mut p = page();
    draw(&mut p, 100, 30);
    let s = p.geom.sections;
    let hit = p.hit(s.x + 2, s.y + 1).unwrap();
    assert_eq!(hit, SettingsHit::Section(1));
    p.click(hit);
    assert_eq!(p.section, Section::Server);
}

#[test]
fn older_palette_names_read_as_the_theme() {
    assert_eq!(palette_name("spectrum"), "theme");
    assert_eq!(palette_name("fire"), "fire");
}

/// Writes each section's dump at the sizes the guide is checked at, to
/// read by eye: `cargo test -p koan-tui dump_settings -- --ignored`.
#[test]
#[ignore]
fn dump_settings() {
    let mut p = page();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-dumps");
    std::fs::create_dir_all(&dir).unwrap();
    for section in p.sections() {
        go_to(&mut p, section);
        for (w, h) in [(50, 20), (80, 24), (120, 40), (200, 60)] {
            let text = lines(&draw(&mut p, w, h)).join("\n");
            let name = format!("settings-{}-{w}x{h}.txt", section.label());
            std::fs::write(dir.join(name), text).unwrap();
        }
    }
}

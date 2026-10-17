use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn figures_are_read_as_typed() {
    assert_eq!(parse_hz("1000"), Some(1000.0));
    assert_eq!(parse_hz("1k"), Some(1000.0));
    assert_eq!(parse_hz("1.5 kHz"), Some(1500.0));
    assert_eq!(parse_hz("200hz"), Some(200.0));
    assert_eq!(parse_hz("loud"), None);
    assert_eq!(parse_number("-3.5", "db"), Some(-3.5));
    assert_eq!(parse_number("+2 dB", "db"), Some(2.0));
    assert_eq!(parse_number("q0.7", "q"), Some(0.7));
    assert_eq!(parse_number("inf", "q"), None);
}

#[test]
fn dropped_paths_are_split_as_a_terminal_quotes_them() {
    assert_eq!(
        parse_paths("/a/b\\ c.txt '/d/e f.wav'\n/g.zip"),
        vec![
            PathBuf::from("/a/b c.txt"),
            PathBuf::from("/d/e f.wav"),
            PathBuf::from("/g.zip"),
        ]
    );
    assert_eq!(parse_paths("file:///x.txt"), vec![PathBuf::from("/x.txt")]);
    assert!(parse_paths("  \n ").is_empty());
}

const DEVICE: &str = "Test DAC";
const RENDERER: &str = "uuid:test";

/// A page over a config of its own: an EQ of eight bands in the device's
/// filters. The guard holds the config directory for the test.
fn page() -> (
    std::sync::MutexGuard<'static, ()>,
    tempfile::TempDir,
    EqPage,
) {
    let guard = koan_core::config::SWITCHING
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    koan_core::config::set_config_dir(dir.path());
    profiles::create_eq("Warm", Starter::Warm).unwrap();
    profiles::set_tunings(DEVICE, &[("Warm".into(), true)]).unwrap();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut page = EqPage::new(tx);
    // The renderer is the output in use, so no test asks the audio system.
    page.open(Some((RENDERER.into(), "Test renderer".into())));
    page.picked = Some(DEVICE.into());
    page.request();
    settle(&mut page, DEVICE);
    (guard, dir, page)
}

/// Until the worker has caught up with every change sent, showing `device`.
fn settle(page: &mut EqPage, device: &str) {
    let start = std::time::Instant::now();
    loop {
        page.poll();
        let profile = match page.view() {
            View::Profile(name) => Some(name.clone()),
            _ => None,
        };
        let caught = page.snap.as_ref().is_some_and(|s| {
            s.seq >= page.seq && s.device.as_deref() == Some(device) && s.asked == profile
        });
        if caught {
            std::thread::sleep(std::time::Duration::from_millis(30));
            page.poll();
            if page.worker.rx.is_empty() {
                return;
            }
        }
        assert!(start.elapsed().as_secs() < 10, "the worker never caught up");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn key(page: &mut EqPage, code: KeyCode) {
    page.handle_key(KeyEvent::from(code));
}

fn chars(page: &mut EqPage, text: &str) {
    for c in text.chars() {
        key(page, KeyCode::Char(c));
    }
}

/// The page drawn at each size, written under `target/eq-dumps`; the
/// 120×40 frame returned.
fn dump(page: &mut EqPage, name: &str) -> String {
    let theme = Theme::default();
    let mut shown = String::new();
    for (w, h) in [(80u16, 24u16), (120, 40), (200, 60), (50, 20)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| page.render(f.area(), f.buffer_mut(), &theme))
            .unwrap();
        let buf = terminal.backend().buffer();
        let text: String = (0..h)
            .map(|y| {
                let line: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
                line.trim_end().to_owned() + "\n"
            })
            .collect();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/eq-dumps");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{name}-{w}x{h}.txt")), &text);
        if w == 120 {
            shown = text;
        }
    }
    shown
}

fn band(name: &str, index: usize) -> koan_core::config::EqFilter {
    match &profiles::detail(name).unwrap().filters[index] {
        DspFilter::Band(b) => b.clone(),
        _ => panic!("a band"),
    }
}

fn row_of(page: &EqPage, row: Row) -> usize {
    page.row_list().iter().position(|r| *r == row).unwrap()
}

#[test]
fn the_chain_shows_the_device_its_eq_and_the_curve() {
    let (_g, _d, mut page) = page();
    let text = dump(&mut page, "chain");
    assert!(text.contains("Test DAC"), "{text}");
    assert!(text.contains("Warm"), "{text}");
    assert!(text.contains("+ add EQ"), "{text}");
    assert!(text.contains("1k"), "the frequency axis: {text}");
    assert!(text.contains("tuned with Warm"), "the sentence: {text}");
    // Switched off from the keyboard, and saved so.
    page.cursor = row_of(&page, Row::Eq(0));
    key(&mut page, KeyCode::Char(' '));
    settle(&mut page, DEVICE);
    assert_eq!(
        profiles::tunings_for(DEVICE),
        vec![("Warm".to_owned(), false)]
    );
    dump(&mut page, "chain-off");
}

#[test]
fn a_band_is_nudged_and_typed_from_the_keyboard() {
    let (_g, _d, mut page) = page();
    page.cursor = row_of(&page, Row::Eq(0));
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert_eq!(*page.view(), View::Profile("Warm".into()));
    page.cursor = row_of(&page, Row::Band(1));
    // On a band, digits, tab and the brackets are the band's, not the pages'.
    assert!(page.takes_keys());
    assert_eq!(page.field, Field::Gain);
    for _ in 0..3 {
        key(&mut page, KeyCode::Char('+'));
    }
    // Shown at once, ahead of the save.
    assert_eq!(page.figure(1, Field::Gain), "+0.3");
    settle(&mut page, DEVICE);
    assert!((band("Warm", 1).gain_db - 0.3).abs() < 1e-9);
    // The frequency, typed.
    key(&mut page, KeyCode::Left);
    assert_eq!(page.field, Field::Freq);
    chars(&mut page, "1.5k");
    assert!(page.typing());
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert_eq!(band("Warm", 1).freq, 1500.0);
    // Out of range is held to it.
    key(&mut page, KeyCode::Right);
    key(&mut page, KeyCode::Right);
    assert_eq!(page.field, Field::Q);
    chars(&mut page, "99");
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert_eq!(band("Warm", 1).q, 20.0);
    let text = dump(&mut page, "profile");
    assert!(text.contains("1500"), "{text}");
    assert!(text.contains("low shelf"), "{text}");
    // A band added and taken away.
    key(&mut page, KeyCode::Char('a'));
    settle(&mut page, DEVICE);
    assert_eq!(profiles::detail("Warm").unwrap().filters.len(), 9);
    page.cursor = row_of(&page, Row::Band(8));
    key(&mut page, KeyCode::Char('x'));
    settle(&mut page, DEVICE);
    assert_eq!(profiles::detail("Warm").unwrap().filters.len(), 8);
    // Back on the chain, on the EQ that was open.
    key(&mut page, KeyCode::Esc);
    assert_eq!(*page.view(), View::Chain);
    assert_eq!(page.row_list()[page.cursor], Row::Eq(0));
}

#[test]
fn a_held_key_saves_as_its_last_press() {
    let (_g, _d, mut page) = page();
    page.open_profile("Warm".into());
    settle(&mut page, DEVICE);
    page.cursor = row_of(&page, Row::Band(2));
    for _ in 0..40 {
        key(&mut page, KeyCode::Char(']'));
    }
    assert_eq!(page.figure(2, Field::Gain), "+30.0", "held to the range");
    settle(&mut page, DEVICE);
    assert_eq!(band("Warm", 2).gain_db, 30.0);
    assert!(page.local.is_none(), "the read caught up");
}

#[test]
fn eqs_are_made_renamed_and_deleted() {
    let (_g, _d, mut page) = page();
    key(&mut page, KeyCode::Char('n'));
    chars(&mut page, "Desk");
    key(&mut page, KeyCode::Enter);
    // Then what it starts from.
    assert!(page.choice.is_some());
    dump(&mut page, "starter");
    key(&mut page, KeyCode::Down);
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert_eq!(*page.view(), View::Profile("Desk".into()));
    assert_eq!(
        profiles::tunings_for(DEVICE),
        vec![("Warm".to_owned(), true), ("Desk".to_owned(), true)]
    );
    key(&mut page, KeyCode::Char('r'));
    key(&mut page, KeyCode::Char('2'));
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert!(profiles::detail("Desk2").is_some());
    assert_eq!(*page.view(), View::Profile("Desk2".into()));
    key(&mut page, KeyCode::Char('D'));
    dump(&mut page, "delete");
    key(&mut page, KeyCode::Down);
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert!(profiles::detail("Desk2").is_none());
    assert_eq!(*page.view(), View::Chain);
    key(&mut page, KeyCode::Char('m'));
    settle(&mut page, DEVICE);
    let text = dump(&mut page, "manage");
    assert!(text.contains("Warm"), "{text}");
}

#[test]
fn a_file_is_imported_and_asked_about() {
    let (_g, dir, mut page) = page();
    let file = dir.path().join("hd650 ParametricEQ.txt");
    std::fs::write(
        &file,
        "Preamp: -6.0 dB\nFilter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70\nFilter 2: ON PK Fc 2000 Hz Gain -2.0 dB Q 1.00\n",
    )
    .unwrap();
    // Dropped on the page, as a terminal quotes it.
    page.paste(&file.display().to_string().replace(' ', "\\ "));
    assert!(page.prompt.is_some());
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    let c = page.choice.as_ref().expect("asked what it is for");
    assert!(c.title.contains("what is this EQ"), "{}", c.title);
    dump(&mut page, "role");
    // A correction: it becomes the device's correction.
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    let o = &page.snap.as_ref().unwrap().overview;
    assert_eq!(o.active.as_deref(), Some("hd650"));
    let text = dump(&mut page, "corrected");
    assert!(text.contains("hd650"), "{text}");
}

#[test]
fn presets_are_saved_chosen_and_marked_edited() {
    let (_g, _d, mut page) = page();
    page.ask_save_preset();
    chars(&mut page, "Desk");
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert_eq!(profiles::preset_for(DEVICE), Some(("Desk".into(), false)));
    page.toggle_eq(0);
    settle(&mut page, DEVICE);
    assert_eq!(profiles::preset_for(DEVICE), Some(("Desk".into(), true)));
    let text = dump(&mut page, "edited");
    assert!(text.contains("(edited)"), "{text}");
    // Flat, from the preset choice, then back.
    page.choose_preset();
    key(&mut page, KeyCode::Home);
    key(&mut page, KeyCode::Enter);
    settle(&mut page, DEVICE);
    assert!(profiles::tunings_for(DEVICE).is_empty());
    page.choose_preset();
    let at = page
        .choice
        .as_ref()
        .unwrap()
        .items
        .iter()
        .position(|i| i.pick == Pick::Preset(Some("Desk".into())))
        .unwrap();
    page.pick(at);
    settle(&mut page, DEVICE);
    assert_eq!(
        profiles::tunings_for(DEVICE),
        vec![("Warm".to_owned(), true)]
    );
}

#[test]
fn bypass_keeps_what_played_as_a_preset_and_puts_it_back() {
    let (_g, _d, mut page) = page();
    profiles::set_tunings(RENDERER, &[("Warm".into(), true)]).unwrap();
    page.picked = None;
    page.request();
    settle(&mut page, RENDERER);
    assert_eq!(page.bypass(), None);
    settle(&mut page, RENDERER);
    assert!(profiles::tunings_for(RENDERER).is_empty(), "flat");
    assert_eq!(profiles::preset_for(RENDERER), None);
    assert_eq!(page.bypass(), None);
    settle(&mut page, RENDERER);
    assert_eq!(
        profiles::tunings_for(RENDERER),
        vec![("Warm".to_owned(), true)]
    );
    assert_eq!(
        profiles::preset_for(RENDERER),
        Some(("Test renderer".into(), false))
    );
}

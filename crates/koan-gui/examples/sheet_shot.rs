//! Draws each sheet the app shows over its window offscreen, dark and light
//! side by side, a PNG per sheet: the search box, the shortcuts, the artwork
//! viewer, organise, a confirmation and a name to give. What the walk cannot
//! reach by a route. No window is shown and nothing on the screen is read.
//!
//! It opens a real engine on a throwaway `KOAN_CONFIG_DIR` (the walk's
//! library is the one meant: `just gui-walk` makes it), and refuses to run
//! without one. `KOAN_SHEETS` names the sheets to draw, by name, comma
//! separated; all of them otherwise.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example sheet_shot -- target/sheets

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use koan_gui::components::sheet;
use koan_gui::platform::actions::{Search, ShowShortcuts};
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::{App, AppContext, Keystroke, SharedString, Window};
use koan_runtime::KoanEngine;
// GPUI's start-up and headless rendering, which only the app's entry and tools reach.
use koan_kit::raw::{HeadlessAppContext, px, size};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

/// What a sheet is opened over a window by: the album and its tracks to
/// show it with.
struct Record {
    album: i64,
    title: String,
    tracks: Vec<i64>,
}

type Open = fn(&Record, &mut Window, &mut App);

const SHEETS: [(&str, Open); 6] = [
    ("search", |_, window, cx| {
        window.dispatch_action(Box::new(Search), cx)
    }),
    ("shortcuts", |_, window, cx| {
        window.dispatch_action(Box::new(ShowShortcuts), cx)
    }),
    ("artwork", |record, window, cx| {
        koan_gui::artwork::show(
            record.album,
            record.title.clone(),
            Some("Artist".into()),
            window,
            cx,
        )
    }),
    ("organise", |record, window, cx| {
        koan_gui::organize::open(record.title.clone(), record.tracks.clone(), window, cx)
    }),
    ("confirm", |_, window, cx| {
        sheet::confirm(
            sheet::Confirm {
                title: "clear the queue".into(),
                verb: "clear".into(),
                destructive: true,
                lines: vec![SharedString::from(
                    "600 tracks come off the queue. Undo puts them back.",
                )],
                note: Some("Saving it as a playlist first keeps it.".into()),
            },
            window,
            cx,
            |_| {},
        )
    }),
    ("name", |_, window, cx| {
        sheet::name(
            "new playlist",
            "create",
            "Late night",
            |name| vec![format!("A playlist called {name}, empty.").into()],
            window,
            cx,
            |_, _| {},
        )
    }),
];

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sheets".into()),
    );
    std::fs::create_dir_all(&out)?;
    let only: Option<Vec<String>> = std::env::var("KOAN_SHEETS")
        .ok()
        .map(|names| names.split(',').map(str::to_owned).collect());

    // A screenshot is no reason to announce a device on the network or take
    // the devices port from a koan already running here.
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    let picked = futures::executor::block_on(engine.clone().random_tracks(200, None))?;
    let track = picked
        .first()
        .ok_or("the configuration's library has no tracks: run `just gui-walk` first")?;
    let album = track.album_id.ok_or("the track has no record")?;
    // The record's tracks among those picked: what organise is opened on.
    let tracks = picked
        .iter()
        .filter(|t| t.album_id == Some(album))
        .map(|t| t.id)
        .collect();
    let record = Record {
        album,
        title: track.album_title.clone(),
        tracks,
    };

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
        koan_kit::raw::init(cx);
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
    });
    cx.allow_parking();

    for (name, open) in SHEETS {
        if only
            .as_ref()
            .is_some_and(|only| !only.iter().any(|o| o == name))
        {
            continue;
        }
        let mut shots = Vec::new();
        for dark in [true, false] {
            let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
                let shell = cx.new(|cx| Shell::new(window, cx));
                let root = cx.new(|cx| WindowRoot::new(shell, window, cx));
                // gpui-component's root, which the kit's sheets and fields draw in.
                cx.new(|cx| koan_kit::raw::base::Root::new(root, window, cx))
            })?;
            let window = window.into();
            cx.update_window(window, |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx)
            })?;
            settle(&mut cx, window)?;
            cx.update_window(window, |_, window, cx| open(&record, window, cx))?;
            settle(&mut cx, window)?;
            shots.push(cx.capture_screenshot(window)?);
            // Closed as a person closes it, before the next: the app's sheets
            // outlive a window. The search box takes Escape twice, once to
            // clear its query.
            for _ in 0..3 {
                cx.update_window(window, |_, window, cx| {
                    if let Ok(escape) = Keystroke::parse("escape") {
                        window.dispatch_keystroke(escape, cx);
                    }
                })?;
                cx.run_until_parked();
            }
            cx.update_window(window, |_, window, _| window.remove_window())?;
        }
        let (w, h) = (shots[0].width(), shots[0].height());
        let mut sheet = image::RgbaImage::new(w * 2, h);
        for (index, shot) in shots.iter().enumerate() {
            image::imageops::replace(&mut sheet, shot, (index as u32 * w) as i64, 0);
        }
        let file = out.join(format!("sheet-{name}.png"));
        sheet.save(&file)?;
        println!("{}", file.display());
    }
    Ok(())
}

/// Lets what arrives from the background land: the page, covers, the wash.
fn settle(
    cx: &mut HeadlessAppContext,
    window: koan_kit::app::AnyWindowHandle,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..3 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(400));
        cx.update_window(window, |_, window, _| window.refresh())?;
    }
    cx.run_until_parked();
    Ok(())
}

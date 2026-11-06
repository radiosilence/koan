//! Walks the phone shell headlessly at an iPhone's size, under touch, and
//! saves a PNG of each page, dark and light: the tabs' first pages, the
//! library's pages, a record, an artist, a playlist, each Settings section and
//! Now Playing, and each page again scrolled part way, its rows under the
//! bar, and to its end, its last row clear of the foot. Nothing is shown and nothing on the screen is read: GPUI's
//! headless renderer draws it, as `walk` does for the desktop shell.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`, whose library folders it scans first.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example phone_walk -- target/phone-walk
//! `KOAN_WALK_PAGES=library,now-playing` walks only those, and
//! `KOAN_WALK_WIDTH=375` walks a narrower phone, an iPhone mini.

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use koan_gui::mobile::PhoneShell;
use koan_gui::navigator::Route;
use koan_gui::{app, root::WindowRoot, theme};
use koan_kit::app::{AppContext, Entity, Modifiers, ScrollEvent};
use koan_runtime::KoanEngine;
// GPUI's start-up, headless rendering and synthetic input, which only the
// app's entry and tools reach.
use koan_kit::raw::{
    AnyWindowHandle, HeadlessAppContext, PlatformInput, ScrollDelta, ScrollSource, TouchPhase,
    point, px, size,
};

/// An iPhone 17 Pro, in points.
const WIDTH: f32 = 402.0;

/// The width walked: an iPhone 17 Pro's, or `KOAN_WALK_WIDTH`'s.
fn width() -> f32 {
    std::env::var("KOAN_WALK_WIDTH")
        .ok()
        .and_then(|w| w.parse().ok())
        .unwrap_or(WIDTH)
}
const HEIGHT: f32 = 874.0;

enum Step {
    Page(Route),
    NowPlaying,
}

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/phone-walk".into()),
    );
    std::fs::create_dir_all(&out)?;
    let only: Option<Vec<String>> = std::env::var("KOAN_WALK_PAGES")
        .ok()
        .map(|pages| pages.split(',').map(str::to_owned).collect());
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
        std::env::set_var("KOAN_GUI_PHONE", "1");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    let steps = prepare(&engine)?;

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
        theme::KoanTheme::set_touch(true, cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
    });

    let mut shell = None;
    let window = cx.open_window(size(px(width()), px(HEIGHT)), |window, cx| {
        let made = cx.new(|cx| PhoneShell::new(window, cx));
        shell = Some(made.clone());
        let root = cx.new(|cx| WindowRoot::new(made, window, cx));
        koan_kit::components::host(root, window, cx)
    })?;
    let window: AnyWindowHandle = window.into();
    let shell: Entity<PhoneShell> = shell.ok_or("the shell was not made")?;
    cx.allow_parking();
    settle(&mut cx, window)?;

    for (name, step) in steps {
        if only
            .as_ref()
            .is_some_and(|only| !only.iter().any(|o| name.starts_with(o.as_str())))
        {
            continue;
        }
        let page = matches!(step, Step::Page(_));
        match step {
            Step::Page(route) => cx.update(|cx| shell.update(cx, |s, cx| s.show(route, cx))),
            Step::NowPlaying => cx.update_window(window, |_, window, cx| {
                shell.update(cx, |s, cx| s.open_now_playing(window, cx))
            })?,
        }
        for dark in [true, false] {
            cx.update_window(window, |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx)
            })?;
            settle(&mut cx, window)?;
            let file = out.join(format!(
                "phone-{name}-{}.png",
                if dark { "dark" } else { "light" }
            ));
            cx.capture_screenshot(window)?.save(&file)?;
        }
        if page {
            scroll(&mut cx, window, SCROLLED)?;
            settle(&mut cx, window)?;
            let file = out.join(format!("phone-{name}-scrolled.png"));
            cx.capture_screenshot(window)?.save(&file)?;
            scroll(&mut cx, window, -SCROLLED)?;
            // The far end, its last row clear of the foot.
            scroll(&mut cx, window, TO_END)?;
            settle(&mut cx, window)?;
            let file = out.join(format!("phone-{name}-end.png"));
            cx.capture_screenshot(window)?.save(&file)?;
            scroll(&mut cx, window, -TO_END)?;
        }
    }
    println!("PNGs in {}", out.display());
    Ok(())
}

/// Scans the configured folders and fills a queue and a playlist, so the
/// pages have something to show. Returns the steps, by name.
fn prepare(engine: &std::sync::Arc<KoanEngine>) -> Result<Vec<(String, Step)>, Box<dyn Error>> {
    use futures::executor::block_on;
    block_on(engine.clone().scan_reporting(false, None))?;
    let tracks = block_on(engine.clone().random_tracks(200, None))?;
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let page = |name: &str, route| (name.to_owned(), Step::Page(route));
    let mut steps = vec![
        page("queue", Route::Queue),
        page("library", Route::Library),
        page("albums", Route::Albums),
        page("artists", Route::Artists),
        page("favourites", Route::Favourites),
        page("recent", Route::RecentlyPlayed),
        page("history", Route::History),
        page("downloads", Route::Downloads),
        page("playlists", Route::Playlists),
        page("search", Route::SearchTab),
        page("settings", Route::Settings),
    ];
    // Each Settings section a phone offers; those that need a signed-in
    // server show what they show without one.
    {
        use koan_gui::settings::Tab;
        for (name, tab) in [
            ("server", Tab::Server),
            ("account", Tab::Account),
            ("people", Tab::People),
            ("playback", Tab::Playback),
            ("devices", Tab::Devices),
            ("integrations", Tab::Integrations),
            ("appearance", Tab::Appearance),
        ] {
            steps.push(page(
                &format!("settings-{name}"),
                Route::SettingsSection(tab),
            ));
        }
    }
    if !ids.is_empty() {
        block_on(engine.clone().replace_queue(ids.clone(), None, false))?;
        block_on(engine.clone().pause())?;
        let playlist = block_on(engine.clone().create_playlist("Walk".into(), ids))?;
        steps.push(page("playlist", Route::Playlist(playlist.id)));
    }
    if let Some(album) = tracks.iter().find_map(|t| t.album_id) {
        steps.push(page("album", Route::Album(album)));
    }
    if let Some(artist) = tracks.iter().find_map(|t| t.artist_id) {
        steps.push(page("artist", Route::Artist(artist)));
    }
    steps.push(("now-playing".into(), Step::NowPlaying));
    Ok(steps)
}

/// How far a page is scrolled for its second picture: its first rows under
/// the bar.
const SCROLLED: f32 = 160.0;

/// Further than any page runs, for its picture at the end.
const TO_END: f32 = 1_000_000.0;

/// Scrolls what is at the screen's middle down by `by` points, as a drag
/// would.
fn scroll(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    by: f32,
) -> Result<(), Box<dyn Error>> {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: point(px(width() / 2.), px(HEIGHT / 2.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-by))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            }),
            cx,
        );
    })?;
    Ok(())
}

/// Lets what arrives from the background land: rows, covers, the accent.
fn settle(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Result<(), Box<dyn Error>> {
    for _ in 0..3 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(400));
        cx.update_window(window, |_, window, _| window.refresh())?;
    }
    cx.run_until_parked();
    Ok(())
}

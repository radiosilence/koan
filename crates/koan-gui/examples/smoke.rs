//! A smoke run of the app as it is used: the real engine against a
//! throwaway library, playing, the pointer moving over every page while the
//! window redraws, the wheel scrolling it, and the page changing under the
//! pointer, with the rainbow on for the pages. Exits non-zero on a panic
//! anywhere in the process, which is what it is for: crashes that only
//! drawing, hovering and playing reach.
//!
//! Headless, as `walk` is: no window is shown and nothing on the screen is
//! read. It refuses to run without a throwaway `KOAN_CONFIG_DIR`. A library
//! from `just gui-library` is silent, so playing it makes no sound.
//!
//! KOAN_CONFIG_DIR=… cargo run --release -p koan-gui --example smoke
//! `KOAN_SMOKE_SECS` (default 30) sets how long the pointer churns over the
//! queue while it plays.

use std::error::Error;
use std::time::{Duration, Instant};

use koan_ffi::KoanEngine;
use koan_gui::navigator::Route;
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::{AppContext, Entity, Modifiers, MouseMoveEvent, ScrollEvent};
// GPUI's start-up, headless rendering and synthetic input, which only the
// app's entry and tools reach.
use koan_kit::raw::{
    AnyWindowHandle, HeadlessAppContext, PlatformInput, ScrollDelta, ScrollSource, TouchPhase,
    point, px, size,
};

const WIDTH: f32 = 1200.0;
const HEIGHT: f32 = 750.0;

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let churn = Duration::from_secs(
        std::env::var("KOAN_SMOKE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30),
    );
    // A panic on any thread, the engine's included, fails the run.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        std::process::exit(101);
    }));
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }

    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    let pages = prepare(&engine)?;

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        koan_kit::raw::init(cx);
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
    });
    let mut shell = None;
    let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
        let made = cx.new(|cx| Shell::new(window, cx));
        shell = Some(made.clone());
        let root = cx.new(|cx| WindowRoot::new(made, window, cx));
        cx.new(|cx| koan_kit::raw::base::Root::new(root, window, cx))
    })?;
    let window: AnyWindowHandle = window.into();
    let shell: Entity<Shell> = shell.ok_or("the shell was not made")?;
    cx.allow_parking();

    // The queue playing, the pointer sweeping it, for the churn's length.
    let start = Instant::now();
    let mut frames = 0usize;
    while start.elapsed() < churn {
        frames += sweep(&mut cx, window, 24)?;
        frames += scroll(&mut cx, window, 20)?;
    }
    println!(
        "queue: {frames} frames over {:.0?} while playing",
        start.elapsed()
    );

    // The rainbow on for the pages, in front, its burst fired: the prism and
    // the burst run only while it is drawn.
    cx.update(|cx| {
        koan_gui::rainbow::set_active(true, cx);
        koan_gui::rainbow::toggle(cx);
        koan_gui::rainbow::celebrate(cx);
    });

    // Every page, each swept and scrolled, then changed under the pointer.
    for (name, route) in &pages {
        cx.update(|cx| shell.update(cx, |shell, cx| shell.show(*route, cx)));
        for dark in [true, false] {
            cx.update_window(window, |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx)
            })?;
            let drawn = sweep(&mut cx, window, 16)? + scroll(&mut cx, window, 30)?;
            println!(
                "{name} ({}): {drawn} frames",
                if dark { "dark" } else { "light" }
            );
        }
    }
    for _ in 0..3 {
        for (_, route) in &pages {
            cx.update(|cx| shell.update(cx, |shell, cx| shell.show(*route, cx)));
            sweep(&mut cx, window, 2)?;
        }
    }
    println!("no panics");
    Ok(())
}

/// Scans the library, queues tracks and plays them, and lists the pages.
fn prepare(engine: &std::sync::Arc<KoanEngine>) -> Result<Vec<(String, Route)>, Box<dyn Error>> {
    use futures::executor::block_on;
    block_on(engine.clone().scan_reporting(false, None))?;
    let tracks = block_on(engine.clone().random_tracks(600, None))?;
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    if ids.is_empty() {
        return Err("the library is empty: run `just gui-library` into the config".into());
    }
    block_on(engine.clone().replace_queue(ids.clone(), Some(0), false))?;
    let playlist = block_on(engine.clone().create_playlist("Smoke".into(), ids))?;
    let mut pages = vec![
        ("queue".to_owned(), Route::Queue),
        ("albums".to_owned(), Route::Albums),
        ("artists".to_owned(), Route::Artists),
        ("tracks".to_owned(), Route::Tracks),
        ("favourites".to_owned(), Route::Favourites),
        ("recent".to_owned(), Route::RecentlyPlayed),
        ("history".to_owned(), Route::History),
        ("downloads".to_owned(), Route::Downloads),
        ("playlist".to_owned(), Route::Playlist(playlist.id)),
    ];
    if let Some(album) = tracks.iter().find_map(|t| t.album_id) {
        pages.push(("album".to_owned(), Route::Album(album)));
    }
    if let Some(artist) = tracks.iter().find_map(|t| t.artist_id) {
        pages.push(("artist".to_owned(), Route::Artist(artist)));
    }
    Ok(pages)
}

/// The pointer moved over a grid of `steps` × `steps` points across the
/// window, a frame drawn after each move.
fn sweep(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    steps: usize,
) -> Result<usize, Box<dyn Error>> {
    let mut frames = 0;
    for row in 0..steps {
        for col in 0..steps {
            // Rows alternate direction, as a hand would move.
            let col = if row % 2 == 0 { col } else { steps - 1 - col };
            let at = point(
                px(WIDTH * (col as f32 + 0.5) / steps as f32),
                px(HEIGHT * (row as f32 + 0.5) / steps as f32),
            );
            cx.update_window(window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            })?;
            cx.run_until_parked();
            frames += 1;
        }
    }
    Ok(frames)
}

/// The wheel down and back at the window's centre, a frame after each step.
fn scroll(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    steps: usize,
) -> Result<usize, Box<dyn Error>> {
    let centre = point(px(WIDTH * 0.6), px(HEIGHT * 0.5));
    for direction in [-1.0, 1.0] {
        for _ in 0..steps {
            cx.update_window(window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position: centre,
                        delta: ScrollDelta::Pixels(point(px(0.), px(40.0 * direction))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            })?;
            cx.run_until_parked();
        }
    }
    Ok(steps * 2)
}

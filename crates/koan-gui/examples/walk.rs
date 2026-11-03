//! Walks every page of the app headlessly: for each, a PNG of the window,
//! dark and light, then a scroll through it by wheel events delivered to the
//! window, timing each frame's layout and paint. Prints each page's frame
//! times and fails when a page's 95th percentile is over the budget. No
//! window is shown and nothing on the screen is read: GPUI's headless
//! renderer draws it, as `window_shot` does.
//!
//! The PNGs are named as the Mac app's evidence renderer names its framed
//! windows (`window-albums-dark.png`, `window-album-<id>-dark.png`), at the
//! same size, so `just gui-compare` can set the two apps side by side. The
//! record, artist and playlist it walks are written to `<out>/ids.env` as
//! the renderer's `KOAN_RENDER_ALBUM`, `KOAN_RENDER_ARTIST` and
//! `KOAN_RENDER_PLAYLIST`, so the Mac draws the same ones. PNGs left from an
//! earlier walk are removed first, so none is compared out of date.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`, whose library folders it scans first. The frame times
//! are the CPU's share of a frame (layout and paint into the scene); the GPU's
//! share is the headless renderer's, not the screen's, and is left out.
//!
//! KOAN_CONFIG_DIR=… cargo run --release -p koan-gui --example walk -- target/gui-walk
//! `KOAN_WALK_BUDGET_MS` (default 6.9, a frame at 144 Hz) sets the budget;
//! `KOAN_WALK_PAGES=albums,queue` walks only those.

use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use koan_gui::navigator::Route;
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::{AppContext, Entity, Modifiers, ScrollEvent};
use koan_runtime::KoanEngine;
// GPUI's start-up, headless rendering and synthetic input, which only the
// app's entry and tools reach.
use koan_kit::raw::{
    AnyWindowHandle, HeadlessAppContext, PlatformInput, ScrollDelta, ScrollSource, TouchPhase,
    point, px, size,
};

/// The Mac evidence renderer's framed window, in points.
const WIDTH: f32 = 1200.0;
const HEIGHT: f32 = 750.0;
/// Wheel steps per direction.
const STEPS: usize = 120;
const STEP: f32 = 40.0;

struct Page {
    name: String,
    route: Route,
}

struct Timing {
    name: String,
    frames: Vec<Duration>,
}

impl Timing {
    fn percentile(&self, p: f64) -> Duration {
        let mut sorted = self.frames.clone();
        sorted.sort_unstable();
        let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted.get(at).copied().unwrap_or_default()
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/gui-walk".into()),
    );
    std::fs::create_dir_all(&out)?;
    for entry in std::fs::read_dir(&out)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("window-") && name.ends_with(".png") {
            std::fs::remove_file(entry.path())?;
        }
    }
    let budget = Duration::from_secs_f64(
        std::env::var("KOAN_WALK_BUDGET_MS")
            .ok()
            .and_then(|ms| ms.parse::<f64>().ok())
            .unwrap_or(6.9)
            / 1000.0,
    );
    let only: Option<Vec<String>> = std::env::var("KOAN_WALK_PAGES")
        .ok()
        .map(|pages| pages.split(',').map(str::to_owned).collect());

    // A walk is no reason to announce a device on the network or take the
    // devices port from a koan already running here.
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    let pages = prepare(&engine, &out)?;

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

    let mut timings = Vec::new();
    for page in pages {
        if only
            .as_ref()
            .is_some_and(|only| !only.iter().any(|o| page.name.starts_with(o.as_str())))
        {
            continue;
        }
        cx.update(|cx| shell.update(cx, |shell, cx| shell.show(page.route, cx)));
        for dark in [true, false] {
            cx.update_window(window, |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx)
            })?;
            settle(&mut cx, window)?;
            let file = out.join(format!(
                "window-{}-{}.png",
                page.name,
                if dark { "dark" } else { "light" }
            ));
            cx.capture_screenshot(window)?.save(&file)?;
        }
        timings.push(Timing {
            name: page.name.clone(),
            frames: scroll(&mut cx, window)?,
        });
    }

    println!("page            frames   p50 ms   p95 ms   max ms");
    let mut over = Vec::new();
    for t in &timings {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let p95 = t.percentile(0.95);
        println!(
            "{:<14} {:>7} {:>8.2} {:>8.2} {:>8.2}{}",
            t.name,
            t.frames.len(),
            ms(t.percentile(0.5)),
            ms(p95),
            ms(t.percentile(1.0)),
            if p95 > budget { "  over budget" } else { "" }
        );
        if p95 > budget {
            over.push(t.name.clone());
        }
    }
    println!("PNGs in {}", out.display());
    if !over.is_empty() {
        return Err(format!(
            "over the {:.1} ms budget at the 95th percentile: {}",
            budget.as_secs_f64() * 1000.0,
            over.join(", ")
        )
        .into());
    }
    Ok(())
}

/// Scans the configured folders, and makes what the pages need to show
/// something: a queue and a playlist. Returns every page, the ones that need
/// a record, an artist or a playlist with theirs, whose ids go to
/// `<out>/ids.env` for the Mac's renderer.
fn prepare(
    engine: &std::sync::Arc<KoanEngine>,
    out: &std::path::Path,
) -> Result<Vec<Page>, Box<dyn Error>> {
    use futures::executor::block_on;
    block_on(engine.clone().scan_reporting(false, None))?;
    let tracks = block_on(engine.clone().random_tracks(600, None))?;
    let mut ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    ids.sort_unstable();
    let album = tracks.iter().find_map(|t| t.album_id);
    let artist = tracks.iter().find_map(|t| t.artist_id);
    let mut pages = vec![
        Page {
            name: "queue".into(),
            route: Route::Queue,
        },
        Page {
            name: "albums".into(),
            route: Route::Albums,
        },
        Page {
            name: "artists".into(),
            route: Route::Artists,
        },
        Page {
            name: "tracks".into(),
            route: Route::Tracks,
        },
        Page {
            name: "favourites".into(),
            route: Route::Favourites,
        },
        Page {
            name: "recent".into(),
            route: Route::RecentlyPlayed,
        },
        Page {
            name: "history".into(),
            route: Route::History,
        },
        Page {
            name: "downloads".into(),
            route: Route::Downloads,
        },
    ];
    let mut env = String::new();
    if !ids.is_empty() {
        block_on(engine.clone().replace_queue(ids.clone(), None, false))?;
        block_on(engine.clone().pause())?;
        let playlist = block_on(engine.clone().create_playlist("Walk".into(), ids))?;
        env.push_str(&format!("KOAN_RENDER_PLAYLIST={}\n", playlist.id));
        pages.push(Page {
            name: "playlist".into(),
            route: Route::Playlist(playlist.id),
        });
    }
    if let Some(album) = album {
        env.push_str(&format!("KOAN_RENDER_ALBUM={album}\n"));
        pages.push(Page {
            name: format!("album-{album}"),
            route: Route::Album(album),
        });
    }
    if let Some(artist) = artist {
        env.push_str(&format!("KOAN_RENDER_ARTIST={artist}\n"));
        pages.push(Page {
            name: "artist".into(),
            route: Route::Artist(artist),
        });
    }
    std::fs::write(out.join("ids.env"), env)?;
    Ok(pages)
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

/// Scrolls the page down and back with wheel events at the window's centre,
/// timing each frame drawn after one.
fn scroll(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
) -> Result<Vec<Duration>, Box<dyn Error>> {
    let centre = point(px(WIDTH * 0.6), px(HEIGHT * 0.5));
    let mut frames = Vec::with_capacity(STEPS * 2);
    for direction in [-1.0, 1.0] {
        for _ in 0..STEPS {
            let took = cx.update_window(window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position: centre,
                        delta: ScrollDelta::Pixels(point(px(0.), px(STEP * direction))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    }),
                    cx,
                );
                let start = Instant::now();
                window.draw(cx).clear(cx);
                start.elapsed()
            })?;
            frames.push(took);
            cx.run_until_parked();
        }
    }
    Ok(frames)
}

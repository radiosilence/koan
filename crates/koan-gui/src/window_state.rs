//! The window's place and size, and how it was laid out, kept across
//! launches.
//!
//! Platform state rather than configuration, as an `NSWindow` frame is to the
//! Mac app, so it lives in a file of its own beside `config.toml`: one line,
//! `x y width height maximized sidebar sidebar-width grouped lyrics-width`. It is read
//! and written off the main thread, but for the last write as the app quits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use koan_kit::app::*;

pub const FILE: &str = "desktop-window";

/// Whether the sidebar shows. Read as the shell opens; written with the
/// bounds.
pub static SIDEBAR: AtomicBool = AtomicBool::new(true);
/// The sidebar's width in points, as an `f32`'s bits.
pub static SIDEBAR_WIDTH: AtomicU32 = AtomicU32::new(f32::to_bits(220.0));

/// The lyrics panel's width in points, as an `f32`'s bits; nothing (0)
/// until the person drags it, so it follows the text size until then.
pub static LYRICS_WIDTH: AtomicU32 = AtomicU32::new(0);

/// Whether the queue is laid out under record headings ("group by album").
/// Read where the queue is laid out, off the main thread.
pub static GROUPED: AtomicBool = AtomicBool::new(true);

fn path() -> PathBuf {
    koan_core::places::state().join(FILE)
}

/// What the file holds: the bounds and whether the window was maximized.
#[derive(Clone, Copy)]
pub struct Saved {
    bounds: Bounds<Pixels>,
    maximized: bool,
}

/// Reads the file. For the background executor.
pub fn read() -> Option<Saved> {
    let text = std::fs::read_to_string(path()).ok()?;
    let mut fields = text.split_whitespace();
    let mut next = || fields.next()?.parse::<f32>().ok();
    let (x, y, w, h) = (next()?, next()?, next()?, next()?);
    let maximized = next().is_some_and(|m| m != 0.0);
    // Absent from a file an older build wrote: shown, and grouped.
    SIDEBAR.store(next().is_none_or(|on| on != 0.0), Ordering::Relaxed);
    if let Some(width) = next() {
        SIDEBAR_WIDTH.store(width.to_bits(), Ordering::Relaxed);
    }
    GROUPED.store(next().is_none_or(|on| on != 0.0), Ordering::Relaxed);
    if let Some(width) = next() {
        LYRICS_WIDTH.store(width.to_bits(), Ordering::Relaxed);
    }
    Some(Saved {
        bounds: Bounds::new(
            Point {
                x: Pixels::from(x),
                y: Pixels::from(y),
            },
            Size {
                width: Pixels::from(w),
                height: Pixels::from(h),
            },
        ),
        maximized,
    })
}

/// The saved bounds where they still fit on a display, or the saved size
/// centred.
pub fn place(saved: Saved, cx: &App) -> WindowBounds {
    // Wayland neither reports nor sets a position, so on Linux the window is
    // centred at its saved size.
    let on_a_display = !cfg!(target_os = "linux")
        && cx
            .displays()
            .iter()
            .any(|display| display.bounds().intersects(&saved.bounds));
    let bounds = if on_a_display {
        saved.bounds
    } else {
        Bounds::centered(None, saved.bounds.size, cx)
    };
    if saved.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

/// Writes the window's bounds on the background executor.
pub fn save(bounds: WindowBounds, cx: &App) {
    cx.background_executor()
        .spawn(async move { write(bounds) })
        .detach();
}

/// Writes the window's bounds now: for the app quitting, which waits for
/// nothing spawned.
pub fn write(bounds: WindowBounds) {
    let (b, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, 0),
        // A full-screen window opens windowed at its restored size.
        WindowBounds::Fullscreen(b) => (b, 0),
        WindowBounds::Maximized(b) => (b, 1),
    };
    let line = format!(
        "{} {} {} {} {maximized} {} {} {} {}\n",
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
        u8::from(SIDEBAR.load(Ordering::Relaxed)),
        f32::from_bits(SIDEBAR_WIDTH.load(Ordering::Relaxed)),
        u8::from(GROUPED.load(Ordering::Relaxed)),
        f32::from_bits(LYRICS_WIDTH.load(Ordering::Relaxed)),
    );
    if let Err(e) = std::fs::write(path(), line) {
        log::info!("window state: not saved: {e}");
    }
}

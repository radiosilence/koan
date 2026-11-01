//! The window's place and size, kept across launches.
//!
//! Platform state rather than configuration, as an `NSWindow` frame is to the
//! Mac app, so it lives in a file of its own beside `config.toml`: one line,
//! `x y width height maximized`.

use std::path::PathBuf;

use gpui_kit::*;

const FILE: &str = "desktop-window";

fn path() -> PathBuf {
    koan_ffi::config_dir().join(FILE)
}

/// The bounds last saved, where they still fit on a display. Wayland neither
/// reports nor sets a window's position: the origin saved there is zero and
/// the compositor places the window, so only the size carries over.
pub fn restore(cx: &App) -> Option<WindowBounds> {
    let text = std::fs::read_to_string(path()).ok()?;
    let mut fields = text.split_whitespace();
    let mut next = || fields.next()?.parse::<f32>().ok();
    let (x, y, w, h) = (next()?, next()?, next()?, next()?);
    let maximized = next().is_some_and(|m| m != 0.0);
    let size = size(px(w), px(h));
    let saved = Bounds::new(point(px(x), px(y)), size);
    let on_a_display = cx
        .displays()
        .iter()
        .any(|display| display.bounds().intersects(&saved));
    let bounds = if on_a_display {
        saved
    } else {
        Bounds::centered(None, size, cx)
    };
    Some(if maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    })
}

pub fn save(bounds: WindowBounds) {
    let (b, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, 0),
        // A full-screen window opens windowed at its restored size.
        WindowBounds::Fullscreen(b) => (b, 0),
        WindowBounds::Maximized(b) => (b, 1),
    };
    let line = format!(
        "{} {} {} {} {maximized}\n",
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
    );
    if let Err(e) = std::fs::write(path(), line) {
        log::info!("window state: not saved: {e}");
    }
}

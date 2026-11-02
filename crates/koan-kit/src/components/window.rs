//! A window of the app's own beside its main one (Settings, EQ, About):
//! titled, centred at its size in points, with a least size or none.

use gpui_kit::*;

pub struct SecondaryWindow {
    title: SharedString,
    app_id: SharedString,
    size: (f32, f32),
    min: Option<(f32, f32)>,
    resizable: bool,
    /// The content runs under the title bar, which the window draws as a
    /// strip of its own (`strip_height`).
    transparent: bool,
}

impl SecondaryWindow {
    /// `title` in its own case, as the title bar shows it; `app_id` names
    /// the app to the desktop on Linux.
    pub fn new(title: impl Into<SharedString>, app_id: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            app_id: app_id.into(),
            size: (480.0, 360.0),
            min: None,
            resizable: true,
            transparent: false,
        }
    }

    /// Its size in points as it opens.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = (width, height);
        self
    }

    /// The least it can be resized to, in points.
    pub fn min(mut self, width: f32, height: f32) -> Self {
        self.min = Some((width, height));
        self
    }

    /// Opens at its size and keeps it.
    pub fn fixed(mut self) -> Self {
        self.resizable = false;
        self
    }

    /// The content runs to the top edge, under the title bar's buttons on
    /// macOS; the window draws its top `strip_height` itself.
    pub fn transparent_titlebar(mut self) -> Self {
        self.transparent = true;
        self
    }

    /// The strip a window with a transparent title bar keeps along its top:
    /// the height of the title bar it replaces.
    pub fn strip_height() -> Pixels {
        px(if cfg!(target_os = "macos") {
            30.0
        } else {
            28.0
        })
    }

    pub fn options(self, cx: &App) -> WindowOptions {
        let points = |(w, h): (f32, f32)| size(px(w), px(h));
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                points(self.size),
                cx,
            ))),
            window_min_size: self.min.map(points),
            titlebar: Some(TitlebarOptions {
                title: Some(self.title),
                appears_transparent: self.transparent,
                ..Default::default()
            }),
            is_resizable: self.resizable,
            app_id: Some(self.app_id.to_string()),
            ..Default::default()
        }
    }
}

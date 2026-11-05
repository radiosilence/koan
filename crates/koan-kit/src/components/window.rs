//! The app's windows: the main one (`MainWindow`), and those of its own
//! beside it (Settings, EQ, About), titled, centred at their size in points,
//! with a least size or none.

use gpui::prelude::*;
use gpui::*;

use super::ToolbarButton;
use crate::theme::Icon;

/// What a window's content sits in (`Root`): Tab and Shift-Tab kept inside
/// an open sheet or menu. Every window and offscreen render goes through
/// here.
pub fn host(view: impl Into<AnyView>, window: &mut Window, cx: &mut App) -> Entity<super::Root> {
    // koan-kit's fields draw no caret or selection where the platform draws
    // them, so the platform's own text editing is asked for where it has it
    // (iOS: UIKit's handles, loupe and edit menu).
    window.set_native_text_interaction(true);
    cx.new(|_| super::Root::new(view))
}

/// Opens a window with `build`'s view under `host`, handing back both.
fn open_hosted<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<(AnyWindowHandle, Entity<V>)> {
    let mut built = None;
    let window = cx.open_window(options, |window, cx| {
        let view = build(window, cx);
        built = Some(view.clone());
        host(view, window, cx)
    })?;
    Ok((window.into(), built.expect("the window ran its build")))
}

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

    /// Opens it with `build`'s view as its content, under the root koan-kit's
    /// sheets, menus and fields draw in.
    pub fn open<V: Render>(
        self,
        cx: &mut App,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Result<(AnyWindowHandle, Entity<V>)> {
        let options = self.options(cx);
        open_hosted(options, cx, build)
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

/// The app's main window: at the place it was left in, or at its size
/// centred; on macOS its content runs under the title bar, the window's
/// buttons standing where the app's toolbar keeps room for them, and the app
/// drags the window by its own toolbar.
pub struct MainWindow {
    title: SharedString,
    app_id: SharedString,
    size: (f32, f32),
    min: Option<Size<Pixels>>,
    place: Option<WindowBounds>,
    buttons: Option<Point<Pixels>>,
}

impl MainWindow {
    pub fn new(title: impl Into<SharedString>, app_id: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            app_id: app_id.into(),
            size: (1180.0, 760.0),
            min: None,
            place: None,
            buttons: None,
        }
    }

    /// Its size in points when it has no place of its own yet.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = (width, height);
        self
    }

    pub fn min(mut self, min: Size<Pixels>) -> Self {
        self.min = Some(min);
        self
    }

    /// Where it was when last closed, put back on a display.
    pub fn place(mut self, place: Option<WindowBounds>) -> Self {
        self.place = place;
        self
    }

    /// Where the window's buttons stand on macOS: on the toolbar's ground.
    pub fn buttons_at(mut self, origin: Point<Pixels>) -> Self {
        self.buttons = Some(origin);
        self
    }

    /// Opens it with `build`'s view as its content, under the root koan-kit's
    /// sheets, menus and fields draw in.
    pub fn open<V: Render>(
        self,
        cx: &mut App,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Result<AnyWindowHandle> {
        let (w, h) = self.size;
        let bounds = self.place.unwrap_or_else(|| {
            WindowBounds::Windowed(Bounds::centered(None, size(px(w), px(h)), cx))
        });
        let mac = cfg!(target_os = "macos");
        let options = WindowOptions {
            window_bounds: Some(bounds),
            window_min_size: self.min,
            titlebar: Some(TitlebarOptions {
                title: Some(self.title),
                appears_transparent: mac,
                traffic_light_position: self.buttons.filter(|_| mac),
            }),
            app_owns_titlebar_drag: mac,
            app_id: Some(self.app_id.to_string()),
            ..Default::default()
        };
        open_hosted(options, cx, build).map(|(handle, _)| handle)
    }
}

/// Whether the compositor leaves the window's decorations to the app (GNOME
/// on Wayland): its toolbar or top strip then carries the window's buttons.
pub fn draws_decorations(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Bare space that moves the window, as a title bar's does: dragged, it
/// moves the window; double-clicked, it zooms it as the system's title bar
/// would. `area`'s id keys whether a press is under way.
pub fn moves_window(mut area: Stateful<Div>, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    // A `Stateful` element always has its id.
    let id = area
        .interactivity()
        .element_id
        .clone()
        .expect("a stateful element has an id");
    let pressed = window.use_keyed_state(super::child(&id, "pressed"), cx, |_, _| false);
    let set = |to: bool| {
        let pressed = pressed.clone();
        move |cx: &mut App| pressed.update(cx, |p, _| *p = to)
    };
    let (down, up, out, moved) = (set(true), set(false), set(false), set(false));
    area.window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| down(cx))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| up(cx))
        .on_mouse_down_out(move |_, _, cx| out(cx))
        .on_mouse_move(move |_, window, cx| {
            if *pressed.read(cx) {
                moved(cx);
                window.start_window_move();
            }
        })
        .on_click(|event, window, _| {
            if event.click_count() != 2 {
                return;
            }
            if cfg!(target_os = "macos") {
                window.titlebar_double_click();
            } else {
                window.zoom_window();
            }
        })
}

/// The window's own buttons, for a toolbar where the app draws the
/// decorations (`draws_decorations`): minimise, maximise, and close, which
/// runs `on_close` before the window goes.
pub fn window_buttons(on_close: impl Fn(&mut Window, &mut App) + 'static) -> Vec<AnyElement> {
    vec![
        div()
            .window_control_area(WindowControlArea::Min)
            .child(
                ToolbarButton::new("minimise", Icon::Subtract, "minimise")
                    .on_click(|_, window, _| window.minimize_window()),
            )
            .into_any_element(),
        div()
            .window_control_area(WindowControlArea::Max)
            .child(
                ToolbarButton::new("maximise", Icon::Expand, "maximise")
                    .on_click(|_, window, _| window.zoom_window()),
            )
            .into_any_element(),
        close_button(on_close),
    ]
}

/// The window's close button alone, for a window of the app's own beside
/// the main one where the app draws the decorations.
pub fn close_button(on_close: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .window_control_area(WindowControlArea::Close)
        .child(
            ToolbarButton::new("close", Icon::Close, "close").on_click(move |_, window, cx| {
                on_close(window, cx);
                window.remove_window();
            }),
        )
        .into_any_element()
}

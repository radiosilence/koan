//! The window: a toolbar across the top, the sidebar beside the stage, and
//! the transport along the foot under both.

use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::engine::command;
use crate::navigator::{Navigator, Page};
use crate::queue::QueueView;
use crate::sidebar::Sidebar;
use crate::theme::{self, HAIRLINE, Palette, space, text};
use crate::transport::Transport;

actions!(koan, [GoBack, GoForward, TogglePlay]);

pub const CONTEXT: &str = "koan";

pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some(CONTEXT);
    vec![
        KeyBinding::new("space", TogglePlay, ctx),
        KeyBinding::new("secondary-[", GoBack, ctx),
        KeyBinding::new("secondary-]", GoForward, ctx),
    ]
}

const TOOLBAR: Pixels = px(38.);
const TOOLBAR_BUTTON: Pixels = px(36.);
/// Room left for the window's buttons where the toolbar runs under them.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: Pixels = px(78.);
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHTS: Pixels = px(0.);

pub struct Shell {
    nav: Navigator,
    focus: FocusHandle,
    sidebar: Entity<Sidebar>,
    /// Held for the life of the window, so the queue's selection and scroll
    /// position survive a visit to another page.
    queue: Entity<QueueView>,
    transport: Entity<Transport>,
    /// The toolbar was pressed and the next move drags the window.
    moving_window: bool,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = cx.entity();
        let queue = cx.new(QueueView::new);
        window.focus(&queue.focus_handle(cx), cx);
        Self {
            nav: Navigator::default(),
            focus: cx.focus_handle(),
            sidebar: cx.new(|cx| Sidebar::new(&shell, cx)),
            queue,
            transport: cx.new(Transport::new),
            moving_window: false,
        }
    }

    pub fn page(&self) -> Page {
        self.nav.current()
    }

    pub fn go(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.go(page) {
            self.arrived(window, cx);
        }
    }

    pub fn forget(&mut self, page: Page, cx: &mut Context<Self>) {
        self.nav.forget(page);
        cx.notify();
    }

    fn arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.current() == Page::Queue {
            window.focus(&self.queue.focus_handle(cx), cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.back() {
            self.arrived(window, cx);
        }
    }

    fn forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.forward() {
            self.arrived(window, cx);
        }
    }

    fn toggle_play(&mut self, _: &TogglePlay, _: &mut Window, cx: &mut Context<Self>) {
        command(cx, "play/pause", |e| e.toggle_play_pause());
    }

    fn toolbar(&self, p: &Palette, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, label: &'static str, enabled: bool| {
            let surface = p.surface;
            let hover = p.hover;
            div()
                .id(id)
                .size(TOOLBAR_BUTTON)
                .flex()
                .items_center()
                .justify_center()
                .text_size(text::CONTROL)
                .text_color(p.ink)
                .when(!enabled, |d| d.opacity(0.4))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(surface))
                        .active(move |s| s.bg(hover))
                })
                .child(label)
        };
        let title = match self.nav.current() {
            Page::Queue => "queue",
            Page::Albums => "albums",
            Page::Artists => "artists",
            Page::Tracks => "tracks",
            Page::Favourites => "favourites",
            Page::RecentlyPlayed => "recently played",
            Page::History => "history",
            Page::Downloads => "downloads",
            Page::Playlist(_) => "playlist",
        };
        div()
            .id("toolbar")
            .h(TOOLBAR)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(TRAFFIC_LIGHTS + space::S)
            .pr(space::L)
            .gap(space::XS)
            .border_b(HAIRLINE)
            .border_color(p.rule)
            .child(
                button("back", "←", self.nav.can_go_back())
                    .on_click(cx.listener(|this, _, window, cx| this.back(&GoBack, window, cx))),
            )
            .child(
                button("forward", "→", self.nav.can_go_forward()).on_click(
                    cx.listener(|this, _, window, cx| this.forward(&GoForward, window, cx)),
                ),
            )
            // The toolbar's bare space moves the window, and its buttons do
            // not: a press that wobbles on back is still a click.
            .child(
                div()
                    .id("toolbar-drag")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(space::M)
                    .text_size(text::CONTROL)
                    .text_color(p.ink)
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.moving_window = true),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.moving_window = false),
                    )
                    .on_mouse_down_out(cx.listener(|this, _, _, _| this.moving_window = false))
                    .on_mouse_move(cx.listener(|this, _, window, _| {
                        if std::mem::take(&mut this.moving_window) {
                            window.start_window_move();
                        }
                    }))
                    .on_double_click(|_, window, _| {
                        if cfg!(target_os = "macos") {
                            window.titlebar_double_click();
                        } else {
                            window.zoom_window();
                        }
                    })
                    .child(title),
            )
            // Where the compositor leaves decorations to the app (GNOME on
            // Wayland), the toolbar carries the window's buttons.
            .when(
                matches!(window.window_decorations(), Decorations::Client { .. }),
                |d| {
                    d.child(
                        button("minimise", "–", true)
                            .window_control_area(WindowControlArea::Min)
                            .on_click(|_, window, _| window.minimize_window()),
                    )
                    .child(
                        button("maximise", "□", true)
                            .window_control_area(WindowControlArea::Max)
                            .on_click(|_, window, _| window.zoom_window()),
                    )
                    .child(
                        button("close", "×", true)
                            .window_control_area(WindowControlArea::Close)
                            .on_click(|_, window, _| window.remove_window()),
                    )
                },
            )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let stage = match self.nav.current() {
            Page::Queue => self.queue.clone().into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(text::BODY)
                .text_color(p.muted)
                .child("not in the desktop app yet")
                .into_any_element(),
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(Self::forward))
            .on_action(cx.listener(Self::toggle_play))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.back(&GoBack, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.forward(&GoForward, window, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.ink)
            .child(self.toolbar(&p, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(self.sidebar.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .border_l(HAIRLINE)
                            .border_color(p.rule)
                            .child(stage),
                    ),
            )
            .child(self.transport.clone())
    }
}

pub const MIN_SIZE: Size<Pixels> = Size {
    width: px(220. + 640.),
    height: px(420.),
};

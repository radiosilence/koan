//! The window: a toolbar across the top, the sidebar beside the stage, and
//! the transport along the foot under both.
//!
//! Each region is a cached view, so a region that changes (the playhead's
//! slider, a row of the queue) redraws itself and the views that hold it,
//! and the others are reused as they were.

use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::navigator::{Navigator, Route};
use crate::platform::actions::{
    self, ActualSize, Back, Bigger, Forward, ShowAlbums, ShowArtists, ShowFavourites, ShowHistory,
    ShowQueue, ShowRecentlyPlayed, ShowShortcuts, ShowTracks, Smaller,
};
use crate::platform::shortcuts_sheet::{Dismissed, ShortcutsSheet};

use crate::library::{self, stage::LibraryStage};
use crate::paint::Paint;
use crate::queue::QueueView;
use crate::session::Session;
use crate::sidebar::{self, Sidebar};
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, TypeRole};
use crate::transport::{self, Transport};
use crate::window_state;

const TOOLBAR: Pixels = px(38.);
const TOOLBAR_BUTTON: Pixels = px(36.);
/// Room above the sidebar for the window's buttons, which sit on its ground.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: Pixels = TOOLBAR;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHTS: Pixels = px(0.);

pub struct Shell {
    nav: Entity<Navigator>,
    focus: FocusHandle,
    sidebar: Entity<Sidebar>,
    /// Held for the life of the window, so the queue's selection and scroll
    /// position survive a visit to another page.
    queue: Entity<QueueView>,
    transport: Entity<Transport>,
    /// The library's pages, once the browsers' saved sorts and filters have
    /// been read.
    library: Option<Entity<LibraryStage>>,
    /// The toolbar was pressed and the next move drags the window.
    moving_window: bool,
    /// The route focus was last moved for: the navigator notifies for more
    /// than moves (a deleted playlist forgotten during a scan), and those
    /// must not take the keys from wherever they are.
    arrived_at: Route,
    _arrivals: Subscription,
    /// Saves the session when the window loses focus.
    _activation: Subscription,
    /// Watches the window's one question host, which draws over everything.
    _asking: Subscription,
    /// The keyboard shortcuts, while they are shown.
    shortcuts: Option<(Entity<ShortcutsSheet>, Subscription)>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|_| Navigator::default());
        let queue = cx.new(|cx| QueueView::new(&nav, cx));
        window.focus(&queue.focus_handle(cx), cx);
        // Whatever moved the navigator, the page it lands on takes the keys.
        let arrivals = cx.observe_in(&nav, window, |this, _, window, cx| this.arrived(window, cx));
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                Session::save_if_changed(cx);
            }
        });
        let navigator = nav.clone();
        cx.spawn_in(window, async move |this, cx| {
            let browse = cx
                .background_executor()
                .spawn(async { library::browse::Browse::load() })
                .await;
            this.update_in(cx, |this, window, cx| {
                let engine = crate::engine::engine(cx);
                let lib = library::Lib::new(engine, browse, navigator, None, cx);
                this.library = Some(cx.new(|cx| LibraryStage::new(lib, window, cx)));
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            sidebar: cx.new(|cx| Sidebar::new(&nav, cx)),
            nav,
            focus: cx.focus_handle(),
            queue,
            transport: cx.new(|cx| Transport::new(window, cx)),
            library: None,
            moving_window: false,
            arrived_at: Route::Queue,
            _arrivals: arrivals,
            _activation: activation,
            _asking: {
                let asking = crate::collections::asking(cx);
                cx.observe(&asking, |_, _, cx| cx.notify())
            },
            shortcuts: None,
        }
    }

    fn arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self.nav.read(cx).current();
        if route == self.arrived_at {
            cx.notify();
            return;
        }
        self.arrived_at = route;
        self.refocus(window, cx);
    }

    fn back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        self.nav.update(cx, |nav, cx| {
            if nav.back() {
                cx.notify();
            }
        });
    }

    fn forward(&mut self, _: &Forward, _: &mut Window, cx: &mut Context<Self>) {
        self.nav.update(cx, |nav, cx| {
            if nav.forward() {
                cx.notify();
            }
        });
    }

    /// Goes to `route`, as a link or a menu would.
    pub fn show(&mut self, route: Route, cx: &mut Context<Self>) {
        self.nav.update(cx, |nav, cx| {
            if nav.go(route) {
                cx.notify();
            }
        });
    }

    /// The text size in the theme's steps, within its range.
    fn step_text_size(by: i32, cx: &mut App) {
        let now = cx.global::<theme::KoanTheme>().text_size as i32;
        let step = theme::tokens::TEXT_SIZE_STEP as i32;
        let range = theme::tokens::TEXT_SIZES;
        let next = (now + by * step).clamp(*range.start() as i32, *range.end() as i32);
        theme::KoanTheme::set_text_size(next as u32, cx);
    }

    fn show_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcuts.is_some() {
            return;
        }
        let sheet = cx.new(ShortcutsSheet::new);
        let dismissed = cx.subscribe_in(&sheet, window, |this, _, _: &Dismissed, window, cx| {
            this.shortcuts = None;
            this.arrived_at = this.nav.read(cx).current();
            this.refocus(window, cx);
        });
        window.focus(&sheet.focus_handle(cx), cx);
        self.shortcuts = Some((sheet, dismissed));
        cx.notify();
    }

    /// The page showing takes the keys again.
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.read(cx).current() == Route::Queue {
            window.focus(&self.queue.focus_handle(cx), cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Bare space that moves the window, as a title bar's does.
    fn moves_window(area: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        area.window_control_area(WindowControlArea::Drag)
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
    }

    fn toolbar(&self, p: &Paint, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, icon: Option<Icon>, label: &'static str, enabled: bool| {
            let surface = p.surface;
            let hover = p.hover;
            div()
                .id(id)
                .size(TOOLBAR_BUTTON)
                .flex()
                .items_center()
                .justify_center()
                .text_role(TypeRole::Control, &p.look)
                .text_color(p.ink)
                .when(!enabled, |d| d.opacity(0.4))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(surface))
                        .active(move |s| s.bg(hover))
                })
                .child(if let Some(icon) = icon.filter(|_| p.look.icons) {
                    KoanIcon::new(icon)
                        .role(TypeRole::Control)
                        .color(p.ink)
                        .into_any_element()
                } else {
                    div().child(label).into_any_element()
                })
        };
        let nav = self.nav.read(cx);
        let (can_go_back, can_go_forward) = (nav.can_go_back(), nav.can_go_forward());
        let title = match nav.current() {
            Route::Queue => "queue",
            Route::Albums => "albums",
            Route::Artists => "artists",
            Route::Tracks => "tracks",
            Route::Favourites => "favourites",
            Route::RecentlyPlayed => "recently played",
            Route::OnDevice => "downloaded",
            Route::History => "history",
            Route::Downloads => "downloads",
            Route::Playlist(_) => "playlist",
            Route::Album(_) => "record",
            Route::Artist(_) => "artist",
        };
        div()
            .id("toolbar")
            .h(TOOLBAR)
            .bg(p.bg)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .px(p.space(Space::S))
            .gap(p.space(Space::Xs))
            .child(
                button("back", Some(Icon::Back), "back", can_go_back)
                    .on_click(cx.listener(|this, _, window, cx| this.back(&Back, window, cx))),
            )
            .child(
                button("forward", Some(Icon::Forward), "forward", can_go_forward).on_click(
                    cx.listener(|this, _, window, cx| this.forward(&Forward, window, cx)),
                ),
            )
            // The toolbar's bare space moves the window, and its buttons do
            // not: a press that wobbles on back is still a click.
            .child(
                Self::moves_window(div().id("toolbar-drag"), cx)
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(p.space(Space::M))
                    .text_role(TypeRole::Control, &p.look)
                    .text_color(p.ink)
                    .child(title),
            )
            // Where the compositor leaves decorations to the app (GNOME on
            // Wayland), the toolbar carries the window's buttons.
            .when(
                matches!(window.window_decorations(), Decorations::Client { .. }),
                |d| {
                    d.child(
                        button("minimise", None, "–", true)
                            .window_control_area(WindowControlArea::Min)
                            .on_click(|_, window, _| window.minimize_window()),
                    )
                    .child(
                        button("maximise", None, "□", true)
                            .window_control_area(WindowControlArea::Max)
                            .on_click(|_, window, _| window.zoom_window()),
                    )
                    .child(
                        button("close", None, "×", true)
                            .window_control_area(WindowControlArea::Close)
                            // Removing the window skips the close request, so
                            // its bounds are saved here rather than there.
                            .on_click(|_, window, cx| {
                                window_state::save(window.window_bounds(), cx);
                                window.remove_window();
                            }),
                    )
                },
            )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let stage = match self.nav.read(cx).current() {
            Route::Queue => self
                .queue
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            route if LibraryStage::draws(route) => match &self.library {
                Some(library) => library
                    .clone()
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element(),
                None => div().size_full().into_any_element(),
            },
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_role(TypeRole::Body, &p.look)
                .text_color(p.muted)
                .child("not in the desktop app yet")
                .into_any_element(),
        };
        div()
            .key_context(actions::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(Self::forward))
            .on_action(cx.listener(|this, _: &ShowQueue, _, cx| this.show(Route::Queue, cx)))
            .on_action(cx.listener(|this, _: &ShowAlbums, _, cx| this.show(Route::Albums, cx)))
            .on_action(cx.listener(|this, _: &ShowArtists, _, cx| this.show(Route::Artists, cx)))
            .on_action(
                cx.listener(|this, _: &ShowFavourites, _, cx| this.show(Route::Favourites, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowHistory, _, cx| this.show(Route::History, cx)))
            .on_action(cx.listener(|this, _: &ShowRecentlyPlayed, _, cx| {
                this.show(Route::RecentlyPlayed, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTracks, _, cx| this.show(Route::Tracks, cx)))
            .on_action(|_: &Bigger, _, cx| Self::step_text_size(1, cx))
            .on_action(|_: &Smaller, _, cx| Self::step_text_size(-1, cx))
            .on_action(|_: &ActualSize, _, cx| theme::KoanTheme::set_text_size(100, cx))
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.show_shortcuts(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.back(&Back, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.forward(&Forward, window, cx)),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.ink)
            // The sidebar runs from the window's top edge to the transport,
            // the window's buttons on its ground; the toolbar heads the page
            // column alone.
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(
                        div()
                            .w(sidebar::WIDTH)
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .bg(p.bg)
                            .when(TRAFFIC_LIGHTS > px(0.), |d| {
                                d.child(
                                    Self::moves_window(div().id("sidebar-drag"), cx)
                                        .h(TRAFFIC_LIGHTS)
                                        .flex_shrink_0(),
                                )
                            })
                            .child(self.sidebar.clone().cached(
                                StyleRefinement::default().w_full().flex_1().min_h(px(0.)),
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_l(theme::HAIRLINE)
                            .border_color(p.rule)
                            .child(self.toolbar(&p, window, cx))
                            .child(div().flex_1().min_h(px(0.)).child(stage)),
                    ),
            )
            .child(
                self.transport.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(transport::HEIGHT)
                        .flex_shrink_0(),
                ),
            )
            .children(
                crate::collections::overlay(window, cx)
                    .map(|ask| deferred(div().absolute().inset_0().child(ask)).with_priority(3)),
            )
            .children(self.shortcuts.as_ref().map(|(sheet, _)| {
                deferred(div().absolute().inset_0().child(sheet.clone())).with_priority(2)
            }))
    }
}

pub const MIN_SIZE: Size<Pixels> = Size {
    width: px(220. + 640.),
    height: px(420.),
};

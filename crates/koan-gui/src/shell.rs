//! The window: a toolbar across the top, the sidebar beside the stage, and
//! the transport along the foot under both.
//!
//! Each region is a cached view, so a region that changes (the playhead's
//! slider, a row of the queue) redraws itself and the views that hold it,
//! and the others are reused as they were.

use koan_kit::app::*;
// gpui-component's focus and click extensions, for the shell's own handling.
use koan_kit::raw::base::InteractiveElementExt as _;
// the bounds a cached view keeps between frames.
use koan_kit::raw::StyleRefinement;
// overlays placed against another element.
use koan_kit::raw::deferred;
// the platform window and its options.
use koan_kit::raw::{Decorations, WindowControlArea};

use crate::components::sheet as ask;
use crate::components::{RowMetrics, SplitView, Toolbar, ToolbarButton, WINDOW_BUTTONS};
use crate::navigator::{Navigator, Route};
use crate::platform::actions::{
    self, ActualSize, AddMusic, Back, Bigger, ClearArtworkCache, ClearDownloadedFiles, Forward,
    Search, ShowAlbums, ShowArtists, ShowFavourites, ShowHistory, ShowQueue, ShowRecentlyPlayed,
    ShowShortcuts, ShowTracks, Smaller, ToggleLyrics, ToggleSidebar,
};
use crate::platform::shortcuts_sheet::{Dismissed, ShortcutsSheet};

use crate::library::{self, stage::LibraryStage};
use crate::paint::Paint;
use crate::queue::QueueView;
use crate::session::Session;
use crate::sidebar::Sidebar;
use crate::theme::{self, Icon, KoanStyled, TypeRole};
use crate::transport::{self, Transport};
use crate::window_state;

pub struct Shell {
    nav: Entity<Navigator>,
    focus: FocusHandle,
    sidebar: Entity<Sidebar>,
    /// Held for the life of the window, so the queue's selection and scroll
    /// position survive a visit to another page.
    queue: Entity<QueueView>,
    /// The queue's controls, in the toolbar while the queue shows.
    queue_controls: Entity<library::controls::ControlsSlot>,
    transport: Entity<Transport>,
    /// What is wrong with the server, across the top of every page.
    banner: Entity<crate::server_banner::ServerBanner>,
    /// The library's pages, once the browsers' saved sorts and filters have
    /// been read.
    library: Option<Entity<LibraryStage>>,
    /// The downloads page, while it is on stage: made on arriving and let go
    /// on leaving, so its slices wake nothing while it is not shown.
    downloads: Option<Entity<crate::downloads::DownloadsPage>>,
    /// The kōan menu, drawn in the window where there is no menu bar.
    koan_menu: crate::components::MenuSlot<()>,
    /// The toolbar was pressed and the next move drags the window.
    moving_window: bool,
    /// Whether the sidebar shows, and how wide; kept with the window's
    /// bounds.
    sidebar_shown: bool,
    sidebar_width: Pixels,
    /// The route focus was last moved for: the navigator notifies for more
    /// than moves (a deleted playlist forgotten during a scan), and those
    /// must not take the keys from wherever they are.
    arrived_at: Route,
    _arrivals: Subscription,
    /// Saves the session when the window loses focus.
    _activation: Subscription,
    /// The keyboard shortcuts, while they are shown.
    shortcuts: Option<(Entity<ShortcutsSheet>, Subscription)>,
    /// The lyrics beside the page, while they show. Dropped when hidden, so
    /// a closed panel follows no song.
    lyrics: Option<Entity<crate::lyrics::LyricsPanel>>,
    /// ⇧⌘K's picker, while it is open.
    picker: Option<(Entity<crate::search::picker::PickerSheet>, Subscription)>,
    /// Watches the window's one question host, which draws over everything.
    _asking: Subscription,
    /// Watches the artwork viewer, and while it shows, the sleeve it waits
    /// for.
    _artwork: [Subscription; 2],
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|_| Navigator::default());
        // A measuring run can open on the page it measures.
        #[cfg(feature = "profiler")]
        if let Some(route) = crate::measure::route() {
            nav.update(cx, |nav, _| nav.go(route));
        }
        let queue = cx.new(|cx| QueueView::new(&nav, cx));
        let queue_controls = cx.new(|cx| {
            let mut slot = library::controls::ControlsSlot::default();
            slot.show(&queue, QueueView::controls, cx);
            slot
        });
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
                let feed = lib.feed.clone();
                this.queue.update(cx, |queue, cx| queue.set_feed(feed, cx));
                let covers = lib.covers.clone();
                let search = lib.search.clone();
                this.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_covers(covers.clone(), cx);
                    sidebar.set_search(search, covers, window, cx);
                });
                this.library = Some(cx.new(|cx| LibraryStage::new(lib, window, cx)));
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            sidebar: cx.new(|cx| Sidebar::new(&nav, cx)),
            transport: cx.new(|cx| Transport::new(&nav, window, cx)),
            banner: cx.new(crate::server_banner::ServerBanner::new),
            nav,
            focus: cx.focus_handle(),
            queue,
            queue_controls,
            library: None,
            downloads: None,
            koan_menu: crate::components::MenuSlot::default(),
            moving_window: false,
            sidebar_shown: window_state::SIDEBAR.load(std::sync::atomic::Ordering::Relaxed),
            sidebar_width: f32::from_bits(
                window_state::SIDEBAR_WIDTH.load(std::sync::atomic::Ordering::Relaxed),
            )
            .into(),
            arrived_at: Route::Queue,
            _arrivals: arrivals,
            _activation: activation,
            shortcuts: None,
            picker: None,
            lyrics: None,
            _asking: {
                let asking = ask::asking(cx);
                cx.observe(&asking, |_, _, cx| cx.notify())
            },
            _artwork: {
                let viewer = crate::artwork::artwork(cx);
                let sleeves = crate::artwork::sleeves(cx);
                [
                    // Closed, the page it was over takes the keys again.
                    cx.observe_in(&viewer, window, |this, _, window, cx| {
                        if !crate::artwork::is_showing(cx) {
                            this.refocus(window, cx);
                        }
                        cx.notify();
                    }),
                    cx.observe(&sleeves, |_, _, cx| {
                        if crate::artwork::is_showing(cx) {
                            cx.notify();
                        }
                    }),
                ]
            },
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

    /// ⌥⌘L, `L` and the toolbar's button: the lyrics beside the page, or not.
    fn toggle_lyrics(&mut self, _: &ToggleLyrics, _: &mut Window, cx: &mut Context<Self>) {
        self.lyrics = match self.lyrics.take() {
            Some(_) => None,
            None => Some(cx.new(crate::lyrics::LyricsPanel::new)),
        };
        cx.notify();
    }

    /// ⌘K and `/`: the sidebar's search field, the sidebar shown first.
    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_shown {
            self.toggle_sidebar(&ToggleSidebar, window, cx);
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.focus_search(window, cx));
    }

    /// ⇧⌘K: the picker, over the window.
    fn add_music(&mut self, _: &AddMusic, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        let engine = crate::engine::engine(cx);
        let picker = cx.new(|cx| crate::search::picker::PickerSheet::new(engine, window, cx));
        let dismissed = cx.subscribe_in(
            &picker,
            window,
            |this, _, _: &crate::search::picker::Dismissed, window, cx| {
                this.picker = None;
                this.arrived_at = this.nav.read(cx).current();
                this.refocus(window, cx);
            },
        );
        self.picker = Some((picker, dismissed));
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

    /// Hides or shows the sidebar, as the Mac's toolbar button and ⌃⌘S do,
    /// and keeps the choice with the window's bounds.
    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_shown = !self.sidebar_shown;
        window_state::SIDEBAR.store(self.sidebar_shown, std::sync::atomic::Ordering::Relaxed);
        window_state::save(window.window_bounds(), cx);
        cx.notify();
    }

    /// Asks before forgetting every cover kept on this device, saying how
    /// many and how much, then fetches what is on screen again.
    fn clear_artwork(
        &mut self,
        _: &ClearArtworkCache,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(covers) = self.library.as_ref().map(|l| l.read(cx).covers()) else {
            return;
        };
        let dir = covers.read(cx).dir();
        let window = window.window_handle();
        cx.spawn(async move |_, cx| {
            let (count, bytes) = cx
                .background_executor()
                .spawn(async move { crate::library::covers::Covers::on_disk(&dir) })
                .await;
            window
                .update(cx, |_, window, cx| {
                    ask::confirm(
                        ask::Confirm {
                            title: "clear artwork cache".into(),
                            verb: "clear".into(),
                            destructive: true,
                            lines: vec![
                                format!(
                                    "{} kept on this device are deleted: {}.",
                                    crate::library::chrome::count(count as u64, "cover", "covers"),
                                    crate::time::bytes(bytes)
                                )
                                .into(),
                            ],
                            note: Some(
                                "Covers are fetched again as they are shown, the ones on screen at once."
                                    .into(),
                            ),
                        },
                        window,
                        cx,
                        move |cx| covers.update(cx, |c, cx| c.clear(None, cx)),
                    );
                })
                .ok();
        })
        .detach();
    }

    /// Asks before throwing away every file downloaded from the server,
    /// saying how much it frees and that the tracks stay.
    fn clear_downloads(
        &mut self,
        _: &ClearDownloadedFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = cx.global::<crate::mirror::Mirror>().cache.read(cx).0;
        ask::confirm(
            ask::Confirm {
                title: "clear downloaded files".into(),
                verb: "clear".into(),
                destructive: true,
                lines: vec![
                    format!(
                        "The copies of tracks downloaded from your server are deleted from this device: {}.",
                        crate::time::bytes(bytes)
                    )
                    .into(),
                ],
                note: Some(
                    "The tracks stay in the library and download again when played.".into(),
                ),
            },
            window,
            cx,
            |cx| {
                crate::engine::command(cx, "clear downloaded files", |e| e.clear_download_cache())
            },
        );
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

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = self.nav.read(cx);
        let (can_go_back, can_go_forward, route) =
            (nav.can_go_back(), nav.can_go_forward(), nav.current());
        // The page's own controls: a library page's filter field and buttons.
        let page = if route == Route::Queue {
            Some(self.queue_controls.clone().into())
        } else {
            self.library
                .as_ref()
                .filter(|_| LibraryStage::draws(route))
                .and_then(|library| library.read(cx).toolbar())
        };
        Toolbar::new()
            // With the sidebar hidden the window's buttons sit over the
            // toolbar, which leaves them their room.
            .under_window_buttons(!self.sidebar_shown)
            // No global menu bar off macOS: the menus open from here.
            .when(cfg!(not(target_os = "macos")), |toolbar| {
                toolbar.leading(self.koan_menu.trigger_below(
                    (),
                    div().id("koan-menu").child(ToolbarButton::new(
                        "koan-menu-button",
                        Icon::More,
                        "kōan",
                    )),
                    |this: &mut Self| &mut this.koan_menu,
                    |_, window, cx| {
                        Some(crate::components::Menu::build(
                            window,
                            cx,
                            crate::platform::menus::in_window,
                        ))
                    },
                    cx,
                ))
            })
            .leading(
                ToolbarButton::new("sidebar", Icon::Sidebar, "sidebar").on_click(cx.listener(
                    |this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx),
                )),
            )
            .leading(
                ToolbarButton::new("back", Icon::Back, "back")
                    .disabled(!can_go_back)
                    .on_click(cx.listener(|this, _, window, cx| this.back(&Back, window, cx))),
            )
            .leading(
                ToolbarButton::new("forward", Icon::Forward, "forward")
                    .disabled(!can_go_forward)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.forward(&Forward, window, cx)),
                    ),
            )
            // The toolbar's bare space moves the window, and its buttons do
            // not: a press that wobbles on back is still a click.
            .principal(Self::moves_window(div().id("toolbar-drag"), cx).size_full())
            .when_some(page, |toolbar, page| toolbar.trailing(page))
            .trailing(
                ToolbarButton::new("lyrics", Icon::Lyrics, "lyrics")
                    .selected(self.lyrics.is_some())
                    .on_click(|_, window, cx| {
                        window.dispatch_action(ToggleLyrics.boxed_clone(), cx)
                    }),
            )
            // Where the compositor leaves decorations to the app (GNOME on
            // Wayland), the toolbar carries the window's buttons.
            .when(
                matches!(window.window_decorations(), Decorations::Client { .. }),
                |toolbar| {
                    toolbar
                        .trailing(
                            div().window_control_area(WindowControlArea::Min).child(
                                ToolbarButton::new("minimise", Icon::Subtract, "minimise")
                                    .on_click(|_, window, _| window.minimize_window()),
                            ),
                        )
                        .trailing(
                            div().window_control_area(WindowControlArea::Max).child(
                                ToolbarButton::new("maximise", Icon::Expand, "maximise")
                                    .on_click(|_, window, _| window.zoom_window()),
                            ),
                        )
                        .trailing(
                            div().window_control_area(WindowControlArea::Close).child(
                                ToolbarButton::new("close", Icon::Close, "close")
                                    // Removing the window skips the close
                                    // request, so its bounds are saved here,
                                    // now: the app quits with it.
                                    .on_click(|_, window, _| {
                                        window_state::write(window.window_bounds());
                                        window.remove_window();
                                    }),
                            ),
                        )
                },
            )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let washed = cx.global::<theme::KoanTheme>().wash_window;
        let transport = {
            let transport = self.transport.clone();
            move || {
                transport.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(transport::HEIGHT)
                        .flex_shrink_0(),
                )
            }
        };
        let route = self.nav.read(cx).current();
        if route != Route::Downloads {
            self.downloads = None;
        }
        let stage = match route {
            Route::Downloads => {
                let nav = self.nav.clone();
                self.downloads
                    .get_or_insert_with(|| {
                        cx.new(|cx| crate::downloads::DownloadsPage::new(&nav, cx))
                    })
                    .clone()
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element()
            }
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
            .on_action(cx.listener(Self::toggle_sidebar))
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
            .on_action(cx.listener(Self::clear_downloads))
            .on_action(cx.listener(Self::clear_artwork))
            .on_action(|_: &Bigger, _, cx| Self::step_text_size(1, cx))
            .on_action(|_: &Smaller, _, cx| Self::step_text_size(-1, cx))
            .on_action(|_: &ActualSize, _, cx| theme::KoanTheme::set_text_size(100, cx))
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.show_shortcuts(window, cx)),
            )
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::search))
            .on_action(cx.listener(Self::toggle_lyrics))
            .on_action(cx.listener(Self::add_music))
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
            // column alone. With "wash the whole window" the sidebar and the
            // toolbar have no ground, the wash runs under them, and the
            // transport spans the whole window; otherwise they are on `bg`
            // and the transport spans the page column only.
            .child(
                div().flex_1().min_h_0().child(
                    SplitView::new(
                        "sidebar-split",
                        self.sidebar_width,
                        RowMetrics::SIDEBAR_MIN,
                        RowMetrics::SIDEBAR_MAX,
                        RowMetrics::SIDEBAR,
                    )
                    .when(self.sidebar_shown, |split| {
                        split.leading(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .when(!washed, |d| d.bg(p.bg))
                                .when(WINDOW_BUTTONS.height > Pixels::ZERO, |d| {
                                    d.child(
                                        Self::moves_window(div().id("sidebar-drag"), cx)
                                            .h(WINDOW_BUTTONS.height)
                                            .flex_shrink_0(),
                                    )
                                })
                                .child(self.sidebar.clone().cached(
                                    StyleRefinement::default().w_full().flex_1().min_h_0(),
                                )),
                        )
                    })
                    .trailing(
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(self.toolbar(window, cx))
                            .child(self.banner.clone())
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .child(div().flex_1().min_w_0().h_full().child(stage))
                                    .children(self.lyrics.clone().map(|lyrics| {
                                        lyrics.cached(
                                            StyleRefinement::default()
                                                .w(RowMetrics::inspector(&p.look))
                                                .h_full()
                                                .flex_none(),
                                        )
                                    })),
                            )
                            .when(!washed, |d| d.child(transport())),
                    )
                    .on_resize(cx.listener(|this, width: &Pixels, _, cx| {
                        this.sidebar_width = *width;
                        cx.notify();
                    }))
                    .on_commit(|width: &Pixels, window, cx| {
                        window_state::SIDEBAR_WIDTH.store(
                            f32::from(*width).to_bits(),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        window_state::save(window.window_bounds(), cx);
                    }),
                ),
            )
            .when(washed, |d| d.child(transport()))
            .children(
                ask::presented(window, cx)
                    .map(|s| deferred(div().absolute().inset_0().child(s)).with_priority(2)),
            )
            .children(
                ask::overlay(window, cx)
                    .map(|q| deferred(div().absolute().inset_0().child(q)).with_priority(3)),
            )
            .children(
                crate::artwork::overlay(window, cx)
                    .map(|v| deferred(div().absolute().inset_0().child(v)).with_priority(2)),
            )
            .children(self.shortcuts.as_ref().map(|(sheet, _)| {
                deferred(div().absolute().size_full().child(sheet.clone())).with_priority(2)
            }))
            .children(self.picker.as_ref().map(|(picker, _)| {
                deferred(div().absolute().size_full().child(picker.clone())).with_priority(2)
            }))
    }
}

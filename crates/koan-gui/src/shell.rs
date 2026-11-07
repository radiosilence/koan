//! The window: the sidebar from the top edge, beside the page column (its
//! toolbar, then the stage), and the transport along the foot under both.
//! On macOS the window's buttons sit on the sidebar's ground.
//!
//! Each region is a cached view, so a region that changes (the playhead's
//! slider, a row of the queue) redraws itself and the views that hold it,
//! and the others are reused as they were.

use koan_kit::app::*;

use crate::components::sheet::{self as ask, Layer};
use crate::components::{
    Fading, RowMetrics, Slide, SplitView, Toolbar, ToolbarButton, WINDOW_BUTTONS,
};
use crate::navigator::{Navigator, Route};
use crate::platform::actions::{
    self, ActualSize, AddMusic, Back, Bigger, FocusNext, FocusPrevious, Forward, NextRegion,
    PreviousRegion, Search, ShowAlbums, ShowArtists, ShowFavourites, ShowHistory, ShowQueue,
    ShowRecentlyPlayed, ShowShortcuts, ShowTracks, Smaller, ToggleLyrics, ToggleSidebar, help,
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
    /// The output this device is still waiting for, once it has waited a while.
    output_wait: Entity<crate::output_wait::OutputWaitBanner>,
    /// The library's pages, once the browsers' saved sorts and filters have
    /// been read.
    library: Option<Entity<LibraryStage>>,
    /// The downloads page, while it is on stage: made on arriving and let go
    /// on leaving, so its slices wake nothing while it is not shown.
    downloads: Option<Entity<crate::downloads::DownloadsPage>>,
    /// The kōan menu, drawn in the window where there is no menu bar.
    koan_menu: crate::components::MenuSlot<()>,
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
    /// The lyrics beside the page, while they show or slide away; dropped
    /// once gone, so a closed panel follows no song.
    lyrics: Option<Entity<crate::lyrics::LyricsPanel>>,
    lyrics_shown: bool,
    lyrics_slide: crate::components::SlideState,
    /// The lyrics panel's width once dragged; until then, and after a
    /// double click on its edge, the inspector's, which follows the text
    /// size. Kept with the window's bounds.
    lyrics_width: Option<Pixels>,
    /// The search box over the window, while it is open.
    _opened: Subscription,
    search_bar: Option<(Entity<crate::search::bar::SearchBar>, Subscription)>,
    /// What had the keyboard when the search box opened, which has it back
    /// when the box closes.
    before_search: Option<(FocusHandle, Route)>,
    /// The search box or the shortcuts just put away, fading out where they
    /// stood.
    leaving: Option<(AnyView, Fading)>,
    /// Watches the window's one question host, which draws over everything.
    _asking: Subscription,
    /// Watches the artwork viewer, and while it shows, the sleeve it waits
    /// for.
    _artwork: [Subscription; 2],
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A menu closing after what held the keyboard went (its page left
        // behind) gives it to the page showing.
        let (shell, here) = (cx.weak_entity(), window.window_handle());
        crate::components::on_menu_focus_lost(
            move |window, cx| {
                if window.window_handle() == here {
                    shell
                        .update(cx, |shell, cx| shell.focus_page(window, cx))
                        .ok();
                }
            },
            cx,
        );
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
                let for_queue = lib.clone();
                this.queue
                    .update(cx, |queue, cx| queue.set_lib(for_queue, cx));
                let feed = lib.feed.clone();
                this.transport
                    .update(cx, |transport, cx| transport.set_feed(feed, cx));
                let covers = lib.covers.clone();
                let search = lib.search.clone();
                this.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_covers(covers, cx);
                    sidebar.set_search(search, cx);
                });
                this.library = Some(cx.new(|cx| LibraryStage::new(lib, window, cx)));
                cx.notify();
            })
            .ok();
        })
        .detach();
        let sidebar = cx.new(|cx| Sidebar::new(&nav, cx));
        // The sidebar's click took the keyboard from the page; opening a
        // place gives it to the page, the one showing included.
        let opened = cx.subscribe_in(
            &sidebar,
            window,
            |this, _, _: &crate::sidebar::Opened, window, cx| {
                this.arrived_at = this.nav.read(cx).current();
                this.focus_page(window, cx);
            },
        );
        Self {
            _opened: opened,
            sidebar,
            transport: cx.new(|cx| Transport::new(&nav, window, cx)),
            banner: cx.new(crate::server_banner::ServerBanner::new),
            output_wait: cx.new(crate::output_wait::OutputWaitBanner::new),
            nav,
            focus: cx.focus_handle(),
            queue,
            queue_controls,
            library: None,
            downloads: None,
            koan_menu: crate::components::MenuSlot::default(),
            sidebar_shown: window_state::SIDEBAR.load(std::sync::atomic::Ordering::Relaxed),
            sidebar_width: f32::from_bits(
                window_state::SIDEBAR_WIDTH.load(std::sync::atomic::Ordering::Relaxed),
            )
            .into(),
            arrived_at: Route::Queue,
            _arrivals: arrivals,
            _activation: activation,
            shortcuts: None,
            search_bar: None,
            before_search: None,
            leaving: None,
            lyrics: None,
            lyrics_shown: false,
            lyrics_slide: Default::default(),
            lyrics_width: Some(f32::from_bits(
                window_state::LYRICS_WIDTH.load(std::sync::atomic::Ordering::Relaxed),
            ))
            .filter(|w| *w > 0.0)
            .map(Pixels::from),
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

    /// The queue's view, for tools that drive the window headlessly.
    pub fn queue(&self) -> &Entity<QueueView> {
        &self.queue
    }

    /// The library's pages, once made, for tools that drive the window
    /// headlessly.
    pub fn library(&self) -> Option<&Entity<LibraryStage>> {
        self.library.as_ref()
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
            if let Some((sheet, _)) = this.shortcuts.take() {
                this.put_away(sheet.into(), cx);
            }
            this.arrived_at = this.nav.read(cx).current();
            this.refocus(window, cx);
        });
        window.focus(&sheet.focus_handle(cx), cx);
        self.shortcuts = Some((sheet, dismissed));
        self.leaving = None;
        cx.notify();
    }

    /// An overlay put away: drawn fading out where it stood, then let go.
    fn put_away(&mut self, view: AnyView, cx: &mut Context<Self>) {
        let Some(fading) = Fading::start(cx) else {
            return;
        };
        self.leaving = Some((view, fading));
        let started = std::time::Instant::now();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Fading::LENGTH).await;
            this.update(cx, |this, cx| {
                if this.leaving.take_if(|(_, f)| f.began_by(started)).is_some() {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// ⌥⌘L, `L` and the toolbar's button: the lyrics beside the page, or not.
    fn toggle_lyrics(&mut self, _: &ToggleLyrics, _: &mut Window, cx: &mut Context<Self>) {
        self.lyrics_shown = !self.lyrics_shown;
        if self.lyrics_shown && self.lyrics.is_none() {
            self.lyrics = Some(cx.new(crate::lyrics::LyricsPanel::new));
        }
        cx.notify();
    }

    /// ⌘K, `/`, ⇧⌘K and the sidebar's field: the search box over the
    /// window, holding the keyboard.
    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search_bar(window, cx);
    }

    fn add_music(&mut self, _: &AddMusic, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search_bar(window, cx);
    }

    fn open_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((bar, _)) = &self.search_bar {
            window.focus(&bar.focus_handle(cx), cx);
            return;
        }
        let Some(lib) = self.library.as_ref().map(|l| l.read(cx).lib()) else {
            return;
        };
        let route = self.nav.read(cx).current();
        self.before_search = window.focused(cx).map(|f| (f, route));
        let bar = cx.new(|cx| crate::search::bar::SearchBar::new(lib, window, cx));
        let dismissed = cx.subscribe_in(
            &bar,
            window,
            |this, _, _: &crate::search::bar::Dismissed, window, cx| {
                if let Some((bar, _)) = this.search_bar.take() {
                    this.put_away(bar.into(), cx);
                }
                let route = this.nav.read(cx).current();
                this.arrived_at = route;
                // Back where the keyboard was, unless the box opened a page:
                // then that page has it.
                match this.before_search.take().filter(|(_, at)| *at == route) {
                    Some((before, _)) => {
                        window.focus(&before, cx);
                        cx.notify();
                    }
                    None => this.focus_page(window, cx),
                }
            },
        );
        self.search_bar = Some((bar, dismissed));
        self.leaving = None;
        cx.notify();
    }

    /// The page showing takes the keys again.
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Typing in a field that outlives the route moved it (the sidebar's
        // search lists its results as it is typed): the field keeps the
        // keyboard. A field of the page left behind (its filter, the picker)
        // goes with it, so the page showing takes the keyboard instead.
        let lasting = self.sidebar.focus_handle(cx).contains_focused(window, cx)
            || self
                .search_bar
                .as_ref()
                .is_some_and(|(bar, _)| bar.focus_handle(cx).contains_focused(window, cx));
        if lasting && typing(&window.context_stack()) {
            cx.notify();
            return;
        }
        self.focus_page(window, cx);
    }

    /// The page showing takes the keyboard, wherever it was.
    fn focus_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self.nav.read(cx).current();
        if route == Route::Queue {
            window.focus(&self.queue.focus_handle(cx), cx);
        } else if route == Route::Downloads {
            window.focus(&self.downloads(cx).focus_handle(cx), cx);
        } else {
            window.focus(&self.focus, cx);
            // A library page is made as the stage draws it, and takes the
            // keyboard then.
            if let Some(library) = self.library.as_ref().filter(|_| LibraryStage::draws(route)) {
                library.update(cx, |stage, cx| stage.take_keyboard(window, cx));
            }
        }
        cx.notify();
    }

    /// The downloads page, made when first shown and dropped on leaving.
    fn downloads(&mut self, cx: &mut Context<Self>) -> Entity<crate::downloads::DownloadsPage> {
        let nav = self.nav.clone();
        self.downloads
            .get_or_insert_with(|| cx.new(|cx| crate::downloads::DownloadsPage::new(&nav, cx)))
            .clone()
    }

    /// The window's regions in F6's order: the sidebar while it shows, the
    /// page, the transport.
    fn regions(&self, cx: &App) -> Vec<FocusHandle> {
        let route = self.nav.read(cx).current();
        let page = if route == Route::Queue {
            Some(self.queue.focus_handle(cx))
        } else if route == Route::Downloads {
            self.downloads.as_ref().map(|d| d.focus_handle(cx))
        } else {
            self.library
                .as_ref()
                .filter(|_| LibraryStage::draws(route))
                .and_then(|library| library.read(cx).page_focus())
        };
        self.sidebar_shown
            .then(|| self.sidebar.focus_handle(cx))
            .into_iter()
            .chain(page)
            .chain([self.transport.focus_handle(cx)])
            .collect()
    }

    /// F6: the next region takes the keyboard, from whichever holds it now.
    fn region(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let regions = self.regions(cx);
        let Some(last) = regions.len().checked_sub(1) else {
            return;
        };
        let at = regions.iter().position(|r| r.contains_focused(window, cx));
        let next = match at {
            Some(at) => (at as isize + by).rem_euclid(regions.len() as isize) as usize,
            None if by < 0 => last,
            None => 0,
        };
        window.focus(&regions[next], cx);
        self.focus_moved(cx);
    }

    /// The keyboard moved: the cached regions draw their focus rings again,
    /// since a cached view replays its last frame unless told.
    fn focus_moved(&mut self, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |_, cx| cx.notify());
        self.transport.update(cx, |_, cx| cx.notify());
        self.queue.update(cx, |_, cx| cx.notify());
        if let Some(library) = &self.library {
            library.update(cx, |_, cx| cx.notify());
        }
        if let Some(downloads) = &self.downloads {
            downloads.update(cx, |_, cx| cx.notify());
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
    pub fn clear_artwork(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                        // Run once the window is handed back, so the
                        // sleeves' textures leave its atlas with them.
                        move |cx| {
                            let covers = covers.clone();
                            cx.defer(move |cx| covers.update(cx, |c, cx| c.clear(None, cx)))
                        },
                    );
                })
                .ok();
        })
        .detach();
    }

    fn toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                ToolbarButton::new("sidebar", Icon::Sidebar, "sidebar")
                    .help(help("show or hide the sidebar", &ToggleSidebar, cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_sidebar(&ToggleSidebar, window, cx)
                    })),
            )
            .leading(
                ToolbarButton::new("back", Icon::Back, "back")
                    .help(help("back", &Back, cx))
                    .disabled(!can_go_back)
                    .on_click(cx.listener(|this, _, window, cx| this.back(&Back, window, cx))),
            )
            .leading(
                ToolbarButton::new("forward", Icon::Forward, "forward")
                    .help(help("forward", &Forward, cx))
                    .disabled(!can_go_forward)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.forward(&Forward, window, cx)),
                    ),
            )
            // The toolbar's bare space moves the window, and its buttons do
            // not: a press that wobbles on back is still a click.
            .principal(
                crate::components::moves_window(div().id("toolbar-drag"), window, cx)
                    .size_full()
                    .flex()
                    .items_center()
                    // With the sidebar hidden nothing else names the pages
                    // with no header of their own.
                    .children(
                        (!self.sidebar_shown)
                            .then_some(match route {
                                Route::Queue => Some("queue"),
                                Route::History => Some("history"),
                                Route::Downloads => Some("downloads"),
                                _ => None,
                            })
                            .flatten()
                            .map(crate::components::ToolbarTitle::new),
                    ),
            )
            .when_some(page, |toolbar, page| toolbar.trailing(page))
            .trailing(
                ToolbarButton::new("lyrics", Icon::Lyrics, "lyrics")
                    .selected(self.lyrics_shown)
                    .help(help("lyrics panel", &ToggleLyrics, cx))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(ToggleLyrics.boxed_clone(), cx)
                    }),
            )
            // Where the compositor leaves decorations to the app (GNOME on
            // Wayland), the toolbar carries the window's buttons.
            .when(crate::components::draws_decorations(window), |toolbar| {
                // Removing the window skips the close request, so its bounds
                // are saved as it closes: the app quits with it.
                crate::components::window_buttons(|window, _| {
                    window_state::write(window.window_bounds())
                })
                .into_iter()
                .fold(toolbar, |toolbar, button| toolbar.trailing(button))
            })
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The width has eased to nothing: the panel goes with it.
        if !self.lyrics_shown && self.lyrics.is_some() && self.lyrics_slide.closed() {
            self.lyrics = None;
        }
        let p = Paint::of(window, cx);
        let washed = theme::KoanTheme::washes_window(cx);
        // The strip above the sidebar: the traffic lights' room on macOS, a
        // title bar's where the app draws the window's decorations.
        let strip = if crate::components::draws_decorations(window) {
            crate::components::ToolbarMetrics::HEIGHT
        } else {
            WINDOW_BUTTONS.height
        };
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
            Route::Downloads => self
                .downloads(cx)
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
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
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                crate::components::sheet::step_focus(true, window, cx);
                this.focus_moved(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevious, window, cx| {
                crate::components::sheet::step_focus(false, window, cx);
                this.focus_moved(cx);
            }))
            .on_action(cx.listener(|this, _: &NextRegion, window, cx| this.region(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousRegion, window, cx| this.region(-1, window, cx)),
            )
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
            .on_action(|_: &Bigger, _, cx| Self::step_text_size(1, cx))
            .on_action(|_: &Smaller, _, cx| Self::step_text_size(-1, cx))
            .on_action(|_: &ActualSize, _, cx| theme::KoanTheme::set_text_size(100, cx))
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.show_shortcuts(window, cx)),
            )
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
                    .shown(self.sidebar_shown)
                    .map(|split| {
                        split.leading(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .when(!washed, |d| d.bg(p.bg))
                                // Room for the window's buttons, which full
                                // screen hides, and a place to move the
                                // window by: on macOS under the traffic
                                // lights; where the app draws its own
                                // decorations (GNOME on Wayland), the
                                // toolbar's height, level with the page's
                                // toolbar, as a title bar would be.
                                .when(strip > Pixels::ZERO && !window.is_fullscreen(), |d| {
                                    d.child(
                                        crate::components::moves_window(
                                            div().id("sidebar-drag"),
                                            window,
                                            cx,
                                        )
                                        .h(strip)
                                        .flex_shrink_0(),
                                    )
                                })
                                .child(
                                    // A landmark to a screen reader: where to go.
                                    div()
                                        .id("sidebar-region")
                                        .role(a11y::Role::Navigation)
                                        .aria_label("library")
                                        .w_full()
                                        .flex_1()
                                        .min_h_0()
                                        .flex()
                                        .flex_col()
                                        .child(self.sidebar.clone().cached(
                                            StyleRefinement::default().w_full().flex_1().min_h_0(),
                                        )),
                                ),
                        )
                    })
                    .trailing(
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(self.toolbar(window, cx))
                            .child(self.banner.clone())
                            .child(self.output_wait.clone())
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    // The page: the main landmark.
                                    .child(
                                        div()
                                            .id("stage")
                                            .role(a11y::Role::Main)
                                            .flex_1()
                                            .min_w_0()
                                            .h_full()
                                            .child(stage),
                                    )
                                    .child(
                                        Slide::new(
                                            "lyrics-slide",
                                            &self.lyrics_slide,
                                            self.lyrics_shown,
                                            lyrics_width,
                                        )
                                        .resizable(
                                            RowMetrics::inspector_min(&p.look),
                                            RowMetrics::inspector_max(&p.look),
                                            inspector,
                                        )
                                        .on_resize(cx.listener(|this, width: &Pixels, _, cx| {
                                            this.lyrics_width = Some(*width);
                                            cx.notify();
                                        }))
                                        .on_commit(cx.listener(
                                            move |this, width: &Pixels, window, cx| {
                                                // The reset width is no
                                                // width of the person's: it
                                                // follows the text size.
                                                this.lyrics_width =
                                                    (*width != inspector).then_some(*width);
                                                window_state::LYRICS_WIDTH.store(
                                                    this.lyrics_width
                                                        .map_or(0.0, f32::from)
                                                        .to_bits(),
                                                    std::sync::atomic::Ordering::Relaxed,
                                                );
                                                window_state::save(window.window_bounds(), cx);
                                            },
                                        ))
                                        .when_some(
                                            self.lyrics.clone(),
                                            |slide, lyrics| {
                                                slide.child(
                                                    lyrics.cached(
                                                        StyleRefinement::default()
                                                            .w(lyrics_width)
                                                            .h_full()
                                                            .flex_none(),
                                                    ),
                                                )
                                            },
                                        ),
                                    ),
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
            .children(ask::presented(window, cx).map(|s| ask::layered(Layer::Sheet, s)))
            .children(ask::overlay(window, cx).map(|q| ask::layered(Layer::Question, q)))
            .children(crate::artwork::overlay(window, cx).map(|v| ask::layered(Layer::Sheet, v)))
            .children(
                self.shortcuts
                    .as_ref()
                    .map(|(sheet, _)| ask::layered(Layer::Sheet, sheet.clone())),
            )
            .children(
                self.search_bar
                    .as_ref()
                    .map(|(bar, _)| ask::layered(Layer::Sheet, bar.clone())),
            )
            // Under the same ids as when it showed, so it does not come up
            // again as it goes.
            .children(
                self.leaving
                    .as_ref()
                    .map(|(view, fading)| ask::layered(Layer::Sheet, fading.draw(view.clone()))),
            )
    }
}

/// Whether the keyboard is in a text field: a context in its path is a
/// field's.
fn typing(stack: &[KeyContext]) -> bool {
    stack.iter().any(|c| c.contains("Input"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_keeps_the_keyboard_when_the_route_moves() {
        let field = KeyContext::parse("Input").unwrap();
        let page = KeyContext::parse("koan").unwrap();
        assert!(typing(&[page.clone(), field]));
        assert!(!typing(&[page]));
    }
}

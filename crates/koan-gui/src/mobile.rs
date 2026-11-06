//! The window on a phone, as the iOS app lays it out: a bar naming the page
//! with the way back, the page, the transport, and the tabs along the foot,
//! each tab a history of its own (`Navigator::switch`). Clear of the status
//! bar, the notch and the home indicator; the wash runs under them.
//!
//! The pages are the desktop's: the queue, the library's pages, downloads
//! and Settings' panes. The Library tab's first page lists the library's
//! sections, as the iOS app's does, and Settings' lists its panes.

use koan_kit::app::*;

use crate::components::flight;
use crate::components::sheet::{self as ask, Layer};
use crate::components::{
    Clear, NavRow, ScrollArea, TabStrip, Toolbar, ToolbarButton, ToolbarTitle,
};
use crate::library::{self, stage::LibraryStage};
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::queue::QueueView;
use crate::theme::Icon;
use crate::transport::{self, Transport};

/// Whether the app takes the phone's layout: on iOS, and on a desktop with
/// `KOAN_GUI_PHONE` set, so it can be walked at a phone's size.
pub fn phone() -> bool {
    static PHONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PHONE.get_or_init(|| cfg!(target_os = "ios") || std::env::var_os("KOAN_GUI_PHONE").is_some())
}

/// The tabs in the order the bar shows them, each with its first page.
const TABS: [(Route, &str, Icon); 4] = [
    (Route::Queue, "queue", Icon::QueueSection),
    (Route::Library, "library", Icon::Library),
    (Route::Settings, "settings", Icon::Settings),
    (Route::SearchTab, "search", Icon::Search),
];

/// The phone's shell, for what opens a window on a desktop (Settings, EQ, a
/// link Settings answers) to open its screen instead.
struct Screens(WeakEntity<PhoneShell>);

impl Global for Screens {}

/// On a phone, runs `f` on the shell in its window: whether it did.
fn on_shell(
    cx: &mut App,
    f: impl FnOnce(&mut PhoneShell, &mut Window, &mut Context<PhoneShell>),
) -> bool {
    let Some(shell) = cx.try_global::<Screens>().and_then(|s| s.0.upgrade()) else {
        return false;
    };
    let Some(window) = crate::app::main_window(cx) else {
        return false;
    };
    window
        .update(cx, |_, window, cx| {
            shell.update(cx, |s, cx| f(s, window, cx))
        })
        .is_ok()
}

/// Settings, on `tab` or on its list of sections: on a phone, its screen on
/// the Settings tab. Whether the phone took it.
pub fn open_settings(tab: Option<crate::settings::Tab>, cx: &mut App) -> bool {
    on_shell(cx, |shell, window, cx| shell.go_settings(tab, window, cx))
}

/// A link Settings answers (an invite, a pairing), on a phone: on its
/// section's screen. Whether the phone took it.
pub fn answer_settings(link: crate::settings::Incoming, cx: &mut App) -> bool {
    on_shell(cx, |shell, window, cx| {
        let settings = shell.settings(window, cx);
        settings.update(cx, |s, cx| s.answer(link, window, cx));
        let tab = settings.read(cx).tab();
        shell.go_settings(Some(tab), window, cx);
    })
}

/// EQ for `device`, or the output in use: on a phone, its screen on the tab
/// showing. Whether the phone took it.
pub fn open_eq(device: Option<String>, cx: &mut App) -> bool {
    on_shell(cx, |shell, window, cx| shell.go_eq(device, window, cx))
}

/// The Search tab's first page: the field, and what it finds under it, as
/// the iOS app's.
struct SearchPage {
    field: Entity<crate::components::field::FieldState>,
    results: Entity<crate::library::shelf::ShelfPage>,
    _typed: Subscription,
}

pub struct PhoneShell {
    nav: Entity<Navigator>,
    queue: Entity<QueueView>,
    /// The queue's own controls, in the bar while it shows.
    queue_controls: Entity<library::controls::ControlsSlot>,
    transport: Entity<Transport>,
    library: Option<Entity<LibraryStage>>,
    downloads: Option<Entity<crate::downloads::DownloadsPage>>,
    /// Settings' panes, made on first showing and kept, as the desktop keeps
    /// its Settings window's.
    settings: Option<Entity<crate::settings::SettingsWindow>>,
    /// What is wrong with the server, and the output still opening, under
    /// the bar on every page.
    banner: Entity<crate::server_banner::ServerBanner>,
    output_wait: Entity<crate::output_wait::OutputWaitBanner>,
    /// How far a swipe from the edge has gone toward going back, while one is
    /// under way.
    back_swipe: Option<f32>,
    /// A swipe let go: from how far it had gone, when, and whether it goes
    /// back or returns. The rest of the way eases on.
    settling: Option<(f32, std::time::Instant, bool)>,
    /// Now Playing going back down, since when.
    closing: Option<std::time::Instant>,
    /// The tab and page last drawn, so a move forward is seen and slid in.
    seen: Option<(usize, Route)>,
    /// Now Playing over everything, while it is open: a transport of its own
    /// in its full form, made on opening and let go on closing.
    now_playing: Option<(Entity<Transport>, Subscription, Subscription)>,
    /// A track's details over everything, while they are asked for: from a
    /// row's menu or Now Playing's title.
    info: Option<(i64, Entity<crate::library::info::TrackInfo>)>,
    /// EQ's model and page, made on first showing and kept.
    eq: Option<(Entity<crate::dsp::Dsp>, Entity<crate::dsp::EqPage>)>,
    /// The Search tab's field and the results under it, made once the
    /// library has loaded.
    search: Option<SearchPage>,
    _subscriptions: Vec<Subscription>,
}

impl PhoneShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|_| Navigator::default());
        let queue = cx.new(|cx| QueueView::new(&nav, cx));
        let queue_controls = cx.new(|cx| {
            let mut slot = library::controls::ControlsSlot::default();
            slot.show(&queue, QueueView::controls, cx);
            slot
        });
        let transport = cx.new(|cx| {
            let mut transport = Transport::new(&nav, window, cx);
            transport.set_form(transport::Form::Mini, cx);
            transport
        });
        window.focus(&queue.focus_handle(cx), cx);
        cx.set_global(Screens(cx.weak_entity()));
        let navigator = nav.clone();
        cx.spawn_in(window, async move |this, cx| {
            let browse = cx
                .background_executor()
                .spawn(async { library::browse::Browse::load() })
                .await;
            this.update_in(cx, |this, window, cx| {
                let engine = crate::engine::engine(cx);
                let lib = library::Lib::new(engine, browse, navigator, None, cx);
                this.queue
                    .update(cx, |queue, cx| queue.set_lib(lib.clone(), cx));
                let feed = lib.feed.clone();
                this.transport
                    .update(cx, |transport, cx| transport.set_feed(feed, cx));
                // The playlists page follows the library's playlists, and
                // the details over every page what the library asks for.
                let feed = lib.feed.clone();
                this._subscriptions
                    .push(cx.observe(&feed, |_, _, cx| cx.notify()));
                let library_nav = lib.nav.clone();
                this._subscriptions
                    .push(cx.observe(&library_nav, |_, _, cx| cx.notify()));
                this.library = Some(cx.new(|cx| LibraryStage::new(lib, window, cx)));
                cx.notify();
            })
            .ok();
        })
        .detach();
        let subscriptions = vec![
            // A link followed from Now Playing goes to its page, which Now
            // Playing would cover.
            cx.observe(&nav, |this, _, cx| {
                this.now_playing = None;
                if flight::flying(cx) == Some(transport::NOW_PLAYING) {
                    flight::land(cx);
                }
                cx.notify();
            }),
            cx.subscribe_in(
                &transport,
                window,
                |this, _, _: &transport::Expand, window, cx| this.open_now_playing(window, cx),
            ),
            {
                let asking = ask::asking(cx);
                cx.observe(&asking, |_, _, cx| cx.notify())
            },
            // The activity card comes and goes with what is under way.
            {
                let activity = cx
                    .global::<crate::library::activity::ActivityHandle>()
                    .0
                    .clone();
                cx.observe(&activity, |_, _, cx| cx.notify())
            },
        ];
        Self {
            nav,
            queue,
            queue_controls,
            transport,
            library: None,
            downloads: None,
            settings: None,
            search: None,
            eq: None,
            info: None,
            back_swipe: None,
            settling: None,
            closing: None,
            seen: None,
            now_playing: None,
            banner: cx.new(crate::server_banner::ServerBanner::new),
            output_wait: cx.new(crate::output_wait::OutputWaitBanner::new),
            _subscriptions: subscriptions,
        }
    }

    fn search_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = Paint::of(window, cx);
        if self.search.is_none()
            && let Some(lib) = self.library.as_ref().map(|l| l.read(cx).lib())
        {
            let field = cx.new(|cx| {
                crate::components::field::FieldState::new(window, cx)
                    .placeholder("albums, artists, tracks")
            });
            let search = lib.search.clone();
            let typed = cx.subscribe(
                &field,
                move |_, field, event: &crate::components::field::InputEvent, cx| {
                    if matches!(event, crate::components::field::InputEvent::Change) {
                        let text = field.read(cx).value().to_string();
                        search.update(cx, |search, cx| search.set_query(&text, cx));
                    }
                },
            );
            let results = cx
                .new(|cx| crate::library::shelf::ShelfPage::results(lib, window, cx).below_field());
            self.search = Some(SearchPage {
                field,
                results,
                _typed: typed,
            });
        }
        let Some(page) = &self.search else {
            return div().size_full().into_any_element();
        };
        let asked = self
            .library
            .as_ref()
            .is_some_and(|l| !l.read(cx).lib().search.read(cx).query().is_empty());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .mt(crate::components::bar_inset(window, cx))
                    .px(p.space(crate::theme::Space::L))
                    .py(p.space(crate::theme::Space::M))
                    .child(
                        crate::components::FilterField::new(&page.field)
                            .label("search")
                            .fill(),
                    ),
            )
            // An empty field finds nothing: the last query's results go with
            // its text.
            .when(asked, |d| {
                d.child(
                    div().flex_1().min_h_0().child(
                        page.results
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
            })
            .into_any_element()
    }

    /// A row that opens `route` on the tab showing.
    fn opens(&self, id: &'static str, label: &'static str, icon: Icon, route: Route) -> NavRow {
        let nav = self.nav.clone();
        NavRow::new(id, label).icon(icon).on_click(move |_, _, cx| {
            nav.update(cx, |nav, cx| {
                if nav.go(route) {
                    cx.notify();
                }
            })
        })
    }

    /// The Library tab's first page: the library's sections, as the iOS app
    /// lists them. The tracks have no section here: a phone finds a track
    /// through its record or search.
    fn sections(&self, _: &mut Context<Self>) -> AnyElement {
        ScrollArea::new(
            "library-sections",
            div()
                .flex()
                .flex_col()
                .child(self.opens("lib-albums", "albums", Icon::Album, Route::Albums))
                .child(self.opens("lib-artists", "artists", Icon::Artist, Route::Artists))
                .child(self.opens(
                    "lib-favourites",
                    "favourites",
                    Icon::Favourite,
                    Route::Favourites,
                ))
                .child(self.opens(
                    "lib-playlists",
                    "playlists",
                    Icon::Playlist,
                    Route::Playlists,
                ))
                .child(self.opens(
                    "lib-recent",
                    "recently played",
                    Icon::RecentlyPlayed,
                    Route::RecentlyPlayed,
                ))
                .child(self.opens(
                    "lib-downloaded",
                    "downloaded",
                    Icon::OnDevice,
                    Route::OnDevice,
                ))
                .child(self.opens("lib-history", "history", Icon::History, Route::History))
                .child(self.opens(
                    "lib-downloads",
                    "downloads",
                    Icon::Downloads,
                    Route::Downloads,
                )),
        )
        .under_bar()
        .into_any_element()
    }

    /// The playlists, each opening its page.
    fn playlists(&self, cx: &mut Context<Self>) -> AnyElement {
        let playlists = self
            .library
            .as_ref()
            .map(|library| library.read(cx).lib().feed.read(cx).playlists.clone())
            .unwrap_or_default();
        let nav = self.nav.clone();
        ScrollArea::new(
            "playlists",
            div()
                .flex()
                .flex_col()
                .children(playlists.into_iter().map(|(id, name)| {
                    let nav = nav.clone();
                    NavRow::data(("playlist", id as u64), name)
                        .icon(Icon::Playlist)
                        .on_click(move |_, _, cx| {
                            nav.update(cx, |nav, cx| {
                                if nav.go(Route::Playlist(id)) {
                                    cx.notify();
                                }
                            })
                        })
                })),
        )
        .under_bar()
        .into_any_element()
    }

    /// Settings' model and panes, made on first showing and kept, as the
    /// desktop keeps its Settings window's.
    fn settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<crate::settings::SettingsWindow> {
        self.settings
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut settings = crate::settings::SettingsWindow::new(
                        crate::settings::Tab::General,
                        window,
                        cx,
                    );
                    settings.as_screen();
                    settings
                })
            })
            .clone()
    }

    /// How far a swipe back stands, while one is under way or easing to
    /// its end; going back once it reaches it.
    fn swipe(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<f32> {
        // A move forward within a tab is the swipe back run the other way:
        // the new page comes in from the trailing edge over the one left.
        let nav = self.nav.read(cx);
        let now = (nav.tab(), nav.current());
        if let Some(was) = self.seen
            && was.0 == now.0
            && was.1 != now.1
            && nav.behind() == Some(was.1)
            && self.back_swipe.is_none()
            && self.settling.is_none()
        {
            self.settling = Some((1.0, std::time::Instant::now(), false));
        }
        self.seen = Some(now);
        if let Some(progress) = self.back_swipe {
            return Some(progress);
        }
        let (from, started, back) = self.settling?;
        let t = started.elapsed().as_secs_f32() / crate::theme::Motion::SETTLE.as_secs_f32();
        let to = if back { 1.0 } else { 0.0 };
        if t >= 1.0 {
            self.settling = None;
            if back {
                self.back(cx);
            } else if let Some(library) = &self.library {
                library.update(cx, |library, _| library.drop_under());
            }
            return None;
        }
        window.request_animation_frame();
        Some(from + (to - from) * crate::theme::Motion::quick_out(t))
    }

    /// What the record page in front flies its sleeve under.
    fn sleeve(&self, cx: &App) -> Option<crate::components::FlightKey> {
        match self.nav.read(cx).current() {
            Route::Album(id) => Some(library::tile::sleeve_key(id)),
            _ => None,
        }
    }

    /// The record page's sleeve let go, eased to `to` (0 its tile, 1 the
    /// header) as the page settles.
    fn release_sleeve(&self, to: f32, cx: &mut App) {
        if self.sleeve(cx).is_some() && flight::flying(cx) == self.sleeve(cx) {
            flight::release(
                to,
                crate::theme::Motion::SETTLE,
                crate::components::Ease::QuickOut,
                cx,
            );
        }
    }

    /// A record's page swiped `progress` of the way back: its header's
    /// sleeve follows the finger toward its tile on the page under it,
    /// landing there if that page draws it.
    fn fly_back(&self, progress: f32, cx: &mut App) {
        let Some(key) = self.sleeve(cx) else { return };
        if flight::flying(cx) == Some(key) || flight::recall(key, None, cx) {
            flight::drive(1.0 - progress, cx);
        }
    }

    /// The page under a swipe back: the library's made ahead and kept to be
    /// the page, the shell's own drawn as they are.
    fn under(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let full = || StyleRefinement::default().size_full();
        if LibraryStage::draws(route) {
            let view = self
                .library
                .as_ref()
                .and_then(|library| library.update(cx, |l, cx| l.under(route, window, cx)));
            return match view {
                Some(view) => view.cached(full()).into_any_element(),
                None => div().into_any_element(),
            };
        }
        if route == Route::Downloads {
            return match &self.downloads {
                Some(page) => page.clone().cached(full()).into_any_element(),
                None => div().into_any_element(),
            };
        }
        self.stage(route, window, cx)
    }

    /// Back by the bar's button: the page slides away as a swipe back's
    /// release does, and the move happens as it lands.
    fn animate_back(&mut self, cx: &mut Context<Self>) {
        if self.nav.read(cx).at_root() || self.settling.is_some() {
            return;
        }
        self.back_swipe = None;
        self.settling = Some((0.0, std::time::Instant::now(), true));
        if let Some(key) = self.sleeve(cx) {
            flight::recall(
                key,
                Some((
                    crate::theme::Motion::SETTLE,
                    crate::components::Ease::QuickOut,
                )),
                cx,
            );
        }
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.nav.update(cx, |nav, cx| {
            if nav.back() {
                cx.notify();
            }
        });
    }

    fn stage(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let full = || StyleRefinement::default().size_full();
        if self.nav.read(cx).current() != Route::Downloads {
            self.downloads = None;
        }
        match route {
            Route::Queue => self.queue.clone().cached(full()).into_any_element(),
            Route::Library => self.sections(cx),
            Route::SearchTab => self.search_page(window, cx),
            Route::Playlists => self.playlists(cx),
            Route::Downloads => {
                let nav = self.nav.clone();
                self.downloads
                    .get_or_insert_with(|| {
                        cx.new(|cx| crate::downloads::DownloadsPage::new(&nav, cx))
                    })
                    .clone()
                    .cached(full())
                    .into_any_element()
            }
            Route::Settings => {
                let settings = self.settings(window, cx);
                // No Library pane: a phone keeps no folders to scan, and
                // its library is the server's, as on iOS.
                let mut tabs = settings.read(cx).tabs(cx);
                tabs.retain(|tab| *tab != crate::settings::Tab::Library);
                let nav = self.nav.clone();
                crate::components::ScrollArea::new(
                    "settings-sections",
                    div()
                        .flex()
                        .flex_col()
                        .children(tabs.into_iter().map(|tab| {
                            let (nav, settings) = (nav.clone(), settings.clone());
                            crate::components::NavRow::new(
                                SharedString::from(format!("settings-{}", tab.title())),
                                tab.title().to_lowercase(),
                            )
                            .icon(tab.icon())
                            .on_click(move |_, window, cx| {
                                settings.update(cx, |s, cx| s.choose(tab, window, cx));
                                nav.update(cx, |nav, cx| {
                                    if nav.go(Route::SettingsSection(tab)) {
                                        cx.notify();
                                    }
                                });
                            })
                        })),
                )
                .under_bar()
                .into_any_element()
            }
            Route::Eq => match &self.eq {
                Some((_, page)) => page.clone().cached(full()).into_any_element(),
                None => div().size_full().into_any_element(),
            },
            Route::SettingsSection(tab) => {
                let settings = self.settings(window, cx);
                // Back and forward land here too: the pane follows the route.
                settings.update(cx, |s, cx| s.choose(tab, window, cx));
                settings.cached(full()).into_any_element()
            }
            route if LibraryStage::draws(route) => match &self.library {
                Some(library) => library.clone().cached(full()).into_any_element(),
                None => div().size_full().into_any_element(),
            },
            _ => div().size_full().into_any_element(),
        }
    }

    /// The bar over the page: back while the tab has somewhere to go back
    /// to, and the name of a page that has no header of its own.
    /// A page's bar, drawn with its page so that it comes and goes with it
    /// as a page slides in or is swiped back: `root` when the page is its
    /// tab's first, `live` for the page in front, whose controls are the
    /// ones shown (the page under a swipe draws its name and back alone).
    fn bar(&self, route: Route, root: bool, live: bool, cx: &mut Context<Self>) -> Toolbar {
        // Named where the page has no heading of its own, or where its heading
        // scrolls away with its rows.
        let title = match route {
            Route::SearchTab => Some("search"),
            Route::Library => Some("library"),
            Route::Playlists => Some("playlists"),
            Route::Eq => Some("EQ"),
            Route::Settings => Some("settings"),
            Route::SettingsSection(tab) => Some(tab.title()),
            // A browser's heading scrolls away with its rows: the bar keeps
            // its name, as the iOS app's does.
            Route::Albums => Some("albums"),
            Route::Artists => Some("artists"),
            Route::Tracks => Some("tracks"),
            _ => None,
        };
        // The page's own controls: the queue's, a library page's filter and
        // sort.
        let library_controls = self
            .library
            .as_ref()
            .filter(|_| live)
            .filter(|library| LibraryStage::draws(route) && library.read(cx).has_controls(cx))
            .and_then(|library| library.read(cx).toolbar());
        // A browser's filter field is its list's, scrolling away under its
        // heading; the bar holds its buttons, as every page's.
        let controls: Option<AnyView> = if !live {
            None
        } else if route == Route::Queue {
            Some(self.queue_controls.clone().into())
        } else {
            library_controls
        };
        Toolbar::new()
            .flush()
            .when(!root, |bar| {
                bar.leading(
                    ToolbarButton::new("back", Icon::Back, "back")
                        .on_click(cx.listener(|this, _, _, cx| this.animate_back(cx))),
                )
            })
            .principal(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .children(title.map(ToolbarTitle::new)),
            )
            .when_some(controls, |bar, controls: AnyView| bar.trailing(controls))
    }

    /// Settings' tab, on `tab`'s screen or its list of sections.
    fn go_settings(
        &mut self,
        tab: Option<crate::settings::Tab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.now_playing = None;
        let at = TABS
            .iter()
            .position(|(root, ..)| *root == Route::Settings)
            .unwrap_or(0);
        self.nav.update(cx, |nav, cx| {
            if nav.tab() != at {
                nav.switch(at, Route::Settings);
            }
            // The tab as it was left may be deep in a section: back to its
            // list first, then on to the section asked for.
            if !nav.at_root() {
                nav.switch(at, Route::Settings);
            }
            if let Some(tab) = tab {
                nav.go(Route::SettingsSection(tab));
            }
            cx.notify();
        });
        if let Some(tab) = tab {
            let settings = self.settings(window, cx);
            settings.update(cx, |s, cx| s.choose(tab, window, cx));
        }
    }

    /// EQ's screen on the tab showing.
    fn go_eq(&mut self, device: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.now_playing = None;
        match &self.eq {
            Some((_, page)) => page.update(cx, |p, cx| p.show(device, cx)),
            None => {
                let dsp = crate::dsp::Dsp::new(crate::engine::engine(cx), cx);
                let page = cx.new(|cx| crate::dsp::EqPage::new(dsp.clone(), device, window, cx));
                self.eq = Some((dsp, page));
            }
        }
        self.nav.update(cx, |nav, cx| {
            nav.go(Route::Eq);
            cx.notify();
        });
    }

    /// Goes to `route` on the tab showing, as a link would; for tools that
    /// walk the pages headlessly.
    pub fn show(&mut self, route: Route, cx: &mut Context<Self>) {
        self.now_playing = None;
        self.nav.update(cx, |nav, cx| {
            if nav.go(route) {
                cx.notify();
            }
        });
    }

    /// Opens Now Playing, as tapping the mini player does.
    pub fn open_now_playing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.now_playing.is_some() {
            return;
        }
        let nav = self.nav.clone();
        let full = cx.new(|cx| {
            let mut full = Transport::new(&nav, window, cx);
            full.set_form(transport::Form::Full, cx);
            full
        });
        if let Some(lib) = self.library.as_ref().map(|l| l.read(cx).lib()) {
            let feed = lib.feed.clone();
            full.update(cx, |full, cx| {
                full.set_feed(feed, cx);
                full.set_lib(lib);
            });
        }
        // Going down, the art shrinks back into the mini player's sleeve as
        // Now Playing slides away.
        let closed = cx.subscribe(&full, |this, full, _: &transport::Collapse, cx| {
            if this.closing.is_none() && full.read(cx).shows_art() {
                flight::recall(
                    transport::NOW_PLAYING,
                    Some((crate::theme::Motion::SETTLE, crate::components::Ease::InOut)),
                    cx,
                );
            }
            this.closing.get_or_insert_with(std::time::Instant::now);
            cx.notify();
        });
        // The title asks for the track's details, over the page Now Playing
        // closes to.
        let info = cx.subscribe(&full, |this, _, asked: &transport::ShowInfo, cx| {
            this.now_playing = None;
            if let Some(lib) = this.library.as_ref().map(|l| l.read(cx).lib()) {
                lib.nav.update(cx, |nav, cx| nav.show_info(asked.0, cx));
            }
            cx.notify();
        });
        // The mini player's sleeve grows into Now Playing's art as it slides
        // up, on the slide's clock.
        let art = self.transport.read(cx).cover();
        flight::depart(
            transport::NOW_PLAYING,
            &art,
            None,
            crate::theme::Motion::SETTLE * 2,
            crate::components::Ease::QuickOut,
            cx,
        );
        self.closing = None;
        self.now_playing = Some((full, closed, info));
        cx.notify();
    }

    fn tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let showing = self.nav.read(cx).tab();
        TABS.iter().enumerate().fold(
            TabStrip::new().bar(),
            |strip, (ix, &(root, title, icon))| {
                let nav = self.nav.clone();
                strip.tab(icon, title, ix == showing, move |_, _, cx| {
                    nav.update(cx, |nav, cx| {
                        nav.switch(ix, root);
                        cx.notify();
                    });
                })
            },
        )
    }
}

impl Render for PhoneShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        // Now Playing over the page, on a wash of its own as the iOS app's
        // bleed is, coming up from the foot and going back down to it.
        if self
            .closing
            .is_some_and(|at| at.elapsed() >= crate::theme::Motion::SETTLE)
        {
            self.closing = None;
            self.now_playing = None;
        } else if self.closing.is_some() {
            window.request_animation_frame();
        }
        let now_playing = self.now_playing.as_ref().map(|(full, ..)| {
            let wash = cx.global::<crate::root::WindowWash>().0.clone();
            div().absolute().inset_0().occlude().child(
                crate::components::SlideUp::new(
                    div()
                        .relative()
                        .size_full()
                        .map(|d| match wash {
                            Some(wash) => d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(wash.cached(StyleRefinement::default().size_full())),
                            ),
                            None => d.bg(p.bg),
                        })
                        .child(full.clone().cached(StyleRefinement::default().size_full())),
                )
                .leaving(self.closing.is_some()),
            )
        });
        let swipe = self.swipe(window, cx);
        let route = self.nav.read(cx).current();
        crate::components::page_shown(route, window, cx);
        let stage = self.stage(route, window, cx);
        let wanted = self
            .library
            .as_ref()
            .and_then(|library| library.read(cx).lib().nav.read(cx).info);
        if self.info.as_ref().map(|(id, _)| *id) != wanted {
            let lib = self.library.as_ref().map(|library| library.read(cx).lib());
            self.info = wanted.zip(lib).map(|(id, lib)| {
                (
                    id,
                    cx.new(|cx| crate::library::info::TrackInfo::new(lib, id, window, cx)),
                )
            });
        }
        let activity = cx
            .global::<crate::library::activity::ActivityHandle>()
            .0
            .clone();
        // The page fills the screen, top to foot. Over it, frosted: at the
        // top the status bar, the bar and the notices under it; at the foot
        // the mini player, the tabs and the home indicator. Each measures
        // itself into the room pages leave for it (`bar_inset`,
        // `foot_inset`).
        let insets = crate::components::system_insets(Clear::System, window);
        let width = window.viewport_size().width - insets.left - insets.right;
        // A flight between a record's tile and its header runs between
        // where the two will rest: the page under stands a third of its
        // way behind, the page in front `progress` of the way off.
        let (behind, off) = swipe.map_or((Pixels::ZERO, Pixels::ZERO), |progress| {
            (-width * 0.3 * (1.0 - progress), width * progress)
        });
        flight::page_offsets(behind, off, cx);
        // The page a swipe back uncovers, drawn under the one going, a third
        // of its way behind and dimmed as UIKit's navigation does.
        let under = swipe.and_then(|progress| {
            let nav = self.nav.read(cx);
            let behind = nav.behind()?;
            let behind_root = nav.history_ix() == 1;
            let bar = self.bar(behind, behind_root, false, cx);
            let page = self.under(behind, window, cx);
            Some((progress, under_bar(bar, page, None, false, insets.top)))
        });
        let stage = {
            let root = self.nav.read(cx).at_root();
            // What is wrong with the server and the output still opening
            // hang under the page in front's bar.
            let notices = div()
                .child(self.banner.clone())
                .child(self.output_wait.clone());
            under_bar(
                self.bar(route, root, true, cx),
                stage,
                Some(notices),
                true,
                insets.top,
            )
        };
        div()
            .key_context(crate::platform::actions::CONTEXT)
            // The swipe from the edge: the page follows the finger, and goes
            // back only once the platform says the swipe went far enough.
            .on_back_gesture(cx.listener(|this, event: &BackGestureEvent, _, cx| {
                // Now Playing covers the page: the page underneath stays put.
                if this.now_playing.is_some() {
                    return;
                }
                let now = std::time::Instant::now();
                match event.phase {
                    BackGesturePhase::Started | BackGesturePhase::Progressed => {
                        this.settling = None;
                        this.back_swipe = (!this.nav.read(cx).at_root()).then_some(event.progress);
                        if let Some(progress) = this.back_swipe {
                            this.fly_back(progress, cx);
                        }
                    }
                    // Let go, the sleeve is handed the rest of the way on the
                    // page's curve: to its tile, or back to the header.
                    BackGesturePhase::Committed => {
                        if let Some(from) = this.back_swipe.take() {
                            this.settling = Some((from, now, true));
                            this.release_sleeve(0.0, cx);
                        }
                    }
                    BackGesturePhase::Cancelled => {
                        if let Some(from) = this.back_swipe.take() {
                            this.settling = Some((from, now, false));
                            this.release_sleeve(1.0, cx);
                        }
                    }
                }
                cx.notify();
            }))
            .relative()
            .size_full()
            .text_color(p.ink)
            .pl(insets.left)
            .pr(insets.right)
            .child(
                div()
                    .id("stage")
                    .role(a11y::Role::Main)
                    .relative()
                    .size_full()
                    .map(|d| match under {
                        None => d.child(stage),
                        // The pages are drawn over the window's wash, which
                        // stays put: the page going sits over the uncovered
                        // strip alone, a third of its way behind and dimmed
                        // as UIKit's navigation does, so the two never
                        // overlap and neither needs a ground of its own.
                        Some((progress, under)) => d
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .w(width * progress)
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .w(width)
                                            .left(-width * 0.3 * (1.0 - progress))
                                            .child(under),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .bg(p.look.veil(0.25 * (1.0 - progress))),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .w(width)
                                    .left(width * progress)
                                    .overflow_hidden()
                                    .border_l(p.look.hairline)
                                    .border_color(p.rule)
                                    .child(stage),
                            ),
                    })
                    // What the library is busy with, floating over the foot of
                    // every page while anything runs, as the iOS app has it:
                    // the sidebar foot that holds it on the desktop is gone.
                    .children(activity.read(cx).shows(cx).then(|| {
                        div()
                            .absolute()
                            .left(p.space(crate::theme::Space::M))
                            .right(p.space(crate::theme::Space::M))
                            .bottom(
                                crate::components::foot_inset(window, cx)
                                    + p.space(crate::theme::Space::S),
                            )
                            .child(crate::components::Rise::in_place(
                                div()
                                    .px(p.space(crate::theme::Space::S))
                                    .py(p.space(crate::theme::Space::Xs))
                                    .bg(p.look.palette.surface.hsla())
                                    .border(p.look.hairline)
                                    .border_color(p.rule)
                                    .child(activity.clone()),
                            ))
                    })),
            )
            // The foot over the page: the mini player, its playhead along the
            // foot's edge, over the tabs under a rule; frosted as the iOS
            // app's bar and running on under the home indicator.
            .child(
                div()
                    .id("phone-foot")
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .occlude()
                    .pb(insets.bottom)
                    .child(crate::components::Frost::foot())
                    .child(crate::components::foot_probe())
                    .child(
                        self.transport.clone().cached(
                            StyleRefinement::default()
                                .w_full()
                                .h(p.look.spaced(transport::MINI_HEIGHT))
                                .flex_shrink_0(),
                        ),
                    )
                    .child(
                        div()
                            .border_t(p.look.hairline)
                            .border_color(p.rule)
                            .child(self.tabs(cx)),
                    ),
            )
            // A tap on the status bar takes the page back to its top, as on
            // iOS.
            .child(
                div()
                    .id("status-bar-tap")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(insets.top)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav.update(cx, |nav, cx| {
                            let route = nav.current();
                            nav.choose(route);
                            cx.notify();
                        });
                    })),
            )
            .children(now_playing)
            // A sleeve flying between pages, over them and under any sheet.
            .child(flight::overlay())
            .children(ask::presented(window, cx).map(|s| ask::layered(Layer::Sheet, s)))
            .children(ask::overlay(window, cx).map(|q| ask::layered(Layer::Question, q)))
            .children(crate::artwork::overlay(window, cx).map(|v| ask::layered(Layer::Sheet, v)))
            .children(
                self.info
                    .as_ref()
                    .map(|(_, info)| ask::layered(Layer::Sheet, info.clone())),
            )
            .into_any_element()
    }
}

/// A page under its bar, the two moving as one: the page fills the screen
/// and scrolls under the bar, which is frosted and reaches up under the
/// status bar (`status`). `notices` hang under the bar, frosted with it. The
/// page leaves the room they take at its top (`bar_inset`), so at rest
/// nothing is under them; the page in front (`live`) measures it.
fn under_bar(
    bar: Toolbar,
    page: AnyElement,
    notices: Option<Div>,
    live: bool,
    status: Pixels,
) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(page)
        .child(
            div()
                .id("phone-bar")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .flex()
                .flex_col()
                .occlude()
                .pt(status)
                .child(crate::components::Frost::bar())
                .when(live, |d| d.child(crate::components::bar_probe()))
                .child(bar)
                .children(notices),
        )
        .into_any_element()
}

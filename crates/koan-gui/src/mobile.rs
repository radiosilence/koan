//! The window on a phone, as the iOS app lays it out: a bar naming the page
//! with the way back, the page, the transport, and the tabs along the foot,
//! each tab a history of its own (`Navigator::switch`). Clear of the status
//! bar, the notch and the home indicator; the wash runs under them.
//!
//! The pages are the desktop's: the queue, the library's pages, downloads
//! and Settings' panes. The Library tab's first page is the sidebar's list.

use koan_kit::app::*;

use crate::components::sheet::{self as ask, Layer};
use crate::components::{Clear, KoanInsets, TabStrip, Toolbar, ToolbarButton, ToolbarTitle};
use crate::library::{self, stage::LibraryStage};
use crate::navigator::{Chosen, Navigator, Route};
use crate::paint::Paint;
use crate::queue::QueueView;
use crate::sidebar::Sidebar;
use crate::theme::{Icon, KoanStyled, TypeRole};
use crate::transport::{self, Transport};

/// Whether the app takes the phone's layout: on iOS, and on a desktop with
/// `KOAN_GUI_PHONE` set, so it can be walked at a phone's size.
pub fn phone() -> bool {
    cfg!(target_os = "ios") || std::env::var_os("KOAN_GUI_PHONE").is_some()
}

/// The tabs in the order the bar shows them, each with its first page.
const TABS: [(Route, &str, Icon); 3] = [
    (Route::Queue, "queue", Icon::QueueSection),
    (Route::Library, "library", Icon::Library),
    (Route::Settings, "settings", Icon::Settings),
];

pub struct PhoneShell {
    nav: Entity<Navigator>,
    queue: Entity<QueueView>,
    sidebar: Entity<Sidebar>,
    transport: Entity<Transport>,
    library: Option<Entity<LibraryStage>>,
    downloads: Option<Entity<crate::downloads::DownloadsPage>>,
    /// Settings' panes, made on first showing and kept, as the desktop keeps
    /// its Settings window's.
    settings: Option<Entity<crate::settings::SettingsWindow>>,
    /// The search sheet over the tab showing, while it is open.
    search: Option<(Entity<crate::search::bar::SearchBar>, Subscription)>,
    _subscriptions: Vec<Subscription>,
}

impl PhoneShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|_| Navigator::default());
        let queue = cx.new(|cx| QueueView::new(&nav, cx));
        let sidebar = cx.new(|cx| Sidebar::new(&nav, cx));
        let transport = cx.new(|cx| Transport::new(&nav, window, cx));
        window.focus(&queue.focus_handle(cx), cx);
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
                let (covers, search) = (lib.covers.clone(), lib.search.clone());
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
        let subscriptions = vec![cx.observe(&nav, |_, _, cx| cx.notify()), {
            let asking = ask::asking(cx);
            cx.observe(&asking, |_, _, cx| cx.notify())
        }];
        Self {
            nav,
            queue,
            sidebar,
            transport,
            library: None,
            downloads: None,
            settings: None,
            search: None,
            _subscriptions: subscriptions,
        }
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

    fn back(&mut self, cx: &mut Context<Self>) {
        self.nav.update(cx, |nav, cx| {
            if nav.back() {
                cx.notify();
            }
        });
    }

    fn stage(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let full = || StyleRefinement::default().size_full();
        if route != Route::Downloads {
            self.downloads = None;
        }
        match route {
            Route::Queue => self.queue.clone().cached(full()).into_any_element(),
            Route::Library => self.sidebar.clone().cached(full()).into_any_element(),
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
                let tabs = settings.read(cx).tabs(cx);
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
                .into_any_element()
            }
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
    fn bar(&self, route: Route, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = self.nav.read(cx);
        let title = match route {
            Route::Queue => Some("queue"),
            Route::Library => Some("library"),
            Route::Settings => Some("settings"),
            Route::SettingsSection(tab) => Some(tab.title()),
            Route::History => Some("history"),
            Route::Downloads => Some("downloads"),
            _ => None,
        };
        Toolbar::new()
            .when(!nav.at_root(), |bar| {
                bar.leading(
                    ToolbarButton::new("back", Icon::Back, "back")
                        .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                )
            })
            .principal(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .children(title.map(ToolbarTitle::new)),
            )
    }

    /// The search sheet over the tab showing, holding the keyboard. Search
    /// is a tab of the bar's that opens it rather than a place of its own,
    /// so what was showing stays under it.
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            return;
        }
        let Some(lib) = self.library.as_ref().map(|l| l.read(cx).lib()) else {
            return;
        };
        let bar = cx.new(|cx| crate::search::bar::SearchBar::new(lib, window, cx));
        window.focus(&bar.focus_handle(cx), cx);
        let dismissed = cx.subscribe(&bar, |this, _, _: &crate::search::bar::Dismissed, cx| {
            this.search = None;
            cx.notify();
        });
        self.search = Some((bar, dismissed));
        cx.notify();
    }

    fn tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let showing = self.nav.read(cx).tab();
        let strip =
            TABS.iter()
                .enumerate()
                .fold(TabStrip::new(), |strip, (ix, &(root, title, icon))| {
                    let nav = self.nav.clone();
                    strip.tab(icon, title, ix == showing, move |_, _, cx| {
                        nav.update(cx, |nav, cx| {
                            let chosen = nav.switch(ix, root);
                            if chosen == Chosen::Moved || nav.at_root() {
                                cx.notify();
                            }
                        });
                    })
                });
        let shell = cx.entity();
        strip.tab(
            Icon::Search,
            "search",
            self.search.is_some(),
            move |_, window, cx| shell.update(cx, |shell, cx| shell.open_search(window, cx)),
        )
    }
}

impl Render for PhoneShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let route = self.nav.read(cx).current();
        let stage = self.stage(route, window, cx);
        div()
            .key_context(crate::platform::actions::CONTEXT)
            .on_back_gesture(cx.listener(|this, _, _, cx| this.back(cx)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.ink)
            .clear_of_system(Clear::System, window)
            .child(self.bar(route, cx))
            .child(
                div()
                    .id("stage")
                    .role(a11y::Role::Main)
                    .flex_1()
                    .min_h_0()
                    .child(stage),
            )
            .child(
                self.transport.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(transport::HEIGHT)
                        .flex_shrink_0(),
                ),
            )
            .child(
                div()
                    .pt(p.space(crate::theme::Space::S))
                    .text_role(TypeRole::Fine, &p.look)
                    .child(self.tabs(cx)),
            )
            .children(ask::presented(window, cx).map(|s| ask::layered(Layer::Sheet, s)))
            .children(ask::overlay(window, cx).map(|q| ask::layered(Layer::Question, q)))
            .children(crate::artwork::overlay(window, cx).map(|v| ask::layered(Layer::Sheet, v)))
            .children(
                self.search
                    .as_ref()
                    .map(|(bar, _)| ask::layered(Layer::Sheet, bar.clone())),
            )
    }
}

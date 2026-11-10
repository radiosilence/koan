//! The touch shell with a tablet's room: the desktop's sidebar, page and
//! lyrics inspector side by side, the desktop's transport bar over the foot,
//! all inside the safe area. A narrower window (Split View, Stage Manager) is
//! a phone's layout, so the two meet in one shell and the place in the app
//! survives a resize across them.

use koan_kit::app::*;

use super::TouchShell;
use crate::components::{Clear, NavRow, RowMetrics, ScrollArea, Slide, SplitView};
use crate::navigator::Route;
use crate::paint::Paint;
use crate::sidebar::Sidebar;
use crate::theme::{self, Space};
use crate::transport;
use crate::window_state;

/// Narrower than this the page cannot stand between the sidebar and the
/// lyrics, so the lyrics take the sidebar's place while they show.
const ROOM_FOR_BOTH: f32 = 1000.0;

/// Settings' list of sections beside the pane showing.
const SECTIONS: f32 = 240.0;

impl TouchShell {
    /// The sidebar, made the first time a window is wide enough to show it,
    /// so that a phone keeps nothing it never draws.
    pub(super) fn sidebar(&mut self, cx: &mut Context<Self>) -> Entity<Sidebar> {
        if let Some(sidebar) = &self.sidebar {
            return sidebar.clone();
        }
        let nav = self.nav.clone();
        let sidebar = cx.new(|cx| Sidebar::new(&nav, cx));
        self.sidebar = Some(sidebar.clone());
        self.wire_sidebar(cx);
        sidebar
    }

    /// Gives the sidebar what only exists once the library has loaded.
    pub(super) fn wire_sidebar(&self, cx: &mut Context<Self>) {
        let (Some(sidebar), Some(library)) = (&self.sidebar, &self.library) else {
            return;
        };
        let lib = library.read(cx).lib();
        let (covers, search) = (lib.covers.clone(), lib.search.clone());
        sidebar.update(cx, |sidebar, cx| {
            sidebar.set_covers(covers, cx);
            sidebar.set_search(search, cx);
        });
    }

    /// Whether the sidebar is showing, not only wanted: the lyrics take its
    /// room in a window too narrow for both.
    pub(super) fn sidebar_showing(&self, window: &Window) -> bool {
        self.sidebar_shown && !(self.lyrics_shown && narrow(window))
    }

    /// The toolbar's sidebar button and its shortcut. Bringing the sidebar
    /// back to a window too narrow for both puts the lyrics away.
    pub(super) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_showing(window) {
            self.sidebar_shown = false;
        } else {
            self.sidebar_shown = true;
            if narrow(window) {
                self.lyrics_shown = false;
            }
        }
        window_state::SIDEBAR.store(self.sidebar_shown, std::sync::atomic::Ordering::Relaxed);
        window_state::save(window.window_bounds(), cx);
        cx.notify();
    }

    pub(super) fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        self.lyrics_shown = !self.lyrics_shown;
        if self.lyrics_shown && self.lyrics.is_none() {
            self.lyrics = Some(cx.new(crate::lyrics::LyricsPanel::new));
        }
        cx.notify();
    }

    /// Settings as a tablet has it: its sections beside the pane showing, so
    /// a section is one tap from any other.
    pub(super) fn settings_split(
        &mut self,
        route: Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = Paint::of(window, cx);
        let settings = self.settings(window, cx);
        let tabs = settings.read(cx).tabs(cx);
        // The section the route names (a link, back and forward, a phone's
        // push carried across), which Settings may show before the server
        // has said it offers it; else the one last shown, if this device
        // offers it. Only a section Settings refuses falls back to the
        // first, so the two never take turns.
        let last = settings.read(cx).tab();
        let wanted = match route {
            Route::SettingsSection(tab) => tab,
            _ if tabs.contains(&last) => last,
            _ => tabs.first().copied().unwrap_or(last),
        };
        if !settings.update(cx, |s, cx| s.choose(wanted, window, cx))
            && let Some(first) = tabs.first().copied()
        {
            settings.update(cx, |s, cx| s.choose(first, window, cx));
        }
        let showing = settings.read(cx).tab();
        let shell = cx.entity();
        let sections = tabs.into_iter().map(|tab| {
            let (settings, shell, nav) = (settings.clone(), shell.clone(), self.nav.clone());
            NavRow::new(
                SharedString::from(format!("settings-{}", tab.title())),
                tab.title().to_lowercase(),
            )
            .icon(tab.icon())
            .selected(tab == showing)
            .on_click(move |_, window, cx| {
                // The route names the section, so the stage keeps it.
                settings.update(cx, |s, cx| s.choose(tab, window, cx));
                nav.update(cx, |nav, _| nav.go(Route::SettingsSection(tab)));
                shell.update(cx, |_, cx| cx.notify());
            })
        });
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .w(p.look.spaced(SECTIONS))
                    .flex_none()
                    .h_full()
                    .border_r(p.look.hairline)
                    .border_color(p.rule)
                    .child(
                        ScrollArea::new(
                            "settings-sections",
                            div().flex().flex_col().children(sections),
                        )
                        .under_bar(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(settings.cached(StyleRefinement::default().size_full())),
            )
            .into_any_element()
    }

    /// The window with a tablet's room: `pages` is the page in front, with
    /// its bar, and what a swipe back uncovers.
    pub(super) fn regular(
        &mut self,
        pages: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The width has eased to nothing: the panel goes with it.
        if !self.lyrics_shown && self.lyrics.is_some() && self.lyrics_slide.closed() {
            self.lyrics = None;
        }
        let p = Paint::of(window, cx);
        let insets = crate::components::system_insets(Clear::System, window);
        let washed = theme::KoanTheme::washes_window(cx);
        let inspector = RowMetrics::inspector(&p.look);
        let lyrics_width = self.lyrics_width.unwrap_or(inspector).clamp(
            RowMetrics::inspector_min(&p.look),
            RowMetrics::inspector_max(&p.look),
        );
        let (bar_inset, foot_inset) = (
            crate::components::bar_inset(window, cx),
            crate::components::foot_inset(window, cx),
        );
        let sidebar = self.sidebar(cx);
        let showing = self.sidebar_showing(window);
        let foot = div()
            .id("tablet-foot")
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
                        .h(transport::HEIGHT)
                        .flex_shrink_0(),
                ),
            );
        let lyrics = Slide::new(
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
        .on_commit(cx.listener(move |this, width: &Pixels, window, cx| {
            this.lyrics_width = window_state::keep_lyrics_width(*width, inspector, window, cx);
        }))
        .when_some(self.lyrics.clone(), |slide, lyrics| {
            slide.child(
                lyrics.cached(
                    StyleRefinement::default()
                        .w(lyrics_width)
                        .h_full()
                        .flex_none(),
                ),
            )
        });
        div()
            .relative()
            .size_full()
            .child(
                SplitView::new(
                    "sidebar-split",
                    self.sidebar_width,
                    RowMetrics::SIDEBAR_MIN,
                    RowMetrics::SIDEBAR_MAX,
                    RowMetrics::SIDEBAR,
                )
                .shown(showing)
                .leading(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .when(!washed, |d| d.bg(p.bg))
                        // Clear of the status bar at the top and of the
                        // transport at the foot.
                        .pt(insets.top + p.space(Space::S))
                        .pb(foot_inset)
                        .child(
                            div()
                                .id("sidebar-region")
                                .role(a11y::Role::Navigation)
                                .aria_label("library")
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .child(sidebar.cached(
                                    StyleRefinement::default().w_full().flex_1().min_h_0(),
                                )),
                        ),
                )
                .trailing(
                    div().relative().size_full().child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .child(div().relative().flex_1().min_w_0().h_full().child(pages))
                            .child(div().h_full().pt(bar_inset).pb(foot_inset).child(lyrics)),
                    ),
                )
                .on_resize(cx.listener(|this, width: &Pixels, _, cx| {
                    this.sidebar_width = *width;
                    cx.notify();
                }))
                .on_commit(|width: &Pixels, window, cx| {
                    window_state::keep_sidebar_width(*width, window, cx)
                }),
            )
            // Under both columns whether or not the window is washed: a
            // tablet's page column alone is too narrow for the bar to keep
            // the record's name beside its controls.
            .child(foot)
            .into_any_element()
    }
}

fn narrow(window: &Window) -> bool {
    f32::from(window.viewport_size().width) < ROOM_FOR_BOTH
}

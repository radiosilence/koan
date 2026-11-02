//! The Settings window: everything needed to set koan up without a terminal.
//! The Mac app's `SettingsView`, in the kōan look only.
//!
//! The model and this device's sharing are app-wide entities (`Settings`),
//! made as the app opens and kept for its life, so an action started from
//! the window finishes after it closes. The window is a thin root: the tab
//! row, the pane chosen, the status line and the sheet slot, the pane a
//! cached view of its own that observes what it draws.

mod form;
pub mod model;
pub(crate) mod panes;
mod qr;
pub mod sharing;
mod sheet;

use std::sync::Arc;

use crate::theme::{
    self, HAIRLINE, Icon, KoanIcon, KoanStyled, KoanTheme, Look, Space, Tone, TypeRole, label,
};
use koan_ffi::{ConnectionInfo, KoanEngine};
use koan_kit::app::*;
// gpui-component's focus and click extensions, for the shell's own handling.
use koan_kit::raw::base::Root;
// the bounds a cached view keeps between frames.
use koan_kit::raw::StyleRefinement;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{px, relative, size};
// the platform window and its options.
use koan_kit::raw::{Decorations, TitlebarOptions, WindowBounds, WindowOptions};

use crate::mirror::{Mirror, Slice};
use model::SettingsModel;
use panes::Pane;
pub use panes::Tab;
use sharing::Sharing;
pub use sheet::Sheets;

/// The app's settings model, this device's sharing, and the Settings window
/// while it is open.
pub struct Settings {
    pub model: Entity<SettingsModel>,
    pub sharing: Entity<Sharing>,
    window: Option<WindowHandle<Root>>,
}

impl Global for Settings {}

/// Makes the model and sharing, and applies the saved appearance. Call once
/// the mirror is installed.
pub fn install(engine: Arc<KoanEngine>, cx: &mut App) {
    let model = cx.new(|cx| SettingsModel::new(engine.clone(), cx));
    let sharing = cx.new(|_| Sharing::new(engine.clone()));
    cx.set_global(Settings {
        model,
        sharing,
        window: None,
    });
    // Quitting with a device waiting to pair declines it rather than leaving
    // it waiting; the answer is sent as the app goes, within the time the
    // app is given to.
    cx.on_app_quit(|cx| {
        let model = cx.global::<Settings>().model.clone();
        let (engine, waiting) = model.update(cx, |m, _| {
            (m.engine().clone(), std::mem::take(&mut m.waiting_pairs))
        });
        async move {
            for pair in waiting {
                engine.clone().decline_pairing(pair).await.ok();
            }
        }
    })
    .detach();
    // `appearance` waits for saves still landing, so it is read off the
    // main thread.
    let read = cx
        .background_executor()
        .spawn(async move { engine.appearance() });
    cx.spawn(async move |cx| {
        let saved = read.await;
        cx.update(|cx| {
            KoanTheme::set_record_colours(saved.record_colours, cx);
            KoanTheme::set_wash_tint(u32::from(saved.wash_tint), cx);
            KoanTheme::set_wash_window(saved.wash_window, cx);
            cx.global_mut::<KoanTheme>().icons = saved.icons;
            KoanTheme::set_text_size(u32::from(saved.text_size), cx);
        });
    })
    .detach();
}

/// Serves this device's library again where sharing was left on. What the
/// app calls as it opens, and nothing else: an offscreen render must not
/// start a server.
pub fn resume(cx: &mut App) {
    let sharing = cx.global::<Settings>().sharing.clone();
    sharing.update(cx, |sharing, cx| sharing.resume(cx));
}

/// Opens the Settings window, or brings the open one forward.
pub fn open(cx: &mut App) {
    let held = cx.global::<Settings>().window;
    if let Some(handle) = held
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let model = cx.global::<Settings>().model.clone();
    // Activation is not reported by every Linux compositor, so the file is
    // read again on opening as well.
    model.update(cx, |model, cx| model.reload(cx));
    let bounds = Bounds::centered(None, size(px(900.), px(820.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(600.), px(480.))),
        titlebar: Some(TitlebarOptions {
            // Hidden on macOS; what task switchers show elsewhere.
            title: Some("Settings".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        app_id: Some("koan-gui".into()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| SettingsWindow::new(Tab::Library, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    match opened {
        Ok(handle) => {
            cx.global_mut::<Settings>().window = Some(handle);
            // Now the Settings window: it takes what a closed one left unseen.
            let model = cx.global::<Settings>().model.clone();
            model.update(cx, |_, cx| cx.notify());
        }
        Err(e) => log::error!("the settings window did not open: {e}"),
    }
}

/// A link from outside that Settings answers.
pub enum Incoming {
    /// An invite, as its link reads.
    Invite(String),
    /// A device waiting to pair: the server it asked, and the pairing's id.
    Pairing { server: String, id: String },
}

/// Opens Settings on the pane that answers `link` and hands it over: an
/// invite signs in (asking first where it would replace an account), and a
/// pairing asks whether to sign the device in.
pub fn answer(link: Incoming, cx: &mut App) {
    open(cx);
    let Some(handle) = cx.global::<Settings>().window else {
        return;
    };
    handle
        .update(cx, |root, window, cx| {
            if let Ok(view) = root.view().clone().downcast::<SettingsWindow>() {
                view.update(cx, |settings, cx| settings.answer(link, window, cx));
            }
        })
        .ok();
}

/// Opens Settings on the server's pane: the sign-in the banner offers.
pub fn open_server(cx: &mut App) {
    open(cx);
    let Some(handle) = cx.global::<Settings>().window else {
        return;
    };
    handle
        .update(cx, |root, window, cx| {
            if let Ok(view) = root.view().clone().downcast::<SettingsWindow>() {
                view.update(cx, |settings, cx| settings.choose(Tab::Server, window, cx));
            }
        })
        .ok();
}

/// Whether `window` is the Settings window.
pub fn is_settings_window(window: AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<Settings>()
        .and_then(|settings| settings.window)
        .is_some_and(|held| AnyWindowHandle::from(held) == window)
}

/// What every pane reads, handed to each as it is made.
#[derive(Clone)]
pub struct Ctx {
    pub model: Entity<SettingsModel>,
    pub sharing: Entity<Sharing>,
    pub sheets: Entity<Sheets>,
    pub connection: Entity<Slice<Option<ConnectionInfo>>>,
}

/// Whether the server signed in to lists an OpenSubsonic extension.
pub fn offers(connection: &Option<ConnectionInfo>, name: &str) -> bool {
    connection
        .as_ref()
        .is_some_and(|c| c.extensions.iter().any(|e| e.name == name))
}

pub struct SettingsWindow {
    ctx: Ctx,
    tab: Tab,
    pane: Pane,
    /// The server's accounts, for its admins: loading is what says the
    /// account is an admin's, and so whether there is a People tab at all.
    people: Option<Entity<panes::people::People>>,
    people_for: Option<String>,
    /// Pressed on the strip along the top, to move the window on the next
    /// move: a click alone is not a drag.
    moving_window: bool,
    _people: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    pub fn new(tab: Tab, window: &mut Window, cx: &mut koan_kit::app::Context<Self>) -> Self {
        let settings = cx.global::<Settings>();
        let ctx = Ctx {
            model: settings.model.clone(),
            sharing: settings.sharing.clone(),
            sheets: cx.new(|_| Sheets::default()),
            connection: cx.global::<Mirror>().connection.clone(),
        };
        let subscriptions = vec![
            theme::follow_appearance(window, cx),
            cx.observe_in(&ctx.model, window, |this, _, window, cx| {
                this.follow_account(window, cx)
            }),
            cx.observe_in(&ctx.sharing, window, |this, _, window, cx| {
                this.follow_account(window, cx)
            }),
            cx.observe_in(&ctx.connection, window, |this, _, window, cx| {
                this.follow_account(window, cx)
            }),
            cx.observe(&ctx.sheets, |_, _, cx| cx.notify()),
            // The tab underline, toggles and buttons take the record's accent,
            // in the window and in the cached pane.
            theme::observe_accent_then(cx, |this: &mut SettingsWindow, cx| {
                App::notify(cx, this.pane.view().entity_id());
            }),
            // The CLI and the TUI write the same file; coming back to this
            // window is the moment to notice they did.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.ctx.model.update(cx, |model, cx| model.reload(cx));
                }
            }),
            cx.on_release(|_, cx| {
                if cx.has_global::<Settings>() {
                    cx.global_mut::<Settings>().window = None;
                }
            }),
        ];
        // A sheet left unanswered as the window closes is answered the way
        // its Cancel would be.
        window.on_window_should_close(cx, {
            let sheets = ctx.sheets.clone();
            move |window, cx| {
                answer_unanswered(&sheets, window, cx);
                true
            }
        });
        let pane = Pane::new(tab, &ctx, None, window, cx);
        let mut this = Self {
            ctx,
            tab,
            pane,
            people: None,
            people_for: None,
            moving_window: false,
            _people: None,
            _subscriptions: subscriptions,
        };
        this.follow_account(window, cx);
        this
    }

    /// The account Settings administers: the one signed in to, or the one
    /// this device shares its library under.
    fn account(&self, cx: &App) -> Option<String> {
        let sharing = self.ctx.sharing.read(cx);
        if sharing.serving.is_some() {
            return sharing.account.clone();
        }
        let settings = self.ctx.model.read(cx).settings()?;
        settings
            .remote_signed_in
            .then(|| settings.remote_username.clone())
    }

    /// Asks for the server's accounts again for each account and server
    /// signed in as, where the server has koan's accounts.
    fn follow_account(&mut self, window: &mut Window, cx: &mut koan_kit::app::Context<Self>) {
        let offered = offers(&self.ctx.connection.read(cx).0, panes::people::EXTENSION);
        let url = self
            .ctx
            .model
            .read(cx)
            .settings()
            .map(|s| s.remote_url.clone())
            .unwrap_or_default();
        let key = self
            .account(cx)
            .filter(|_| offered)
            .map(|account| format!("{account} {url}"));
        if key != self.people_for {
            self.people_for = key.clone();
            self.people = key.map(|_| {
                let engine = self.ctx.model.read(cx).engine().clone();
                cx.new(|cx| panes::people::People::new(engine, cx))
            });
            self._people = self.people.as_ref().map(|people| {
                cx.observe_in(people, window, |this, _, window, cx| {
                    this.follow_account(window, cx)
                })
            });
        }
        // A tab that went away (signed out, an account no longer an admin)
        // gives way to the server's.
        if !self.tabs(cx).contains(&self.tab) {
            self.show(Tab::Server, window, cx);
        }
        // A key or an invite made in a window that closed before it could
        // show it. Only the Settings window takes them: a closing one is
        // not it any more.
        let live = cx.has_global::<Settings>()
            && cx
                .global::<Settings>()
                .window
                .is_some_and(|h| h.window_id() == window.window_handle().window_id());
        let unseen = if live {
            self.ctx
                .model
                .update(cx, |m, _| std::mem::take(&mut m.unseen))
        } else {
            Vec::new()
        };
        for sheet in unseen {
            self.ctx
                .sheets
                .update(cx, |s, cx| s.show(sheet, window, cx));
        }
        cx.notify();
    }

    fn tabs(&self, cx: &App) -> Vec<Tab> {
        let connection = &self.ctx.connection.read(cx).0;
        let signed = self.account(cx).is_some();
        let mut tabs = vec![Tab::Library, Tab::Server];
        if signed
            && (offers(connection, panes::people::EXTENSION)
                || offers(connection, panes::account::PASSWORDS)
                || offers(connection, panes::account::API_KEYS))
        {
            tabs.push(Tab::Account);
        }
        if signed
            && self
                .people
                .as_ref()
                .is_some_and(|people| people.read(cx).accounts.is_some())
        {
            tabs.push(Tab::People);
        }
        tabs.extend([Tab::Playback, Tab::Eq, Tab::Devices]);
        if signed
            && connection.as_ref().is_some_and(|c| {
                c.scrobbling
                    || c.extensions
                        .iter()
                        .any(|e| e.name == panes::integrations::MCP)
            })
        {
            tabs.push(Tab::Integrations);
        }
        tabs.push(Tab::Appearance);
        tabs
    }

    pub fn choose(&mut self, tab: Tab, window: &mut Window, cx: &mut koan_kit::app::Context<Self>) {
        if tab != self.tab {
            self.show(tab, window, cx);
            cx.notify();
        }
    }

    fn answer(
        &mut self,
        link: Incoming,
        window: &mut Window,
        cx: &mut koan_kit::app::Context<Self>,
    ) {
        match link {
            Incoming::Invite(text) => {
                self.choose(Tab::Server, window, cx);
                if let Ok(pane) = self.pane.view().downcast::<panes::server::ServerPane>() {
                    pane.update(cx, |pane, cx| pane.join(&text, false, window, cx));
                }
            }
            Incoming::Pairing { server, id } => {
                // Approving signs the device in to the account here, so the
                // link must name this device's server: the one signed in to,
                // or this device while it shares its library.
                let sharing = self.ctx.sharing.read(cx).serving.is_some();
                let ours = self.ctx.model.read(cx).settings().is_some_and(|s| {
                    s.remote_signed_in && s.remote_url.trim_end_matches('/') == server
                });
                if !sharing && !ours {
                    let host = panes::server::host(&server);
                    self.ctx.model.update(cx, |m, cx| {
                        m.report(
                            format!("A device is asking to sign in to {host}, which kōan is not signed in to. Approve it on {host}'s own pair page instead."),
                            cx,
                        )
                    });
                    return;
                }
                self.choose(Tab::Devices, window, cx);
                if let Ok(pane) = self.pane.view().downcast::<panes::devices::DevicesPane>() {
                    pane.update(cx, |pane, cx| pane.approve_pair(id, window, cx));
                }
            }
        }
    }

    /// Makes the pane for `tab`. A tab switch swaps the page at once, and
    /// the page left goes with its fields.
    fn show(&mut self, tab: Tab, window: &mut Window, cx: &mut koan_kit::app::Context<Self>) {
        self.tab = tab;
        self.pane = Pane::new(tab, &self.ctx, self.people.clone(), window, cx);
    }
}

impl Render for SettingsWindow {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut koan_kit::app::Context<Self>,
    ) -> impl IntoElement {
        KoanTheme::drive(window, cx);
        let look = Look::of(window, cx);
        let tabs = self.tabs(cx);
        let entity = cx.entity();
        let row = div()
            .flex()
            .justify_center()
            .gap(look.space(Space::L))
            .px(look.space(Space::M))
            .pb(look.space(Space::Xs))
            .children(tabs.into_iter().map(|tab| {
                let entity = entity.clone();
                tab_item(tab, tab == self.tab, &look).on_click(move |_, window, cx| {
                    entity.update(cx, |this, cx| this.choose(tab, window, cx))
                })
            }));
        let status = status_line(self.ctx.model.read(cx), &look);
        let sheets = self.ctx.sheets.clone();
        let sheet = Sheets::render(&sheets, &look, window, cx);
        let fill = || {
            let mut style = StyleRefinement::default();
            style.size.width = Some(relative(1.0).into());
            style.size.height = Some(relative(1.0).into());
            style
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(look.palette.bg.hsla())
            .child(self.top_strip(window, cx))
            .child(row)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.pane.view().cached(fill())),
            )
            .child(status)
            .children(sheet)
    }
}

impl SettingsWindow {
    /// The strip along the top, on the window's ground: the window's own
    /// buttons sit in it on macOS. It moves the window, and where the
    /// compositor leaves decorations to the app (GNOME on Wayland), it holds
    /// a close button.
    fn top_strip(&self, window: &Window, cx: &mut koan_kit::app::Context<Self>) -> Stateful<Div> {
        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let look = Look::of(window, cx);
        div()
            .id("settings-top")
            .flex()
            .justify_end()
            .items_center()
            .h(px(if cfg!(target_os = "macos") { 30. } else { 28. }))
            .px(look.space(Space::S))
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
            .when(client, |strip| {
                strip.child(
                    crate::components::Button::new(
                        "settings-close",
                        crate::components::ButtonKind::Toolbar,
                    )
                    .label("Close")
                    .icon(Icon::Close)
                    .on_click({
                        let sheets = self.ctx.sheets.clone();
                        move |_, window, cx| {
                            answer_unanswered(&sheets, window, cx);
                            window.remove_window();
                        }
                    }),
                )
            })
    }
}

/// As the window closes: sheets left unanswered are answered the way their
/// Cancel would be, and those showing something only once go to the next
/// Settings window.
fn answer_unanswered(sheets: &Entity<Sheets>, window: &mut Window, cx: &mut App) {
    // No longer the Settings window: what it hands on below waits for the
    // next one rather than coming back to this one as it goes.
    if cx.has_global::<Settings>() {
        cx.global_mut::<Settings>().window = None;
    }
    for dismissed in sheets.read(cx).unanswered() {
        dismissed(window, cx);
    }
    let kept = sheets.update(cx, |s, _| s.take_kept());
    if !kept.is_empty() && cx.has_global::<Settings>() {
        let model = cx.global::<Settings>().model.clone();
        model.update(cx, |m, cx| {
            m.unseen.extend(kept);
            cx.notify();
        });
    }
}

/// A Settings tab, as the spec draws the Mac's: the label in `fine`,
/// lowercase, under its glyph when icons are on; `muted`, and the chosen one
/// `accent` and underlined.
fn tab_item(tab: Tab, selected: bool, look: &Look) -> Stateful<Div> {
    let id = ElementId::Name(tab.title().into());
    let tone = if selected { Tone::Accent } else { Tone::Muted };
    let color = look.text_color(tone);
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .gap(look.space(Space::Xs))
        .px(look.space(Space::S))
        .pt(look.space(Space::S))
        .cursor_pointer()
        .text_role(TypeRole::Fine, look)
        .text_color(color)
        .when(look.icons, |item| {
            item.child(KoanIcon::new(tab.icon()).role(TypeRole::Title).color(color))
        })
        .child(
            div()
                .pb(px(5.))
                .border_b(HAIRLINE)
                .border_color(if selected {
                    look.accent.hsla()
                } else {
                    look.accent.hsla().opacity(0.)
                })
                .child(label(tab.title())),
        )
}

/// The result of the last action, or why it failed: one line, always in the
/// same place, since an action that reports nothing looks like it did
/// nothing.
fn status_line(model: &SettingsModel, look: &Look) -> Div {
    let running: Vec<&str> = model.running().map(SharedString::as_ref).collect();
    let (text, tone, icon) = match (&model.last_error, &model.last_result) {
        (Some(error), _) => (error.clone(), Tone::Bad, Some(Icon::WarningFilled)),
        // What is under way, until it says how it went.
        _ if !running.is_empty() => (
            format!("{}…", running.join(" · ")).into(),
            Tone::Muted,
            Some(Icon::Pending),
        ),
        (None, Some(result)) => (result.clone(), Tone::Muted, Some(Icon::Success)),
        (None, None) => (SharedString::default(), Tone::Muted, None),
    };
    let color = look.text_color(tone);
    div()
        .flex()
        .items_center()
        .gap(look.space(Space::Xs))
        .min_h(look.line_height(TypeRole::Fine) * 2. + look.space(Space::S) * 2.)
        .px(look.space(Space::L))
        .py(look.space(Space::S))
        .border_t(HAIRLINE)
        .border_color(look.palette.rule.hsla())
        .text_role(TypeRole::Fine, look)
        .text_color(color)
        .children(icon.map(|icon| KoanIcon::new(icon).role(TypeRole::Fine).color(color)))
        .child(div().line_clamp(2).child(text))
}

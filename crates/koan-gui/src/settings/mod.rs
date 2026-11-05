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

use crate::theme::{self, Icon, KoanTheme, Look, Space, Tone};
use koan_kit::app::*;
use koan_runtime::{ConnectionInfo, KoanEngine};

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
    window: Option<(AnyWindowHandle, WeakEntity<SettingsWindow>)>,
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
        // All at once: quitting waits only a moment for them.
        let declines = waiting
            .into_iter()
            .map(|pair| engine.clone().decline_pairing(pair));
        let declines = futures::future::join_all(declines);
        async move {
            declines.await;
        }
    })
    .detach();
    apply_appearance(engine, cx);
}

/// Draws the app as `[appearance]` says: as it opens, and once the settings
/// are reset.
pub fn apply_appearance(engine: Arc<KoanEngine>, cx: &mut App) {
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
            KoanTheme::set_wash_drift(u32::from(saved.wash_drift), u32::from(saved.wash_turn), cx);
            KoanTheme::set_wash_window(saved.wash_window, cx);
            cx.global_mut::<KoanTheme>().icons = saved.icons;
            KoanTheme::set_text_size(u32::from(saved.text_size), cx);
            cx.refresh_windows();
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
    let held = cx.global::<Settings>().window.clone();
    if let Some((handle, _)) = held
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
    let opened = crate::components::SecondaryWindow::new("Settings", crate::app::APP_ID)
        .size(900.0, 820.0)
        .min(600.0, 480.0)
        .transparent_titlebar()
        .open(cx, |window, cx| {
            cx.new(|cx| SettingsWindow::new(Tab::Library, window, cx))
        });
    match opened {
        Ok((handle, view)) => {
            cx.global_mut::<Settings>().window = Some((handle, view.downgrade()));
            // Opened from the menu bar with no window: in the Dock again.
            crate::platform::residency::settle_dock(cx);
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
    // A link that launched the app arrives before the settings are read:
    // answered then, it would be answered as signed out.
    let model = cx.global::<Settings>().model.clone();
    if model.read(cx).settings().is_none() {
        model.update(cx, |m, cx| m.when_loaded(cx, move |cx| answer(link, cx)));
        return;
    }
    open(cx);
    let Some((handle, view)) = cx.global::<Settings>().window.clone() else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            view.update(cx, |settings, cx| settings.answer(link, window, cx))
        })
        .ok();
}

/// Opens Settings on the server's pane: the sign-in the banner offers.
pub fn open_server(cx: &mut App) {
    open(cx);
    let Some((handle, view)) = cx.global::<Settings>().window.clone() else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            view.update(cx, |settings, cx| settings.choose(Tab::Server, window, cx))
        })
        .ok();
}

/// Whether two addresses name one server: scheme and host compared without
/// regard to case, a scheme's default port the same as none, and a trailing
/// slash ignored.
fn same_server(a: &str, b: &str) -> bool {
    fn parts(url: &str) -> Option<(String, String, u16, String)> {
        let (scheme, rest) = url.trim().split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let default = match scheme.as_str() {
            "https" => 443,
            "http" => 80,
            _ => return None,
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !port.contains(']') => (host, port.parse().ok()?),
            _ => (authority, default),
        };
        Some((
            scheme,
            host.to_ascii_lowercase(),
            port,
            path.trim_end_matches('/').to_owned(),
        ))
    }
    parts(a).is_some_and(|a| parts(b).is_some_and(|b| a == b))
}

/// Whether `url` reaches this device's own server: its port, at an address
/// that is this machine or on its network (loopback, private, link-local,
/// the tailnet's, or a `.local` name).
fn on_this_device(url: &str, port: u16) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let Some((host, at)) = authority.rsplit_once(':') else {
        return false;
    };
    if at.parse::<u16>().ok() != Some(port) {
        return false;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                // The tailnet's addresses.
                || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        Err(_) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || host.ends_with(".local")
        }
    }
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
            // The system's settings can turn the login item off the same way.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.ctx.model.update(cx, |model, cx| model.reload(cx));
                    crate::platform::residency::read_login(cx);
                    this.forget_settled_pairs(window, cx);
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
                .as_ref()
                .is_some_and(|h| h.0.window_id() == window.window_handle().window_id());
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
                let (sharing_on, serving, signed_in, named) = {
                    let sharing = self.ctx.sharing.read(cx);
                    let settings = self.ctx.model.read(cx).settings();
                    let serving = sharing.serving.is_some()
                        && settings.is_some_and(|s| {
                            let status = sharing.status.as_ref();
                            let known = [
                                status.and_then(|st| st.current.as_deref()),
                                status.and_then(|st| st.fallback.as_deref()),
                                status.and_then(|st| st.funnel.as_deref()),
                                Some(s.server_url.as_str()),
                            ];
                            known
                                .into_iter()
                                .flatten()
                                .any(|url| same_server(url, &server))
                                || on_this_device(&server, s.share_port)
                        });
                    let signed_in = settings.filter(|s| s.remote_signed_in);
                    let named = signed_in.is_some_and(|s| same_server(&s.remote_url, &server));
                    (
                        sharing.serving.is_some(),
                        serving,
                        signed_in.is_some(),
                        named,
                    )
                };
                if serving || named {
                    self.approve(id, window, cx);
                    return;
                }
                let host = panes::server::host(&server);
                let foreign = format!(
                    "A device is asking to sign in to {host}, which kōan is not signed in to. Approve it on {host}'s own pair page instead."
                );
                if !signed_in || sharing_on {
                    self.ctx.model.update(cx, |m, cx| m.report(foreign, cx));
                    return;
                }
                // The server signed in to under another of its names (its
                // address on the network, a public one): it knows the
                // pairings waiting on it.
                let engine = self.ctx.model.read(cx).engine().clone();
                let model = self.ctx.model.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let found = engine.pairing_info(id.clone()).await.is_ok();
                    this.update_in(cx, |this, window, cx| {
                        if found {
                            this.approve(id, window, cx);
                        } else {
                            model.update(cx, |m, cx| m.report(foreign, cx));
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn approve(&mut self, id: String, window: &mut Window, cx: &mut koan_kit::app::Context<Self>) {
        self.choose(Tab::Devices, window, cx);
        if let Ok(pane) = self.pane.view().downcast::<panes::devices::DevicesPane>() {
            pane.update(cx, |pane, cx| pane.approve_pair(id, window, cx));
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
        let look = Look::of(window, cx);
        let tabs = self.tabs(cx);
        let entity = cx.entity();
        let row = tabs
            .into_iter()
            .fold(crate::components::TabStrip::new(), |strip, tab| {
                let entity = entity.clone();
                strip.tab(
                    tab.icon(),
                    tab.title(),
                    tab == self.tab,
                    move |_, window, cx| entity.update(cx, |this, cx| this.choose(tab, window, cx)),
                )
            });
        let status = status_line(self.ctx.model.read(cx));
        let sheets = self.ctx.sheets.clone();
        let sheet = Sheets::render(&sheets, &look, window, cx);
        let fill = || StyleRefinement::default().size_full();
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
    /// Pairings asked about here that were answered elsewhere (another
    /// window, the web UI, a phone) are no longer waiting: their questions go,
    /// and quitting does not decline them. Asked of the server when the
    /// window comes to the front, as the file is read again then.
    fn forget_settled_pairs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (model, sheets) = (self.ctx.model.clone(), self.ctx.sheets.clone());
        let (engine, pairs) = {
            let m = model.read(cx);
            (m.engine().clone(), m.waiting_pairs.clone())
        };
        for pair in pairs {
            let (engine, model, sheets) = (engine.clone(), model.clone(), sheets.clone());
            cx.spawn_in(window, async move |_, cx| {
                // No such pairing: approved, declined or lapsed.
                let settled = matches!(
                    engine.pairing_info(pair.clone()).await,
                    Err(koan_runtime::KoanError::BadArgument { .. })
                );
                if settled {
                    cx.update(|window, cx| {
                        model.update(cx, |m, _| m.waiting_pairs.retain(|p| *p != pair));
                        sheets.update(cx, |s, cx| s.withdraw(&pair, window, cx));
                    })
                    .ok();
                }
            })
            .detach();
        }
    }

    /// The strip along the top, on the window's ground: the window's own
    /// buttons sit in it on macOS. It moves the window, and where the
    /// compositor leaves decorations to the app (GNOME on Wayland), it holds
    /// a close button.
    fn top_strip(
        &self,
        window: &mut Window,
        cx: &mut koan_kit::app::Context<Self>,
    ) -> Stateful<Div> {
        let client = crate::components::draws_decorations(window);
        let look = Look::of(window, cx);
        let strip = div()
            .id("settings-top")
            .flex()
            .justify_end()
            .items_center()
            .h(crate::components::SecondaryWindow::strip_height())
            .px(look.space(Space::S));
        crate::components::moves_window(strip, window, cx).when(client, |strip| {
            let sheets = self.ctx.sheets.clone();
            strip.child(crate::components::close_button(move |window, cx| {
                answer_unanswered(&sheets, window, cx)
            }))
        })
    }
}

/// As the Settings window closes: it stops being the Settings window, and
/// its sheets are handed on (`hand_on_unanswered`).
fn answer_unanswered(sheets: &Entity<Sheets>, window: &mut Window, cx: &mut App) {
    // No longer the Settings window: what it hands on below waits for the
    // next one rather than coming back to this one as it goes.
    if cx.has_global::<Settings>() {
        cx.global_mut::<Settings>().window = None;
    }
    hand_on_unanswered(sheets, window, cx);
}

/// As any window holding Settings' sheets closes: those left unanswered
/// are answered the way their Cancel would be, and those showing something
/// only once go to the next Settings window.
pub(crate) fn hand_on_unanswered(sheets: &Entity<Sheets>, window: &mut Window, cx: &mut App) {
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

/// The result of the last action, or why it failed: one line, always in the
/// same place, since an action that reports nothing looks like it did
/// nothing.
pub(crate) fn status_line(model: &SettingsModel) -> crate::components::StatusLine {
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
    crate::components::StatusLine::new(text, tone, icon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_server_under_its_spellings() {
        assert!(same_server(
            "https://Music.Example.com/",
            "https://music.example.com"
        ));
        assert!(same_server(
            "https://music.example.com:443",
            "https://music.example.com/"
        ));
        assert!(same_server("http://10.0.0.2:4533", "http://10.0.0.2:4533/"));
        assert!(!same_server(
            "http://music.example.com",
            "https://music.example.com"
        ));
        assert!(!same_server(
            "https://music.example.com",
            "https://evil.example.com"
        ));
        assert!(!same_server(
            "https://a.example.com:8443",
            "https://a.example.com"
        ));
    }

    #[test]
    fn this_device_is_its_port_on_its_network() {
        assert!(on_this_device("http://192.168.1.20:4533", 4533));
        assert!(on_this_device("http://studio.local:4533/", 4533));
        assert!(on_this_device("http://100.101.1.2:4533", 4533));
        assert!(on_this_device("http://[fe80::1]:4533", 4533));
        assert!(!on_this_device("http://192.168.1.20:4534", 4533));
        assert!(!on_this_device("https://music.example.com:4533", 4533));
        assert!(!on_this_device("http://8.8.8.8:4533", 4533));
    }
}

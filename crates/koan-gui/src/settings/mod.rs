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
mod panes;
mod qr;
mod sheet;
pub mod sharing;

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{ConnectionInfo, KoanEngine};
use crate::theme::{
    self, HAIRLINE, Icon, KoanIcon, KoanStyled, KoanTheme, Look, Space, Tone, TypeRole, label,
};

use crate::mirror::{Mirror, Slice};
use model::SettingsModel;
use panes::{Pane, Tab};
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
    let settings = cx.global::<Settings>();
    if let Some(handle) = settings.window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let model = settings.model.clone();
    // Activation is not reported by every Linux compositor, so the file is
    // read again on opening as well.
    model.update(cx, |model, cx| model.reload(cx));
    let bounds = Bounds::centered(None, size(px(900.), px(820.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(600.), px(480.))),
        titlebar: Some(TitlebarOptions {
            title: None,
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
        Ok(handle) => cx.global_mut::<Settings>().window = Some(handle),
        Err(e) => log::error!("the settings window did not open: {e}"),
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
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    pub fn new(tab: Tab, window: &mut Window, cx: &mut gpui_kit::Context<Self>) -> Self {
        let settings = cx.global::<Settings>();
        let ctx = Ctx {
            model: settings.model.clone(),
            sharing: settings.sharing.clone(),
            sheets: cx.new(|_| Sheets::default()),
            connection: cx.global::<Mirror>().connection.clone(),
        };
        let subscriptions = vec![
            theme::follow_appearance(window, cx),
            cx.observe(&ctx.model, |this, _, cx| this.follow_account(cx)),
            cx.observe(&ctx.sharing, |this, _, cx| this.follow_account(cx)),
            cx.observe(&ctx.connection, |this, _, cx| this.follow_account(cx)),
            cx.observe(&ctx.sheets, |_, _, cx| cx.notify()),
            // The CLI and the TUI write the same file; coming back to this
            // window is the moment to notice they did.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.ctx.model.update(cx, |model, cx| model.reload(cx));
                }
            }),
            cx.on_release(|_, cx| {
                if let Some(settings) = cx.try_global_mut::<Settings>() {
                    settings.window = None;
                }
            }),
        ];
        let pane = Pane::new(tab, &ctx, None, window, cx);
        let mut this = Self {
            ctx,
            tab,
            pane,
            people: None,
            people_for: None,
            _subscriptions: subscriptions,
        };
        this.follow_account(cx);
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
    fn follow_account(&mut self, cx: &mut gpui_kit::Context<Self>) {
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
                let people = cx.new(|cx| panes::people::People::new(engine, cx));
                cx.observe(&people, |_, _, cx| cx.notify()).detach();
                people
            });
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
        tabs.extend([Tab::Playback, Tab::Devices]);
        if signed
            && connection.as_ref().is_some_and(|c| {
                c.scrobbling || c.extensions.iter().any(|e| e.name == panes::integrations::MCP)
            })
        {
            tabs.push(Tab::Integrations);
        }
        tabs.push(Tab::Appearance);
        tabs
    }

    fn choose(&mut self, tab: Tab, window: &mut Window, cx: &mut gpui_kit::Context<Self>) {
        if tab != self.tab {
            self.show(tab, window, cx);
            cx.notify();
        }
    }

    /// Makes the pane for `tab`. A tab switch swaps the page at once, and
    /// the page left goes with its fields.
    fn show(&mut self, tab: Tab, window: &mut Window, cx: &mut gpui_kit::Context<Self>) {
        self.tab = tab;
        self.pane = Pane::new(tab, &self.ctx, self.people.clone(), window, cx);
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        KoanTheme::drive(window, cx);
        let look = Look::of(window, cx);
        let tabs = self.tabs(cx);
        // A tab that went away (signed out, an account no longer an admin)
        // gives way to the server's.
        if !tabs.contains(&self.tab) {
            self.show(Tab::Server, window, cx);
        }
        let entity = cx.entity();
        let row = div()
            .flex()
            .justify_center()
            .gap(look.space(Space::L))
            .px(look.space(Space::M))
            .pb(look.space(Space::Xs))
            .children(tabs.into_iter().map(|tab| {
                let entity = entity.clone();
                tab_item(tab, tab == self.tab, &look).on_click(
                    move |_, window, cx| {
                        entity.update(cx, |this, cx| this.choose(tab, window, cx))
                    },
                )
            }));
        let status = status_line(self.ctx.model.read(cx), &look);
        let sheet = self.ctx.sheets.read(cx).render(&self.ctx.sheets, &look, window, cx);
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
            // The window's buttons sit above the row, on its ground.
            .pt(px(if cfg!(target_os = "macos") { 30. } else { 8. }))
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
    let (text, tone, icon) = match (&model.last_error, &model.last_result) {
        (Some(error), _) => (error.clone(), Tone::Bad, Some(Icon::WarningFilled)),
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

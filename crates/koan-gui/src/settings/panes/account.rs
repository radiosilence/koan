//! Account: the signed-in account's own keys to the server: a device linked
//! by QR code, its password, the API keys apps sign in with, and app
//! passwords for apps that cannot use a key. Each where the server lists it.

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_gui::components::{Button, ButtonKind, FormRow};
use koan_gui::theme::Look;

use super::keys::{Keys, Kind};
use super::link::LinkDevice;
use super::people;
use crate::settings::form::{page, section};
use crate::settings::sheet::{self, Action, Sheet};
use crate::settings::{Ctx, offers};

/// Setting passwords from the app.
pub const PASSWORDS: &str = "koanPasswords";
pub const API_KEYS: &str = "koanApiKeys";
pub const APP_PASSWORDS: &str = "koanAppPasswords";

pub struct AccountPane {
    ctx: Ctx,
    account: Option<String>,
    link: Option<Entity<LinkDevice>>,
    api_keys: Option<Entity<Keys>>,
    app_passwords: Option<Entity<Keys>>,
    _subscriptions: Vec<Subscription>,
}

impl AccountPane {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            _subscriptions: vec![
                cx.observe(&ctx.model, |_, _, cx| cx.notify()),
                cx.observe(&ctx.sharing, |_, _, cx| cx.notify()),
                cx.observe_in(&ctx.connection, window, |pane, _, window, cx| pane.follow(window, cx)),
            ],
            ctx,
            account: None,
            link: None,
            api_keys: None,
            app_passwords: None,
        };
        pane.follow(window, cx);
        pane
    }

    /// Makes the sections the server offers now, keeping those it already
    /// had, so a reconnect does not ask for every list again.
    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let connection = self.ctx.connection.read(cx).0.clone();
        let sharing = self.ctx.sharing.read(cx);
        let account = if sharing.serving.is_some() {
            sharing.account.clone()
        } else {
            self.ctx
                .model
                .read(cx)
                .settings()
                .filter(|s| s.remote_signed_in)
                .map(|s| s.remote_username.clone())
        };
        if account != self.account {
            self.account = account.clone();
            self.link = None;
        }
        let engine = self.ctx.model.read(cx).engine().clone();
        self.link = match (&account, offers(&connection, people::EXTENSION)) {
            (Some(account), true) => self.link.take().or_else(|| {
                let (engine, account, connection) = (engine.clone(), account.clone(), self.ctx.connection.clone());
                Some(cx.new(|cx| LinkDevice::new(engine, account, connection, cx)))
            }),
            _ => None,
        };
        self.api_keys = offers(&connection, API_KEYS).then(|| {
            self.api_keys.take().unwrap_or_else(|| {
                let ctx = self.ctx.clone();
                cx.new(|cx| Keys::new(Kind::ApiKeys, ctx, window, cx))
            })
        });
        // Only while the server can be reached: offline the section is not
        // there at all.
        let reachable = connection.as_ref().is_some_and(|c| !c.offline);
        self.app_passwords = (offers(&connection, APP_PASSWORDS) && reachable).then(|| {
            self.app_passwords.take().unwrap_or_else(|| {
                let ctx = self.ctx.clone();
                cx.new(|cx| Keys::new(Kind::AppPasswords, ctx, window, cx))
            })
        });
        cx.notify();
    }

    fn change_password(&self, window: &mut Window, cx: &mut App) {
        let current = sheet::field("Current password", true, window, cx);
        let new = sheet::field("New password", true, window, cx);
        let model = self.ctx.model.clone();
        let sheet = Sheet::new("Change your password")
            .message("This device stays signed in. Your other devices, and other apps using this account, will have to sign in again.")
            .field("Current password", current)
            .field("New password", new)
            .action(
                Action::new("Change", move |values, _, cx| {
                    let (current, new) = (values[0].clone(), values[1].clone());
                    model.update(cx, |m, cx| m.change_password(current, new, cx))
                })
                .ready(|values| !values[0].is_empty() && !values[1].is_empty()),
            );
        self.ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
    }
}

impl Render for AccountPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let connection = &self.ctx.connection.read(cx).0;
        // A device sharing its own library resets its account's password as
        // the server's host, under Server, rather than changing it here.
        let sharing = self.ctx.sharing.read(cx).serving.is_some();
        let mut sections = Vec::new();
        if let Some(link) = &self.link {
            sections.push(link.clone().into_any_element());
        }
        if let Some(account) = self.account.clone().filter(|_| offers(connection, PASSWORDS) && !sharing) {
            let entity = cx.entity();
            sections.push(section(
                Some("Password"),
                vec![FormRow::new("User")
                    .value(account)
                    .control(
                        Button::new("change-password", ButtonKind::Bordered)
                            .label("Change Password…")
                            .on_click(move |_, window, cx| entity.read(cx).change_password(window, cx)),
                    )
                    .into_any_element()],
                Some("This device stays signed in. Your other devices, and other apps using this account, will have to sign in again.".into()),
                &look,
            ));
        }
        sections.extend(self.api_keys.clone().map(IntoElement::into_any_element));
        sections.extend(self.app_passwords.clone().map(IntoElement::into_any_element));
        page("account", &look, sections)
    }
}

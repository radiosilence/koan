//! Server → Shares its library: the first account, sharing on and off, the
//! port, and where invites send phones away from home.

use crate::components::{Button, ButtonKind, CopyText, Picker, TextField, Toggle};
use crate::theme::{Icon, Look, Tone, TypeRole};
use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
use koan_runtime::{InviteTo, Settings};

use super::link::LinkDevice;
use super::server::ServerPane;
use crate::settings::Ctx;
use crate::settings::form::{Row, buttons, note, section, text};
use crate::settings::model::SettingsModel;
use crate::settings::sharing::Landed;
use crate::settings::sheet::{self, Action, Sheet};

pub struct ShareForm {
    username: Entity<InputState>,
    password: Entity<InputState>,
    again: Entity<InputState>,
    port: Entity<InputState>,
    address: Entity<InputState>,
    account_error: Option<SharedString>,
    creating: bool,
    link: Option<(String, Entity<LinkDevice>)>,
    _subscriptions: Vec<Subscription>,
}

impl ShareForm {
    pub fn new(ctx: &Ctx, window: &mut Window, cx: &mut Context<ServerPane>) -> Self {
        let field = |placeholder: &'static str,
                     secret: bool,
                     window: &mut Window,
                     cx: &mut Context<ServerPane>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(secret)
            })
        };
        let username = field("Username", false, window, cx);
        let password = field("Password", true, window, cx);
        let again = field("Password again", true, window, cx);
        let port = field("4000", false, window, cx);
        let address = field("", false, window, cx);
        if let Some(settings) = ctx.model.read(cx).settings() {
            let (p, a) = (settings.share_port.to_string(), settings.share_url.clone());
            port.update(cx, |s, cx| s.set_value(p, window, cx));
            address.update(cx, |s, cx| s.set_value(a, window, cx));
        }
        // The address moves with the network, so it is worked out again each
        // time the page opens.
        ctx.sharing.update(cx, |sharing, cx| sharing.refresh(cx));
        let subscriptions = vec![
            cx.subscribe_in(&port, window, |pane, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let ctx = pane.ctx.clone();
                    pane.share.commit_port(&ctx, cx);
                }
            }),
            cx.subscribe_in(&address, window, |pane, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let ctx = pane.ctx.clone();
                    pane.share.commit_address(&ctx, cx);
                }
            }),
            cx.observe(&username, |_, _, cx| cx.notify()),
            cx.observe(&password, |_, _, cx| cx.notify()),
            cx.observe(&again, |_, _, cx| cx.notify()),
        ];
        Self {
            username,
            password,
            again,
            port,
            address,
            account_error: None,
            creating: false,
            link: None,
            _subscriptions: subscriptions,
        }
    }

    /// Turns sharing on or off. A start that fails turns the setting off
    /// again, so the switch never reads as sharing beside the reason it is
    /// not; launch still retries a setting left on.
    pub fn set_sharing(ctx: &Ctx, on: bool, cx: &mut App) {
        ctx.model
            .update(cx, |m, cx| m.edit(cx, move |s| s.share_enabled = on));
        let port = ctx
            .model
            .read(cx)
            .settings()
            .map(|s| s.share_port)
            .unwrap_or(4000);
        let model = ctx.model.clone();
        ctx.sharing.update(cx, |sharing, cx| {
            if on {
                sharing.start(port, Some(after_start(model)), cx);
            } else {
                sharing.stop(cx);
            }
        });
    }

    /// With the values just typed, so the start never reads a setting still
    /// being written. A start stops the running server first, so a failed
    /// one leaves nothing serving and the switch says so.
    fn restart_if_sharing(ctx: &Ctx, port: u16, cx: &mut App) {
        if !ctx
            .model
            .read(cx)
            .settings()
            .is_some_and(|s| s.share_enabled)
        {
            return;
        }
        let model = ctx.model.clone();
        ctx.sharing.update(cx, |sharing, cx| {
            sharing.start(port, Some(after_start(model)), cx)
        });
    }

    fn commit_port(&mut self, ctx: &Ctx, cx: &mut Context<ServerPane>) {
        let typed = self.port.read(cx).value().trim().to_owned();
        let Some(current) = ctx.model.read(cx).settings().map(|s| s.share_port) else {
            return;
        };
        match typed.parse::<u16>() {
            Ok(port) if port > 0 => {
                if port != current {
                    ctx.model
                        .update(cx, |m, cx| m.edit(cx, move |s| s.share_port = port));
                    Self::restart_if_sharing(ctx, port, cx);
                }
            }
            _ => ctx.model.update(cx, |m, cx| {
                m.report("A port is a number from 1 to 65535.", cx)
            }),
        }
    }

    /// Through the engine, which owns where invites go and writes the
    /// address itself; the model reads it back rather than writing its older
    /// copy. The server answers to the names it was started with, the custom
    /// address's among them, so a running one starts again.
    fn commit_address(&mut self, ctx: &Ctx, cx: &mut Context<ServerPane>) {
        let typed = self.address.read(cx).value().trim().to_owned();
        let Some(settings) = ctx.model.read(cx).settings() else {
            return;
        };
        if typed == settings.share_url {
            return;
        }
        let port = settings.share_port;
        let after = ctx.clone();
        let landed: crate::settings::sharing::Landed = Box::new(move |failed, cx| {
            after.model.update(cx, |m, cx| m.reload(cx));
            if !failed {
                Self::restart_if_sharing(&after, port, cx);
            }
        });
        ctx.sharing.update(cx, |sharing, cx| {
            sharing.set_invite_to(InviteTo::Custom, Some(typed), Some(landed), cx)
        });
    }

    fn create(&mut self, ctx: &Ctx, window: AnyWindowHandle, cx: &mut Context<ServerPane>) {
        let name = self.username.read(cx).value().trim().to_owned();
        let secret = self.password.read(cx).value().to_string();
        self.creating = true;
        let ctx2 = ctx.clone();
        let pane = cx.entity();
        ctx.sharing.update(cx, |sharing, cx| {
            sharing.create_account(name, secret, cx, move |_, result, cx| {
                let made = result.is_ok();
                pane.update(cx, |pane, cx| {
                    pane.share.creating = false;
                    pane.share.account_error = result.err().map(Into::into);
                    cx.notify();
                });
                // The password is the server's now: not kept in the form.
                if made {
                    window
                        .update(cx, |_, window, cx| {
                            pane.update(cx, |pane, cx| {
                                for field in [&pane.share.password, &pane.share.again] {
                                    field.update(cx, |s, cx| s.set_value("", window, cx));
                                }
                            })
                        })
                        .ok();
                }
                if made {
                    ShareForm::set_sharing(&ctx2, true, cx);
                }
            });
        });
        cx.notify();
    }

    fn reset_password(account: String, ctx: &Ctx, window: &mut Window, cx: &mut App) {
        let first = sheet::field("New password", true, window, cx);
        let second = sheet::field("Again", true, window, cx);
        let model = ctx.model.clone();
        let engine = model.read(cx).engine().clone();
        let who = account.clone();
        let sheet = Sheet::new(format!("New password for {account}"))
            .message("Its phones and other devices are signed out and sign in again with the new one.")
            .field("New password", first)
            .field("Again", second)
            .action(
                Action::new("Reset", move |values, _, cx| {
                    let password = values[0].clone();
                    let who = who.clone();
                    model.update(cx, |m, cx| {
                        let engine = engine.clone();
                        m.run(
                            "Resetting the password",
                            crate::settings::model::Uses::NONE,
                            cx,
                            move |_| engine.reset_server_admin_password(password),
                            move |m, result, cx| match result {
                                Ok(()) => m.done(
                                    format!("{who}'s password is changed. Its other devices sign in again."),
                                    cx,
                                ),
                                Err(e) => m.report(crate::settings::model::describe(&e), cx),
                            },
                        )
                    })
                })
                .ready(|values| !values[0].is_empty() && values[0] == values[1]),
            );
        ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
    }

    pub fn sections(
        &mut self,
        ctx: &Ctx,
        settings: &Settings,
        look: &Look,
        cx: &mut Context<ServerPane>,
    ) -> Vec<AnyElement> {
        if settings.remote_signed_in {
            return vec![section(
                None,
                vec![text(
                    format!(
                        "This device is signed in to {}. Sign out of it under Connects to a server, then share this device's own library here.",
                        super::server::host(&settings.remote_url)
                    ),
                    TypeRole::Body,
                    Tone::Muted,
                    look,
                )
                .into_any_element()],
                None,
                look,
            )];
        }
        let Some(account) = ctx.sharing.read(cx).account.clone() else {
            return vec![self.create_account(ctx, look, cx)];
        };
        let mut sections = vec![self.running(ctx, settings, &account, look, cx)];
        if settings.share_enabled {
            sections.push(away_from_home(ctx, settings, &self.address, look, cx));
        }
        if ctx.sharing.read(cx).serving.is_some() {
            let link = match &self.link {
                Some((who, link)) if *who == account => link.clone(),
                _ => {
                    let engine = ctx.model.read(cx).engine().clone();
                    let link = cx.new(|cx| {
                        LinkDevice::new(engine, account.clone(), ctx.connection.clone(), cx)
                    });
                    self.link = Some((account.clone(), link.clone()));
                    link
                }
            };
            sections.push(link.into_any_element());
        } else {
            self.link = None;
        }
        sections
    }

    fn create_account(&self, ctx: &Ctx, look: &Look, cx: &mut Context<ServerPane>) -> AnyElement {
        let name = self.username.read(cx).value();
        let password = self.password.read(cx).value();
        let again = self.again.read(cx).value();
        let ready =
            !self.creating && !name.trim().is_empty() && !password.is_empty() && password == again;
        let pane = cx.entity();
        let ctx = ctx.clone();
        let mut rows = vec![
            Row::new("Username")
                .control(div().w_full().child(TextField::new(&self.username)))
                .into_any_element(),
            Row::new("Password")
                .control(div().w_full().child(TextField::new(&self.password)))
                .into_any_element(),
            Row::new("Again")
                .control(div().w_full().child(TextField::new(&self.again)))
                .into_any_element(),
        ];
        if let Some(error) = &self.account_error {
            rows.push(note(error.clone(), Icon::Warning, Tone::Bad, look));
        }
        rows.push(buttons(
            vec![
                Button::new("create-account", ButtonKind::Prominent)
                    .label("Create Account and Share")
                    .disabled(!ready)
                    .on_click(move |_, window, cx| {
                        let handle = window.window_handle();
                        pane.update(cx, |pane, cx| pane.share.create(&ctx, handle, cx))
                    })
                    .into_any_element(),
            ],
            look,
        ));
        section(
            Some("Your account"),
            rows,
            Some("Your phone signs in with this account. It is the server's admin: it invites other people and approves devices.".into()),
            look,
        )
    }

    fn running(
        &self,
        ctx: &Ctx,
        settings: &Settings,
        account: &str,
        look: &Look,
        cx: &mut Context<ServerPane>,
    ) -> AnyElement {
        let sharing = ctx.sharing.read(cx);
        let mut rows = vec![{
            let ctx = ctx.clone();
            Toggle::new(
                "share",
                "Share this device's library",
                settings.share_enabled,
            )
            .disabled(sharing.busy())
            .on_change(move |on, window, cx| {
                if *on {
                    ShareForm::set_sharing(&ctx, true, cx);
                    return;
                }
                // Every phone and device on it loses the library at once.
                let off = ctx.clone();
                let sheet = Sheet::new("Stop sharing this device's library?")
                    .message("Your phone and other devices signed in to this device lose its library, and can't play from it, until sharing is on again. Their accounts and keys are kept.")
                    .action(
                        Action::new("Stop Sharing", move |_, _, cx| {
                            ShareForm::set_sharing(&off, false, cx)
                        })
                        .destructive(),
                    );
                ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
            })
            .into_any_element()
        }];
        if let Some(url) = sharing.serving.as_ref().and_then(|s| s.lan_url.clone()) {
            rows.push(
                Row::new("On your network")
                    .control(CopyText::new("copy-lan", url).help("Copy the address"))
                    .into_any_element(),
            );
        }
        if let Some(failure) = &sharing.failure {
            rows.push(note(failure.clone(), Icon::Warning, Tone::Bad, look));
        }
        let reset_ctx = ctx.clone();
        let who = account.to_owned();
        rows.push(
            Row::data(account.to_owned())
                .control(
                    Button::new("reset-admin", ButtonKind::Compact)
                        .label("Reset Password…")
                        .on_click(move |_, window, cx| {
                            ShareForm::reset_password(who.clone(), &reset_ctx, window, cx)
                        }),
                )
                .into_any_element(),
        );
        rows.push(
            Row::new("Port")
                .control(
                    div()
                        .w(look.spaced(120.0))
                        .child(TextField::new(&self.port)),
                )
                .into_any_element(),
        );
        section(
            Some("Share this device"),
            rows,
            Some("Devices on your network reach this device while kōan is open and the computer is awake. Sign-ins and music cross your network unencrypted, as with any plain http:// server.".into()),
            look,
        )
    }
}

/// Where invites send phones: this network, the tailnet, the internet
/// through Tailscale Funnel, or a tunnel's address.
fn away_from_home(
    ctx: &Ctx,
    settings: &Settings,
    address: &Entity<InputState>,
    look: &Look,
    cx: &App,
) -> AnyElement {
    let sharing = ctx.sharing.read(cx);
    // From the engine's last reading, with the address beside it, so the
    // two never show different moments.
    let choice = sharing
        .status
        .as_ref()
        .map(|s| s.to)
        .unwrap_or(settings.share_invite_to);
    let serving = sharing.serving.is_some();
    let tailscale = sharing.status.as_ref().and_then(|s| s.tailscale.clone());
    let checked = sharing.status.is_some();
    let options = invite_options(
        tailscale.as_ref().map(|t| t.funnel_allowed),
        checked,
        choice,
        serving,
    );
    let mut rows = vec![
        Row::new("Tailscale")
            .value(match (&tailscale, checked) {
                (Some(t), _) => format!("Detected · {}", t.name),
                (None, true) => "Not detected".to_owned(),
                (None, false) => "Looking…".to_owned(),
            })
            .into_any_element(),
    ];
    let values: Vec<InviteTo> = options.iter().map(|(_, v)| *v).collect();
    let picker_ctx = ctx.clone();
    let funnel_host = tailscale
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_default();
    rows.push(
        Row::new("Invite phones to")
            .control(
                Picker::new(
                    "invite-to",
                    options.iter().map(|(label, _)| label.as_str()),
                    values.iter().position(|v| *v == choice),
                )
                .named("invite phones to")
                .disabled(sharing.busy())
                .on_change(move |index, window, cx| {
                    let to = values[*index];
                    if to == InviteTo::Funnel {
                        let sharing = picker_ctx.sharing.clone();
                        let model = picker_ctx.model.clone();
                        let sheet = Sheet::new("Publish kōan on the internet?")
                            .message(format!(
                                "Tailscale Funnel publishes this device at https://{funnel_host} with a certificate of its own, and keeps it published whenever this device shares. Anyone can reach it; only accounts made here can sign in."
                            ))
                            .action(Action::new("Publish", move |_, _, cx| {
                                let model = model.clone();
                                let reload: Landed = Box::new(move |_, cx| model.update(cx, |m, cx| m.reload(cx)));
                                sharing.update(cx, |s, cx| s.set_invite_to(InviteTo::Funnel, None, Some(reload), cx));
                            }));
                        picker_ctx.sheets.update(cx, |s, cx| s.show(sheet, window, cx));
                    } else {
                        let model = picker_ctx.model.clone();
                        let reload: Landed = Box::new(move |_, cx| model.update(cx, |m, cx| m.reload(cx)));
                        picker_ctx
                            .sharing
                            .update(cx, |s, cx| s.set_invite_to(to, None, Some(reload), cx));
                    }
                }),
            )
            .into_any_element(),
    );
    if serving && let Some(addresses) = &sharing.status {
        if choice == InviteTo::Custom {
            rows.push(
                Row::new("Address")
                    .control(div().w_full().child(TextField::new(address)))
                    .into_any_element(),
            );
        } else {
            rows.push(
                Row::new("Address")
                    .value(addresses.current.clone().unwrap_or_default())
                    .into_any_element(),
            );
        }
        if choice == InviteTo::Funnel && addresses.funnel.is_none() {
            rows.push(note(
                "Not published on the internet yet: invites use the address above.",
                Icon::Warning,
                Tone::Bad,
                look,
            ));
        }
    }
    if let Some(error) = &sharing.funnel_error {
        rows.push(note(error.clone(), Icon::Warning, Tone::Bad, look));
    }
    section(
        Some("Away from home"),
        rows,
        Some(invite_note(choice).into()),
        look,
    )
}

fn invite_options(
    funnel_allowed: Option<bool>,
    checked: bool,
    choice: InviteTo,
    serving: bool,
) -> Vec<(String, InviteTo)> {
    let mut options = vec![("This network".to_owned(), InviteTo::Network)];
    let running = funnel_allowed.is_some();
    if running || choice == InviteTo::Tailnet {
        let missing = !running && checked;
        options.push((
            if missing {
                "Your tailnet (Tailscale not running)"
            } else {
                "Your tailnet"
            }
            .to_owned(),
            InviteTo::Tailnet,
        ));
    }
    // Publishing needs a server to publish.
    if (funnel_allowed == Some(true) && serving) || choice == InviteTo::Funnel {
        options.push(("Tailscale Funnel".to_owned(), InviteTo::Funnel));
    }
    options.push(("Custom address".to_owned(), InviteTo::Custom));
    options
}

fn invite_note(choice: InviteTo) -> &'static str {
    match choice {
        InviteTo::Network => "Phones reach this device on your network only.",
        InviteTo::Tailnet => {
            "Phones with Tailscale reach this device from anywhere, encrypted, and nothing is on the internet."
        }
        InviteTo::Funnel => {
            "On the internet over HTTPS, for phones without Tailscale. Taken down when this device stops sharing, and put back when it starts."
        }
        InviteTo::Custom => {
            "A tunnel's HTTPS address, such as Cloudflare Tunnel's. Empty, invites use the address shown."
        }
    }
}

/// After a start: one that failed turns the setting off again; either way
/// the model reads the file, where a start may have moved the Funnel's
/// address.
fn after_start(model: Entity<SettingsModel>) -> Landed {
    Box::new(move |failed, cx| {
        model.update(cx, |m, cx| {
            if failed {
                m.edit(cx, |s| s.share_enabled = false);
            } else {
                m.reload(cx);
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{InviteTo, invite_options};

    fn values(options: Vec<(String, InviteTo)>) -> Vec<InviteTo> {
        options.into_iter().map(|(_, v)| v).collect()
    }

    #[test]
    fn offers_what_tailscale_allows() {
        use InviteTo::*;
        assert_eq!(
            values(invite_options(None, true, Network, true)),
            [Network, Custom]
        );
        assert_eq!(
            values(invite_options(Some(true), true, Network, true)),
            [Network, Tailnet, Funnel, Custom]
        );
        // Funnel needs a server running to publish.
        assert_eq!(
            values(invite_options(Some(true), true, Network, false)),
            [Network, Tailnet, Custom]
        );
        // A choice already made stays offered, Tailscale gone or not.
        assert_eq!(
            values(invite_options(None, true, Funnel, false)),
            [Network, Funnel, Custom]
        );
        assert_eq!(
            invite_options(None, true, Tailnet, true)[1].0,
            "Your tailnet (Tailscale not running)"
        );
    }
}

//! People: the server's accounts, for its admins. Add someone, set what they
//! can do, send them an invite, remove them.
//!
//! Only a server listing `koanInvite` has these: accounts and their invites
//! arrived together, and making one answers with its invite. Only an admin
//! may see them, which the server says by refusing anyone else the list.

use std::sync::Arc;

use crate::components::{Button, ButtonKind, CopyButton, MenuPanel, MenuRow, Picker, TextField};
use crate::theme::{Icon, Look, Space, Tone, TypeRole, label};
use koan_kit::app::input::InputState;
use koan_kit::app::*;
use koan_runtime::{AccountRole, Invite, KoanEngine, KoanError, ServerAccount};

use crate::settings::form::{buttons, item, page, section, text};
use crate::settings::model::describe;
use crate::settings::qr::Code;
use crate::settings::sheet::{self, Action, Sheet};
use crate::settings::{Ctx, offers};

pub const EXTENSION: &str = "koanInvite";

const ROLES: [AccountRole; 3] = [AccountRole::Readonly, AccountRole::User, AccountRole::Admin];

fn role_label(role: AccountRole) -> &'static str {
    match role {
        AccountRole::Readonly => "Listen only",
        AccountRole::User => "Listen and edit",
        AccountRole::Admin => "Admin",
    }
}

/// What a role allows, for the question before one changes.
fn role_detail(role: AccountRole) -> &'static str {
    match role {
        AccountRole::Readonly => "They can listen, and no longer change playlists or favourites.",
        AccountRole::User => {
            "They can listen and change their playlists and favourites, but not manage the server."
        }
        AccountRole::Admin => "They can manage the server and everyone's accounts, as you can.",
    }
}

/// The accounts, asked for once per account and server signed in as.
pub struct People {
    engine: Arc<KoanEngine>,
    pub accounts: Option<Vec<ServerAccount>>,
    pub error: Option<String>,
    loading: Option<Task<()>>,
}

impl People {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        let mut people = Self {
            engine,
            accounts: None,
            error: None,
            loading: None,
        };
        people.load(cx);
        people
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        self.loading = Some(cx.spawn(async move |this, cx| {
            let asked = engine.server_accounts().await;
            this.update(cx, |people, cx| {
                match asked {
                    Ok(accounts) => people.accounts = Some(accounts),
                    // Refused to anyone not an admin: no list, and no People
                    // tab.
                    Err(KoanError::Refused { .. }) => people.accounts = None,
                    // Not reached this time: the list shown stays, with why.
                    Err(e) => people.error = Some(describe(&e)),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Runs a change and reads the list again either way, so a refused
    /// change shows what is stored. An invite it answers with is handed on.
    fn attempt<T: 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl Future<Output = Result<T, KoanError>> + 'static,
        landed: impl FnOnce(T, &mut App) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let made = this.update(cx, |people, cx| {
                people.load(cx);
                match result {
                    Ok(made) => {
                        people.error = None;
                        Some(made)
                    }
                    Err(e) => {
                        people.error = Some(describe(&e));
                        cx.notify();
                        None
                    }
                }
            });
            if let Ok(Some(made)) = made {
                cx.update(|cx| landed(made, cx));
            }
        })
        .detach();
    }
}

pub struct PeoplePane {
    ctx: Ctx,
    people: Entity<People>,
    username: Entity<InputState>,
    role: AccountRole,
    _subscriptions: Vec<Subscription>,
}

impl PeoplePane {
    pub fn new(
        ctx: Ctx,
        people: Entity<People>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::theme::label("Username")));
        Self {
            _subscriptions: vec![
                cx.observe(&people, |_, _, cx| cx.notify()),
                cx.observe(&ctx.connection, |_, _, cx| cx.notify()),
                cx.observe(&ctx.model, |_, _, cx| cx.notify()),
                cx.observe(&ctx.sharing, |_, _, cx| cx.notify()),
                cx.observe(&username, |_, _, cx| cx.notify()),
            ],
            ctx,
            people,
            username,
            role: AccountRole::Readonly,
        }
    }

    fn signed_in_as(&self, cx: &App) -> String {
        let sharing = self.ctx.sharing.read(cx);
        if sharing.serving.is_some() {
            return sharing.account.clone().unwrap_or_default();
        }
        self.ctx
            .model
            .read(cx)
            .settings()
            .map(|s| s.remote_username.clone())
            .unwrap_or_default()
    }

    /// Shows an invite once it is made.
    fn show_invite(&self, window: AnyWindowHandle) -> impl FnOnce(Invite, &mut App) + 'static {
        let (sheets, model) = (self.ctx.sheets.clone(), self.ctx.model.clone());
        move |invite, cx| {
            // Shown this once, so it must reach someone: the window it was
            // asked for in, or, when that has closed, the next Settings
            // window.
            let mut sheet = Some(invite_sheet(invite));
            window
                .update(cx, |_, window, cx| {
                    if let Some(sheet) = sheet.take() {
                        sheets.update(cx, |s, cx| s.show(sheet, window, cx));
                    }
                })
                .ok();
            if let Some(sheet) = sheet {
                model.update(cx, |m, cx| {
                    m.unseen.push(sheet);
                    cx.notify();
                });
            }
        }
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.username.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        self.username
            .update(cx, |s, cx| s.set_value("", window, cx));
        let (engine, role) = (self.people.read(cx).engine.clone(), self.role);
        let shown = self.show_invite(window.window_handle());
        self.people.update(cx, |people, cx| {
            people.attempt(cx, engine.create_server_account(name, role), shown)
        });
    }

    fn set_role(&self, account: String, role: AccountRole, window: &mut Window, cx: &mut App) {
        let people = self.people.clone();
        let sheet = Sheet::new(format!(
            "Make {account} {}?",
            role_label(role).to_lowercase()
        ))
        .message(role_detail(role))
        .action(Action::new("Change", move |_, _, cx| {
            let account = account.clone();
            people.update(cx, |people, cx| {
                let engine = people.engine.clone();
                people.attempt(cx, engine.set_server_account_role(account, role), |_, _| {})
            })
        }));
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn invite(&self, account: String, reset: bool, window: &mut Window, cx: &mut App) {
        let shown = self.show_invite(window.window_handle());
        let people = self.people.clone();
        let run = move |cx: &mut App| {
            people.update(cx, |people, cx| {
                let engine = people.engine.clone();
                people.attempt(cx, engine.invite_server_account(account, reset), shown)
            })
        };
        if !reset {
            run(cx);
            return;
        }
        let run = std::cell::RefCell::new(Some(run));
        let sheet = Sheet::new("Give them a new password?")
            .message(
                "The invite carries the new password. Their devices will have to sign in again.",
            )
            .action(Action::new("New Password", move |_, _, cx| {
                if let Some(run) = run.borrow_mut().take() {
                    run(cx)
                }
            }));
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn set_password(&self, account: String, window: &mut Window, cx: &mut App) {
        let field = sheet::field("New password", true, window, cx);
        let people = self.people.clone();
        let sheet = Sheet::new(format!("Set a password for {account}"))
            .message("Their devices will have to sign in again with it.")
            .field("New password", field)
            .action(
                Action::new("Set", move |values, _, cx| {
                    let (account, password) = (account.clone(), values[0].clone());
                    people.update(cx, |people, cx| {
                        let engine = people.engine.clone();
                        people.attempt(
                            cx,
                            engine.set_server_account_password(account, password),
                            |_, _| {},
                        )
                    })
                })
                .ready(|values| !values[0].is_empty()),
            );
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn delete(&self, account: String, window: &mut Window, cx: &mut App) {
        let people = self.people.clone();
        let sheet = Sheet::new(format!("Delete {account}?"))
            .message("Their devices stop working, and their playlists and favourites go.")
            .action(
                Action::new("Delete", move |_, _, cx| {
                    let account = account.clone();
                    people.update(cx, |people, cx| {
                        let engine = people.engine.clone();
                        people.attempt(cx, engine.delete_server_account(account), |_, _| {})
                    })
                })
                .destructive(),
            );
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }
}

impl Render for PeoplePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let people = self.people.read(cx);
        let Some(accounts) = people.accounts.clone() else {
            return page("people", &look, Vec::new());
        };
        let error = people.error.clone();
        let me = self.signed_in_as(cx);
        let passwords = offers(&self.ctx.connection.read(cx).0, super::account::PASSWORDS);
        let entity = cx.entity();
        let mut rows = Vec::new();
        for (index, account) in accounts.iter().enumerate() {
            let name = account.username.clone();
            let role = account.role;
            let picker = {
                let entity = entity.clone();
                let name = name.clone();
                Picker::new(
                    ("role", index),
                    ROLES.map(role_label),
                    ROLES.iter().position(|r| *r == role),
                )
                .named(format!("{name}'s role"))
                .on_change(move |index, window, cx| {
                    let role = ROLES[*index];
                    entity.update(cx, |this, cx| this.set_role(name.clone(), role, window, cx))
                })
            };
            let menu = more_menu(
                index,
                &name,
                name == me,
                passwords,
                &entity,
                &look,
                window,
                cx,
            );
            rows.push(item(
                name.clone(),
                (name == me).then(|| "you".into()),
                vec![picker.into_any_element(), menu],
                &look,
            ));
        }
        let ready = !self.username.read(cx).value().trim().is_empty();
        let role = self.role;
        rows.push(buttons(
            vec![
                div()
                    .w(look.spaced(200.0))
                    .child(TextField::new(&self.username))
                    .into_any_element(),
                Picker::new(
                    "new-role",
                    ROLES.map(role_label),
                    ROLES.iter().position(|r| *r == role),
                )
                .named("new person's role")
                .on_change({
                    let entity = entity.clone();
                    move |index, _, cx| {
                        let role = ROLES[*index];
                        entity.update(cx, |pane, cx| {
                            pane.role = role;
                            cx.notify();
                        })
                    }
                })
                .into_any_element(),
                Button::new("add-person", ButtonKind::Bordered)
                    .label("Add")
                    .disabled(!ready)
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |pane, cx| pane.add(window, cx))
                    })
                    .into_any_element(),
            ],
            &look,
        ));
        let footer = error.clone().unwrap_or_else(|| {
            "Adding someone makes their invite: one link that sets kōan up with the account.".into()
        });
        let tone = if error.is_some() {
            Tone::Bad
        } else {
            Tone::Muted
        };
        page(
            "people",
            &look,
            vec![
                section(Some("People"), rows, None, &look),
                text(footer, TypeRole::Fine, tone, &look).into_any_element(),
            ],
        )
    }
}

/// An account's ⋯: invite, and for anyone but you, a password, a new one
/// with an invite, and deleting them. A new password for your own account
/// would sign this app out too.
#[allow(clippy::too_many_arguments)]
fn more_menu(
    index: usize,
    account: &str,
    me: bool,
    passwords: bool,
    pane: &Entity<PeoplePane>,
    look: &Look,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let open = window.use_keyed_state(("people-more-open", index), cx, |_, _| false);
    let shown = *open.read(cx);
    let set = |open: &Entity<bool>, to: bool| {
        let open = open.clone();
        move |cx: &mut App| {
            open.update(cx, |o, cx| {
                *o = to;
                cx.notify();
            })
        }
    };
    type Run = Box<dyn Fn(&mut Window, &mut App)>;
    let row = |id: &'static str, text: &str, run: Run| {
        let close = set(&open, false);
        MenuRow::new(id, text).on_click(move |_, window, cx| {
            close(cx);
            run(window, cx);
        })
    };
    let mut panel = MenuPanel::new();
    {
        let (pane, account) = (pane.clone(), account.to_owned());
        panel = panel.child(row(
            "invite",
            "Invite",
            Box::new(move |window, cx| {
                pane.update(cx, |this, cx| {
                    this.invite(account.clone(), false, window, cx)
                })
            }),
        ));
    }
    if !me {
        if passwords {
            let (pane, account) = (pane.clone(), account.to_owned());
            panel = panel.child(row(
                "set-password",
                "Set Password…",
                Box::new(move |window, cx| {
                    pane.update(cx, |this, cx| {
                        this.set_password(account.clone(), window, cx)
                    })
                }),
            ));
        }
        let (pane2, account2) = (pane.clone(), account.to_owned());
        panel = panel.child(row(
            "reset",
            "New Password and Invite…",
            Box::new(move |window, cx| {
                pane2.update(cx, |this, cx| {
                    this.invite(account2.clone(), true, window, cx)
                })
            }),
        ));
        let (pane3, account3) = (pane.clone(), account.to_owned());
        panel = panel.child(
            row(
                "delete",
                "Delete…",
                Box::new(move |window, cx| {
                    pane3.update(cx, |this, cx| this.delete(account3.clone(), window, cx))
                }),
            )
            .destructive(true),
        );
    }
    let opener = set(&open, true);
    let closer = set(&open, false);
    div()
        .relative()
        .child(
            Button::icon_only(
                ("people-more", index),
                Icon::MoreCircled,
                label(format!("More for {account}")),
            )
            .on_click(move |_, _, cx| opener(cx)),
        )
        .when(shown, |this| {
            this.child(crate::components::anchored_panel(
                ("people-menu", index),
                panel,
                closer,
                look,
            ))
        })
        .into_any_element()
}

/// An invite, ready to send. The server sends no mail; this hands the email
/// to whatever the admin sends mail with.
pub fn invite_sheet(invite: Invite) -> Sheet {
    let code = Code::new(&invite.qr_link);
    let message = "Opening the link on a phone, tablet or computer with kōan installed signs in and loads the library, on each device, for a week. The QR code works for ten minutes.";
    let mut sheet = Sheet::new(format!("Invite for {}", invite.username))
        .message(message)
        .done()
        .kept();
    sheet = sheet.content(move |look, _, _| {
        let (link, text_body, mailto) = (
            invite.link.clone(),
            invite.email_text.clone(),
            invite.mailto.clone(),
        );
        let mut column = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(look.space(Space::S))
            .children(code.as_ref().map(|code| {
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .items_center()
                    .gap(look.space(Space::Xs))
                    .child(code.element(look.spaced(168.0), look.spaced(12.0)))
                    .child(text(
                        "Scan with a phone's camera to open it there.",
                        TypeRole::Fine,
                        Tone::Muted,
                        look,
                    ))
            }))
            .child(
                Button::new("open-mail", ButtonKind::Link)
                    .label("Open in Mail")
                    .icon(Icon::Mail)
                    .on_click(move |_, _, cx| cx.open_url(&mailto)),
            )
            .child(CopyButton::new("copy-email", "Copy Email", text_body))
            .child(CopyButton::new("copy-invite", "Copy Link", link).icon(Icon::Share));
        if let Some(password) = &invite.password {
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .pt(look.space(Space::S))
                    .child(text(
                        "For other Subsonic apps",
                        TypeRole::Fine,
                        Tone::Ink,
                        look,
                    ))
                    .child(text(
                        format!("Server URL  {}", invite.server),
                        TypeRole::Meta,
                        Tone::Ink,
                        look,
                    ))
                    .child(text(
                        format!("Username    {}", invite.username),
                        TypeRole::Meta,
                        Tone::Ink,
                        look,
                    ))
                    .child(text(
                        format!("Password    {password}"),
                        TypeRole::Meta,
                        Tone::Ink,
                        look,
                    ))
                    .child(text(
                        "Shown this once: the server keeps only its hash.",
                        TypeRole::Fine,
                        Tone::Muted,
                        look,
                    )),
            );
        }
        column.into_any_element()
    });
    sheet
}

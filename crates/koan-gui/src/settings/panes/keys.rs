//! The signed-in account's API keys and app passwords: the same list twice.
//! An API key signs another Subsonic app in as the account, and each device
//! kōan is signed in on has one; an app password is for apps that sign in
//! only with a token and salt. Revoking either is how a lost device or app
//! is cut off.

use std::sync::Arc;

use crate::components::{Button, ButtonKind, CopyButton, Spinner, TextField, Tooltip};
use crate::theme::{Look, Space, Tone, TypeRole, label};
use koan_ffi::{KoanEngine, KoanError};
use koan_kit::app::input::InputState;
use koan_kit::app::*;

use crate::settings::Ctx;
use crate::settings::form::{buttons, day, item, section, text};
use crate::settings::model::describe;
use crate::settings::sheet::{Action, Sheet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ApiKeys,
    AppPasswords,
}

impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Kind::ApiKeys => "Key",
            Kind::AppPasswords => "App Password",
        }
    }
}

/// One row, whichever list it is in.
#[derive(Clone)]
struct Row {
    id: i64,
    name: String,
    created: Option<i64>,
    last_used: Option<i64>,
    this_device: bool,
}

pub struct Keys {
    kind: Kind,
    engine: Arc<KoanEngine>,
    ctx: Ctx,
    rows: Option<Vec<Row>>,
    error: Option<String>,
    name: Entity<InputState>,
    loading: Option<Task<()>>,
    _name: Subscription,
}

async fn list(engine: Arc<KoanEngine>, kind: Kind) -> Result<Vec<Row>, KoanError> {
    Ok(match kind {
        Kind::ApiKeys => engine
            .api_keys()
            .await?
            .into_iter()
            .map(|k| Row {
                id: k.id,
                name: k.name,
                created: k.created,
                last_used: k.last_used,
                this_device: k.this_device,
            })
            .collect(),
        Kind::AppPasswords => engine
            .app_passwords()
            .await?
            .into_iter()
            .map(|p| Row {
                id: p.id,
                name: p.name,
                created: p.created,
                last_used: p.last_used,
                this_device: false,
            })
            .collect(),
    })
}

impl Keys {
    pub fn new(kind: Kind, ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("The app it is for"));
        let mut keys = Self {
            kind,
            engine: ctx.model.read(cx).engine().clone(),
            ctx,
            rows: None,
            error: None,
            _name: cx.observe(&name, |_, _, cx| cx.notify()),
            name,
            loading: None,
        };
        keys.load(cx);
        keys
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let (engine, kind) = (self.engine.clone(), self.kind);
        self.loading = Some(cx.spawn(async move |this, cx| {
            let listed = list(engine, kind).await;
            this.update(cx, |keys, cx| {
                match listed {
                    Ok(rows) => keys.rows = Some(rows),
                    Err(e) => keys.error = Some(describe(&e)),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Makes one, then shows it this once: the server keeps only its hash
    /// or a sealed copy.
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let (engine, kind) = (self.engine.clone(), self.kind);
        let sheets = self.ctx.sheets.clone();
        let model = self.ctx.model.clone();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let made = match kind {
                Kind::ApiKeys => engine.create_api_key(name).await.map(|k| (k.name, k.key)),
                Kind::AppPasswords => engine
                    .create_app_password(name)
                    .await
                    .map(|p| (p.name, p.password)),
            };
            this.update(cx, |keys, cx| {
                match &made {
                    Ok(_) => keys.error = None,
                    Err(e) => keys.error = Some(describe(e)),
                }
                keys.load(cx);
            })
            .ok();
            let Ok((name, secret)) = made else { return };
            // Shown this once, so it must reach someone: the window it was
            // made in, or, when that has closed, the next Settings window.
            let sheet = shown_once(kind, name, secret);
            let mut sheet = Some(sheet);
            let shown = handle.update(cx, |_, window, cx| {
                this.update(cx, |keys, cx| {
                    keys.name.update(cx, |s, cx| s.set_value("", window, cx))
                })
                .ok();
                if let Some(sheet) = sheet.take() {
                    sheets.update(cx, |s, cx| s.show(sheet, window, cx));
                }
            });
            if shown.is_err()
                && let Some(sheet) = sheet
            {
                model.update(cx, |m, cx| {
                    m.unseen.push(sheet);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn revoke(&self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let sheet = Sheet::new(format!("Revoke \u{201C}{}\u{201D}?", row.name))
            .message("Whatever signs in with it stops working.")
            .action(
                Action::new("Revoke", move |_, _, cx| {
                    entity.update(cx, |keys, cx| {
                        let (engine, kind, id) = (keys.engine.clone(), keys.kind, row.id);
                        cx.spawn(async move |this, cx| {
                            let result = match kind {
                                Kind::ApiKeys => engine.revoke_api_key(id).await,
                                Kind::AppPasswords => engine.revoke_app_password(id).await,
                            };
                            this.update(cx, |keys, cx| {
                                keys.error = result.err().map(|e| describe(&e));
                                keys.load(cx);
                            })
                            .ok();
                        })
                        .detach();
                    })
                })
                .destructive(),
            );
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }
}

/// A key or app password just made, shown this once.
fn shown_once(kind: Kind, name: String, secret: String) -> Sheet {
    let noun = kind.noun();
    Sheet::new(format!("{noun} for {name}"))
        .kept()
        .message("This is the only time it is shown. Copy it into the app now; if it is lost, revoke it and make another.")
        .content(move |look, _, _| {
            div()
                .flex()
                .flex_col()
                .gap(look.space(Space::S))
                .child(text(secret.clone(), TypeRole::Control, Tone::Strong, look))
                .child(CopyButton::new(
                    "copy-secret",
                    format!("Copy {noun}"),
                    secret.clone(),
                ))
                .into_any_element()
        })
        .done()
}

fn detail(row: &Row) -> String {
    let made = row.created.map(|at| format!("Made {}", day(at)));
    let used = row.last_used.map_or_else(
        || "never used".to_owned(),
        |at| format!("last used {}", day(at)),
    );
    made.into_iter()
        .chain([used])
        .collect::<Vec<_>>()
        .join(" · ")
}

impl Render for Keys {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let entity = cx.entity();
        let mut rows = Vec::new();
        match &self.rows {
            Some(list) => {
                for (index, row) in list.iter().enumerate() {
                    let control = if row.this_device {
                        // Revoking it is signing out, which Sign Out does properly.
                        div()
                            .id(("this-device", index))
                            .child(text(
                                label("This device"),
                                TypeRole::Control,
                                Tone::Muted,
                                &look,
                            ))
                            .tooltip(|window, cx| {
                                Tooltip::view(label("To stop using it, sign out"), window, cx)
                            })
                            .into_any_element()
                    } else {
                        let entity = entity.clone();
                        let row = row.clone();
                        Button::new(("revoke", index), ButtonKind::Compact)
                            .label("Revoke…")
                            .destructive(true)
                            .on_click(move |_, window, cx| {
                                let row = row.clone();
                                entity.update(cx, |keys, cx| keys.revoke(row, window, cx))
                            })
                            .into_any_element()
                    };
                    rows.push(item(
                        row.name.clone(),
                        Some(detail(row).into()),
                        vec![control],
                        &look,
                    ));
                }
                let ready = !self.name.read(cx).value().trim().is_empty();
                let new_label = match self.kind {
                    Kind::ApiKeys => "New Key",
                    Kind::AppPasswords => "New App Password",
                };
                rows.push(buttons(
                    vec![
                        div()
                            .w(look.spaced(260.0))
                            .child(TextField::new(&self.name))
                            .into_any_element(),
                        Button::new("new-secret", ButtonKind::Bordered)
                            .label(new_label)
                            .disabled(!ready)
                            .on_click(move |_, window, cx| {
                                entity.update(cx, |keys, cx| keys.create(window, cx))
                            })
                            .into_any_element(),
                    ],
                    &look,
                ));
            }
            None if self.error.is_none() => {
                rows.push(Spinner::new(look.font_size(TypeRole::Body)).into_any_element())
            }
            None => {}
        }
        let (header, about) = match self.kind {
            Kind::ApiKeys => (
                "API keys",
                "A key signs another Subsonic app in as you, without your password. Each device kōan is signed in on has one too; revoking it signs that device out.",
            ),
            Kind::AppPasswords => (
                "App passwords",
                "For Subsonic apps that sign in only with a token and salt, and so cannot use an API key. Changing your password revokes them all.",
            ),
        };
        let footer: SharedString = self
            .error
            .clone()
            .unwrap_or_else(|| about.to_owned())
            .into();
        let tone = if self.error.is_some() {
            Tone::Bad
        } else {
            Tone::Muted
        };
        div()
            .child(section(Some(header), rows, None, &look))
            .child(text(footer, TypeRole::Fine, tone, &look).pt(look.space(Space::Xs)))
    }
}

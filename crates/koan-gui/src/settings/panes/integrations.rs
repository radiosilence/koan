//! Integrations: what the server does on the account's behalf. Serving
//! assistants over MCP, and scrobbling to ListenBrainz. Each where the
//! server has it.

use crate::components::{Button, ButtonKind, CopyButton, CopyText, Spinner, TextField};
use crate::theme::{Icon, Look, Tone, TypeRole};
use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
use koan_runtime::{Assistants, ScrobblingConnection};

use crate::settings::form::{Row, buttons, note, page, section, text};
use crate::settings::model::{count, describe};
use crate::settings::sheet::{Action, Sheet};
use crate::settings::{Ctx, offers};

pub const MCP: &str = "koanMcp";

pub struct IntegrationsPane {
    ctx: Ctx,
    assistants: Option<Result<Assistants, String>>,
    asked_assistants: bool,
    /// `None` until the server has answered.
    scrobbling: Option<Result<Option<ScrobblingConnection>, String>>,
    scrobbling_on: Option<bool>,
    scrobbling_error: Option<String>,
    token: Entity<InputState>,
    busy: bool,
    assistants_task: Option<Task<()>>,
    status_task: Option<Task<()>>,
    connecting: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl IntegrationsPane {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::theme::label("ListenBrainz user token"))
                .masked(true)
        });
        let mut pane = Self {
            _subscriptions: vec![
                cx.observe(&ctx.connection, |pane, _, cx| pane.follow(cx)),
                cx.observe(&token, |_, _, cx| cx.notify()),
                cx.subscribe_in(&token, window, |pane, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        pane.connect(window, cx);
                    }
                }),
            ],
            ctx,
            assistants: None,
            asked_assistants: false,
            scrobbling: None,
            scrobbling_on: None,
            scrobbling_error: None,
            token,
            busy: false,
            assistants_task: None,
            status_task: None,
            connecting: None,
        };
        pane.follow(cx);
        pane
    }

    /// Asks the server for what it now offers, once each.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let connection = self.ctx.connection.read(cx).0.clone();
        let engine = self.ctx.model.read(cx).engine().clone();
        if offers(&connection, MCP) && !self.asked_assistants {
            self.asked_assistants = true;
            let engine = engine.clone();
            self.assistants_task = Some(cx.spawn(async move |this, cx| {
                let asked = engine.assistants().await.map_err(|e| describe(&e));
                this.update(cx, |pane, cx| {
                    pane.assistants = Some(asked);
                    cx.notify();
                })
                .ok();
            }));
        }
        let on = connection.as_ref().map(|c| c.scrobbling);
        if on != self.scrobbling_on {
            self.scrobbling_on = on;
            if on == Some(true) {
                self.load_scrobbling(cx);
            }
        }
        cx.notify();
    }

    fn load_scrobbling(&mut self, cx: &mut Context<Self>) {
        let engine = self.ctx.model.read(cx).engine().clone();
        self.scrobbling = None;
        self.status_task = Some(cx.spawn(async move |this, cx| {
            let status = engine.scrobbling_status().await.map_err(|e| describe(&e));
            this.update(cx, |pane, cx| {
                pane.scrobbling_error = status.as_ref().err().cloned();
                pane.scrobbling = Some(status);
                cx.notify();
            })
            .ok();
        }));
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.token.read(cx).value().trim().to_owned();
        if typed.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        let engine = self.ctx.model.read(cx).engine().clone();
        self.connecting = Some(cx.spawn_in(window, async move |this, cx| {
            let connected = engine.connect_scrobbling(typed).await;
            this.update_in(cx, |pane, window, cx| {
                pane.busy = false;
                match connected {
                    Ok(connection) => {
                        pane.scrobbling = Some(Ok(connection));
                        pane.scrobbling_error = None;
                        pane.token.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                    Err(e) => pane.scrobbling_error = Some(describe(&e)),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn disconnect(&self, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let sheet = Sheet::new("Disconnect ListenBrainz?")
            .message("The server stops sending your plays to ListenBrainz and forgets the token. What was sent stays there.")
            .action(
                Action::new("Disconnect", move |_, _, cx| {
                    entity.update(cx, |pane, cx| {
                        pane.busy = true;
                        let engine = pane.ctx.model.read(cx).engine().clone();
                        pane.connecting = Some(cx.spawn(async move |this, cx| {
                            let result = engine.disconnect_scrobbling().await;
                            this.update(cx, |pane, cx| {
                                pane.busy = false;
                                match result {
                                    Ok(()) => {
                                        pane.scrobbling = Some(Ok(None));
                                        pane.scrobbling_error = None;
                                    }
                                    Err(e) => pane.scrobbling_error = Some(describe(&e)),
                                }
                                cx.notify();
                            })
                            .ok();
                        }));
                        cx.notify();
                    })
                })
                .destructive(),
            );
        self.ctx
            .sheets
            .update(cx, |s, cx| s.show(sheet, window, cx));
    }

    fn assistants_section(&self, look: &Look, _cx: &mut Context<Self>) -> AnyElement {
        let mut rows = Vec::new();
        let mut footer = "Add the address to Claude or another assistant that speaks MCP as a custom connector. It signs in through your server, and can then search your library, make playlists and play music on your devices, as you.".to_owned();
        let mut tone = Tone::Muted;
        match &self.assistants {
            Some(Ok(assistants)) => {
                let url = assistants.mcp_url.clone();
                let connect = assistants.connect_url.clone();
                rows.push(
                    Row::new("Address")
                        .control(CopyText::new("copy-mcp", url.clone()))
                        .into_any_element(),
                );
                rows.push(buttons(
                    vec![
                        CopyButton::new("copy-mcp-address", "Copy Address", url).into_any_element(),
                    ],
                    look,
                ));
                // A link of its own, on its own line: it goes somewhere,
                // where the button beside the address acts on it.
                rows.push(
                    Button::new("how-to-connect", ButtonKind::Link)
                        .label("How to Connect an Assistant")
                        .icon(Icon::OpenExternal)
                        .on_click(move |_, _, cx| cx.open_url(&connect))
                        .into_any_element(),
                );
            }
            Some(Err(error)) => {
                footer = error.clone();
                tone = Tone::Ink;
            }
            None => rows.push(
                Spinner::new(look.font_size(TypeRole::Body))
                    .announced("assistants-loading", "loading assistants")
                    .into_any_element(),
            ),
        }
        div()
            .child(section(Some("Assistants"), rows, None, look))
            .child(text(footer, TypeRole::Fine, tone, look))
            .into_any_element()
    }

    fn scrobbling_section(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let mut rows = Vec::new();
        match &self.scrobbling {
            Some(Ok(Some(c))) => {
                rows.push(
                    text(
                        format!("Scrobbling to ListenBrainz as {}", c.account),
                        TypeRole::Body,
                        Tone::Ink,
                        look,
                    )
                    .into_any_element(),
                );
                if let Some(refused) = &c.error {
                    rows.push(note(refused.clone(), Icon::Warning, Tone::Bad, look));
                    rows.push(text("Disconnect, then connect again with a current token. Plays recorded meanwhile are kept and sent.", TypeRole::Meta, Tone::Muted, look).into_any_element());
                } else if c.pending > 0 {
                    let plays = if c.pending == 1 { "play" } else { "plays" };
                    rows.push(
                        text(
                            format!("{} {plays} waiting to be sent", count(c.pending as u64)),
                            TypeRole::Meta,
                            Tone::Muted,
                            look,
                        )
                        .into_any_element(),
                    );
                }
                rows.push(buttons(
                    vec![
                        Button::new("disconnect-scrobbling", ButtonKind::Bordered)
                            .label("Disconnect…")
                            .destructive(true)
                            .disabled(self.busy)
                            .on_click(move |_, window, cx| {
                                entity.update(cx, |pane, cx| pane.disconnect(window, cx))
                            })
                            .into_any_element(),
                    ],
                    look,
                ));
            }
            None => rows.push(
                Spinner::new(look.font_size(TypeRole::Body))
                    .announced("scrobbling-loading", "loading scrobbling")
                    .into_any_element(),
            ),
            // The server could not say whether the account is connected, so
            // neither state is shown.
            Some(Err(_)) => rows.push(buttons(
                vec![
                    Button::new("scrobbling-again", ButtonKind::Bordered)
                        .label("Try Again")
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |pane, cx| pane.load_scrobbling(cx))
                        })
                        .into_any_element(),
                ],
                look,
            )),
            Some(Ok(None)) => {
                let ready = !self.busy && !self.token.read(cx).value().trim().is_empty();
                rows.push(
                    Row::new("User token")
                        .control(div().w_full().child(TextField::new(&self.token)))
                        .into_any_element(),
                );
                rows.push(buttons(
                    vec![
                        Button::new("find-token", ButtonKind::Link)
                            .label("Find your token")
                            .on_click(|_, _, cx| cx.open_url("https://listenbrainz.org/settings/"))
                            .into_any_element(),
                        Button::new("connect-scrobbling", ButtonKind::Prominent)
                            .label("Connect")
                            .disabled(!ready)
                            .on_click(move |_, window, cx| {
                                entity.update(cx, |pane, cx| pane.connect(window, cx))
                            })
                            .into_any_element(),
                    ],
                    look,
                ));
            }
        }
        if let Some(error) = &self.scrobbling_error {
            rows.push(note(error.clone(), Icon::Warning, Tone::Bad, look));
        }
        section(
            Some("Scrobbling"),
            rows,
            Some("The server sends what you play to ListenBrainz, from every app signed in as you, your history included when you connect. The token is kept on the server.".into()),
            look,
        )
    }
}

impl Render for IntegrationsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let connection = self.ctx.connection.read(cx).0.clone();
        let mut sections = Vec::new();
        if offers(&connection, MCP) {
            sections.push(self.assistants_section(&look, cx));
        }
        if connection.as_ref().is_some_and(|c| c.scrobbling) {
            sections.push(self.scrobbling_section(&look, cx));
        }
        page("integrations", &look, sections)
    }
}

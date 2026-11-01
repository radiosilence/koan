//! Link a Device: a QR code that signs another device in as this account,
//! the account's own invite.
//!
//! Its token lapses in ten minutes, so a fresh one is fetched when the
//! section is made (each time its page is shown) and again before it
//! lapses. A server that refuses (one older than self-invites, to an
//! account that is not an admin) is not asked again, and the section is not
//! drawn. One that could not be reached is asked once more half a minute
//! later, then not until the connection to it changes or the page is shown
//! again.

use std::sync::Arc;
use std::time::Duration;

use crate::components::{Button, ButtonKind};
use crate::theme::{Icon, Look, Space, Tone, TypeRole};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{ConnectionInfo, Invite, KoanEngine, KoanError};

use crate::mirror::Slice;
use crate::settings::form::{section, text};
use crate::settings::qr::Code;

const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Fetched again before the token lapses at ten minutes.
const REFRESH_AFTER: Duration = Duration::from_secs(9 * 60);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gone {
    /// The server refused: not asked again.
    Refused,
    /// Unreached after the retry, while the connection stood like this.
    Unreached { linked: bool, offline: bool },
}

pub struct LinkDevice {
    engine: Arc<KoanEngine>,
    username: String,
    connection: Entity<Slice<Option<ConnectionInfo>>>,
    invite: Option<(Invite, Code)>,
    gone: Option<Gone>,
    copied: bool,
    asking: Option<Task<()>>,
    _connection: Subscription,
}

fn standing(connection: &Option<ConnectionInfo>) -> (bool, bool) {
    connection
        .as_ref()
        .map_or((false, false), |c| (c.linked, c.offline))
}

impl LinkDevice {
    pub fn new(
        engine: Arc<KoanEngine>,
        username: String,
        connection: Entity<Slice<Option<ConnectionInfo>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut link = Self {
            engine,
            username,
            _connection: cx.observe(&connection, |link, connection, cx| {
                let (linked, offline) = standing(&connection.read(cx).0);
                if let Some(Gone::Unreached {
                    linked: l,
                    offline: o,
                }) = link.gone
                    && (l, o) != (linked, offline)
                {
                    link.gone = None;
                    link.ask(cx);
                }
            }),
            connection,
            invite: None,
            gone: None,
            copied: false,
            asking: None,
        };
        link.ask(cx);
        link
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let username = self.username.clone();
        self.asking = Some(cx.spawn(async move |this, cx| {
            let mut retried = false;
            loop {
                let asked = engine
                    .clone()
                    .invite_server_account(username.clone(), false)
                    .await;
                let wait = match asked {
                    Ok(invite) => {
                        retried = false;
                        // Encoded off the main thread: the matrix is the
                        // costly part, and the page need not wait for it.
                        let link = invite.qr_link.clone();
                        let code = cx
                            .background_executor()
                            .spawn(async move { Code::new(&link) })
                            .await;
                        let kept = this.update(cx, |link, cx| {
                            link.invite = code.map(|code| (invite, code));
                            cx.notify();
                        });
                        if kept.is_err() {
                            return;
                        }
                        REFRESH_AFTER
                    }
                    Err(KoanError::Refused { .. }) => {
                        this.update(cx, |link, cx| {
                            link.invite = None;
                            link.gone = Some(Gone::Refused);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Err(_) if retried => {
                        this.update(cx, |link, cx| {
                            let (linked, offline) = standing(&link.connection.read(cx).0);
                            link.invite = None;
                            link.gone = Some(Gone::Unreached { linked, offline });
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Err(_) => {
                        retried = true;
                        RETRY_AFTER
                    }
                };
                cx.background_executor().timer(wait).await;
            }
        }));
    }
}

impl Render for LinkDevice {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        if self.gone.is_some() {
            return div();
        }
        let rows = match &self.invite {
            Some((invite, code)) => {
                let link = invite.qr_link.clone();
                let entity = cx.entity();
                vec![
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(look.space(Space::S))
                        .py(look.space(Space::S))
                        .child(code.element(px(220.), px(14.)))
                        .child(text(
                            format!(
                                "Scan with your phone's camera to sign it in as {}.",
                                self.username
                            ),
                            TypeRole::Fine,
                            Tone::Muted,
                            &look,
                        ))
                        .child(
                            Button::new("copy-link", ButtonKind::Bordered)
                                .label(if self.copied { "Copied" } else { "Copy Link" })
                                .icon(Icon::Copy)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                                    entity.update(cx, |link, cx| {
                                        link.copied = true;
                                        cx.notify();
                                    })
                                }),
                        )
                        .into_any_element(),
                ]
            }
            // The code's room is kept while it is asked for, so the page does
            // not move when it arrives.
            None => vec![
                div()
                    .flex()
                    .w_full()
                    .justify_center()
                    .py(look.space(Space::S))
                    .child(
                        div()
                            .size(px(220.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::components::Spinner::new(px(24.))),
                    )
                    .into_any_element(),
            ],
        };
        div().child(section(
            Some("Link a Device"),
            rows,
            Some("kōan opens on the other device and loads the library, or offers the download first.".into()),
            &look,
        ))
    }
}

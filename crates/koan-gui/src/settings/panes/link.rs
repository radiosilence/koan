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

use crate::components::CopyButton;
use crate::theme::{Look, Space, Tone, TypeRole};
use koan_kit::app::*;
use koan_runtime::{ConnectionInfo, Invite, KoanEngine, KoanError};

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
        let ready = self.invite.is_some();
        let link = self
            .invite
            .as_ref()
            .map(|(invite, _)| invite.qr_link.clone());
        let side = look.spaced(220.0);
        // Laid out the same while the code is asked for, the caption and the
        // button held invisible, so nothing moves when it arrives.
        let code = match &self.invite {
            Some((_, code)) => code.element(side, look.spaced(14.0)).into_any_element(),
            None => crate::components::Spinner::new(look.spaced(24.0))
                .reserving(Size {
                    width: side,
                    height: side,
                })
                .into_any_element(),
        };
        let rows = vec![
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(look.space(Space::S))
                .py(look.space(Space::S))
                .child(code)
                .child(
                    text(
                        format!(
                            "Scan with your phone's camera to sign it in as {}.",
                            self.username
                        ),
                        TypeRole::Fine,
                        Tone::Muted,
                        &look,
                    )
                    .when(!ready, |d| d.invisible()),
                )
                .child(
                    div().when(!ready, |d| d.invisible()).child(
                        CopyButton::new("copy-link", "Copy Link", link.unwrap_or_default())
                            .disabled(!ready),
                    ),
                )
                .into_any_element(),
        ];
        div().child(section(
            Some("Link a Device"),
            rows,
            Some("kōan opens on the other device and loads the library, or offers the download first.".into()),
            &look,
        ))
    }
}

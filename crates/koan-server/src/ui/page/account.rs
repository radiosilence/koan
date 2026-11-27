//! Account: who is signed in; linking a device by its camera; two-step
//! sign-in; the API keys and app passwords Subsonic apps sign in with; and
//! the account's other pages.

use askama::Template;
use koan_core::invite::{self, Invite};
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::{Logged, UiState, is_admin, open, paths};

pub(in crate::ui) mod app_passwords;
pub(in crate::ui) mod keys;
pub(in crate::ui) mod two_step;

pub(in crate::ui) struct Account;

/// A credential as its list shows it: its name, when it was made and last
/// used.
pub(in crate::ui) struct Credential {
    pub(in crate::ui) id: i64,
    pub(in crate::ui) name: String,
    pub(in crate::ui) created: String,
    pub(in crate::ui) used: Option<String>,
}

impl Credential {
    pub(in crate::ui) fn new(id: i64, name: &str, created: i64, used: Option<i64>) -> Self {
        Self {
            id,
            name: name.to_owned(),
            created: super::scrobbling::day(created),
            used: used.map(super::scrobbling::day),
        }
    }

    fn detail(&self) -> String {
        match &self.used {
            Some(used) => format!("Created {} · last used {used}", self.created),
            None => format!("Created {} · never used", self.created),
        }
    }
}

/// The sign-in link as a QR code, for a phone to scan off this screen.
struct LinkDevice {
    qr: Option<kit::Qr<'static>>,
    link: String,
}

#[derive(Template)]
#[template(path = "account/page.html")]
struct AccountPage<'a> {
    username: &'a str,
    admin: bool,
    link: Option<LinkDevice>,
    /// The keys' and app passwords' sections, absent without sign-in.
    credentials: Option<(Html, Html)>,
    /// The two-step section, and whether two-step sign-in is off.
    two_step: Option<(Html, bool)>,
}

impl AccountPage<'_> {
    fn sign_out(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::sign_out())
    }

    fn more(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        let mut more = Vec::new();
        if self.admin {
            more.push((
                paths::users(),
                "Users",
                ": make accounts and invite people.",
            ));
        }
        more.push((
            paths::assistants(),
            "Assistants",
            ": connect Claude or another assistant to your music.",
        ));
        more.push((
            paths::scrobbling(),
            "Scrobbling",
            ": send what you play to ListenBrainz or Last.fm.",
        ));
        more
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

fn link_device(conn: &rusqlite::Connection, server: &str, user: &AuthUser) -> Option<LinkDevice> {
    let keys = crate::auth::signing_keys().logged()?;
    let token =
        invite::mint_token(conn, &keys.0, user.user_id, invite::QR_TOKEN_TTL_SECS).logged()?;
    let link = Invite::with_token(server, &user.username, &token, None).qr_link();
    Some(LinkDevice {
        qr: kit::Qr::new(&link).map(|q| q.label("Sign-in link as a QR code")),
        link,
    })
}

impl Page for Account {
    const SECTION: Section = Section::Account;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        let (link, credentials, two_step) = if s.auth_enabled {
            let (st, u) = (s.clone(), user.clone());
            blocking(move || {
                let db = open(&st.pool)?;
                let demo = crate::demo::enabled() && u.username == crate::demo::USERNAME;
                let two_step = if demo {
                    None
                } else {
                    let on = koan_auth::second_factor::enabled(&db.conn, u.user_id).logged()?;
                    Some((two_step::for_page(&st, &db.conn, u.user_id)?, !on))
                };
                let link = st
                    .server_url
                    .as_deref()
                    .and_then(|server| link_device(&db.conn, server, &u));
                let keys = keys::section(&db.conn, u.user_id, None)?;
                let passwords = app_passwords::section(&db.conn, u.user_id, None)?;
                Some((link, Some((keys, passwords)), two_step))
            })
            .await
            .ok_or(Refusal::Unavailable)?
        } else {
            (None, None, None)
        };
        let view = AccountPage {
            username: &user.username,
            admin: is_admin(user),
            link,
            credentials,
            two_step,
        };
        Ok(Body::of("Account", &view))
    }
}

/// A credential's list: each one's name, when it was made and last used, and
/// a button revoking it. `made` is a credential just made, shown this once.
#[derive(Template)]
#[template(path = "account/credentials.html")]
pub(in crate::ui) struct Credentials {
    pub(in crate::ui) id: &'static str,
    pub(in crate::ui) heading: &'static str,
    pub(in crate::ui) about: &'static str,
    pub(in crate::ui) what: &'static str,
    pub(in crate::ui) form: kit::Form<'static>,
    pub(in crate::ui) field: kit::Input<'static>,
    pub(in crate::ui) create: &'static str,
    pub(in crate::ui) rows: Vec<Credential>,
    pub(in crate::ui) empty: &'static str,
    pub(in crate::ui) made: Option<String>,
    pub(in crate::ui) revoke: fn(i64) -> String,
    pub(in crate::ui) question: &'static str,
}

impl Credentials {
    fn revoke_button(&self, c: &Credential) -> kit::Button<'static> {
        kit::Button::quiet("Revoke")
            .bad()
            .post((self.revoke)(c.id))
            .confirm(self.question, "Revoke")
    }

    fn detail(&self, c: &Credential) -> String {
        c.detail()
    }

    fn copy(&self, secret: &str) -> kit::CopyField<'static> {
        kit::CopyField::new(format!("New {}", self.what), secret.to_owned(), "Copy")
            .copied("Copied")
    }
}

/// The most a credential's name may be.
pub(in crate::ui) const MAX_NAME: usize = 100;

/// A name a form posted, trimmed: none where it is blank or too long.
pub(in crate::ui) fn name(posted: &str) -> Option<String> {
    let name = posted.trim();
    (!name.is_empty() && name.chars().count() <= MAX_NAME).then(|| name.to_owned())
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct Named {
    pub(in crate::ui) name: String,
}

/// Keys and app passwords belong to accounts.
pub(in crate::ui) const NO_ACCOUNTS: &str =
    "Keys and app passwords belong to accounts, and this server runs without sign-in.";

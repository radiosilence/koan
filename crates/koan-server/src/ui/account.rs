//! The signed-in user's account: who they are, signing out, and the
//! credentials Subsonic clients sign in with — API keys and app passwords, each
//! shown once when made, listed with when it was last used, revocable.

use askama::Template;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::Event;
use koan_core::auth::Role;
use koan_core::db::queries::api_keys::{self, ApiKeyRow};
use koan_core::db::queries::app_passwords::{self, AppPasswordRow};

use super::pages::{COPY_ERROR, respond};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;

pub(super) const MAX_NAME: usize = 100;

const REVOKE_KEY: &str = "Revoke this key? Clients using it will stop working.";
const REVOKE_APP_PASSWORD: &str = "Revoke this app password? The app using it will stop working.";

fn day(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

struct Credential<'a> {
    id: i64,
    name: &'a str,
    created: String,
    used: Option<String>,
}

impl<'a> Credential<'a> {
    fn new(id: i64, name: &'a str, created: i64, used: Option<i64>) -> Self {
        Self {
            id,
            name,
            created: day(created),
            used: used.map(day),
        }
    }
}

#[derive(Template)]
#[template(path = "account/credentials.html")]
struct Credentials<'a> {
    id: &'a str,
    rows: Vec<Credential<'a>>,
    empty: &'a str,
    /// Where each row's revoke posts, below its id.
    revoke: &'a str,
    confirm: &'a str,
}

fn key_list(keys: &[ApiKeyRow]) -> Html {
    Credentials {
        id: "keys",
        rows: keys
            .iter()
            .map(|k| Credential::new(k.id, &k.name, k.created_at, k.last_used_at))
            .collect(),
        empty: "No keys yet.",
        revoke: "/account/keys",
        confirm: REVOKE_KEY,
    }
    .html()
}

fn app_password_list(passwords: &[AppPasswordRow]) -> Html {
    Credentials {
        id: "app-passwords",
        rows: passwords
            .iter()
            .map(|p| Credential::new(p.id, &p.name, p.created_at, p.last_used_at))
            .collect(),
        empty: "No app passwords yet.",
        revoke: "/account/app-passwords",
        confirm: REVOKE_APP_PASSWORD,
    }
    .html()
}

/// The account page. `credentials` is the rendered key and app password
/// lists, absent when the server runs without sign-in.
#[derive(Template)]
#[template(path = "pages/account.html")]
struct AccountPage<'a> {
    username: &'a str,
    admin: bool,
    credentials: Option<(Html, Html)>,
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let credentials = if s.auth_enabled {
        let st = s.clone();
        let user_id = user.user_id;
        let (keys, passwords) = blocking(move || {
            let db = open(&st.pool)?;
            Some((
                api_keys::list_api_keys(&db.conn, Some(user_id)).ok()?,
                app_passwords::list_app_passwords(&db.conn, user_id).ok()?,
            ))
        })
        .await
        .unwrap_or_default();
        Some((key_list(&keys), app_password_list(&passwords)))
    } else {
        None
    };
    let inner = AccountPage {
        username: &user.username,
        admin: user.role == Role::Admin,
        credentials,
    }
    .html();
    respond(&s, &headers, &user, "Account", &inner)
}

/// The name Datastar posted as `signal`, if it is one.
fn posted_name(body: &[u8], signal: &str) -> Option<String> {
    let name = serde_json::from_slice::<serde_json::Value>(body)
        .ok()?
        .get(signal)?
        .as_str()?
        .trim()
        .to_owned();
    (!name.is_empty() && name.chars().count() <= MAX_NAME).then_some(name)
}

fn refused(result: &str, message: &str) -> Response {
    events(vec![patch(
        &format!("<div id={result} class=\"{COPY_ERROR}\" role=alert>{message}</div>"),
        None,
    )])
}

#[derive(Template)]
#[template(path = "account/made.html")]
struct Made<'a> {
    result: &'a str,
    what: &'a str,
    secret: &'a str,
}

/// A credential made: shown once with a copy button, the list redrawn, the
/// name field cleared.
fn made(result: &str, what: &str, secret: &str, list: Html, signal: &str) -> Response {
    events(vec![
        patch(
            &Made {
                result,
                what,
                secret,
            }
            .html(),
            None,
        ),
        patch(&list, None),
        Event::default()
            .event("datastar-patch-signals")
            .data(format!("signals {{\"{signal}\":\"\"}}")),
    ])
}

const NO_ACCOUNTS: &str =
    "Keys and app passwords belong to accounts, and this server runs without sign-in.";

pub(super) async fn create_key(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    if !s.auth_enabled {
        return refused("key-result", NO_ACCOUNTS);
    }
    let Some(name) = posted_name(&body, "keyname") else {
        return refused("key-result", "Give the key a name of up to 100 characters.");
    };
    let created = blocking(move || {
        let db = open(&s.pool)?;
        let (_, key) = api_keys::create_api_key(&db.conn, user.user_id, &name).ok()?;
        let keys = api_keys::list_api_keys(&db.conn, Some(user.user_id)).ok()?;
        Some((key, keys))
    })
    .await;
    match created {
        Some((key, keys)) => made("key-result", "key", &key, key_list(&keys), "keyname"),
        None => refused("key-result", "The key could not be made."),
    }
}

pub(super) async fn revoke_key(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    let keys = blocking(move || {
        let db = open(&s.pool)?;
        api_keys::revoke_api_key(&db.conn, id, Some(user.user_id)).ok()?;
        api_keys::list_api_keys(&db.conn, Some(user.user_id)).ok()
    })
    .await
    .unwrap_or_default();
    events(vec![
        patch(&key_list(&keys), None),
        patch("<div id=key-result></div>", None),
    ])
}

pub(super) async fn create_app_password(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    if !s.auth_enabled {
        return refused("app-password-result", NO_ACCOUNTS);
    }
    let Some(name) =
        posted_name(&body, "appname").and_then(|n| app_passwords::app_password_name(&n))
    else {
        return refused(
            "app-password-result",
            "Give the app password a name of up to 100 characters.",
        );
    };
    let key = koan_core::auth::app_password_key(&s.auth.private_pem);
    let created = blocking(move || {
        let db = open(&s.pool)?;
        let password = match app_passwords::create_app_password(&db.conn, &key, user.user_id, &name)
        {
            Ok((_, password)) => password,
            Err(e @ app_passwords::CreateAppPasswordError::TooMany) => {
                return Some(Err(e.to_string()));
            }
            Err(_) => return None,
        };
        let passwords = app_passwords::list_app_passwords(&db.conn, user.user_id).ok()?;
        Some(Ok((password, passwords)))
    })
    .await;
    match created {
        Some(Err(reason)) => refused("app-password-result", &reason),
        Some(Ok((password, passwords))) => made(
            "app-password-result",
            "app password",
            &password,
            app_password_list(&passwords),
            "appname",
        ),
        None => refused("app-password-result", "The app password could not be made."),
    }
}

pub(super) async fn revoke_app_password(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    let passwords = blocking(move || {
        let db = open(&s.pool)?;
        app_passwords::revoke_app_password(&db.conn, id, user.user_id).ok()?;
        app_passwords::list_app_passwords(&db.conn, user.user_id).ok()
    })
    .await
    .unwrap_or_default();
    events(vec![
        patch(&app_password_list(&passwords), None),
        patch("<div id=app-password-result></div>", None),
    ])
}

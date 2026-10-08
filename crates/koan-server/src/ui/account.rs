//! The signed-in user's account: who they are, signing out, and the
//! credentials Subsonic clients sign in with — API keys and app passwords, each
//! shown once when made, listed with when it was last used, revocable.

use std::fmt::Write as _;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::Event;
use koan_core::auth::Role;
use koan_core::db::queries::api_keys::{self, ApiKeyRow};
use koan_core::db::queries::app_passwords::{self, AppPasswordRow};

use super::pages::{COPY_ERROR, COPY_INPUT, COPY_ROW, EMPTY, SUB, respond};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

const MAX_NAME: usize = 100;

const REVOKE_KEY: &str = "Revoke this key? Clients using it will stop working.";
const REVOKE_APP_PASSWORD: &str = "Revoke this app password? The app using it will stop working.";

fn day(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

const ROW: &str = "flex min-w-0 items-center gap-3 border-b border-rule px-1 py-2.5";

/// One credential's row: its name, when it was made and last used, and a
/// revoke button posting to `revoke`.
fn row(out: &mut String, name: &str, created: i64, used: Option<i64>, revoke: &str, confirm: &str) {
    let used = used.map_or_else(
        || "never used".to_owned(),
        |t| format!("last used {}", day(t)),
    );
    let _ = write!(
        out,
        "<li class=\"{ROW}\"><span class=\"min-w-0 flex-1 overflow-hidden text-ellipsis wrap-anywhere\">{name}\
<small class=\"block text-meta text-muted\">Created {created} · {used}</small></span>\
<button class=\"quiet\" data-on:click=\"confirm('{confirm}') && @post('{revoke}')\">Revoke</button></li>",
        name = escape(name),
        created = day(created),
    );
}

fn key_list(keys: &[ApiKeyRow]) -> String {
    let mut out = String::from("<ul class=\"mt-4\" id=keys>");
    if keys.is_empty() {
        let _ = write!(out, "<li class=\"{ROW} {EMPTY}\">No keys yet.</li>");
    }
    for k in keys {
        let revoke = format!("/account/keys/{}/revoke", k.id);
        row(
            &mut out,
            &k.name,
            k.created_at,
            k.last_used_at,
            &revoke,
            REVOKE_KEY,
        );
    }
    out.push_str("</ul>");
    out
}

fn app_password_list(passwords: &[AppPasswordRow]) -> String {
    let mut out = String::from("<ul class=\"mt-4\" id=app-passwords>");
    if passwords.is_empty() {
        let _ = write!(
            out,
            "<li class=\"{ROW} {EMPTY}\">No app passwords yet.</li>"
        );
    }
    for p in passwords {
        let revoke = format!("/account/app-passwords/{}/revoke", p.id);
        row(
            &mut out,
            &p.name,
            p.created_at,
            p.last_used_at,
            &revoke,
            REVOKE_APP_PASSWORD,
        );
    }
    out.push_str("</ul>");
    out
}

/// A form making one credential, posting `signal` (the name) to `action`, with
/// the place its result lands.
fn create_form(
    action: &str,
    signal: &str,
    placeholder: &str,
    label: &str,
    button: &str,
    result: &str,
) -> String {
    format!(
        "<form class=\"mb-2 flex max-w-form gap-2\" data-on:submit__prevent=\"@post('{action}')\">\
<input class=\"flex-1\" name={signal} data-bind:{signal} placeholder=\"{placeholder}\" maxlength={MAX_NAME} required \
autocomplete=off aria-label=\"{label}\"><button class=\"primary\">{button}</button></form><div id={result}></div>"
    )
}

/// The page's links elsewhere: assistants and scrobbling for everyone,
/// accounts for an admin.
fn more(user: &AuthUser) -> String {
    let users = if user.role == Role::Admin {
        "<li><a href=\"/users\" data-nav=users>Users</a>: make accounts and invite people.</li>"
    } else {
        ""
    };
    format!(
        "<h2>More</h2><ul class=\"my-2 flex flex-col gap-1.5\">{users}\
<li><a href=\"/connect\" data-nav=connect>Assistants</a>: connect Claude or another assistant to your music.</li>\
<li><a href=\"/scrobbling\" data-nav=scrobbling>Scrobbling</a>: send what you play to ListenBrainz.</li>\
<li><a href=\"https://github.com/radiosilence/koan/releases/tag/v{v}\">kōan {v}</a></li></ul>",
        v = env!("CARGO_PKG_VERSION")
    )
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let inner = if !s.auth_enabled {
        format!(
            "<h1>Account</h1><p class=\"{EMPTY}\">This server runs without sign-in, so there are no \
accounts, keys or app passwords.</p>{}",
            more(&user)
        )
    } else {
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
        format!(
            "<h1>Account</h1><form class=\"{SUB} flex flex-wrap items-center gap-x-3 gap-y-1\" method=post \
action=\"/auth/signout\"><span>Signed in as <strong class=\"text-ink\">{name}</strong></span>\
<button class=\"quiet px-0\">Sign out</button></form>\
<h2>API keys</h2><p class=\"{SUB}\">A Subsonic client that supports API keys can sign in with one instead of \
your password. It acts as you, with your permissions, until you revoke it.</p>{key_form}{keys}\
<h2>App passwords</h2><p class=\"{SUB}\">For a Subsonic app that only signs in with a username and password: \
give it your username and an app password in place of yours. Make one per app. kōan keeps it encrypted, \
never shows it again, and it stops working when you revoke it or change your password.</p>\
{password_form}{passwords}{more}",
            name = escape(&user.username),
            key_form = create_form(
                "/account/keys",
                "keyname",
                "Name, e.g. phone",
                "Key name",
                "Create key",
                "key-result"
            ),
            keys = key_list(&keys),
            password_form = create_form(
                "/account/app-passwords",
                "appname",
                "Name, e.g. Arpeggi",
                "App password name",
                "Create app password",
                "app-password-result"
            ),
            passwords = app_password_list(&passwords),
            more = more(&user),
        )
    };
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

/// A credential made: shown once with a copy button, the list redrawn, the
/// name field cleared.
fn made(result: &str, what: &str, secret: &str, list: String, signal: &str) -> Response {
    events(vec![
        patch(
            &format!(
                "<div id={result}><p class=\"mt-3 mb-0\">Copy the {what} now: it is not shown again.</p>\
<div class=\"{COPY_ROW}\"><input id={result}-value readonly value=\"{secret}\" \
aria-label=\"New {what}\" class=\"{COPY_INPUT}\">\
<button data-on:click=\"navigator.clipboard.writeText(document.getElementById('{result}-value').value)\">Copy</button>\
</div></div>",
                secret = escape(secret)
            ),
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

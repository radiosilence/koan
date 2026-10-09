//! Accounts, for admins: create one, change its role or password, delete it,
//! and produce an invite for it — the link and the email to send it in. The
//! server sends nothing itself; the admin sends the email from their own
//! client.

use std::fmt::Write as _;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;
use koan_core::db::queries::auth::{self as users, UserRow};
use koan_core::invite::{self, AccountError, Invite};

use super::pages::{COPY_ERROR, COPY_INPUT, COPY_ROW, EMPTY, SUB, respond};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

/// A one-line form: the field, an option or two, the button.
const FORM: &str = "mb-2 flex max-w-form gap-2";

const ROLES: [(Role, &str); 3] = [
    (Role::Readonly, "Listen only"),
    (Role::User, "Listen and edit"),
    (Role::Admin, "Admin"),
];

fn role_select(id: i64, current: Role) -> String {
    let mut out = format!(
        "<select class=\"px-2 py-1.5\" aria-label=Access data-on:change=\"@post('/users/{id}/role?role=' + el.value)\">"
    );
    for (role, label) in ROLES {
        let sel = if role == current { " selected" } else { "" };
        let _ = write!(out, "<option value={}{sel}>{label}</option>", role.as_str());
    }
    out.push_str("</select>");
    out
}

fn user_list(rows: &[UserRow], me: i64) -> String {
    let mut out = String::from("<ul id=users>");
    for u in rows {
        // Not on your own row: a new password signs out every session, this
        // one included. The name reaches the confirmation as data: escaped
        // into the expression itself, it would be decoded back and run.
        let others = if u.id == me {
            String::new()
        } else {
            format!(
                "<button class=\"quiet\" data-on:click=\"@post('/users/{id}/password/form')\">Password</button>\
<button class=\"quiet bad\" data-username=\"{name}\" data-on:click=\"confirm('Delete ' + el.dataset.username + \
'? Their devices stop working and their playlists and favourites go.') && @post('/users/{id}/delete')\">Delete</button>",
                name = escape(&u.username),
                id = u.id,
            )
        };
        let _ = write!(
            out,
            "<li class=\"flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-rule px-1 py-2.5\">\
<span class=\"min-w-[8em] flex-1 truncate wrap-anywhere\">{name}{you}</span>{select}\
<button data-on:click=\"@post('/users/{id}/invite')\">Invite</button>{others}</li>",
            name = escape(&u.username),
            you = if u.id == me {
                "<small class=\"ml-2 text-meta text-muted\">you</small>"
            } else {
                ""
            },
            select = role_select(u.id, u.role),
            id = u.id,
        );
    }
    out.push_str("</ul>");
    out
}

fn result(html: &str) -> Event {
    patch(&format!("<div id=user-result>{html}</div>"), None)
}

fn failure(message: &str) -> Event {
    result(&format!(
        "<p class=\"{COPY_ERROR}\" role=alert>{}</p>",
        escape(message)
    ))
}

fn invite_panel(i: &Invite) -> String {
    let e = escape;
    let details = match &i.password {
        Some(password) => format!(
            "<p class=\"{SUB}\">The password is shown this once: the server keeps only its hash.</p>\
<dl class=\"mt-3.5 grid grid-cols-[max-content_1fr] gap-x-3.5 gap-y-1 text-control [&_dd]:wrap-anywhere \
[&_dd]:select-all [&_dt]:text-muted\"><dt>Server URL</dt><dd>{server}</dd><dt>Username</dt><dd>{user}</dd>\
<dt>Password</dt><dd><code>{password}</code></dd></dl>",
            server = e(&i.server),
            user = e(&i.username),
            password = e(password),
        ),
        None => String::new(),
    };
    format!(
        "<div class=\"mt-2 mb-4 max-w-panel border border-rule px-4 pt-1 pb-4\">\
<h2>Invite for <span class=\"normal-case\">{user}</span></h2><p class=\"{SUB}\">Send this from your own mail. Opening the link on a phone, tablet or Mac with koan \
installed signs in and loads the library, on each device, for a week.</p>\
<div class=\"{COPY_ROW}\"><input id=invite-link readonly value=\"{link}\" \
aria-label=\"Invite link\" class=\"{COPY_INPUT}\">\
<button data-copy=invite-link>Copy link</button></div>\
<div class=\"mt-2.5 flex flex-wrap gap-2\"><a class=\"inline-block border border-muted px-3 py-1.5 \
text-meta text-ink lowercase hover:bg-hover/30 hover:no-underline\" href=\"{mailto}\">Open in Mail</a>\
<button data-copy-email>Copy email</button>\
<button data-share-email data-show=\"'share' in navigator\">Share…</button></div>{details}\
<textarea id=invite-text hidden readonly data-subject=\"{subject}\">{text}</textarea>\
<template id=invite-html>{html}</template></div>",
        user = e(&i.username),
        link = e(&i.link()),
        mailto = e(&i.mailto()),
        subject = e(&i.email_subject()),
        text = e(&i.email_text()),
        html = i.email_html(),
    )
}

/// An invite carrying a token signed with this server's key.
fn token_invite(
    s: &UiState,
    server: &str,
    id: i64,
    username: &str,
    password: Option<&str>,
) -> Result<Invite, AccountError> {
    let keys = crate::auth::signing_keys().map_err(|e| AccountError::Other(Box::new(e)))?;
    let db = open(&s.pool).ok_or_else(|| AccountError::Other("no database".into()))?;
    let token = invite::mint_token(&db.conn, &keys.0, id)?;
    Ok(Invite::with_token(server, username, &token, password))
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "admins only").into_response()
}

fn usable(s: &UiState, user: &AuthUser) -> bool {
    s.auth_enabled && user.role == Role::Admin
}

fn list(s: &UiState) -> Vec<UserRow> {
    open(&s.pool)
        .and_then(|db| users::list_users(&db.conn).ok())
        .unwrap_or_default()
}

fn origin_of(s: &UiState, headers: &HeaderMap) -> Option<String> {
    crate::origin::origin(headers, s.public_url.as_deref())
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let intro = format!(
        "<h1>Users</h1><p class=\"{SUB}\">Everyone who can sign in to this server. \
An invite is a link that sets koan up with the account in one tap.</p>"
    );
    if !s.auth_enabled {
        let inner = format!(
            "{intro}<p class=\"{EMPTY}\">This server runs without sign-in, so it has no accounts.</p>"
        );
        return respond(&s, &headers, &user, "Users", &inner);
    }
    if user.role != Role::Admin {
        return forbidden();
    }
    let st = s.clone();
    let rows = blocking(move || Some(list(&st))).await.unwrap_or_default();
    let mut options = String::new();
    for (role, label) in ROLES {
        let sel = if role == Role::Readonly {
            " selected"
        } else {
            ""
        };
        let _ = write!(
            options,
            "<option value={}{sel}>{label}</option>",
            role.as_str()
        );
    }
    let inner = format!(
        "{intro}<form class=\"{FORM}\" data-on:submit__prevent=\"@post('/users')\">\
<input class=\"flex-1\" name=username data-bind:newuser placeholder=Username maxlength=64 required \
autocomplete=off autocapitalize=none spellcheck=false aria-label=Username>\
<select class=\"px-2 py-1.5\" data-bind:newrole aria-label=Access>{options}</select>\
<button class=\"primary\">Create and invite</button></form>\
<div id=user-result></div>{}",
        user_list(&rows, user.user_id)
    );
    respond(&s, &headers, &user, "Users", &inner)
}

/// Datastar posts its signals as JSON: `newuser` and `newrole`.
pub(super) async fn create(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let signals = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
    let field = |k: &str| {
        signals
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let username = field("newuser");
    let role: Role = field("newrole").parse().unwrap_or(Role::Readonly);
    let Some(server) = origin_of(&s, &headers) else {
        return events(vec![failure(
            "The server's address is unknown: set sharing.public_url.",
        )]);
    };
    let me = user.user_id;
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let made = invite::create_account(&db.conn, &username, role).and_then(|made| {
            token_invite(&s, &server, made.id, username.trim(), Some(&made.password))
        });
        Some((made, list(&s)))
    })
    .await;
    let Some((made, rows)) = made else {
        return events(vec![failure("The account could not be made.")]);
    };
    match made {
        Ok(i) => events(vec![
            result(&invite_panel(&i)),
            patch(&user_list(&rows, me), None),
            Event::default()
                .event("datastar-patch-signals")
                .data("signals {\"newuser\":\"\"}"),
        ]),
        Err(e) => events(vec![failure(&e.to_string())]),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct InviteQuery {
    #[serde(default)]
    reset: bool,
}

pub(super) async fn invite(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<InviteQuery>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let Some(server) = origin_of(&s, &headers) else {
        return events(vec![failure(
            "The server's address is unknown: set sharing.public_url.",
        )]);
    };
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let password = if q.reset {
            match invite::set_password(&db.conn, &row.username, None) {
                Ok(p) => {
                    crate::clients::registry().disconnect(&row.username);
                    Some(p)
                }
                Err(e) => return Some(Err(e)),
            }
        } else {
            None
        };
        Some(token_invite(
            &s,
            &server,
            row.id,
            &row.username,
            password.as_deref(),
        ))
    })
    .await;
    match made {
        Some(Ok(i)) => events(vec![result(&invite_panel(&i))]),
        Some(Err(e)) => events(vec![failure(&e.to_string())]),
        None => events(vec![failure("No such account.")]),
    }
}

fn clear_password_signal() -> Event {
    Event::default()
        .event("datastar-patch-signals")
        .data("signals {\"setpassword\":\"\"}")
}

/// The form asking for an account's new password. Its own route, so a value
/// typed into one account's form and never submitted cannot reach another's:
/// Datastar posts every signal with every request.
pub(super) async fn password_form(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let row = blocking(move || users::get_user_by_id(&open(&s.pool)?.conn, id).ok()?).await;
    let Some(row) = row else {
        return events(vec![failure("No such account.")]);
    };
    let name = escape(&row.username);
    events(vec![
        clear_password_signal(),
        result(&format!(
            "<form class=\"{FORM}\" data-on:submit__prevent=\"@post('/users/{id}/password')\">\
<input class=\"flex-1\" type=password data-bind:setpassword placeholder=\"New password for {name}\" \
minlength=8 required autocomplete=new-password aria-label=\"New password for {name}\">\
<button class=\"primary\">Set password</button></form>\
<p class=\"{SUB}\">Signs {name} out of every device. \
<button class=\"quiet\" data-on:click=\"@post('/users/{id}/invite?reset=true')\">Generate one and \
invite</button></p>",
        )),
    ])
}

/// Set the password the form posts as its `setpassword` signal.
pub(super) async fn set_password(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let password = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("setpassword")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let done = invite::set_password(&db.conn, &row.username, Some(&password));
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((row, done))
    })
    .await;
    match done {
        Some((row, Ok(_))) => events(vec![
            result(&format!(
                "<p class=\"{COPY_ROW}\">{}'s password is changed, and their devices are signed out.</p>",
                escape(&row.username)
            )),
            clear_password_signal(),
        ]),
        Some((_, Err(e))) => events(vec![failure(&e.to_string())]),
        None => events(vec![failure("No such account.")]),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct RoleQuery {
    role: String,
}

pub(super) async fn set_role(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Query(q): Query<RoleQuery>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let Ok(role) = q.role.parse::<Role>() else {
        return events(vec![failure("Unknown access level.")]);
    };
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let done = invite::set_role(&db.conn, &row.username, role);
        Some((done, list(&s)))
    })
    .await;
    match done {
        Some((Ok(()), rows)) => events(vec![patch(&user_list(&rows, me), None), result("")]),
        // Redraw the list too, so the select goes back to what is stored.
        Some((Err(e), rows)) => events(vec![
            patch(&user_list(&rows, me), None),
            failure(&e.to_string()),
        ]),
        None => events(vec![failure("No such account.")]),
    }
}

pub(super) async fn delete(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    if id == user.user_id {
        return events(vec![failure("You cannot delete your own account here.")]);
    }
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let done = invite::delete_account(&db.conn, &row.username);
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((done, list(&s)))
    })
    .await;
    match done {
        Some((Ok(()), rows)) => events(vec![patch(&user_list(&rows, me), None), result("")]),
        Some((Err(e), _)) => events(vec![failure(&e.to_string())]),
        None => events(vec![failure("No such account.")]),
    }
}

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

use super::pages::respond;
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

const ROLES: [(Role, &str); 3] = [
    (Role::Readonly, "Listen only"),
    (Role::User, "Listen and edit"),
    (Role::Admin, "Admin"),
];

fn role_select(id: i64, current: Role) -> String {
    let mut out = format!(
        "<select aria-label=Access data-on:change=\"@post('/users/{id}/role?role=' + el.value)\">"
    );
    for (role, label) in ROLES {
        let sel = if role == current { " selected" } else { "" };
        let _ = write!(out, "<option value={}{sel}>{label}</option>", role.as_str());
    }
    out.push_str("</select>");
    out
}

fn user_list(rows: &[UserRow], me: i64) -> String {
    let mut out = String::from("<ul class=\"list users\" id=users>");
    for u in rows {
        let delete = if u.id == me {
            String::new()
        } else {
            format!(
                "<button class=quiet data-on:click=\"confirm('Delete {name}? Their devices stop \
working and their playlists and favourites go.') && @post('/users/{id}/delete')\">Delete</button>",
                name = escape(&u.username),
                id = u.id,
            )
        };
        let _ = write!(
            out,
            "<li><span class=t>{name}{you}</span>{select}\
<button data-on:click=\"@post('/users/{id}/invite')\">Invite</button>\
<button class=quiet data-on:click=\"@post('/users/{id}/password')\">Password</button>{delete}</li>",
            name = escape(&u.username),
            you = if u.id == me { "<small>you</small>" } else { "" },
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
        "<p class=\"share error\" role=alert>{}</p>",
        escape(message)
    ))
}

fn invite_panel(i: &Invite) -> String {
    let e = escape;
    let details = match &i.password {
        Some(password) => format!(
            "<p class=sub>The password is shown this once: the server keeps only its hash.</p>\
<dl class=details><dt>Server URL</dt><dd>{server}</dd><dt>Username</dt><dd>{user}</dd>\
<dt>Password</dt><dd><code>{password}</code></dd></dl>",
            server = e(&i.server),
            user = e(&i.username),
            password = e(password),
        ),
        None => String::new(),
    };
    format!(
        "<div class=invite><h2>Invite for {user}</h2>\
<p class=sub>Send this from your own mail. Opening the link on a phone, tablet or Mac with koan \
installed signs in and loads the library, on each device, for a week.</p>\
<div class=share><input id=invite-link readonly value=\"{link}\" aria-label=\"Invite link\">\
<button data-copy=invite-link>Copy link</button></div>\
<div class=invite-actions><a class=button href=\"{mailto}\">Open in Mail</a>\
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
    let token = invite::mint_token(&s.auth.private_pem, id, username)
        .map_err(|e| AccountError::Other(Box::new(e)))?;
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
    let intro = "<h1>Users</h1><p class=sub>Everyone who can sign in to this server. \
An invite is a link that sets koan up with the account in one tap.</p>";
    if !s.auth_enabled {
        let inner = format!(
            "{intro}<p class=empty>This server runs without sign-in, so it has no accounts.</p>"
        );
        return respond(&s, &headers, &user, "Users", "users", &inner);
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
        "{intro}<form class=keyform data-on:submit__prevent=\"@post('/users')\">\
<input name=username data-bind:newuser placeholder=Username maxlength=64 required \
autocomplete=off autocapitalize=none spellcheck=false aria-label=Username>\
<select data-bind:newrole aria-label=Access>{options}</select>\
<button class=primary>Create and invite</button></form>\
<div id=user-result></div>{}",
        user_list(&rows, user.user_id)
    );
    respond(&s, &headers, &user, "Users", "users", &inner)
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

/// Without a `setpassword` signal, the form asking for one; with it, the
/// account's new password. Datastar posts its signals as JSON.
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
        if password.is_empty() {
            return Some((row, None));
        }
        let done = invite::set_password(&db.conn, &row.username, Some(&password));
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((row, Some(done.map(drop))))
    })
    .await;
    let Some((row, done)) = done else {
        return events(vec![failure("No such account.")]);
    };
    let name = escape(&row.username);
    match done {
        None => events(vec![result(&format!(
            "<form class=keyform data-on:submit__prevent=\"@post('/users/{id}/password')\">\
<input type=password data-bind:setpassword placeholder=\"New password for {name}\" \
minlength=8 required autocomplete=new-password aria-label=\"New password for {name}\">\
<button class=primary>Set password</button></form>\
<p class=sub>Signs {name} out of every device. \
<button class=quiet data-on:click=\"@post('/users/{id}/invite?reset=true')\">Generate one and \
invite</button></p>",
        ))]),
        Some(Ok(())) => events(vec![
            result(&format!(
                "<p class=share>{name}'s password is changed, and their devices are signed out.</p>"
            )),
            Event::default()
                .event("datastar-patch-signals")
                .data("signals {\"setpassword\":\"\"}"),
        ]),
        Some(Err(e)) => events(vec![failure(&e.to_string())]),
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

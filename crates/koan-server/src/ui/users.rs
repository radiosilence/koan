//! Accounts, for admins: create one, change its role, delete it, and produce
//! an invite for it — the link and the email to send it in. The server sends
//! nothing itself; the admin sends the email from their own client.

use std::fmt::Write as _;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use koan_core::auth::{self, Role};
use koan_core::db::queries::auth::{self as users, UserRow};
use koan_core::invite::{self, AccountError, Invite};

use super::pages::{COPY_ERROR, COPY_INPUT, COPY_ROW, EMPTY, SUB, respond};
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
        let delete = if u.id == me {
            String::new()
        } else {
            format!(
                "<button class=\"quiet\" data-on:click=\"confirm('Delete {name}? Their devices stop \
working and their playlists and favourites go.') && @post('/users/{id}/delete')\">Delete</button>",
                name = escape(&u.username),
                id = u.id,
            )
        };
        let _ = write!(
            out,
            "<li class=\"flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-rule px-1 py-2.5\">\
<span class=\"min-w-[8em] flex-1 truncate wrap-anywhere\">{name}{you}</span>{select}\
<button data-on:click=\"@post('/users/{id}/invite')\">Invite</button>{delete}</li>",
            name = escape(&u.username),
            you = if u.id == me {
                "<small class=\"ml-2 text-[13px] text-muted\">you</small>"
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
    format!(
        "<div class=\"mt-2 mb-4 max-w-[560px] rounded-lg border border-rule bg-surface px-4 pt-1 pb-4\">\
<h2>Invite for {user}</h2><p class=\"{SUB}\">Send this from your own mail. Opening the link on a phone, tablet or Mac with koan \
installed signs in and loads the library; the details work in any Subsonic app.</p>\
<div class=\"{COPY_ROW}\"><input id=invite-link readonly value=\"{link}\" \
aria-label=\"Invite link\" class=\"{COPY_INPUT}\">\
<button data-copy=invite-link>Copy link</button></div>\
<div class=\"mt-2.5 flex flex-wrap gap-2\"><a class=\"inline-block rounded-md border border-rule bg-rule px-3.5 py-2 \
text-ink hover:border-hover hover:no-underline\" href=\"{mailto}\">Open in Mail</a>\
<button data-copy-email>Copy email</button>\
<button data-share-email data-show=\"'share' in navigator\">Share…</button></div>\
<dl class=\"mt-3.5 grid grid-cols-[max-content_1fr] gap-x-3.5 gap-y-1 text-[14px] [&_dd]:wrap-anywhere \
[&_dd]:select-all [&_dt]:text-muted\"><dt>Server URL</dt><dd>{server}</dd><dt>Username</dt><dd>{user}</dd>\
<dt>Password</dt><dd><code>{password}</code></dd></dl>\
<textarea id=invite-text hidden readonly data-subject=\"{subject}\">{text}</textarea>\
<template id=invite-html>{html}</template></div>",
        user = e(&i.username),
        link = e(&i.link()),
        mailto = e(&i.mailto()),
        server = e(&i.server),
        password = e(&i.password),
        subject = e(&i.email_subject()),
        text = e(&i.email_text()),
        html = i.email_html(),
    )
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
        "{intro}<form class=\"mb-2 flex max-w-[520px] gap-2\" data-on:submit__prevent=\"@post('/users')\">\
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
        let made = auth::subsonic_key()
            .map_err(|e| AccountError::Other(Box::new(e)))
            .and_then(|key| invite::create_account(&db.conn, &key, &username, role))
            .map(|password| Invite::new(&server, &username, &password));
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
        let made = auth::subsonic_key()
            .map_err(|e| AccountError::Other(Box::new(e)))
            .and_then(|key| invite::account_password(&db.conn, &key, &row.username, q.reset))
            .map(|password| Invite::new(&server, &row.username, &password));
        if q.reset && made.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((row, made))
    })
    .await;
    match made {
        Some((_, Ok(i))) => events(vec![result(&invite_panel(&i))]),
        Some((row, Err(AccountError::NotRecoverable(_)))) => events(vec![result(&format!(
            "<p class=\"{COPY_ROW}\" role=alert>{name}'s password is not recoverable: the account \
predates koan keeping it. An invite needs a new password, which signs {name}'s existing \
devices out.</p><button data-on:click=\"@post('/users/{id}/invite?reset=true')\">\
New password and invite</button>",
            name = escape(&row.username),
        ))]),
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

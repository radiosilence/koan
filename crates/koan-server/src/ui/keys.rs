//! The signed-in user's Subsonic API keys: make one (shown once), see when each
//! was last used, revoke.

use std::fmt::Write as _;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::Event;
use koan_core::db::queries::api_keys::{self, ApiKeyRow};

use super::pages::{COPY_ERROR, COPY_INPUT, COPY_ROW, EMPTY, SUB, respond};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

const MAX_NAME: usize = 100;

const REVOKE_CONFIRM: &str = "Revoke this key? Clients using it will stop working.";

fn day(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

const ROW: &str = "flex min-w-0 items-center gap-3 border-b border-rule px-1 py-2.5";

fn key_list(keys: &[ApiKeyRow]) -> String {
    let mut out = String::from("<ul class=\"mt-4\" id=keys>");
    if keys.is_empty() {
        let _ = write!(out, "<li class=\"{ROW} {EMPTY}\">No keys yet.</li>");
    }
    for k in keys {
        let used = k.last_used_at.map_or_else(
            || "never used".to_owned(),
            |t| format!("last used {}", day(t)),
        );
        let _ = write!(
            out,
            "<li class=\"{ROW}\"><span class=\"min-w-0 flex-1 overflow-hidden text-ellipsis wrap-anywhere\">{name}\
<small class=\"block text-meta text-muted\">Created {created} · {used}</small></span>\
<button class=\"quiet\" data-on:click=\"confirm('{REVOKE_CONFIRM}') && @post('/keys/{id}/revoke')\">Revoke</button></li>",
            name = escape(&k.name),
            created = day(k.created_at),
            id = k.id,
        );
    }
    out.push_str("</ul>");
    out
}

fn user_keys(s: &UiState, user_id: i64) -> Option<Vec<ApiKeyRow>> {
    let db = open(&s.pool)?;
    api_keys::list_api_keys(&db.conn, Some(user_id)).ok()
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let intro = format!(
        "<h1>API keys</h1><p class=\"{SUB}\">A Subsonic client can sign in with a key instead of \
your password. It acts as you, with your permissions, until you revoke it.</p>"
    );
    let inner = if !s.auth_enabled {
        format!(
            "{intro}<p class=\"{EMPTY}\">Keys belong to accounts, and this server runs without sign-in.</p>"
        )
    } else {
        let st = s.clone();
        let keys = blocking(move || user_keys(&st, user.user_id))
            .await
            .unwrap_or_default();
        format!(
            "{intro}<form class=\"mb-2 flex max-w-form gap-2\" data-on:submit__prevent=\"@post('/keys')\">\
<input class=\"flex-1\" name=name data-bind:keyname placeholder=\"Name, e.g. phone\" maxlength={MAX_NAME} required \
autocomplete=off aria-label=\"Key name\"><button class=\"primary\">Create key</button></form>\
<div id=key-result></div>{}",
            key_list(&keys)
        )
    };
    respond(&s, &headers, &user, "API keys", &inner)
}

/// Datastar posts its signals as JSON; the name is `keyname`.
pub(super) async fn create(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    let name = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("keyname")?.as_str().map(|n| n.trim().to_owned()))
        .unwrap_or_default();
    if !s.auth_enabled || name.is_empty() || name.chars().count() > MAX_NAME {
        let message = if s.auth_enabled {
            "Give the key a name of up to 100 characters."
        } else {
            "Keys belong to accounts, and this server runs without sign-in."
        };
        return events(vec![patch(
            &format!("<div id=key-result class=\"{COPY_ERROR}\" role=alert>{message}</div>"),
            None,
        )]);
    }
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let (_, key) = api_keys::create_api_key(&db.conn, user.user_id, &name).ok()?;
        let keys = api_keys::list_api_keys(&db.conn, Some(user.user_id)).ok()?;
        Some((key, keys))
    })
    .await;
    let Some((key, keys)) = made else {
        return events(vec![patch(
            &format!(
                "<div id=key-result class=\"{COPY_ERROR}\" role=alert>The key could not be made.</div>"
            ),
            None,
        )]);
    };
    events(vec![
        patch(
            &format!(
                "<div id=key-result><p class=\"mt-3 mb-0\">Copy the key now: it is not shown again.</p>\
<div class=\"{COPY_ROW}\"><input id=new-key readonly value=\"{key}\" \
aria-label=\"New API key\" class=\"{COPY_INPUT}\">\
<button data-on:click=\"navigator.clipboard.writeText(document.getElementById('new-key').value)\">Copy</button>\
</div></div>",
                key = escape(&key)
            ),
            None,
        ),
        patch(&key_list(&keys), None),
        Event::default()
            .event("datastar-patch-signals")
            .data("signals {\"keyname\":\"\"}"),
    ])
}

pub(super) async fn revoke(
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

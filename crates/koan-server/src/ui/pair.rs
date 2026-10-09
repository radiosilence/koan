//! Approving a device that is waiting to be signed in, for someone without
//! the app: type the code the device shows, or follow its link here. See
//! `crate::pair`.
//!
//! Plain forms, laid out like the sign-in page, since the page is reached from
//! a phone that may never have opened the UI. They prove their origin the way
//! the OAuth consent form does.

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::oauth::page;
use super::pages::ERROR;
use super::{UiState, encode, html, open, see_other};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct CodeQuery {
    code: String,
}

/// The code form. Submitted, it goes to the pairing it names.
pub(super) async fn form(State(s): State<UiState>, Query(q): Query<CodeQuery>) -> Response {
    let code = q.code.trim();
    if !code.is_empty() {
        return see_other(&format!("/pair/{}", encode(code)));
    }
    code_page(&s, None)
}

fn code_page(s: &UiState, error: Option<&str>) -> Response {
    if !s.auth_enabled {
        return without_accounts();
    }
    let error = error
        .map(|e| format!("<p class=\"m-0 {ERROR}\" role=alert>{}</p>", escape(e)))
        .unwrap_or_default();
    let status = if error.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let body = format!(
        "<h2>Sign in a device</h2>\
<p>Type the code the device shows.</p>\
<form class=\"grid gap-3.5\" method=get action=\"/pair\">\
<label class=\"grid gap-1.5 text-meta text-muted\">Code<input class=\"text-input\" name=code \
autocomplete=off autocapitalize=characters spellcheck=false placeholder=\"XXXX-XXXX\" maxlength=20 \
required autofocus></label>{error}<button class=\"primary\">Continue</button></form>"
    );
    html(status, page("Sign in a device", &body))
}

fn without_accounts() -> Response {
    html(
        StatusCode::OK,
        page(
            "Sign in a device",
            "<p>Devices sign in to accounts, and this server runs without sign-in.</p>",
        ),
    )
}

/// "Sign in this device as you?", for the pairing an id or a code names.
pub(super) async fn confirm(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(pair): Path<String>,
) -> Response {
    if !s.auth_enabled {
        return without_accounts();
    }
    let Some(info) = crate::pair::pairings().info(&pair) else {
        return code_page(
            &s,
            Some(
                "No device is waiting with that code. Codes last ten minutes: check the device for a new one.",
            ),
        );
    };
    let action = format!("/pair/{}", encode(&pair));
    let body = format!(
        "<h2>Sign in <span class=\"normal-case\">{device}</span>?</h2>{origin}\
<p>It will be signed in as <strong>{user}</strong>, and can do anything your account can until you \
revoke its key.</p>\
<p><small>Approve only a device you are setting up yourself, just now. Anyone can give a device \
any name.</small></p>\
<form class=\"grid gap-3.5\" method=post action=\"{action}/approve\">\
<button class=\"primary\">Approve</button></form>\
<form class=\"grid gap-3.5\" method=post action=\"{action}/decline\">\
<button class=\"quiet\">Decline</button></form>\
<form class=\"grid gap-3.5\" method=post action=\"/auth/signout\">\
<input type=hidden name=next value=\"{action}\"><button class=\"quiet\">Not <span class=\"normal-case\">{user}</span>? Sign out</button></form>",
        device = escape(&info.device),
        origin = origin(&info),
        user = escape(&user.username),
        action = escape(&action),
    );
    html(StatusCode::OK, page("Sign in a device", &body))
}

/// Where the request came from, in plain words. One from outside a private
/// network is marked: a device in the room is on the approver's network.
fn origin(info: &crate::pair::PairInfo) -> String {
    let from = escape(&info.from.to_string());
    if info.local() {
        format!("<p>Requested from {from}, on your network.</p>")
    } else {
        format!(
            "<p class=\"{ERROR}\"><strong>Requested from {from}, from the internet.</strong> \
A device in the room with you is usually on your network.</p>"
        )
    }
}

pub(super) async fn approve(
    state: State<UiState>,
    user: Extension<AuthUser>,
    headers: HeaderMap,
    pair: Path<String>,
) -> Response {
    settle(state, user, headers, pair, false).await
}

pub(super) async fn decline(
    state: State<UiState>,
    user: Extension<AuthUser>,
    headers: HeaderMap,
    pair: Path<String>,
) -> Response {
    settle(state, user, headers, pair, true).await
}

async fn settle(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(pair): Path<String>,
    decline: bool,
) -> Response {
    if !super::session::same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
    }
    if !s.auth_enabled {
        return without_accounts();
    }
    let username = user.username.clone();
    let settled = blocking(move || {
        let db = open(&s.pool)?;
        Some(crate::pair::pairings().settle(&db.conn, &pair, user.user_id, &user.username, decline))
    })
    .await;
    let body = match settled {
        Some(Ok(info)) if decline => format!(
            "<h2>Declined</h2><p>{} was not signed in.</p>",
            escape(&info.device)
        ),
        Some(Ok(info)) => format!(
            "<h2>Signed in</h2><p>{} is signed in as <strong>{}</strong>. Its key is listed under \
API keys, where it can be revoked.</p>",
            escape(&info.device),
            escape(&username)
        ),
        Some(Err(crate::pair::SettleError::NotFound)) => {
            "<h2>Nothing to sign in</h2><p>The device stopped waiting, or its code lapsed. \
Check the device for a new code.</p>"
                .to_owned()
        }
        Some(Err(crate::pair::SettleError::Internal(_))) | None => {
            "<h2>Something went wrong</h2><p>The device could not be signed in. Try again.</p>"
                .to_owned()
        }
    };
    html(StatusCode::OK, page("Sign in a device", &body))
}

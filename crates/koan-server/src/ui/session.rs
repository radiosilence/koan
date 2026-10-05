//! Signing in and out of the web UI, on koan's own login and refresh tokens.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use koan_core::db::queries::auth as auth_queries;

use super::{UiState, encode, html, pages, see_other};
use crate::auth::routes::{authenticate, refresh_token_from, rotate};

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct NextParam {
    next: String,
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    next: String,
}

/// Where to go after signing in: a path on this server, or the albums. Anything
/// else (`//evil.example`, `https://…`) would make the form an open redirect,
/// and the auth routes themselves would loop.
fn local_path(next: &str) -> &str {
    let local = next.starts_with('/')
        && !next.starts_with("//")
        && !next.starts_with("/auth/")
        && !next.starts_with("/login")
        && next.bytes().all(|b| b.is_ascii_graphic() && b != b'\\');
    if local { next } else { "/" }
}

/// A form cannot carry Datastar's header, so these POSTs prove they came from
/// this origin the way a browser does: `Sec-Fetch-Site`, or an `Origin` naming
/// the host the request was sent to.
pub(super) fn same_origin(headers: &HeaderMap) -> bool {
    let get = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if get(header::HeaderName::from_static("sec-fetch-site")) == Some("same-origin") {
        return true;
    }
    match (get(header::ORIGIN), get(header::HOST)) {
        (Some(origin), Some(host)) => origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host)),
        _ => false,
    }
}

fn cross_site() -> Response {
    (StatusCode::FORBIDDEN, "cross-site request refused").into_response()
}

pub(super) async fn login_form(State(s): State<UiState>, Query(q): Query<NextParam>) -> Response {
    let next = local_path(&q.next);
    if !s.auth_enabled {
        return see_other(next);
    }
    html(StatusCode::OK, pages::login(next, None))
}

pub(super) async fn login(
    State(s): State<UiState>,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    match authenticate(&s.auth, &f.username, &f.password).await {
        Ok((_, access, refresh)) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, next.to_owned())],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        Err(resp) => {
            let status = resp.status();
            let message = match status {
                StatusCode::UNAUTHORIZED => "Wrong username or password.",
                StatusCode::TOO_MANY_REQUESTS => {
                    "Too many failed sign-ins for this account. Try again in a minute."
                }
                _ => "Signing in failed. Try again.",
            };
            html(status, pages::login(next, Some(message)))
        }
    }
}

/// Spend the refresh cookie for a new session. A page load lands here when its
/// access cookie has lapsed.
pub(super) async fn resume(
    State(s): State<UiState>,
    Query(q): Query<NextParam>,
    headers: HeaderMap,
) -> Response {
    let next = local_path(&q.next).to_owned();
    if !s.auth_enabled {
        return see_other(&next);
    }
    match rotate_from(&s, &headers).await {
        Some((access, refresh)) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, next),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        None => see_other(&format!("/login?next={}", encode(&next))),
    }
}

/// Keep an open page's session alive past the access token's lifetime. Unlike
/// `/auth/refresh` it answers with cookies only, so the tokens stay out of
/// page script.
pub(super) async fn renew(State(s): State<UiState>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    if !s.auth_enabled {
        return StatusCode::NO_CONTENT.into_response();
    }
    match rotate_from(&s, &headers).await {
        Some((access, refresh)) => (
            StatusCode::NO_CONTENT,
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn rotate_from(s: &UiState, headers: &HeaderMap) -> Option<(String, String)> {
    let supplied = refresh_token_from(None, headers)?;
    let auth = s.auth.clone();
    tokio::task::spawn_blocking(move || rotate(&auth, &supplied).ok())
        .await
        .ok()
        .flatten()
}

pub(super) async fn signout(
    State(s): State<UiState>,
    headers: HeaderMap,
    Form(q): Form<NextParam>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    if let Some(token) = refresh_token_from(None, &headers) {
        let pool = s.pool.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let db = super::open(&pool)?;
            auth_queries::revoke_refresh_token(&db.conn, &token).ok()
        })
        .await;
    }
    // Back to sign in, and then to where the user was: the consent page signs
    // out to let another account approve.
    let to = match local_path(&q.next) {
        "/" => "/login".to_owned(),
        next => format!("/login?next={}", encode(next)),
    };
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, to)],
        s.auth.cleared_cookies(),
    )
        .into_response()
}

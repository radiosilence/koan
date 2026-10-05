//! Signing in and out of the web UI, on koan's own login and refresh tokens.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Form;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{Extensions, HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ipnet::IpNet;
use serde::Deserialize;

use koan_core::db::queries::auth as auth_queries;

use super::{UiState, encode, html, pages, see_other};
use crate::auth::routes::{authenticate, refresh_token_from, rotate, session_for};

/// An authenticating reverse proxy in front of the web UI, whose header names
/// the account signed in to it.
#[derive(Clone)]
pub struct ProxyAuth {
    header: HeaderName,
    from: Arc<Vec<IpNet>>,
}

impl ProxyAuth {
    /// From `graphql.proxy_auth_header` and `graphql.proxy_auth_from`: none
    /// unless both name something. An entry that is neither an address nor a
    /// range is left out, which only narrows who is believed.
    pub fn from_config(header: &str, from: &[String]) -> Option<Self> {
        let header = header.trim();
        if header.is_empty() {
            return None;
        }
        let Ok(header) = HeaderName::from_bytes(header.as_bytes()) else {
            log::error!("graphql.proxy_auth_header {header:?} is not a header name");
            return None;
        };
        let from: Vec<IpNet> = from
            .iter()
            .map(|entry| entry.trim())
            .filter_map(|entry| {
                let net = entry
                    .parse::<IpNet>()
                    .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from));
                if net.is_err() {
                    log::error!("graphql.proxy_auth_from: {entry:?} is not an address or range");
                }
                net.ok()
            })
            .collect();
        if from.is_empty() {
            log::error!(
                "graphql.proxy_auth_header is set but proxy_auth_from names no proxy: proxy sign-in is off"
            );
            return None;
        }
        log::info!("web UI: signing in the account {header} names, from {from:?}");
        Some(Self {
            header,
            from: Arc::new(from),
        })
    }

    /// The username the proxy vouches for: only on a connection from the
    /// proxy itself, and only when it sent the header once with one value. A
    /// proxy that appended to or merged with a client's own header would
    /// otherwise let the client name the account.
    pub(super) fn user<'a>(&self, headers: &'a HeaderMap, ext: &Extensions) -> Option<&'a str> {
        let ConnectInfo(peer) = ext.get::<ConnectInfo<SocketAddr>>()?;
        let peer = peer.ip().to_canonical();
        if !self.from.iter().any(|net| net.contains(&peer)) {
            return None;
        }
        let mut values = headers.get_all(&self.header).iter();
        let name = values.next()?.to_str().ok()?.trim();
        (values.next().is_none() && !name.is_empty() && !name.contains(',')).then_some(name)
    }
}

/// The username a request's proxy vouches for, when proxy sign-in is on.
pub(super) fn vouched<'a>(
    s: &UiState,
    headers: &'a HeaderMap,
    ext: &Extensions,
) -> Option<&'a str> {
    s.proxy_auth.as_ref()?.user(headers, ext)
}

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

pub(super) async fn login_form(
    State(s): State<UiState>,
    Query(q): Query<NextParam>,
    headers: HeaderMap,
    ext: Extensions,
) -> Response {
    let next = local_path(&q.next);
    if !s.auth_enabled {
        return see_other(next);
    }
    if vouched(&s, &headers, &ext).is_some() {
        return see_other(&format!("{PROXY_RESUME}?next={}", encode(next)));
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
            let message = if status == StatusCode::UNAUTHORIZED {
                "Wrong username or password."
            } else {
                "Signing in failed. Try again."
            };
            html(status, pages::login(next, Some(message)))
        }
    }
}

/// Where a page load goes for a session when an authenticating proxy is
/// trusted. A UI path, so the proxy covers it: the paths operators exempt
/// from the proxy (`/auth/login`, `/oauth/token`…) never read the header, and
/// a client's own header reaching them through an exemption signs in no one.
pub(super) const PROXY_RESUME: &str = "/ui/resume";

/// Sign in the account the proxy names, with a fresh session even over a
/// refresh cookie, which may be another account's. Without the header, on to
/// the refresh cookie as usual.
pub(super) async fn proxy_resume(
    State(s): State<UiState>,
    Query(q): Query<NextParam>,
    headers: HeaderMap,
    ext: Extensions,
) -> Response {
    let next = local_path(&q.next).to_owned();
    if !s.auth_enabled {
        return see_other(&next);
    }
    let Some(name) = vouched(&s, &headers, &ext) else {
        return see_other(&format!("/auth/resume?next={}", encode(&next)));
    };
    match session_for(&s.auth, name).await {
        Some((access, refresh)) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, next),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        None => unknown_account(name),
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

/// The proxy signed in someone this server has no account for. Accounts are
/// made by an admin; a proxy cannot create one.
fn unknown_account(name: &str) -> Response {
    log::info!("web UI: the sign-in proxy named {name:?}, who has no account");
    (
        StatusCode::FORBIDDEN,
        [(header::CACHE_CONTROL, "no-store")],
        "Your sign-in proxy names an account this server does not have. Ask an admin to create it.",
    )
        .into_response()
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

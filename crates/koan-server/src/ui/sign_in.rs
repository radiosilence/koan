//! Signing in and out of the web UI, on koan's own login and refresh tokens:
//! the sessions themselves here, and the pages that stand outside the shell
//! (sign-in, its second step, setup, pairing a device, and OAuth's consent)
//! in the modules below, each drawn in the sign-in frame.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use askama::Template;
use axum::Form;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{Extensions, HeaderMap, StatusCode, header};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_kit_web as kit;
use serde::Deserialize;

use koan_core::db::queries::auth as auth_queries;

use super::{Logged, UiState, document, encode, paths, see_other};
use crate::auth::routes::{
    ClientIp, RateLimiter, login_rate_limit, proxied_access, rate_limit, refresh_token_from, rotate,
};
use crate::html::{Html, Render};

pub(super) mod consent;
pub(super) mod login;
pub(super) mod pair;
pub(super) mod second_step;
pub(super) mod setup;

/// The routes outside the gate: signing in, renewing and signing out. The
/// consent and pairing pages sit behind it, as plain forms that prove their
/// origin themselves.
pub(super) fn routes(state: &UiState) -> axum::Router<UiState> {
    let gate = || from_fn_with_state(state.clone(), super::gate);
    // Checking a password is deliberately expensive, so the form shares the
    // JSON login's per-IP window.
    let limited = || from_fn_with_state(state.auth.clone(), login_rate_limit);
    let consent = axum::Router::new()
        .route(
            paths::AUTHORIZE,
            get(consent::authorize).post(consent::approve),
        )
        .layer(gate());
    let pairing = axum::Router::new()
        .route(paths::PAIR, get(pair::form))
        .route(paths::PAIRING, get(pair::confirm))
        .route(paths::PAIR_APPROVE, post(pair::approve))
        .route(paths::PAIR_DECLINE, post(pair::decline))
        .layer(gate());
    axum::Router::new()
        .merge(consent)
        .merge(pairing)
        .route(
            paths::LOGIN,
            get(login::form).merge(post(login::login).layer(limited())),
        )
        .route(
            paths::LOGIN_SECOND,
            post(second_step::second).layer(limited()),
        )
        .route(paths::LOGIN_PASSKEY, post(login::passkey).layer(limited()))
        .route(
            paths::LOGIN_PASSKEY_CHALLENGE,
            post(login::passkey_challenge).layer(from_fn_with_state(
                Arc::new(RateLimiter::new(60, 30)),
                rate_limit,
            )),
        )
        // Making the first admin hashes a password, so it shares sign-in's window.
        .route(
            paths::SETUP,
            get(setup::form).merge(post(setup::create).layer(limited())),
        )
        .route(paths::RESUME, get(resume))
        .route(paths::PROXY_RESUME, get(proxy_resume))
        .route(paths::PROXY_RENEW, post(proxy_renew))
        .route(paths::RENEW, post(renew))
        .route(paths::SIGN_OUT, post(signout))
}

/// The frame every page outside the shell is drawn in: the head, kōan's name
/// and the page's own body.
#[derive(Template)]
#[template(path = "sign_in/frame.html")]
pub(super) struct Frame<'a> {
    base: String,
    title: &'a str,
    body: Html,
    /// The passkey script, for a page with a passkey form.
    passkey: bool,
}

impl Frame<'_> {
    fn head(&self) -> kit::Head<'static> {
        kit::Head::new(super::ASSETS.css.as_str())
            .datastar(&paths::asset("datastar.js"))
            .import_map(&super::ASSETS.imports)
            .noindex()
            .title(format!("{} · kōan", self.title))
    }

    fn icons(&self) -> Html {
        super::Icons {
            under: "./ui/assets",
        }
        .html()
    }

    fn passkey_js(&self) -> &'static str {
        &super::ASSETS.passkey_js
    }
}

/// A page outside the shell, whole, with `status`.
pub(super) fn framed(
    s: &UiState,
    status: StatusCode,
    title: &str,
    body: Html,
    passkey: bool,
) -> Response {
    let frame = Frame {
        base: s.base(),
        title,
        body,
        passkey,
    };
    document(status, &frame.html())
}

/// Where to go after signing in: a path on this server, or the albums. Not
/// the auth routes themselves, which would loop.
pub(super) fn local_path(next: &str) -> &str {
    koan_auth::web::local_path(next)
        .filter(|p| !p.starts_with("/auth/") && !p.starts_with("/login"))
        .unwrap_or("/")
}

/// A form cannot carry Datastar's header, so these POSTs prove they came from
/// this origin the way a browser does: see `koan_auth::web::same_origin`.
pub(super) use koan_auth::web::same_origin;

pub(super) fn cross_site() -> Response {
    (StatusCode::FORBIDDEN, "cross-site request refused").into_response()
}

/// Signed in: on to `next`, with the session's cookies.
pub(super) fn signed_in(s: &UiState, next: &str, access: &str, refresh: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, next.to_owned())],
        s.auth.session_cookies(access, refresh),
    )
        .into_response()
}

/// An authenticating reverse proxy in front of the web UI, whose header names
/// the account signed in to it: `koan_auth::net::ProxyAuth`, configured by
/// kōan's settings.
#[derive(Clone)]
pub struct ProxyAuth(koan_auth::net::ProxyAuth);

impl ProxyAuth {
    /// From `graphql.proxy_auth_header` and `graphql.proxy_auth_from`. Off
    /// when neither is set; on when both are; anything between is refused.
    pub fn from_config(header: &str, from: &[String]) -> Result<Option<Self>, String> {
        koan_auth::net::ProxyAuth::from_config(
            header,
            from,
            ("graphql.proxy_auth_header", "graphql.proxy_auth_from"),
        )
        .map(|proxy| proxy.map(Self))
    }

    /// What the proxy says about who is signed in: heard only from the
    /// proxy's own address.
    pub(super) fn user<'a>(&self, headers: &'a HeaderMap, ext: &Extensions) -> Vouch<'a> {
        let peer = ext
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| peer.ip());
        self.0.user(peer, headers)
    }
}

pub(super) use koan_auth::net::Vouch;

/// What a request's proxy says about who is signed in.
pub(super) fn vouched<'a>(s: &UiState, headers: &'a HeaderMap, ext: &Extensions) -> Vouch<'a> {
    s.proxy_auth
        .as_ref()
        .map_or(Vouch::Absent, |proxy| proxy.user(headers, ext))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct NextParam {
    pub(super) next: String,
}

/// Sign in the account the proxy names, over whatever session the browser
/// held. Without the header, on to the refresh cookie as usual.
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
    let vouch = vouched(&s, &headers, &ext);
    if vouch == Vouch::Absent {
        return see_other(&format!("{}?next={}", paths::RESUME, encode(&next)));
    }
    match proxied(&s, vouch).await {
        Ok(access) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, next),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            s.auth.proxied_cookies(&access),
        )
            .into_response(),
        Err(refused) => *refused,
    }
}

/// An access token for the account the proxy names, or a refusal that signs
/// the browser out. Never a refresh token: the session is derived again from
/// the header on the next page load, so it cannot outlast the proxy's say-so,
/// and a switch of account at the proxy needs nothing revoked.
async fn proxied(s: &UiState, vouch: Vouch<'_>) -> Result<String, Box<Response>> {
    let Vouch::Named(name) = vouch else {
        return Err(Box::new(unusable_header(s)));
    };
    proxied_access(&s.auth, name).await.ok_or_else(|| {
        log::info!("web UI: the sign-in proxy named {name:?}, who has no account");
        Box::new(refused_by_proxy(
            s,
            "Your sign-in proxy names an account this server does not have. Ask an admin to create it.",
        ))
    })
}

/// Spend the refresh cookie for a new session. A page load lands here when its
/// access cookie has lapsed.
pub(super) async fn resume(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    Query(q): Query<NextParam>,
    headers: HeaderMap,
) -> Response {
    let next = local_path(&q.next).to_owned();
    if !s.auth_enabled {
        return see_other(&next);
    }
    match rotate_from(&s, &headers, from).await {
        Some((access, refresh)) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, next),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        None => see_other(&format!("{}?next={}", paths::LOGIN, encode(&next))),
    }
}

/// Keep an open page's session alive past the access token's lifetime. Unlike
/// `/auth/refresh` it answers with cookies only, so the tokens stay out of
/// page script.
pub(super) async fn renew(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    if !s.auth_enabled {
        return StatusCode::NO_CONTENT.into_response();
    }
    match rotate_from(&s, &headers, from).await {
        Some((access, refresh)) => (
            StatusCode::NO_CONTENT,
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Keep an open page's session alive behind an authenticating proxy, which
/// issues no refresh cookie: a fresh access cookie for the account the header
/// names. The page asks here when `/auth/renew` cannot renew. A UI path, like
/// `PROXY_RESUME`, so nothing under `/auth` reads the header.
pub(super) async fn proxy_renew(
    State(s): State<UiState>,
    headers: HeaderMap,
    ext: Extensions,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    match vouched(&s, &headers, &ext) {
        Vouch::Absent => StatusCode::UNAUTHORIZED.into_response(),
        vouch => match proxied(&s, vouch).await {
            Ok(access) => (StatusCode::NO_CONTENT, s.auth.proxied_cookies(&access)).into_response(),
            Err(refused) => *refused,
        },
    }
}

/// The proxy sent its header but named no one account in it.
pub(super) fn unusable_header(s: &UiState) -> Response {
    log::warn!("web UI: the sign-in proxy sent a header that names no one account");
    refused_by_proxy(
        s,
        "Your sign-in proxy did not name one account. Ask an admin to check its configuration.",
    )
}

/// The proxy signed in no one this server has an account for: a name it does
/// not know (accounts are made by an admin; a proxy cannot create one), or a
/// header that names no one. The browser's own session goes too.
fn refused_by_proxy(s: &UiState, message: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CACHE_CONTROL, "no-store")],
        s.auth.cleared_cookies(),
        message,
    )
        .into_response()
}

async fn rotate_from(s: &UiState, headers: &HeaderMap, from: IpAddr) -> Option<(String, String)> {
    let supplied = refresh_token_from(None, headers)?;
    let auth = s.auth.clone();
    tokio::task::spawn_blocking(move || rotate(&auth, &supplied, Some(from)).ok())
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
            auth_queries::revoke_refresh_token(&db.conn, &token).logged()
        })
        .await;
    }
    // Back to sign in, and then to where the user was: the consent page signs
    // out to let another account approve.
    let to = match local_path(&q.next) {
        "/" => paths::LOGIN.to_owned(),
        next => format!("{}?next={}", paths::LOGIN, encode(next)),
    };
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, to)],
        s.auth.cleared_cookies(),
    )
        .into_response()
}

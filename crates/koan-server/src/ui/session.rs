//! Signing in and out of the web UI, on koan's own login and refresh tokens.

use std::net::{IpAddr, SocketAddr};

use axum::Form;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{Extensions, HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use koan_core::db::queries::auth as auth_queries;

use super::{Logged, UiState, encode, html, pages, see_other};
use crate::auth::routes::{
    Answer, ClientIp, SignedIn, authenticate, finish_second, passkey_sign_in, proxied_access,
    refresh_token_from, rotate,
};

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
    next: String,
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    next: String,
}

/// Where to go after signing in: a path on this server, or the albums. Not
/// the auth routes themselves, which would loop.
fn local_path(next: &str) -> &str {
    koan_auth::web::local_path(next)
        .filter(|p| !p.starts_with("/auth/") && !p.starts_with("/login"))
        .unwrap_or("/")
}

/// A form cannot carry Datastar's header, so these POSTs prove they came from
/// this origin the way a browser does: see `koan_auth::web::same_origin`.
pub(super) use koan_auth::web::same_origin;

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
    if vouched(&s, &headers, &ext) != Vouch::Absent {
        return see_other(&format!("{PROXY_RESUME}?next={}", encode(next)));
    }
    // Nobody to sign in as yet: make the admin first.
    if super::setup::open_for_setup(&s).await.is_some() {
        return see_other("/setup");
    }
    html(StatusCode::OK, pages::login(&s, next, None))
}

pub(super) async fn login(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    ext: Extensions,
    Form(f): Form<LoginForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    // Through the proxy, the proxy says who is signed in, not a password.
    if vouched(&s, &headers, &ext) != Vouch::Absent {
        return see_other(&format!("{PROXY_RESUME}?next={}", encode(next)));
    }
    match authenticate(&s.auth, &f.username, &f.password, None, from).await {
        Ok(SignedIn::Session(_, access, refresh)) => signed_in(&s, next, &access, &refresh),
        Ok(SignedIn::NeedsSecond(user)) => second_step(&s, next, &user, None).await,
        Err(resp) => {
            let status = resp.status();
            let message = match status {
                StatusCode::UNAUTHORIZED => "Wrong username or password.",
                StatusCode::TOO_MANY_REQUESTS => {
                    "Too many failed sign-ins for this account. Try again in a minute."
                }
                _ => "Signing in failed. Try again.",
            };
            html(status, pages::login(&s, next, Some(message)))
        }
    }
}

fn signed_in(s: &UiState, next: &str, access: &str, refresh: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, next.to_owned())],
        s.auth.session_cookies(access, refresh),
    )
        .into_response()
}

/// The page asking for the second factor, its password passed: a ticket
/// holds the sign-in, and with the account's security keys, the challenge
/// they answer.
async fn second_step(
    s: &UiState,
    next: &str,
    user: &auth_queries::UserRow,
    error: Option<&str>,
) -> Response {
    use koan_auth::second_factor::Purpose;
    let Ok((ticket, held)) = s
        .auth
        .users
        .tickets()
        .open(user.id, &user.username, Purpose::SignIn)
    else {
        return html(
            StatusCode::INTERNAL_SERVER_ERROR,
            pages::login(s, next, Some("Signing in failed. Try again.")),
        );
    };
    let (pool, user_id) = (s.pool.clone(), user.id);
    let (totp, ids) = tokio::task::spawn_blocking(move || {
        let db = super::open(&pool)?;
        let factors = koan_auth::second_factor::factors(&db.conn, user_id).logged()?;
        let ids = koan_auth::second_factor::key_ids(&db.conn, user_id).logged()?;
        Some((factors.totp, ids))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    let key_options = s
        .relying_party()
        .filter(|_| !ids.is_empty())
        .map(|rp| koan_auth::webauthn::request_options(&rp, &held.challenge, &ids).to_string());
    html(
        if error.is_some() {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::OK
        },
        pages::second_step(s, next, &ticket, totp, key_options.as_deref(), error),
    )
}

#[derive(Deserialize)]
pub(super) struct PasskeyForm {
    ticket: String,
    #[serde(default)]
    next: String,
    assertion: String,
}

/// A passkey signing in on its own, from the sign-in page.
pub(super) async fn passkey(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    Form(f): Form<PasskeyForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    let (Some(rp), Ok(assertion)) = (s.relying_party(), serde_json::from_str(&f.assertion)) else {
        return html(
            StatusCode::BAD_REQUEST,
            pages::login(&s, next, Some("Signing in failed. Try again.")),
        );
    };
    match passkey_sign_in(&s.auth, f.ticket, assertion, rp, from).await {
        Ok((_, access, refresh)) => signed_in(&s, next, &access, &refresh),
        Err(resp) => {
            let message = match resp.status() {
                StatusCode::UNAUTHORIZED => {
                    "That passkey does not sign in to this server, or did not check it was you."
                }
                StatusCode::GONE => "That took too long. Try again.",
                _ => "Signing in failed. Try again.",
            };
            html(resp.status(), pages::login(&s, next, Some(message)))
        }
    }
}

#[derive(Deserialize)]
pub(super) struct SecondForm {
    ticket: String,
    #[serde(default)]
    next: String,
    #[serde(default)]
    code: String,
    /// A security key's answer, as the sign-in page's script wrote it.
    #[serde(default)]
    assertion: String,
}

/// The second factor for a sign-in whose password passed.
pub(super) async fn second(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    Form(f): Form<SecondForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    let answer = if f.assertion.is_empty() {
        Answer::Code(f.code)
    } else {
        match serde_json::from_str(&f.assertion) {
            Ok(assertion) => Answer::Key(assertion),
            Err(_) => Answer::Code(String::new()),
        }
    };
    let held = s
        .auth
        .users
        .tickets()
        .get(&f.ticket, koan_auth::second_factor::Purpose::SignIn);
    match finish_second(&s.auth, f.ticket, answer, s.relying_party(), from).await {
        Ok((_, access, refresh)) => signed_in(&s, next, &access, &refresh),
        Err(resp) => {
            let status = resp.status();
            let message = match status {
                StatusCode::UNAUTHORIZED => "That was not right. Try again.",
                StatusCode::TOO_MANY_REQUESTS => {
                    "Too many wrong answers for this account. Try again in a minute."
                }
                StatusCode::GONE => {
                    return html(
                        StatusCode::UNAUTHORIZED,
                        pages::login(
                            &s,
                            next,
                            Some("That took too long or too many tries. Sign in again."),
                        ),
                    );
                }
                _ => "Signing in failed. Try again.",
            };
            // A fresh ticket and challenge, since a security key's answer is
            // for one challenge only; the password stays passed.
            let pool = s.pool.clone();
            let user = tokio::task::spawn_blocking(move || {
                let db = super::open(&pool)?;
                auth_queries::get_user_by_id(&db.conn, held?.user_id).logged()?
            })
            .await
            .ok()
            .flatten();
            match user {
                Some(user) if status != StatusCode::TOO_MANY_REQUESTS => {
                    second_step(&s, next, &user, Some(message)).await
                }
                _ => html(status, pages::login(&s, next, Some(message))),
            }
        }
    }
}

/// Where a page load goes for a session when an authenticating proxy is
/// trusted. A UI path, so the proxy covers it: the paths operators exempt
/// from the proxy (`/auth/login`, `/oauth/token`…) never read the header, and
/// a client's own header reaching them through an exemption signs in no one.
pub(super) const PROXY_RESUME: &str = "/ui/resume";

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
        return see_other(&format!("/auth/resume?next={}", encode(&next)));
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
        None => see_other(&format!("/login?next={}", encode(&next))),
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

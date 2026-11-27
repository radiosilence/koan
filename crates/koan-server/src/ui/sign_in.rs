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
        super::icons("./ui/assets")
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

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn page_loads_without_a_session_resume_it_and_script_requests_are_refused() {
        let f = setup(true);
        let r = send(&f.app, get("/album/1?x=y").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/auth/resume?next=%2Falbum%2F1%3Fx%3Dy");

        for req in [
            get("/albums").header("datastar-request", "true"),
            get("/ui/live").header("datastar-request", "true"),
            get(&format!("/ui/stream/{}", f.track_id)),
            get("/albums").header(header::COOKIE, "koan_access=forged"),
        ] {
            let r = send(&f.app, req.body(Body::empty()).unwrap()).await;
            assert!(
                matches!(r.status, StatusCode::UNAUTHORIZED | StatusCode::SEE_OTHER),
                "{}",
                r.status
            );
            assert_ne!(r.status, StatusCode::OK);
        }
        let r = send(
            &f.app,
            get("/albums")
                .header("datastar-request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn signing_in_sets_http_only_cookies_and_goes_only_to_local_paths() {
        let f = setup(true);
        let r = send(
            &f.app,
            form("/login", "username=alice&password=hunter2&next=%2Fartists"),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/artists");
        assert!(!r.body.contains(&r.cookie("koan_access")));
        let cookies = r.cookies();
        assert_eq!(cookies.len(), 3);
        for c in &cookies {
            assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("Secure"));
        }
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("koan_access=") && c.contains("Path=/;"))
        );
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("koan_refresh=") && c.contains("Path=/auth;"))
        );

        for next in [
            "%2F%2Fevil.example",
            "https%3A%2F%2Fevil.example",
            "%2F%5Cevil.example",
            "%2Fauth%2Fresume",
        ] {
            let r = send(
                &f.app,
                form(
                    "/login",
                    &format!("username=alice&password=hunter2&next={next}"),
                ),
            )
            .await;
            assert_eq!(
                (r.status, r.location()),
                (StatusCode::SEE_OTHER, "/"),
                "{next}"
            );
        }
    }

    #[tokio::test]
    async fn wrong_passwords_and_cross_site_forms_do_not_sign_in() {
        let f = setup(true);
        let r = send(
            &f.app,
            form("/login", "username=alice&password=nope&next=%2F"),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert!(r.body.contains("Wrong username or password."));
        assert!(r.cookies().is_empty());

        let req = Request::post("/login")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, "https://evil.example")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("username=alice&password=hunter2"))
            .unwrap();
        let r = send(&f.app, req).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert!(r.cookies().is_empty());
    }

    #[tokio::test]
    async fn resume_spends_the_refresh_cookie_once() {
        let f = setup(true);
        let signed_in = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
        let refresh = signed_in.cookie("koan_refresh");
        let resume = || {
            get("/auth/resume?next=%2Fqueue")
                .header(header::COOKIE, format!("koan_refresh={refresh}"))
                .body(Body::empty())
                .unwrap()
        };

        let r = send(&f.app, resume()).await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/queue"));
        let access = r.cookie("koan_access");
        assert_ne!(r.cookie("koan_refresh"), refresh, "rotated");
        let page = send(
            &f.app,
            get("/queue")
                .header(header::COOKIE, format!("koan_access={access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(page.status, StatusCode::OK);

        // Spent: the same cookie again goes to the sign-in form.
        let r = send(&f.app, resume()).await;
        assert_eq!(
            (r.status, r.location()),
            (StatusCode::SEE_OTHER, "/login?next=%2Fqueue")
        );
        let r = send(&f.app, get("/auth/resume").body(Body::empty()).unwrap()).await;
        assert_eq!(r.location(), "/login?next=%2F");
    }

    #[tokio::test]
    async fn renew_answers_with_cookies_and_no_tokens() {
        let f = setup(true);
        let signed_in = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
        let refresh = signed_in.cookie("koan_refresh");
        let renew = |origin: &str| {
            Request::post("/auth/renew")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, origin)
                .header(header::COOKIE, format!("koan_refresh={refresh}"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            send(&f.app, renew("https://evil.example")).await.status,
            StatusCode::FORBIDDEN
        );
        let r = send(&f.app, renew(ORIGIN)).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
        assert!(r.body.is_empty());
        assert!(!r.cookie("koan_access").is_empty());
    }

    #[tokio::test]
    async fn signing_out_revokes_the_refresh_token() {
        let f = setup(true);
        let signed_in = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
        let refresh = signed_in.cookie("koan_refresh");
        let mut req = form("/auth/signout", "");
        req.headers_mut().insert(
            header::COOKIE,
            format!("koan_refresh={refresh}").parse().unwrap(),
        );
        let r = send(&f.app, req).await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/login"));
        assert!(r.cookies().iter().all(|c| c.contains("Max-Age=0")));

        let r = send(
            &f.app,
            get("/auth/resume")
                .header(header::COOKIE, format!("koan_refresh={refresh}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert!(r.location().starts_with("/login"));
    }

    #[tokio::test]
    async fn an_app_password_does_not_sign_in_to_the_web_ui() {
        let f = setup(true);
        let password = {
            let db = Database::open(f.state.pool.path()).unwrap();
            let user = queries::auth::get_user_by_username(&db.conn, "alice")
                .unwrap()
                .unwrap();
            let key = koan_core::auth::app_password_key(&f.state.private_pem);
            queries::app_passwords::create_app_password(&db.conn, &key, user.id, "arpeggi")
                .unwrap()
                .1
        };
        // Apps only: the web sign-in, and with it GraphQL, MCP and OAuth, takes
        // the account's own password.
        let r = send(
            &f.app,
            form(
                "/login",
                &format!("username=alice&password={password}&next=%2F"),
            ),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert!(r.cookies().is_empty());
    }

    #[tokio::test]
    async fn the_proxy_signs_in_the_account_it_names() {
        let f = setup_behind_proxy();
        let r = send(
            &f.app,
            from_peer(get("/login?next=/artists"), "10.0.0.1")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/ui/resume?next=%2Fartists");

        let r = send(
            &f.app,
            from_peer(get("/ui/resume?next=/artists"), "10.0.0.1")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/artists");
        let access = r.cookie("koan_access");
        let claims = auth::validate_access_token(&f.state.public_pem, &access).unwrap();
        assert_eq!(claims.username, "alice");
    }

    #[tokio::test]
    async fn page_loads_behind_the_proxy_resume_through_it() {
        let f = setup_behind_proxy();
        let r = send(
            &f.app,
            get("/oauth/authorize?x=y").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/ui/resume?next=%2Foauth%2Fauthorize%3Fx%3Dy");

        // Without the header it falls back to the refresh cookie.
        let r = send(
            &f.app,
            get("/ui/resume?next=/queue").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/auth/resume?next=%2Fqueue");
        assert!(r.cookies().is_empty());
    }

    #[tokio::test]
    async fn the_header_counts_only_from_the_proxy_with_one_value() {
        let f = setup_behind_proxy();
        let stranger = from_peer(get("/ui/resume"), "203.0.113.9")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap();
        let unknown_peer = get("/ui/resume")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap();
        for req in [stranger, unknown_peer] {
            let r = send(&f.app, req).await;
            assert_eq!(r.status, StatusCode::SEE_OTHER);
            assert_eq!(r.location(), "/auth/resume?next=%2F");
            assert!(r.cookies().is_empty());
        }
        // From the proxy, a header naming no one account signs in no one, and
        // ends the browser's own session rather than falling back to it.
        for req in unusable_headers() {
            let r = send(&f.app, req.uri("/ui/resume").body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::FORBIDDEN);
            assert_signed_out(&r);
        }
        let r = send(
            &f.app,
            from_peer(get("/login"), "203.0.113.9")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
    }

    /// The paths the docs tell operators to exempt from the proxy, where a
    /// client's own header arrives from the proxy's address unchecked, sign in
    /// no one.
    ///
    /// Only the web UI and the auth routes are mounted here. `/rest`, `/graphql`,
    /// `/mcp`, `/share` and `/push` are other routers, none of which reads the
    /// header at all.
    #[tokio::test]
    async fn paths_exempt_from_the_proxy_ignore_the_header() {
        let f = setup_behind_proxy();
        let app = f
            .app
            .clone()
            .merge(crate::auth::routes::auth_router(f.state.clone()));
        let json = |uri: &str, body: &str| {
            from_peer(Request::post(uri), "10.0.0.1")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("remote-user", "alice")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let form = |uri: &str, body: &str| {
            from_peer(Request::post(uri), "10.0.0.1")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("remote-user", "alice")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let page = |uri: &str| {
            from_peer(get(uri), "10.0.0.1")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap()
        };
        for req in [
            json("/auth/login", r#"{"username":"alice","password":"wrong"}"#),
            json("/auth/refresh", r#"{"refresh_token":"nope"}"#),
            json("/auth/logout", r#"{"refresh_token":"nope"}"#),
            form(
                "/oauth/token",
                "grant_type=refresh_token&refresh_token=nope",
            ),
            json(
                "/oauth/register",
                r#"{"redirect_uris":["https://claude.ai/api/mcp/auth_callback"]}"#,
            ),
            page("/auth/resume"),
            form("/auth/renew", ""),
            page("/.well-known/oauth-authorization-server"),
        ] {
            let uri = req.uri().to_string();
            let r = send(&app, req).await;
            let minted = r
                .cookies()
                .iter()
                .any(|c| c.starts_with("koan_access=") && !c.starts_with("koan_access=;"));
            assert!(!minted, "{uri} signed in: {:?}", r.cookies());
            assert!(!r.body.contains("\"access_token\":\""), "{uri}: {}", r.body);
        }
    }

    /// Requests from the proxy whose `Remote-User` names no one account: sent
    /// twice, merged, or not text.
    fn unusable_headers() -> Vec<axum::http::request::Builder> {
        let from_proxy = || from_peer(get("/"), "10.0.0.1");
        vec![
            from_proxy()
                .header("remote-user", "mallory")
                .header("remote-user", "alice"),
            from_proxy().header("remote-user", "alice, mallory"),
            from_proxy().header(
                "remote-user",
                axum::http::HeaderValue::from_bytes(b"al\xffice").unwrap(),
            ),
        ]
    }

    /// The reply cleared both session cookies and set no new ones.
    fn assert_signed_out(r: &Reply) {
        for name in ["koan_access", "koan_refresh"] {
            assert!(
                r.cookies()
                    .iter()
                    .any(|c| c.starts_with(&format!("{name}=;")) && c.contains("Max-Age=0")),
                "{name} not cleared: {:?}",
                r.cookies()
            );
            assert!(
                !r.cookies()
                    .iter()
                    .any(|c| c.starts_with(&format!("{name}="))
                        && !c.starts_with(&format!("{name}=;"))),
                "{name} set: {:?}",
                r.cookies()
            );
        }
    }

    #[tokio::test]
    async fn an_account_the_server_lacks_is_refused() {
        let f = setup_behind_proxy();
        let r = send(
            &f.app,
            from_peer(get("/ui/resume"), "10.0.0.1")
                .header("remote-user", "bob")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert_signed_out(&r);
    }

    #[tokio::test]
    async fn an_account_named_in_utf8_signs_in() {
        let f = setup_behind_proxy();
        {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            queries::auth::create_user(&db.conn, "josé", "pw", Role::User).unwrap();
        }
        let r = send(
            &f.app,
            from_peer(get("/ui/resume"), "10.0.0.1")
                .header(
                    "remote-user",
                    axum::http::HeaderValue::from_bytes("josé".as_bytes()).unwrap(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        let claims =
            auth::validate_access_token(&f.state.public_pem, &r.cookie("koan_access")).unwrap();
        assert_eq!(claims.username, "josé");
    }

    #[tokio::test]
    async fn a_header_from_the_proxy_naming_no_one_lets_no_session_through() {
        let f = setup_behind_proxy();
        for req in unusable_headers() {
            let r = send(
                &f.app,
                req.uri("/albums")
                    .header(
                        header::COOKIE,
                        format!("koan_access={}", access_token(&f.state)),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(r.status, StatusCode::FORBIDDEN);
            assert_signed_out(&r);
        }
        for req in unusable_headers() {
            let r = send(
                &f.app,
                req.method("POST")
                    .uri("/ui/renew")
                    .header(header::ORIGIN, ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(r.status, StatusCode::FORBIDDEN);
            assert_signed_out(&r);
        }
    }

    /// The comparison with the proxy's account holds for script requests too, not
    /// only page loads.
    #[tokio::test]
    async fn a_script_request_for_another_account_than_the_proxy_names_is_refused() {
        let f = setup_behind_proxy();
        let post = |user: &str| {
            from_peer(Request::post("/history/forget"), "10.0.0.1")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header("datastar-request", "true")
                .header(
                    header::COOKIE,
                    format!("koan_access={}", access_token(&f.state)),
                )
                .header("remote-user", user)
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            send(&f.app, post("bob")).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_ne!(
            send(&f.app, post("alice")).await.status,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_session_for_another_account_than_the_proxy_names_is_resumed() {
        let f = setup_behind_proxy();
        let r = send(
            &f.app,
            from_peer(authed(&f.state, "/albums"), "10.0.0.1")
                .header("remote-user", "bob")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/ui/resume?next=%2Falbums");

        let r = send(
            &f.app,
            from_peer(authed(&f.state, "/albums"), "10.0.0.1")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
    }

    /// Every way a browser through the proxy gets a session.
    fn proxied_requests(user: &str) -> Vec<Request<Body>> {
        let from_proxy = |req: axum::http::request::Builder| {
            from_peer(req, "10.0.0.1")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header("remote-user", user)
        };
        vec![
            from_proxy(Request::get("/ui/resume?next=%2Falbums"))
                .body(Body::empty())
                .unwrap(),
            from_proxy(Request::get("/login"))
                .body(Body::empty())
                .unwrap(),
            from_proxy(Request::post("/login"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("username=alice&password=hunter2"))
                .unwrap(),
            from_proxy(Request::post("/ui/renew"))
                .body(Body::empty())
                .unwrap(),
        ]
    }

    fn refresh_tokens(f: &Fixture) -> i64 {
        let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
        db.conn
            .query_row("SELECT COUNT(*) FROM refresh_tokens", [], |r| r.get(0))
            .unwrap()
    }

    /// A session through the proxy is an access token alone, derived again from
    /// the header on each page load: nothing outlives what the proxy says.
    #[tokio::test]
    async fn no_refresh_cookie_is_issued_through_the_proxy() {
        let f = setup_behind_proxy();
        for req in proxied_requests("alice") {
            let uri = req.uri().clone();
            let r = send(&f.app, req).await;
            assert!(
                !r.cookies()
                    .iter()
                    .any(|c| c.starts_with("koan_refresh=") && !c.starts_with("koan_refresh=;")),
                "{uri} set a refresh cookie: {:?}",
                r.cookies()
            );
        }
        assert_eq!(refresh_tokens(&f), 0, "no refresh token stored");
    }

    /// The proxy switching the browser from alice to bob ends alice's access
    /// within one access token's lifetime: her session is the access token the
    /// proxy's say-so minted, and nothing renews it once the proxy names bob.
    #[tokio::test]
    async fn a_switch_at_the_proxy_ends_the_previous_account_within_one_access_ttl() {
        let f = setup_behind_proxy();
        {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            queries::auth::create_user(&db.conn, "bob", "pw", Role::User).unwrap();
        }
        let resume = |user: &str| {
            from_peer(get("/ui/resume?next=%2Falbums"), "10.0.0.1")
                .header("remote-user", user)
                .body(Body::empty())
                .unwrap()
        };
        let alice = send(&f.app, resume("alice")).await.cookie("koan_access");
        let claims = auth::validate_access_token(&f.state.public_pem, &alice).unwrap();
        assert!(claims.exp - claims.iat <= f.state.access_ttl_secs);

        // Alice's cookie, the proxy now naming bob: a page load hands over, and
        // neither it nor a renewal gives alice anything more.
        let page = send(
            &f.app,
            from_peer(get("/albums"), "10.0.0.1")
                .header(header::COOKIE, format!("koan_access={alice}"))
                .header("remote-user", "bob")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(page.location(), "/ui/resume?next=%2Falbums");
        for req in [resume("bob")]
            .into_iter()
            .chain(proxied_requests("bob").into_iter().skip(3))
        {
            let r = send(&f.app, req).await;
            let access = r.cookie("koan_access");
            let claims = auth::validate_access_token(&f.state.public_pem, &access).unwrap();
            assert_eq!(claims.username, "bob");
        }
        assert_eq!(refresh_tokens(&f), 0, "nothing to renew alice with");
    }

    #[tokio::test]
    async fn a_cross_site_renewal_through_the_proxy_is_refused() {
        let f = setup_behind_proxy();
        let r = send(
            &f.app,
            from_peer(Request::post("/ui/renew"), "10.0.0.1")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, "https://evil.example")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert!(r.cookies().is_empty());
    }

    /// Pages behind the proxy tell the script to renew from the proxy first, so a
    /// refresh cookie from before proxy mode cannot keep a session going.
    #[tokio::test]
    async fn pages_say_when_the_proxy_renews_them() {
        let f = setup_behind_proxy();
        let r = send(
            &f.app,
            from_peer(authed(&f.state, "/albums"), "10.0.0.1")
                .header("remote-user", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("data-renew=\"ui/renew\""), "{}", r.body);

        let f = setup(true);
        let r = send(
            &f.app,
            authed(&f.state, "/albums").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("data-renew=\"auth/renew\""), "{}", r.body);
    }

    /// Off the proxy, a password sign-in keeps its refresh cookie.
    #[tokio::test]
    async fn a_password_sign_in_off_the_proxy_still_refreshes() {
        let f = setup_behind_proxy();
        let r = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
        assert!(!r.cookie("koan_refresh").is_empty());
    }

    #[test]
    fn proxy_auth_is_off_unless_configured() {
        assert!(
            crate::ui::ProxyAuth::from_config("", &[])
                .unwrap()
                .is_none()
        );
        assert!(
            crate::ui::ProxyAuth::from_config("  ", &[])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_proxy_header_without_a_proxy_is_refused() {
        let err = crate::ui::ProxyAuth::from_config("Remote-User", &[])
            .err()
            .unwrap();
        assert!(err.contains("proxy_auth_from is empty"), "{err}");
    }

    #[test]
    fn a_proxy_without_a_header_is_refused() {
        let err = crate::ui::ProxyAuth::from_config("", &["10.0.0.1".into()])
            .err()
            .unwrap();
        assert!(err.contains("proxy_auth_header is empty"), "{err}");
    }

    #[test]
    fn a_proxy_range_covering_every_address_is_refused() {
        for everyone in ["0.0.0.0/0", "::/0"] {
            let err = crate::ui::ProxyAuth::from_config(
                "Remote-User",
                &["10.0.0.1".into(), everyone.into()],
            )
            .err()
            .unwrap();
            assert!(err.contains("covers every address"), "{everyone}: {err}");
        }
    }

    #[test]
    fn a_bad_proxy_setting_is_refused_not_dropped() {
        use crate::ui::ProxyAuth;
        assert!(ProxyAuth::from_config("Remote User", &["10.0.0.1".into()]).is_err());
        assert!(ProxyAuth::from_config("Remote-User", &["not an address".into()]).is_err());
        assert!(
            ProxyAuth::from_config("Remote-User", &["junk".into(), "172.18.0.0/16".into()])
                .is_err()
        );
    }

    /// An entry written as an IPv4-mapped IPv6 address matches the IPv4 peer it
    /// maps, as the peer is compared in that form.
    #[test]
    fn a_mapped_proxy_entry_is_its_ipv4_address() {
        let proxy = crate::ui::ProxyAuth::from_config("Remote-User", &["::ffff:172.18.0.5".into()])
            .unwrap()
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("remote-user", "alice".parse().unwrap());
        let mut ext = axum::http::Extensions::new();
        ext.insert(axum::extract::ConnectInfo(
            "172.18.0.5:1".parse::<std::net::SocketAddr>().unwrap(),
        ));
        assert_eq!(
            proxy.user(&headers, &ext),
            crate::ui::sign_in::Vouch::Named("alice")
        );
    }

    #[test]
    fn proxy_auth_believes_one_header_from_the_proxy() {
        use crate::ui::ProxyAuth;
        let proxy = ProxyAuth::from_config("Remote-User", &["172.18.0.0/16".into()])
            .unwrap()
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("remote-user", "alice".parse().unwrap());
        let at = |peer: &str| {
            let mut ext = axum::http::Extensions::new();
            ext.insert(axum::extract::ConnectInfo(
                peer.parse::<std::net::SocketAddr>().unwrap(),
            ));
            ext
        };
        use crate::ui::sign_in::Vouch;
        assert_eq!(
            proxy.user(&headers, &at("172.18.0.5:1")),
            Vouch::Named("alice")
        );
        assert_eq!(
            proxy.user(&headers, &at("[::ffff:172.18.0.5]:1")),
            Vouch::Named("alice")
        );
        assert_eq!(proxy.user(&headers, &at("172.19.0.5:1")), Vouch::Absent);
        headers.insert("remote-user", "alice,admin".parse().unwrap());
        assert_eq!(proxy.user(&headers, &at("172.18.0.5:1")), Vouch::Unusable);
        assert_eq!(proxy.user(&headers, &at("172.19.0.5:1")), Vouch::Absent);
    }

    #[tokio::test]
    async fn a_spent_username_budget_refuses_strangers_but_not_the_accounts_network() {
        let f = setup(true);
        let from = |ip: [u8; 4]| {
            let mut req = form("/login", "username=alice&password=hunter2");
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    ip, 1234,
                ))));
            req
        };
        let home = [198, 51, 100, 4];
        assert_eq!(send(&f.app, from(home)).await.status, StatusCode::SEE_OTHER);
        // Spent by guesses from elsewhere.
        for _ in 0..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            f.state.users.failed("alice");
        }
        let r = send(&f.app, from([203, 0, 113, 9])).await;
        assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(
            r.body
                .contains("Too many failed sign-ins for this account.")
        );
        assert!(r.cookies().is_empty());
        // The network alice signed in from still signs in.
        assert_eq!(send(&f.app, from(home)).await.status, StatusCode::SEE_OTHER);
    }
}

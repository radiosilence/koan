//! The web UI: sign in, browse the library and play it in the browser.
//!
//! Server-rendered HTML, with Datastar for the parts that change in place
//! (search as you type, the share button) and a small script of
//! its own that swaps only the page content on navigation, so the player keeps
//! playing. Playback happens in the browser, streaming from `/ui/stream`: a
//! headless server has no speakers.
//!
//! Sign-in is koan's own session: the same `HttpOnly` access and refresh
//! cookies the JSON login sets, so tokens never reach page script. The gate
//! accepts a valid access cookie; a page load without one goes through
//! `/auth/resume`, which spends the refresh cookie (scoped to `/auth`, so only
//! that route sees it) for fresh cookies, or on to the sign-in form.

mod account;
mod browse;
mod connect;
mod history;
mod oauth;
mod pages;
mod session;
#[cfg(test)]
mod tests;
mod users;

pub use oauth::RESOURCE_METADATA;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_core::auth;
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries;

use crate::auth::AuthUser;
use crate::auth::routes::{AuthRouteState, RateLimiter, login_rate_limit, rate_limit};
use crate::covers::Covers;
use crate::share::{asset, blocking, not_found};

/// `'unsafe-eval'` because Datastar compiles its attribute expressions.
/// Everything else is this server's own, and nothing is inline.
const PAGE_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-eval'; style-src 'self'; font-src 'self'; \
     img-src 'self'; media-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; \
     frame-ancestors 'none'";

/// Sent by the UI's own navigation, which wants the page content without the shell.
const PARTIAL: &str = "x-koan-partial";

const UI_CSS: &str = include_str!("../../assets/ui.css");
const UI_JS: &str = include_str!("../../assets/ui.js");
const DATASTAR_JS: &str = include_str!("../../assets/datastar.js");
const NO_COVER_SVG: &str = include_str!("../../assets/no-cover.svg");

/// The page's stylesheet and scripts, each URL carrying a hash of its asset.
pub(super) struct AssetUrls {
    pub css: String,
    pub css_hash: String,
    pub ui_js: String,
    pub player_js: String,
    pub datastar_js: String,
}

pub(super) static ASSETS: std::sync::LazyLock<AssetUrls> = std::sync::LazyLock::new(|| {
    use crate::share::versioned;
    AssetUrls {
        css: versioned("/ui/assets/ui.css", UI_CSS),
        css_hash: crate::share::content_hash(UI_CSS),
        ui_js: versioned("/ui/assets/ui.js", UI_JS),
        player_js: versioned("/ui/assets/player.js", crate::share::ENGINE_JS),
        datastar_js: versioned("/ui/assets/datastar.js", DATASTAR_JS),
    }
});

#[derive(Clone)]
pub struct UiState {
    pool: Arc<Pool>,
    covers: Arc<Covers>,
    /// The codecs and genres the filters offer, with when they were read.
    options: Arc<std::sync::Mutex<Option<(std::time::Instant, pages::Options)>>>,
    auth: AuthRouteState,
    auth_enabled: bool,
    /// `sharing.public_url`, the address invites point clients at.
    public_url: Option<String>,
    /// OAuth codes awaiting their token request.
    codes: oauth::Codes,
    /// `mcp.redirect_hosts`.
    redirect_hosts: Arc<Vec<String>>,
}

pub fn router(
    pool: Arc<Pool>,
    auth: AuthRouteState,
    auth_enabled: bool,
    covers: Arc<Covers>,
    public_url: Option<String>,
    redirect_hosts: Vec<String>,
) -> axum::Router {
    let state = UiState {
        pool,
        covers,
        options: Arc::default(),
        auth,
        auth_enabled,
        public_url,
        codes: oauth::Codes::default(),
        redirect_hosts: Arc::new(redirect_hosts),
    };
    let gated = axum::Router::new()
        .route("/", get(pages::albums))
        .route("/albums", get(pages::albums))
        .route("/album/{id}", get(pages::album))
        .route("/album/{id}/share", post(pages::share_album))
        .route("/artist/{id}/share", post(pages::share_artist))
        .route("/artists", get(pages::artists))
        .route("/artist/{id}", get(pages::artist))
        .route("/playlists", get(pages::playlists))
        .route("/playlist/{id}", get(pages::playlist))
        .route("/search", get(pages::search))
        .route("/search/results", get(pages::search_results))
        .route("/queue", get(pages::queue))
        .route("/library", get(pages::library))
        .route("/favourites", get(pages::favourites))
        .route("/history", get(history::page))
        .route("/history/forget", post(history::forget))
        .route("/connect", get(connect::page))
        .route("/account", get(account::page))
        .route("/account/keys", post(account::create_key))
        .route("/account/keys/{id}/revoke", post(account::revoke_key))
        .route("/account/app-passwords", post(account::create_app_password))
        .route(
            "/account/app-passwords/{id}/revoke",
            post(account::revoke_app_password),
        )
        // Where the keys page was.
        .route("/keys", get(|| async { see_other("/account") }))
        .route("/users", get(users::page).post(users::create))
        .route("/users/{id}/invite", post(users::invite))
        .route("/users/{id}/password", post(users::set_password))
        .route("/users/{id}/password/form", post(users::password_form))
        .route("/users/{id}/role", post(users::set_role))
        .route("/users/{id}/delete", post(users::delete))
        .route("/ui/stream/{id}", get(stream))
        .route("/ui/cover/{id}", get(cover))
        .layer(from_fn(require_datastar_on_post))
        .layer(from_fn_with_state(state.clone(), gate))
        .layer(axum::middleware::map_response(stamp_stylesheet));
    // A plain form, since it answers with a redirect to the client: it proves
    // its origin the way the sign-in form does.
    let consent = axum::Router::new()
        .route(
            "/oauth/authorize",
            get(oauth::authorize).post(oauth::approve),
        )
        .layer(from_fn_with_state(state.clone(), gate));
    // Checking a password is deliberately expensive, so the form shares the
    // JSON login's per-IP window.
    let sign_in = get(session::login_form).merge(
        post(session::login).layer(from_fn_with_state(state.auth.clone(), login_rate_limit)),
    );
    axum::Router::new()
        .merge(gated)
        .merge(consent)
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected_resource),
        )
        .route(oauth::RESOURCE_METADATA, get(oauth::protected_resource))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::authorization_server),
        )
        // Unauthenticated, so capped per IP: registering stores nothing, but
        // signs a client id each time.
        .route(
            "/oauth/register",
            post(oauth::register)
                .layer(axum::extract::DefaultBodyLimit::max(
                    oauth::MAX_REGISTRATION_BODY,
                ))
                .layer(from_fn_with_state(
                    Arc::new(RateLimiter::new(3600, 10)),
                    rate_limit,
                )),
        )
        .route(
            "/oauth/token",
            post(oauth::token).layer(from_fn_with_state(
                Arc::new(RateLimiter::new(60, 60)),
                rate_limit,
            )),
        )
        .route("/login", sign_in)
        .route("/auth/resume", get(session::resume))
        .route("/auth/renew", post(session::renew))
        .route("/auth/signout", post(session::signout))
        .route("/ui/assets/{name}", get(ui_asset))
        // Where anything that wants an icon for this host looks first, before
        // reading a page: without it, a favicon service settles for the parent
        // domain's.
        .route(
            "/favicon.ico",
            get(|| ui_asset(Path("icon-192.png".into()), RawQuery(None))),
        )
        .route(
            "/apple-touch-icon.png",
            get(|| ui_asset(Path("apple-touch-icon.png".into()), RawQuery(None))),
        )
        .with_state(state)
}

/// Which stylesheet the markup was written for. A tab open across an upgrade
/// keeps the old one while navigation and patches bring new markup; ui.js
/// swaps the stylesheet when this names another.
async fn stamp_stylesheet(mut res: Response) -> Response {
    if let Ok(v) = HeaderValue::from_str(&ASSETS.css_hash) {
        res.headers_mut().insert("x-koan-css", v);
    }
    res
}

async fn ui_asset(Path(name): Path<String>, query: RawQuery) -> Response {
    const JS: &str = "text/javascript; charset=utf-8";
    match name.as_str() {
        "ui.css" => asset(UI_CSS, "text/css; charset=utf-8", query),
        "ui.js" => asset(UI_JS, JS, query),
        "player.js" => asset(crate::share::ENGINE_JS, JS, query),
        "datastar.js" => asset(DATASTAR_JS, JS, query),
        other => crate::share::binary_asset(other).unwrap_or_else(not_found),
    }
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
}

/// A request the UI's script makes rather than a page the browser navigates to.
/// Such a request cannot follow a redirect to a sign-in page usefully, so it
/// is refused outright and the script reloads.
fn is_navigation(req: &Request) -> bool {
    req.method() == Method::GET
        && !req.headers().contains_key(PARTIAL)
        && !req.headers().contains_key("datastar-request")
        && !req.uri().path().starts_with("/ui/")
}

/// Let a signed-in user through; send a page load to resume its session, and
/// refuse anything else.
async fn gate(State(s): State<UiState>, mut req: Request, next: Next) -> Response {
    let user = if s.auth_enabled {
        match cookie(req.headers(), "koan_access")
            .and_then(|t| auth::validate_access_token(&s.auth.public_pem, t).ok())
        {
            Some(claims) => crate::auth::current_user(&s.pool, claims).await,
            None => None,
        }
    } else {
        Some(AuthUser::anonymous_admin())
    };
    match user {
        Some(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        None if is_navigation(&req) => {
            let here = req
                .uri()
                .path_and_query()
                .map_or("/", |p| p.as_str())
                .to_owned();
            see_other(&format!("/auth/resume?next={}", encode(&here)))
        }
        None => (
            StatusCode::UNAUTHORIZED,
            [(header::CACHE_CONTROL, "no-store")],
            "signed out",
        )
            .into_response(),
    }
}

/// Datastar sends this header on every request it makes, and a cross-site form
/// or a CORS-safelisted fetch cannot set it: a state-changing POST without it
/// did not come from this UI.
async fn require_datastar_on_post(req: Request, next: Next) -> Response {
    if req.method() == Method::POST && !req.headers().contains_key("datastar-request") {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

fn encode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn see_other(location: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, "no-store"),
        ],
    )
        .into_response()
}

/// An HTML page with the UI's security headers.
fn html(status: StatusCode, body: String) -> Response {
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::VARY, HeaderValue::from_static("x-koan-partial"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PAGE_CSP),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    resp
}

/// A Datastar `patch-elements` event. Without a selector the element replaces
/// the one with its id; with one, `mode` says where it goes.
fn patch(html: &str, target: Option<(&str, &str)>) -> Event {
    let mut lines = Vec::new();
    if let Some((selector, mode)) = target {
        lines.push(format!("selector {selector}"));
        lines.push(format!("mode {mode}"));
    }
    lines.extend(html.lines().map(|l| format!("elements {l}")));
    Event::default()
        .event("datastar-patch-elements")
        .data(lines.join("\n"))
}

fn events(events: Vec<Event>) -> Response {
    let stream = tokio_stream::iter(events.into_iter().map(Ok::<_, std::convert::Infallible>));
    let mut resp = Sse::new(stream).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn open(pool: &Pool) -> Option<Handle<'_>> {
    pool.get()
        .inspect_err(|e| log::error!("web UI: cannot open the database: {e}"))
        .ok()
}

/// A track's audio, from a local file; Range requests are honoured, so the
/// browser can seek a stream.
async fn stream(State(s): State<UiState>, Path(id): Path<i64>, headers: HeaderMap) -> Response {
    let path = blocking(move || {
        let db = open(&s.pool)?;
        let t = queries::tracks_by_ids(&db.conn, &[id]).ok()?.pop()?;
        crate::subsonic::track_file_path(&t).map(PathBuf::from)
    })
    .await;
    let Some(path) = path else {
        return not_found();
    };
    match crate::subsonic::serve_local_file(&path, &headers).await {
        Ok(mut resp) => {
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=3600"),
            );
            resp
        }
        Err(_) => not_found(),
    }
}

/// Query parameters for `cover`.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct CoverQuery {
    size: Option<u32>,
    /// The album's cover version, from `pages::cover_url`. With it the URL
    /// names these exact bytes, so the browser may keep them for good.
    v: Option<String>,
}

/// An album's cover at one of `covers::SIZES`, from the art embedded in the
/// first of its tracks that has any.
async fn cover(
    State(s): State<UiState>,
    Path(id): Path<i64>,
    axum::extract::Query(q): axum::extract::Query<CoverQuery>,
) -> Response {
    let size = crate::covers::snap(q.size);
    let found = blocking(move || {
        let tracks = queries::tracks_for_album(&open(&s.pool)?.conn, id).ok()?;
        (!tracks.is_empty()).then(|| s.covers.cover(&tracks, size))
    })
    .await;
    match found {
        Some(Some(art)) => crate::share::jpeg(Some(art), q.v.is_some()),
        // A record with no artwork draws what the apps draw, not a broken
        // image. Not kept for good: art added beside the files changes no
        // track's mtime, so the URL stays the same when it arrives.
        Some(None) => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            NO_COVER_SVG,
        )
            .into_response(),
        None => not_found(),
    }
}

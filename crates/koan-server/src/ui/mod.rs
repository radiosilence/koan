//! The web UI: sign in, browse the library and play it in the browser.
//!
//! Server-rendered HTML, with Datastar for the parts that change in place
//! (search as you type, loading more, the share button) and a small script of
//! its own that swaps only the page content on navigation, so the player keeps
//! playing. Playback happens in the browser, streaming from `/ui/stream`: a
//! headless server has no speakers.
//!
//! Sign-in is koan's own session: the same `HttpOnly` access and refresh
//! cookies the JSON login sets, so tokens never reach page script. The gate
//! accepts a valid access cookie; a page load without one goes through
//! `/auth/resume`, which spends the refresh cookie (scoped to `/auth`, so only
//! that route sees it) for fresh cookies, or on to the sign-in form.

mod keys;
mod pages;
mod session;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_core::auth::{self, Role};
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries;

use crate::auth::AuthUser;
use crate::auth::routes::{AuthRouteState, login_rate_limit};
use crate::share::{asset, blocking, not_found};

/// `'unsafe-eval'` because Datastar compiles its attribute expressions.
/// Everything else is this server's own, and nothing is inline.
const PAGE_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-eval'; style-src 'self'; \
     img-src 'self'; media-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; \
     frame-ancestors 'none'";

/// Sent by the UI's own navigation, which wants the page content without the shell.
const PARTIAL: &str = "x-koan-partial";

const UI_CSS: &str = include_str!("../../assets/ui.css");
const UI_JS: &str = include_str!("../../assets/ui.js");
const DATASTAR_JS: &str = include_str!("../../assets/datastar.js");

#[derive(Clone)]
pub struct UiState {
    pool: Arc<Pool>,
    auth: AuthRouteState,
    auth_enabled: bool,
}

pub fn router(db_path: PathBuf, auth: AuthRouteState, auth_enabled: bool) -> axum::Router {
    let state = UiState {
        pool: Arc::new(Pool::new(db_path)),
        auth,
        auth_enabled,
    };
    let gated = axum::Router::new()
        .route("/", get(pages::albums))
        .route("/albums", get(pages::albums))
        .route("/albums/more", get(pages::albums_more))
        .route("/album/{id}", get(pages::album))
        .route("/album/{id}/share", post(pages::share))
        .route("/artists", get(pages::artists))
        .route("/artists/more", get(pages::artists_more))
        .route("/artist/{id}", get(pages::artist))
        .route("/search", get(pages::search))
        .route("/search/results", get(pages::search_results))
        .route("/queue", get(pages::queue))
        .route("/keys", get(keys::page).post(keys::create))
        .route("/keys/{id}/revoke", post(keys::revoke))
        .route("/ui/stream/{id}", get(stream))
        .route("/ui/cover/{id}", get(cover))
        .layer(from_fn(require_datastar_on_post))
        .layer(from_fn_with_state(state.clone(), gate));
    // Checking a password is deliberately expensive, so the form shares the
    // JSON login's per-IP window.
    let sign_in = get(session::login_form).merge(
        post(session::login).layer(from_fn_with_state(state.auth.clone(), login_rate_limit)),
    );
    axum::Router::new()
        .merge(gated)
        .route("/login", sign_in)
        .route("/auth/resume", get(session::resume))
        .route("/auth/renew", post(session::renew))
        .route("/auth/signout", post(session::signout))
        .route("/ui/assets/{name}", get(ui_asset))
        .with_state(state)
}

async fn ui_asset(Path(name): Path<String>) -> Response {
    const JS: &str = "text/javascript; charset=utf-8";
    match name.as_str() {
        "ui.css" => asset(UI_CSS, "text/css; charset=utf-8"),
        "ui.js" => asset(UI_JS, JS),
        "player.js" => asset(crate::share::ENGINE_JS, JS),
        "datastar.js" => asset(DATASTAR_JS, JS),
        _ => not_found(),
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
        cookie(req.headers(), "koan_access")
            .and_then(|t| auth::validate_access_token(&s.auth.public_pem, t).ok())
            .map(|c| AuthUser {
                user_id: c.sub,
                role: c.role.parse().unwrap_or(Role::Readonly),
                username: c.username,
            })
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

/// An album's cover, from the art embedded in the first of its tracks that has any.
async fn cover(State(s): State<UiState>, Path(id): Path<i64>) -> Response {
    let art = blocking(move || {
        let db = open(&s.pool)?;
        queries::tracks_for_album(&db.conn, id)
            .ok()?
            .iter()
            .find_map(|t| {
                let path = crate::subsonic::track_file_path(t)?;
                koan_core::index::metadata::extract_cover_art(std::path::Path::new(path))
            })
    })
    .await;
    crate::share::image(art)
}

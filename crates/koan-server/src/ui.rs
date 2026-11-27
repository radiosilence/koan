//! The web UI: sign in, browse the library and play it in the browser.
//!
//! Server-rendered HTML patched over server-sent events by Datastar. Each
//! page is a module under [`page`], named as the nav names it and drawn by
//! its own templates under `templates/<page>/` inside the shell
//! ([`layout`]). A link or form that stays in the UI loads its page in
//! place: the server answers with the page's body morphed into `main`, so
//! the shell, and the player in it, stays. Playback happens in the browser,
//! streaming from `/ui/stream`: a headless server has no speakers.
//!
//! A page with parts that change on their own has a live stream
//! ([`stream`]), which sends a region only when it differs from what it last
//! sent. Markup comes from templates and the kit's components alone, so
//! every value is escaped exactly once, where a template writes it; every
//! address comes from [`paths`].
//!
//! Sign-in is koan's own session: the same `HttpOnly` access and refresh
//! cookies the JSON login sets, so tokens never reach page script. The gate
//! accepts a valid access cookie; a page load without one goes through
//! `/auth/resume`, which spends the refresh cookie (scoped to `/auth`, so only
//! that route sees it) for fresh cookies, or on to the sign-in form. The
//! routes behind the gate are declared in [`routes`]; the admin's behind one
//! more layer that refuses everyone else.

#[cfg(test)]
mod escaping;
#[cfg(test)]
mod fixture;
mod layout;
mod oauth;
mod page;
mod parts;
pub(crate) mod paths;
mod sign_in;
mod stream;

pub use oauth::RESOURCE_METADATA;
pub use sign_in::ProxyAuth;
pub use sign_in::setup::SetupGate;

use std::path::PathBuf;
use std::sync::Arc;

use askama::Template;
use askama_datastar::Csp;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, Version, header};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_core::auth::{self, Role};
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries;
use koan_kit_web as kit;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, Predicate};

use crate::auth::AuthUser;
use crate::auth::routes::{AuthRouteState, RateLimiter, login_rate_limit, rate_limit};
use crate::covers::Covers;
use crate::html::{Html, Render};
use crate::share::{asset, blocking, not_found};

const UI_CSS: &str = include_str!("../assets/ui.css");
const UI_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/ui.js"));
const PASSKEY_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/passkey.js"));
/// Zod, which the scripts parse JSON with, bundled into one module by
/// `just zod` (vendor/zod.ts) and committed, so `cargo build` needs no Node.
const ZOD_JS: &str = include_str!("../assets/zod.js");
const NO_COVER_SVG: &str = include_str!("../assets/no-cover.svg");

/// The page's stylesheet and scripts, each URL carrying a hash of its asset.
pub(crate) struct AssetUrls {
    pub css: String,
    pub css_hash: String,
    pub ui_js: String,
    pub passkey_js: String,
    pub player_js: String,
    pub imports: kit::ImportMap,
}

pub(crate) static ASSETS: std::sync::LazyLock<AssetUrls> = std::sync::LazyLock::new(|| {
    use crate::share::versioned;
    let player_js = versioned(&paths::asset("player.js"), crate::share::ENGINE_JS);
    let zod_js = versioned(&paths::asset("zod.js"), ZOD_JS);
    AssetUrls {
        css: versioned(&paths::asset("ui.css"), UI_CSS),
        css_hash: crate::share::content_hash(UI_CSS),
        ui_js: versioned(&paths::asset("ui.js"), UI_JS),
        passkey_js: versioned(&paths::asset("passkey.js"), PASSKEY_JS),
        imports: kit::ImportMap::new([
            ("koan/player", player_js.as_str()),
            ("zod/mini", zod_js.as_str()),
        ]),
        player_js,
    }
});

/// Every page's policy. `'unsafe-eval'` because Datastar compiles its
/// attribute expressions; nothing is inline but the import map naming the
/// player and Zod, allowed by its hash. `base-uri 'self'` for the `<base
/// href>` the pages link relatively from, `media-src` for the player, and
/// `blob:` for the media source it may build.
pub(crate) fn csp() -> Csp {
    Csp::new()
        .datastar()
        .import_map(ASSETS.imports.json())
        .allow("base-uri", "'self'")
        .allow("media-src", "'self'")
        .allow("media-src", "blob:")
}

static PAGE_CSP: std::sync::LazyLock<HeaderValue> =
    std::sync::LazyLock::new(|| csp().header_value());

#[derive(Clone)]
pub struct UiState {
    pool: Arc<Pool>,
    covers: Arc<Covers>,
    /// The codecs and genres the filters offer, with the `library_stamp` they
    /// were read at.
    options: Arc<std::sync::Mutex<Option<(u64, parts::browse::Options)>>>,
    auth: AuthRouteState,
    auth_enabled: bool,
    /// What `/setup` asks for, when `graphql.setup_wizard` lets it make the
    /// first admin.
    setup: Option<SetupGate>,
    /// `server.url`, the address invites point clients at.
    server_url: Option<String>,
    /// OAuth codes awaiting their token request.
    codes: oauth::Codes,
    /// `mcp.redirect_hosts`.
    redirect_hosts: Arc<Vec<String>>,
    /// `graphql.proxy_auth_header`, when an authenticating proxy is trusted.
    proxy_auth: Option<ProxyAuth>,
    /// GraphQL's background jobs, which a rescan started here joins.
    jobs: crate::graphql::jobs::JobRegistry,
    /// This process, as the `seen` a page carries names it.
    boot: stream::Boot,
}

impl UiState {
    /// This server as WebAuthn knows it, from `server.url`: none without one,
    /// or for an address browsers allow no security keys on.
    fn relying_party(&self) -> Option<koan_auth::webauthn::RelyingParty> {
        koan_auth::webauthn::RelyingParty::from_url(self.server_url.as_deref()?, "kōan")
    }

    /// What pages name as `<base href>`; see `crate::base`.
    fn base(&self) -> String {
        crate::base::href(self.server_url.as_deref())
    }

    /// `path`, a route's address as [`paths`] gives it, as the address bar
    /// shows it: under the base path. What `$url` holds, and what a whole
    /// load or the live stream is asked for by.
    fn absolute(&self, path: &str) -> String {
        format!("{}{}", self.base(), path.trim_start_matches('/'))
    }

    /// The route an address the address bar shows names, the base path off.
    fn route<'a>(&self, address: &'a str) -> &'a str {
        let base = crate::base::path_of(self.server_url.as_deref());
        address.strip_prefix(base.as_str()).unwrap_or(address)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn router(
    pool: Arc<Pool>,
    auth: AuthRouteState,
    auth_enabled: bool,
    setup: Option<SetupGate>,
    covers: Arc<Covers>,
    server_url: Option<String>,
    redirect_hosts: Vec<String>,
    proxy_auth: Option<ProxyAuth>,
    jobs: crate::graphql::jobs::JobRegistry,
) -> axum::Router {
    routes(UiState {
        pool,
        covers,
        options: Arc::default(),
        auth,
        auth_enabled,
        setup,
        server_url,
        codes: oauth::Codes::default(),
        redirect_hosts: Arc::new(redirect_hosts),
        proxy_auth,
        jobs,
        boot: stream::Boot::new(),
    })
}

/// Every route the UI answers, in one place: the pages, the live stream, and
/// the actions the pages post to. A page is drawn by [`page::serve`] from the
/// module named after it; an action is a handler in its page's module. The
/// admin's routes sit behind [`admin_only`] as well as the gate, so a route
/// added to them is refused to everyone else by construction.
fn routes(state: UiState) -> axum::Router {
    use page::{
        account, album, albums, artist, artists, assistants, favourites, history, library,
        playlist, playlists, properties, queue, recently_played, scrobbling, search, tracks, users,
    };
    use paths as p;

    let pages = axum::Router::new()
        .route(p::HOME, get(page::serve::<albums::Albums>))
        .route(p::ALBUMS, get(page::serve::<albums::Albums>))
        .route(p::ALBUM, get(page::serve::<album::Album>))
        .route(p::ARTISTS, get(page::serve::<artists::Artists>))
        .route(p::ARTIST, get(page::serve::<artist::Artist>))
        .route(p::TRACKS, get(page::serve::<tracks::Tracks>))
        .route(p::PROPERTIES, get(page::serve::<properties::Properties>))
        .route(p::PLAYLISTS, get(page::serve::<playlists::Playlists>))
        .route(p::PLAYLIST, get(page::serve::<playlist::Playlist>))
        .route(p::SEARCH, get(page::serve::<search::Search>))
        .route(p::QUEUE, get(page::serve::<queue::Queue>))
        .route(p::LIBRARY, get(page::serve::<library::Library>))
        .route(
            p::RECENTLY_PLAYED,
            get(page::serve::<recently_played::RecentlyPlayed>),
        )
        .route(p::FAVOURITES, get(page::serve::<favourites::Favourites>))
        .route(p::HISTORY, get(page::serve::<history::History>))
        .route(p::ACCOUNT, get(page::serve::<account::Account>))
        .route(p::ASSISTANTS, get(page::serve::<assistants::Assistants>))
        .route(p::SCROBBLING, get(page::serve::<scrobbling::Scrobbling>))
        // Where pages were before they had the names the UI gives them.
        .route(
            p::OLD_ASSISTANTS,
            get(|| async { see_other(p::ASSISTANTS) }),
        )
        .route(p::OLD_KEYS, get(|| async { see_other(p::ACCOUNT) }));
    let streams = axum::Router::new().route(p::LIVE, get(stream::serve));
    let actions = axum::Router::new()
        // Library pages
        .route(p::ALBUM_SHARE, post(album::share))
        .route(p::ARTIST_SHARE, post(artist::share))
        .route(p::FAVOURITE, post(parts::heart::toggle))
        .route(p::QUEUE_ROWS, post(queue::rows))
        .route(p::HISTORY_FORGET, post(history::forget))
        // Account
        .route(p::KEYS, post(account::keys::create))
        .route(p::KEY_REVOKE, post(account::keys::revoke))
        .route(p::APP_PASSWORDS, post(account::app_passwords::create))
        .route(p::APP_PASSWORD_REVOKE, post(account::app_passwords::revoke))
        .route(p::TOTP, post(account::two_step::begin_totp))
        .route(p::TOTP_CONFIRM, post(account::two_step::confirm_totp))
        .route(p::TOTP_REMOVE, post(account::two_step::remove_totp))
        .route(p::SECURITY_KEYS, post(account::two_step::finish_key))
        .route(p::SECURITY_KEYS_BEGIN, post(account::two_step::begin_key))
        .route(p::SECURITY_KEY_REMOVE, post(account::two_step::remove_key))
        .route(
            p::RECOVERY_CODES,
            post(account::two_step::new_recovery_codes),
        )
        // Scrobbling
        .route(p::LASTFM_CONNECT, get(scrobbling::lastfm_connect))
        .route(p::LASTFM_RETURN, get(scrobbling::lastfm_return))
        .route(p::LASTFM_DISCONNECT, post(scrobbling::lastfm_disconnect))
        .route(p::LISTENBRAINZ, post(scrobbling::connect))
        .route(p::LISTENBRAINZ_DISCONNECT, post(scrobbling::disconnect))
        // The player's audio and covers
        .route(p::AUDIO, get(audio))
        .route(p::COVER, get(cover));
    let admin = axum::Router::new()
        .route(
            p::USERS,
            get(page::serve::<users::Users>).post(users::create),
        )
        .route(p::USER_INVITE, post(users::invite))
        .route(p::USER_PASSWORD, post(users::set_password))
        .route(p::USER_PASSWORD_FORM, post(users::password_form))
        .route(p::USER_ROLE, post(users::set_role))
        .route(p::USER_DELETE, post(users::delete))
        .route(p::USER_TWO_STEP_REMOVE, post(users::remove_second_factor))
        .route(p::RESCAN, post(library::rescan::start))
        .route(p::CACHES_CLEAR, post(library::caches::clear))
        .route(p::CACHES_WARM, post(library::caches::warm))
        .layer(from_fn(admin_only));
    let gated = pages
        .merge(streams)
        .merge(actions)
        .merge(admin)
        .layer(from_fn(askama_datastar::require_datastar_on_post))
        .layer(from_fn_with_state(state.clone(), gate))
        .layer(axum::middleware::map_response(stamp_stylesheet));
    let base = crate::base::path_of(state.server_url.as_deref());
    axum::Router::new()
        .merge(gated)
        .merge(sign_in::routes(&state))
        .merge(oauth::routes(&base))
        .route(p::ASSETS, get(ui_asset))
        // Where anything that wants an icon for this host looks first, before
        // reading a page: without it, a favicon service settles for the parent
        // domain's.
        .route(
            p::FAVICON,
            get(|| ui_asset(Path("icon-192.png".into()), RawQuery(None))),
        )
        .route(
            p::TOUCH_ICON,
            get(|| ui_asset(Path("apple-touch-icon.png".into()), RawQuery(None))),
        )
        // The default predicate skips `text/event-stream`, since an encoder
        // buffers a live stream, and images. A body served by byte range is
        // left alone too: a range of a compressed body is not the range asked for.
        .layer(
            CompressionLayer::new().compress_when(DefaultPredicate::new().and(
                |_: StatusCode, _: Version, headers: &HeaderMap, _: &Extensions| {
                    !headers.contains_key(header::ACCEPT_RANGES)
                },
            )),
        )
        .with_state(state)
}

/// The admin's routes: refused to every other account, with nothing drawn.
async fn admin_only(req: Request, next: Next) -> Response {
    match req.extensions().get::<AuthUser>() {
        Some(user) if user.role == Role::Admin => next.run(req).await,
        _ => (StatusCode::FORBIDDEN, "admins only").into_response(),
    }
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
        "passkey.js" => asset(PASSKEY_JS, JS, query),
        "zod.js" => asset(ZOD_JS, JS, query),
        "player.js" => asset(crate::share::ENGINE_JS, JS, query),
        "datastar.js" => askama_datastar::script::serve(query).await,
        "geist-mono.woff2" => kit::font::serve(query).await,
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

/// A page the browser navigates to, rather than a request the page makes.
/// Only a navigation can follow a redirect to a sign-in page usefully; the
/// page's own requests are refused outright, and its script renews the
/// session or reloads.
fn is_navigation(req: &Request) -> bool {
    req.method() == Method::GET
        && !askama_datastar::is_datastar(req.headers())
        && !req.uri().path().starts_with("/ui/")
}

/// Let a signed-in user through; send a page load to resume its session, and
/// refuse anything else. Behind an authenticating proxy a session is good only
/// for the account the proxy names, so a browser whose proxy sign-in changed
/// hands over to the new account, and a header from the proxy that names no
/// one account lets no session through.
async fn gate(State(s): State<UiState>, mut req: Request, next: Next) -> Response {
    let user = if s.auth_enabled {
        let vouched = sign_in::vouched(&s, req.headers(), req.extensions());
        if vouched == sign_in::Vouch::Unusable {
            return sign_in::unusable_header(&s);
        }
        let claims = cookie(req.headers(), "koan_access")
            .and_then(|t| auth::validate_access_token(&s.auth.public_pem, t).ok());
        match (claims, vouched) {
            (Some(claims), sign_in::Vouch::Absent) => {
                crate::auth::current_user(&s.pool, claims).await
            }
            (Some(claims), sign_in::Vouch::Named(name)) if name == claims.username => {
                crate::auth::current_user(&s.pool, claims).await
            }
            _ => None,
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
            let resume = if s.proxy_auth.is_some() {
                paths::PROXY_RESUME
            } else {
                paths::RESUME
            };
            see_other(&format!("{resume}?next={}", encode(&here)))
        }
        None => (
            StatusCode::UNAUTHORIZED,
            [(header::CACHE_CONTROL, "no-store")],
            "signed out",
        )
            .into_response(),
    }
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

/// A whole HTML document with the UI's security headers and `policy`.
fn document_with(status: StatusCode, body: &Html, policy: HeaderValue) -> Response {
    let mut resp = (status, body.as_str().to_owned()).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::VARY, HeaderValue::from_static("datastar-request"));
    h.insert(header::CONTENT_SECURITY_POLICY, policy);
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    resp
}

/// A whole HTML document under every page's policy.
fn document(status: StatusCode, body: &Html) -> Response {
    document_with(status, body, PAGE_CSP.clone())
}

/// A rendered view, as a patch of the elements with its ids.
fn patch(html: &Html) -> Event {
    html.patch().into()
}

/// Regions as the whole answer.
fn answer(regions: impl IntoIterator<Item = Html>) -> Response {
    askama_datastar::events(regions.into_iter().map(|h| patch(&h)).collect::<Vec<_>>())
}

/// An answer changing nothing.
fn nothing() -> Response {
    askama_datastar::events(Vec::new())
}

/// What went wrong, as the flash over the page.
fn flash_bad(message: &str) -> Response {
    answer([kit::Flash::bad(message).html()])
}

/// Done, as the flash over the page.
fn flash_ok(message: &str) -> Response {
    answer([kit::Flash::ok(message).html()])
}

/// Signals set from the server, as JSON: how a form is cleared once what it
/// sent has been accepted.
fn signals(json: serde_json::Value) -> Event {
    askama_datastar::PatchSignals::new(json.to_string()).into()
}

/// The signals an action was sent, refused when they do not parse: an
/// action changes something, so it acts on nothing rather than on a default.
/// Datastar sends every signal on the page, so `T` accepts fields it does not
/// name.
struct Sent<T>(T);

impl<T: serde::de::DeserializeOwned, S: Send + Sync> axum::extract::FromRequest<S> for Sent<T> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        let body = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(IntoResponse::into_response)?;
        serde_json::from_slice(&body).map(Sent).map_err(|e| {
            log::info!("web UI: an action's signals did not parse: {e}");
            (StatusCode::BAD_REQUEST, "the request did not parse").into_response()
        })
    }
}

fn open(pool: &Pool) -> Option<Handle<'_>> {
    pool.get()
        .inspect_err(|e| log::error!("web UI: cannot open the database: {e}"))
        .ok()
}

/// A failed read as the page's empty or not-found state, logged where it
/// happened so that a broken database is not mistaken for a missing row.
pub(crate) trait Logged<T> {
    fn logged(self) -> Option<T>;
}

impl<T, E: std::fmt::Display> Logged<T> for Result<T, E> {
    #[track_caller]
    fn logged(self) -> Option<T> {
        let at = std::panic::Location::caller();
        self.inspect_err(|e| {
            log::error!("web UI: a read failed at {}:{}: {e}", at.file(), at.line())
        })
        .ok()
    }
}

/// An error from a library that writes its messages as clauses, as a
/// sentence: its first letter in capitals and a full stop after it.
fn capitalised(message: &str) -> String {
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |c| {
        format!("{}{}.", c.to_uppercase(), chars.as_str())
    })
}

/// Who may change the server rather than only listen to it.
fn is_admin(user: &AuthUser) -> bool {
    user.role == Role::Admin
}

/// Who may favourite, share and keep playlists.
fn may_edit(user: &AuthUser) -> bool {
    user.role.has_permission(Role::User)
}

/// A track's audio, from a local file; Range requests are honoured, so the
/// browser can seek a stream.
async fn audio(State(s): State<UiState>, Path(id): Path<i64>, headers: HeaderMap) -> Response {
    let path = blocking(move || {
        let db = open(&s.pool)?;
        let t = queries::tracks_by_ids(&db.conn, &[id]).logged()?.pop()?;
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
    /// The album's cover version, from `parts::covers`. With it the URL
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
        let tracks = queries::tracks_for_album(&open(&s.pool)?.conn, id).logged()?;
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

/// The icons every document links, under `/ui/assets` or `/share/assets`.
#[derive(Template)]
#[template(path = "layout/icons.html")]
pub(crate) struct Icons {
    pub(crate) under: &'static str,
}

impl Icons {
    pub(crate) fn html(&self) -> Html {
        Render::html(self)
    }
}

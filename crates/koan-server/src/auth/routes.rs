//! Auth HTTP routes: login, refresh, logout.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

use koan_core::auth;
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries::auth as auth_queries;

/// Name of the cookie carrying the refresh token. Scoped to `/auth` so it
/// reaches refresh and logout but is never attached to an API call, and
/// `HttpOnly` so script cannot read it.
pub(crate) const REFRESH_COOKIE: &str = "koan_refresh";
const REFRESH_COOKIE_PATH: &str = "/auth";
/// A cookie left at this narrower path is sent ahead of the one at
/// `REFRESH_COOKIE_PATH` and shadows it, so every response that sets or clears
/// the refresh cookie clears this one too.
const STALE_REFRESH_COOKIE_PATH: &str = "/auth/refresh";

/// Fixed-window per-IP cap on requests. By default, the cap on login attempts:
///
/// Argon2 is tuned to cost ~19MiB and real CPU per verification, which is
/// correct for resisting cracking and ruinous when anyone may trigger it at
/// will: a few hundred concurrent logins exhaust memory and starve every other
/// request. The window is coarse on purpose — it bounds cost, it is not a quota.
const LOGIN_WINDOW_SECS: u64 = 60;
const LOGIN_MAX_PER_WINDOW: u32 = 10;
/// Above this many tracked IPs, drop stale windows before inserting more.
const TRACKED_IPS_MAX: usize = 4096;

pub struct RateLimiter {
    windows: Mutex<HashMap<IpAddr, (u64, u32)>>,
    window_secs: u64,
    max: u32,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(LOGIN_WINDOW_SECS, LOGIN_MAX_PER_WINDOW)
    }
}

impl RateLimiter {
    pub fn new(window_secs: u64, max: u32) -> Self {
        Self {
            windows: Mutex::default(),
            window_secs,
            max,
        }
    }

    /// Returns false when `ip`'s network (see [`network`]) has spent its
    /// allowance for the current window.
    pub(crate) fn allow(&self, ip: IpAddr) -> bool {
        let now = auth::now_unix();
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());

        if windows.len() > TRACKED_IPS_MAX {
            windows.retain(|_, (start, _)| now.saturating_sub(*start) < self.window_secs);
        }

        let entry = windows.entry(network(ip)).or_insert((now, 0));
        if now.saturating_sub(entry.0) >= self.window_secs {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max
    }
}

/// The address a request came from.
///
/// Behind a reverse proxy the TCP peer is the proxy, for every client, so a
/// limit keyed on it is one bucket for everyone. When the peer is on a private
/// or loopback address — a proxy in the cluster or on the host — the last
/// `X-Forwarded-For` entry is the one that proxy appended, and is the client.
/// Earlier entries are whatever the client sent, and are ignored. A public
/// peer is the client, and its header is not believed. Nor is it when the
/// peer is unknown: a listener served without its connection info would
/// otherwise let every client pick its own address.
///
/// IPv4-mapped IPv6 addresses come back as IPv4, so one client is one address
/// whichever way a dual-stack listener or a proxy spelled it.
pub(crate) fn client_ip(request: &axum::extract::Request) -> IpAddr {
    address(request.extensions(), request.headers())
}

/// [`client_ip`] as an extractor, for handlers that also read the body.
pub(crate) struct ClientIp(pub IpAddr);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(address(&parts.extensions, &parts.headers)))
    }
}

fn address(extensions: &axum::http::Extensions, headers: &axum::http::HeaderMap) -> IpAddr {
    let Some(ConnectInfo(peer)) = extensions.get::<ConnectInfo<SocketAddr>>() else {
        return IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED);
    };
    let peer = peer.ip().to_canonical();
    if !is_internal(peer) {
        return peer;
    }
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|ip| ip.trim().parse::<IpAddr>().ok())
        .next_back()
        .map_or(peer, |ip| ip.to_canonical())
}

/// The network an address counts against a per-address limit as: itself, or
/// for IPv6 its /64, which one subscriber is usually given whole. Keyed on the
/// full address, a client would have 2^64 fresh allowances.
pub(crate) fn network(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !((1u128 << 64) - 1)).into()),
    }
}

fn is_internal(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AuthRouteState {
    pub pool: Arc<Pool>,
    pub private_pem: Arc<Vec<u8>>,
    pub public_pem: Arc<Vec<u8>>,
    pub access_ttl_secs: u64,
    pub refresh_ttl_secs: u64,
    /// Mark cookies `Secure`. Only when clients reach koan over HTTPS —
    /// a browser discards a `Secure` cookie delivered over plain `http://`, so
    /// setting this on a LAN deployment silently breaks cookie auth entirely.
    pub cookie_secure: bool,
    pub login_limiter: Arc<RateLimiter>,
    /// The server's one password verifier; see `super::password`.
    pub users: Arc<super::password::PasswordVerifier>,
}

impl AuthRouteState {
    /// `SameSite=Lax` keeps the cookie off cross-site requests, which is what
    /// takes the WebSocket and safelisted-content-type CSRF paths off the table.
    fn cookie(&self, name: &str, value: &str, path: &str, max_age: u64) -> String {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        format!("{name}={value}; HttpOnly; SameSite=Lax; Path={path}; Max-Age={max_age}{secure}")
    }

    fn access_cookie(&self, token: &str) -> String {
        self.cookie("koan_access", token, "/", self.access_ttl_secs)
    }

    fn refresh_cookie(&self, token: &str) -> String {
        self.cookie(
            REFRESH_COOKIE,
            token,
            REFRESH_COOKIE_PATH,
            self.refresh_ttl_secs,
        )
    }

    fn stale_refresh_cookie(&self) -> String {
        self.cookie(REFRESH_COOKIE, "", STALE_REFRESH_COOKIE_PATH, 0)
    }

    /// The cookies that open a session. Appended rather than inserted: a
    /// header array as a response part keeps only the last value per name.
    pub(crate) fn session_cookies(
        &self,
        access_token: &str,
        refresh_token: &str,
    ) -> AppendHeaders<[(axum::http::HeaderName, String); 3]> {
        AppendHeaders([
            (SET_COOKIE, self.access_cookie(access_token)),
            (SET_COOKIE, self.refresh_cookie(refresh_token)),
            (SET_COOKIE, self.stale_refresh_cookie()),
        ])
    }

    /// The one cookie a session vouched for by an authenticating proxy has:
    /// an access token, and any refresh cookie the browser held cleared. Such
    /// a session is derived again from the proxy's header on each page load,
    /// so it never outlives what the proxy says.
    pub(crate) fn proxied_cookies(
        &self,
        access_token: &str,
    ) -> AppendHeaders<[(axum::http::HeaderName, String); 3]> {
        AppendHeaders([
            (SET_COOKIE, self.access_cookie(access_token)),
            (
                SET_COOKIE,
                self.cookie(REFRESH_COOKIE, "", REFRESH_COOKIE_PATH, 0),
            ),
            (SET_COOKIE, self.stale_refresh_cookie()),
        ])
    }

    /// Clears the access cookie and both refresh cookies: what signing out sets.
    pub(crate) fn cleared_cookies(&self) -> AppendHeaders<[(axum::http::HeaderName, String); 3]> {
        AppendHeaders([
            (SET_COOKIE, self.cookie("koan_access", "", "/", 0)),
            (
                SET_COOKIE,
                self.cookie(REFRESH_COOKIE, "", REFRESH_COOKIE_PATH, 0),
            ),
            (SET_COOKIE, self.stale_refresh_cookie()),
        ])
    }
}

/// A hash with the same parameters as a real one, to verify unknown usernames
/// against.
pub(crate) fn dummy_password_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| auth::hash_password("koan-dummy-password").unwrap_or_default())
}

/// Read the refresh token from the request body, falling back to the cookie so a
/// browser client never has to keep one in script-reachable storage.
pub(crate) fn refresh_token_from(
    body: Option<&str>,
    headers: &axum::http::HeaderMap,
) -> Option<String> {
    if let Some(t) = body.filter(|t| !t.is_empty()) {
        return Some(t.to_owned());
    }
    headers
        .get(COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                c.trim()
                    .strip_prefix(&format!("{REFRESH_COOKIE}="))
                    .map(str::to_owned)
            })
        })
}

/// Reject login attempts once an IP has spent its window.
///
/// A middleware rather than an extractor so it runs before the request body is
/// read and before the database is touched.
pub(crate) async fn login_rate_limit(
    State(state): State<AuthRouteState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ip = client_ip(&request);

    if !state.login_limiter.allow(ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(MessageResponse {
                message: "too many login attempts".into(),
            }),
        )
            .into_response();
    }
    next.run(request).await
}

/// `RateLimiter` as a middleware, for routes other than sign-in.
pub(crate) async fn rate_limit(
    State(limiter): State<Arc<RateLimiter>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !limiter.allow(client_ip(&request)) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    next.run(request).await
}

impl AuthRouteState {
    fn open_db(&self) -> Result<Handle<'_>, (StatusCode, String)> {
        self.pool.get().map_err(|e| {
            log::error!("auth db open error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            )
        })
    }
}

// ---------------------------------------------------------------------------
// Request/response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub user: UserInfo,
}

#[derive(Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub role: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RefreshRequest {
    pub refresh_token: Option<String>,
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LogoutRequest {
    pub refresh_token: Option<String>,
}

#[derive(Serialize)]
pub struct MessageResponse {
    pub message: String,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn auth_router(state: AuthRouteState) -> axum::Router {
    let router = axum::Router::new()
        .route(
            "/auth/login",
            post(login).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                login_rate_limit,
            )),
        )
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout));
    auth_perimeter(router, AUTH_TIMEOUT).with_state(state)
}

/// How long a sign-in, refresh or sign-out may take, reading its body included.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// These routes are unauthenticated by definition and the work behind them is
/// deliberately expensive, so they get their own ceiling rather than sharing
/// the GraphQL one. It sheds rather than queues, and each request has a
/// deadline: the permit is held while the body is read, so two clients that
/// never finish sending one would otherwise hold a route shut.
fn auth_perimeter<S>(router: axum::Router<S>, timeout: std::time::Duration) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .layer(
            tower::ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |_: tower::BoxError| async { (StatusCode::SERVICE_UNAVAILABLE, "busy") },
                ))
                .load_shed()
                .concurrency_limit(2),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Check a username and password and open a session: a fresh access token and
/// a stored refresh token. The error is the response to send. Shared by the
/// JSON login and the web UI's sign-in form, so both are one implementation.
///
/// On the blocking pool as a whole: the queries and argon2 both block, and on a runtime worker they stall every other request the server
/// is handling. The password goes through the server's one verifier, so the
/// ceiling on argon2 and the per-username budget on failures hold here as on
/// every other door.
pub(crate) async fn authenticate(
    state: &AuthRouteState,
    username: &str,
    password: &str,
    from: IpAddr,
) -> Result<(auth_queries::UserRow, String, String), Box<Response>> {
    let state = state.clone();
    let (username, password) = (username.to_owned(), password.to_owned());
    tokio::task::spawn_blocking(move || authenticate_blocking(&state, &username, &password, from))
        .await
        .unwrap_or_else(|_| Err(Box::new(StatusCode::INTERNAL_SERVER_ERROR.into_response())))
}

fn authenticate_blocking(
    state: &AuthRouteState,
    username: &str,
    password: &str,
    from: IpAddr,
) -> Result<(auth_queries::UserRow, String, String), Box<Response>> {
    use super::password::Refused;
    let refused = |status, message: &str| {
        Box::new(
            (
                status,
                Json(MessageResponse {
                    message: message.into(),
                }),
            )
                .into_response(),
        )
    };
    if state.users.spent(username, from) {
        return Err(refused(
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed sign-ins for this account; try again in a minute",
        ));
    }
    let user = match state.users.verify(username, password) {
        Ok(user) => {
            state.users.signed_in(username, from);
            user
        }
        Err(Refused::Wrong) => {
            state.users.failed(username);
            return Err(refused(
                StatusCode::UNAUTHORIZED,
                "invalid username or password",
            ));
        }
        Err(Refused::Busy) => {
            return Err(refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "busy; try again in a moment",
            ));
        }
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return Err(Box::new((status, msg).into_response())),
    };

    let access_token = match auth::mint_access_token(
        &state.private_pem,
        user.id,
        &user.username,
        user.role,
        state.access_ttl_secs,
    ) {
        Ok(t) => t,
        Err(e) => {
            log::error!("auth mint token error: {}", e);
            return Err(Box::new(
                (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response(),
            ));
        }
    };

    let refresh_token_id = match auth::random_token() {
        Ok(t) => t,
        Err(e) => {
            log::error!("auth refresh token generation error: {}", e);
            return Err(Box::new(
                (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response(),
            ));
        }
    };
    let refresh_expires = auth::now_unix() as i64 + state.refresh_ttl_secs as i64;
    if let Err(e) =
        auth_queries::store_refresh_token(&db.conn, &refresh_token_id, user.id, refresh_expires)
    {
        log::error!("auth store refresh token error: {}", e);
        return Err(Box::new(
            (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response(),
        ));
    }

    // Clear out expired tokens; a failure here does not fail the sign-in.
    let _ = auth_queries::cleanup_expired_tokens(&db.conn);

    Ok((user, access_token, refresh_token_id))
}

/// An access token for the account an authenticating proxy names, and no
/// refresh token: see `proxied_cookies`. No password is checked; the caller
/// has established that the request came through that proxy. `None` for a
/// name the server has no account for.
pub(crate) async fn proxied_access(state: &AuthRouteState, username: &str) -> Option<String> {
    let state = state.clone();
    let username = username.to_owned();
    tokio::task::spawn_blocking(move || {
        let db = state.open_db().ok()?;
        let user = auth_queries::get_user_by_username(&db.conn, &username).ok()??;
        auth::mint_access_token(
            &state.private_pem,
            user.id,
            &user.username,
            user.role,
            state.access_ttl_secs,
        )
        .map_err(|e| log::error!("auth mint token error: {e}"))
        .ok()
    })
    .await
    .ok()
    .flatten()
}

async fn login(
    State(state): State<AuthRouteState>,
    ClientIp(from): ClientIp,
    Json(req): Json<LoginRequest>,
) -> Response {
    let (user, access_token, refresh_token_id) =
        match authenticate(&state, &req.username, &req.password, from).await {
            Ok(session) => session,
            Err(resp) => return *resp,
        };

    let cookies = state.session_cookies(&access_token, &refresh_token_id);

    let resp = LoginResponse {
        access_token,
        // Also in the body: the CLI and other non-browser clients have no cookie
        // jar and store this in config.local.toml.
        refresh_token: refresh_token_id,
        token_type: "Bearer".into(),
        expires_in: state.access_ttl_secs,
        user: UserInfo {
            id: user.id,
            username: user.username,
            role: user.role.as_str().into(),
        },
    };

    (StatusCode::OK, cookies, Json(resp)).into_response()
}

/// How long after its refresh a spent OAuth refresh token may come back as a
/// retry rather than a replay.
const REPLAY_GRACE_SECS: i64 = 30;

/// Spend a refresh token for a new access token and a new refresh token. The
/// error is the response to send. Shared by the JSON refresh, the web UI's
/// session resume and OAuth.
///
/// A refresh is the account signing in from `from` as surely as a password
/// is, so it marks that network as the account's (see
/// `PasswordVerifier::signed_in`): a browser that keeps its session is how an
/// account's own network stays known. `None` for an OAuth client, whose
/// address is a service's, not the account's.
pub(crate) fn rotate(
    state: &AuthRouteState,
    supplied: &str,
    from: Option<IpAddr>,
) -> Result<(String, String), Box<Response>> {
    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return Err(Box::new((status, msg).into_response())),
    };

    // Atomically consume (validate + revoke) the refresh token in a single
    // statement to prevent TOCTOU races during token rotation.
    let token = match auth_queries::consume_refresh_token(&db.conn, supplied) {
        Ok(Some(t)) => t,
        Ok(None) => {
            match auth_queries::revoke_replayed_grant(&db.conn, supplied, REPLAY_GRACE_SECS) {
                Ok(0) => {}
                Ok(n) => log::warn!("a spent OAuth refresh token came back: revoked {n} tokens"),
                Err(e) => log::error!("auth replay check error: {e}"),
            }
            return Err(Box::new(
                (
                    StatusCode::UNAUTHORIZED,
                    Json(MessageResponse {
                        message: "invalid or expired refresh token".into(),
                    }),
                )
                    .into_response(),
            ));
        }
        Err(e) => {
            log::error!("auth refresh db error: {}", e);
            return Err(Box::new(
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response(),
            ));
        }
    };

    let user = match auth_queries::get_user_by_id(&db.conn, token.user_id) {
        Ok(Some(u)) => u,
        Ok(None) => {
            return Err(Box::new(
                (
                    StatusCode::UNAUTHORIZED,
                    Json(MessageResponse {
                        message: "user not found".into(),
                    }),
                )
                    .into_response(),
            ));
        }
        Err(e) => {
            log::error!("auth refresh user lookup error: {}", e);
            return Err(Box::new(
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response(),
            ));
        }
    };

    // A grant's tokens stay as narrow as the grant.
    let access_token = match auth::mint_scoped_token(
        &state.private_pem,
        user.id,
        &user.username,
        user.role,
        state.access_ttl_secs,
        token.grant.as_ref().map(|_| auth::MCP_SCOPE),
    ) {
        Ok(t) => t,
        Err(e) => {
            log::error!("auth mint token error: {}", e);
            return Err(Box::new(
                (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response(),
            ));
        }
    };

    let new_refresh_id = match auth::random_token() {
        Ok(t) => t,
        Err(e) => {
            log::error!("auth refresh token generation error: {}", e);
            return Err(Box::new(
                (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response(),
            ));
        }
    };
    let refresh_expires = auth::now_unix() as i64 + state.refresh_ttl_secs as i64;
    if let Err(e) = auth_queries::store_grant_token(
        &db.conn,
        &new_refresh_id,
        user.id,
        refresh_expires,
        token.grant.as_ref(),
    ) {
        log::error!("auth store refresh token error: {}", e);
        return Err(Box::new(
            (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response(),
        ));
    }

    if let Some(from) = from {
        state.users.signed_in(&user.username, from);
    }
    Ok((access_token, new_refresh_id))
}

async fn refresh(
    State(state): State<AuthRouteState>,
    ClientIp(from): ClientIp,
    headers: axum::http::HeaderMap,
    body: Option<Json<RefreshRequest>>,
) -> Response {
    let supplied = body.and_then(|Json(req)| req.refresh_token);
    let Some(supplied) = refresh_token_from(supplied.as_deref(), &headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(MessageResponse {
                message: "missing refresh token".into(),
            }),
        )
            .into_response();
    };

    let rotating = state.clone();
    let rotated = tokio::task::spawn_blocking(move || rotate(&rotating, &supplied, Some(from)))
        .await
        .unwrap_or_else(|_| Err(Box::new(StatusCode::INTERNAL_SERVER_ERROR.into_response())));
    let (access_token, new_refresh_id) = match rotated {
        Ok(pair) => pair,
        Err(resp) => return *resp,
    };

    let cookies = state.session_cookies(&access_token, &new_refresh_id);

    let resp = RefreshResponse {
        access_token,
        refresh_token: new_refresh_id,
        token_type: "Bearer".into(),
        expires_in: state.access_ttl_secs,
    };

    (StatusCode::OK, cookies, Json(resp)).into_response()
}

async fn logout(
    State(state): State<AuthRouteState>,
    headers: axum::http::HeaderMap,
    body: Option<Json<LogoutRequest>>,
) -> Response {
    let supplied = body.and_then(|Json(req)| req.refresh_token);
    let token = refresh_token_from(supplied.as_deref(), &headers);
    let revoking = state.clone();
    let revoked = tokio::task::spawn_blocking(move || {
        let db = revoking.open_db()?;
        if let Some(token) = token {
            let _ = auth_queries::revoke_refresh_token(&db.conn, &token);
        }
        Ok(())
    })
    .await
    .unwrap_or_else(|_| {
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        ))
    });
    if let Err((status, msg)) = revoked {
        return (status, msg).into_response();
    }

    let cookies = state.cleared_cookies();

    (
        StatusCode::OK,
        cookies,
        Json(MessageResponse {
            message: "logged out".into(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_limiter_caps_a_single_ip() {
        let limiter = RateLimiter::default();
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        for _ in 0..LOGIN_MAX_PER_WINDOW {
            assert!(limiter.allow(ip));
        }
        assert!(!limiter.allow(ip));

        // Other callers are unaffected.
        assert!(limiter.allow("10.0.0.6".parse().unwrap()));
    }

    #[test]
    fn an_ipv6_client_is_limited_by_its_64() {
        let limiter = RateLimiter::default();
        for n in 0..LOGIN_MAX_PER_WINDOW {
            assert!(limiter.allow(format!("2001:db8:1:2::{n:x}").parse().unwrap()));
        }
        assert!(!limiter.allow("2001:db8:1:2:ffff::1".parse().unwrap()));
        assert!(limiter.allow("2001:db8:1:3::1".parse().unwrap()));
    }

    #[test]
    fn a_mapped_ipv4_address_is_the_ipv4_address() {
        let r = request_from("[::ffff:198.51.100.4]", None);
        assert_eq!(client_ip(&r), "198.51.100.4".parse::<IpAddr>().unwrap());
        // A mapped private address is a proxy like any other.
        let r = request_from("[::ffff:10.42.0.7]", Some("::ffff:203.0.113.9"));
        assert_eq!(client_ip(&r), "203.0.113.9".parse::<IpAddr>().unwrap());
    }

    fn request_from(peer: &str, forwarded: Option<&str>) -> axum::extract::Request {
        let mut request = axum::http::Request::new(axum::body::Body::empty());
        request.extensions_mut().insert(ConnectInfo(
            format!("{peer}:1234").parse::<SocketAddr>().unwrap(),
        ));
        if let Some(f) = forwarded {
            request
                .headers_mut()
                .insert("x-forwarded-for", f.parse().unwrap());
        }
        request
    }

    #[test]
    fn client_ip_believes_only_an_internal_proxy() {
        // Behind the cluster's proxy: the entry it appended, not the client's.
        let r = request_from("10.42.0.7", Some("6.6.6.6, 203.0.113.9"));
        assert_eq!(client_ip(&r), "203.0.113.9".parse::<IpAddr>().unwrap());
        // A public peer is the client, whatever it claims.
        let r = request_from("198.51.100.4", Some("10.0.0.1"));
        assert_eq!(client_ip(&r), "198.51.100.4".parse::<IpAddr>().unwrap());
        // An internal peer with no header is itself.
        let r = request_from("10.42.0.7", None);
        assert_eq!(client_ip(&r), "10.42.0.7".parse::<IpAddr>().unwrap());
        // No peer known: the header is the client's own claim.
        let mut r = axum::http::Request::new(axum::body::Body::empty());
        r.headers_mut()
            .insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert_eq!(client_ip(&r), IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    }

    #[tokio::test]
    async fn stalled_bodies_are_shed_then_timed_out() {
        use tower::ServiceExt as _;
        async fn read(_: axum::body::Bytes) -> StatusCode {
            StatusCode::OK
        }
        // With its state, as `auth_router` does: that is what builds each
        // route's layers once, rather than per request.
        let app = auth_perimeter(
            axum::Router::new().route("/auth/login", post(read)),
            std::time::Duration::from_millis(200),
        )
        .with_state(());
        let stalled = || {
            axum::http::Request::post("/auth/login")
                .body(axum::body::Body::from_stream(tokio_stream::pending::<
                    Result<axum::body::Bytes, std::io::Error>,
                >()))
                .unwrap()
        };
        let held: Vec<_> = (0..2)
            .map(|_| tokio::spawn(app.clone().oneshot(stalled())))
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let shed = app.clone().oneshot(stalled()).await.unwrap();
        assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);
        for h in held {
            assert_eq!(
                h.await.unwrap().unwrap().status(),
                StatusCode::REQUEST_TIMEOUT
            );
        }
        let ok = axum::http::Request::post("/auth/login")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(ok).await.unwrap().status(), StatusCode::OK);
    }

    #[test]
    fn refresh_token_falls_back_to_the_cookie() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            COOKIE,
            format!("a=1; {REFRESH_COOKIE}=from-cookie; b=2")
                .parse()
                .unwrap(),
        );

        assert_eq!(
            refresh_token_from(None, &headers).as_deref(),
            Some("from-cookie")
        );
        assert_eq!(
            refresh_token_from(Some("from-body"), &headers).as_deref(),
            Some("from-body")
        );
        assert_eq!(
            refresh_token_from(None, &axum::http::HeaderMap::new()),
            None
        );
    }

    #[test]
    fn cookies_are_lax_and_only_secure_when_tls_is_in_play() {
        let state = |cookie_secure| AuthRouteState {
            pool: Arc::new(Pool::new("/nonexistent".into())),
            private_pem: Arc::new(Vec::new()),
            public_pem: Arc::new(Vec::new()),
            access_ttl_secs: 900,
            refresh_ttl_secs: 60,
            cookie_secure,
            login_limiter: Arc::new(RateLimiter::default()),
            users: Arc::new(crate::auth::password::PasswordVerifier::new(Arc::new(
                Pool::new("/nonexistent/koan.db".into()),
            ))),
        };

        let plain = state(false).access_cookie("tok");
        assert!(plain.contains("SameSite=Lax"));
        assert!(plain.contains("HttpOnly"));
        assert!(!plain.contains("Secure"));

        assert!(state(true).access_cookie("tok").contains("; Secure"));

        // Every cookie reaches the browser, not only the last one set.
        let resp = (StatusCode::OK, state(false).session_cookies("a", "r")).into_response();
        let set: Vec<_> = resp.headers().get_all(SET_COOKIE).iter().collect();
        assert_eq!(set.len(), 3);
        assert_eq!(
            (StatusCode::OK, state(false).cleared_cookies())
                .into_response()
                .headers()
                .get_all(SET_COOKIE)
                .iter()
                .count(),
            3
        );

        // The refresh cookie never rides along on an API call.
        let refresh = state(false).refresh_cookie("tok");
        assert!(refresh.contains("Path=/auth;"));
        assert!(refresh.contains("HttpOnly"));
    }
}

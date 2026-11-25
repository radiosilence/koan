//! Auth HTTP routes: login, refresh, logout.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

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

pub use koan_auth::limit::{RateLimiter, network};
pub use koan_auth::net::ForwardedFrom;

/// The address a request came from: see `koan_auth::net::client_address`.
/// Whose `X-Forwarded-For` is believed is set on each request as a
/// [`ForwardedFrom`] extension; without one, `Private`.
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
    let peer = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| *peer);
    let trust = extensions
        .get::<ForwardedFrom>()
        .copied()
        .unwrap_or_default();
    koan_auth::net::client_address(peer, trust, headers)
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
    /// The second factor, for an account that has one: a code from its
    /// authenticator app, or a recovery code.
    #[serde(default)]
    pub code: Option<String>,
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
    auth_perimeter(router, AUTH_TIMEOUT, AUTH_BODY_TIMEOUT).with_state(state)
}

/// How long a sign-in, refresh or sign-out may take once its body has arrived.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a client has to send the body of a sign-in, refresh or sign-out.
const AUTH_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The largest body these routes read. Each is a small JSON object.
const AUTH_BODY_LIMIT: usize = 16 * 1024;

/// These routes are unauthenticated by definition and the work behind them is
/// deliberately expensive, so they get their own ceiling rather than sharing
/// the GraphQL one. It sheds rather than queues, and each request has a
/// deadline. The body is read whole, under its own deadline, before a permit
/// is taken: a client that never finishes sending one holds nothing but its
/// own connection.
fn auth_perimeter<S>(
    router: axum::Router<S>,
    timeout: std::time::Duration,
    body_timeout: std::time::Duration,
) -> axum::Router<S>
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
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                buffer_body(request, next, body_timeout)
            },
        ))
}

/// Read a request's body into memory within `deadline`, then pass it on.
async fn buffer_body(
    request: axum::extract::Request,
    next: axum::middleware::Next,
    deadline: std::time::Duration,
) -> Response {
    use futures_util::StreamExt as _;
    let (parts, body) = request.into_parts();
    let read = async {
        let mut stream = body.into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;
            if bytes.len() + chunk.len() > AUTH_BODY_LIMIT {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    match tokio::time::timeout(deadline, read).await {
        Ok(Ok(bytes)) => {
            next.run(axum::extract::Request::from_parts(parts, bytes.into()))
                .await
        }
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// What a password sign-in came to.
pub(crate) enum SignedIn {
    /// A session: the account, a fresh access token and a stored refresh token.
    Session(auth_queries::UserRow, String, String),
    /// The password passed, and the account has a second factor that was not
    /// given: see `koan_auth::second_factor`.
    NeedsSecond(auth_queries::UserRow),
}

/// What a person gave as their second factor.
pub(crate) enum Answer {
    /// A code from an authenticator app, or a recovery code.
    Code(String),
    /// A security key's answer to the ticket's challenge.
    Key(koan_auth::webauthn::Assertion),
}

fn refused(status: StatusCode, message: &str) -> Box<Response> {
    Box::new(
        (
            status,
            Json(MessageResponse {
                message: message.into(),
            }),
        )
            .into_response(),
    )
}

fn internal(what: &str, e: impl std::fmt::Display) -> Box<Response> {
    log::error!("auth {what}: {e}");
    Box::new((StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response())
}

/// Check a username and password, and with an account that has a second
/// factor `code` too, and open a session. The error is the response to send.
/// Shared by the JSON login and the web UI's sign-in form, so both are one
/// implementation.
///
/// On the blocking pool as a whole: the queries and argon2 both block, and on
/// a runtime worker they stall every other request the server is handling.
/// The password goes through the server's one verifier, so the ceiling on
/// argon2 and the per-username budget on failures hold here as on every
/// other door. The code is checked only once the password has passed.
pub(crate) async fn authenticate(
    state: &AuthRouteState,
    username: &str,
    password: &str,
    code: Option<&str>,
    from: IpAddr,
) -> Result<SignedIn, Box<Response>> {
    let state = state.clone();
    let (username, password) = (username.to_owned(), password.to_owned());
    let code = code.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        authenticate_blocking(&state, &username, &password, code.as_deref(), from)
    })
    .await
    .unwrap_or_else(|_| Err(Box::new(StatusCode::INTERNAL_SERVER_ERROR.into_response())))
}

fn authenticate_blocking(
    state: &AuthRouteState,
    username: &str,
    password: &str,
    code: Option<&str>,
    from: IpAddr,
) -> Result<SignedIn, Box<Response>> {
    use super::password::Refused;
    if state.users.spent(username, from) {
        return Err(refused(
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed sign-ins for this account; try again in a minute",
        ));
    }
    let user = match state.users.verify(username, password) {
        Ok(user) => user,
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

    let db = state
        .open_db()
        .map_err(|(status, msg)| Box::new((status, msg).into_response()))?;
    let second = koan_auth::second_factor::enabled(&db.conn, user.id)
        .map_err(|e| internal("second factor lookup", e))?;
    if second {
        let Some(code) = code.filter(|c| !c.trim().is_empty()) else {
            return Ok(SignedIn::NeedsSecond(user));
        };
        check_code(state, &db.conn, &user, code)?;
    }
    state.users.signed_in(username, from);
    let (access, refresh) = open_session(state, &db.conn, &user)?;
    Ok(SignedIn::Session(user, access, refresh))
}

/// Check a second-factor code for `user`, on the account's budget for wrong
/// codes.
fn check_code(
    state: &AuthRouteState,
    conn: &rusqlite::Connection,
    user: &auth_queries::UserRow,
    code: &str,
) -> Result<(), Box<Response>> {
    if state.users.codes_spent(&user.username) {
        return Err(refused(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong codes for this account; try again in a minute",
        ));
    }
    let key = koan_auth::second_factor::totp_key(&state.private_pem);
    match koan_auth::second_factor::check_code(conn, &key, user.id, code) {
        Ok(true) => Ok(()),
        Ok(false) => {
            state.users.code_failed(&user.username);
            Err(refused(StatusCode::UNAUTHORIZED, "wrong code"))
        }
        Err(e) => Err(internal("second factor check", e)),
    }
}

/// Finish a sign-in that `ticket` holds, its password already passed, with
/// the second factor: a code, or a security key's answer checked as `rp`'s.
/// A ticket that has lapsed, or has taken too many wrong answers, is `GONE`:
/// the password must be given again.
pub(crate) async fn finish_second(
    state: &AuthRouteState,
    ticket: String,
    answer: Answer,
    rp: Option<koan_auth::webauthn::RelyingParty>,
    from: IpAddr,
) -> Result<(auth_queries::UserRow, String, String), Box<Response>> {
    use koan_auth::second_factor::Purpose;
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let tickets = state.users.tickets();
        let gone = || refused(StatusCode::GONE, "sign in again");
        // Taken while it is answered, so that one answer cannot pass twice
        // in requests racing each other; given back when it is wrong.
        let held = tickets.take(&ticket, Purpose::SignIn).ok_or_else(gone)?;
        let db = state
            .open_db()
            .map_err(|(status, msg)| Box::new((status, msg).into_response()))?;
        let user = auth_queries::get_user_by_id(&db.conn, held.user_id)
            .map_err(|e| internal("user lookup", e))?
            .filter(|u| u.username == held.username)
            .ok_or_else(gone)?;
        let checked = match answer {
            Answer::Code(code) => check_code(&state, &db.conn, &user, &code),
            Answer::Key(assertion) => {
                if state.users.codes_spent(&user.username) {
                    Err(refused(
                        StatusCode::TOO_MANY_REQUESTS,
                        "too many wrong answers for this account; try again in a minute",
                    ))
                } else {
                    let ok = match &rp {
                        Some(rp) => koan_auth::second_factor::check_assertion(
                            &db.conn,
                            rp,
                            user.id,
                            &held.challenge,
                            &assertion,
                        )
                        .map_err(|e| internal("security key check", e))?,
                        None => false,
                    };
                    if ok {
                        Ok(())
                    } else {
                        state.users.code_failed(&user.username);
                        Err(refused(
                            StatusCode::UNAUTHORIZED,
                            "that security key is not one of this account's",
                        ))
                    }
                }
            }
        };
        if let Err(e) = checked {
            tickets.missed_answer(&ticket, held);
            return Err(e);
        }
        state.users.signed_in(&user.username, from);
        let (access, refresh) = open_session(&state, &db.conn, &user)?;
        Ok((user, access, refresh))
    })
    .await
    .unwrap_or_else(|_| Err(Box::new(StatusCode::INTERNAL_SERVER_ERROR.into_response())))
}

/// Sign in with a passkey alone, answering the challenge `ticket` holds: the
/// passkey names its account and verifies the person, so nothing else is
/// asked. A ticket that has lapsed is `GONE`.
pub(crate) async fn passkey_sign_in(
    state: &AuthRouteState,
    ticket: String,
    assertion: koan_auth::webauthn::Assertion,
    rp: koan_auth::webauthn::RelyingParty,
    from: IpAddr,
) -> Result<(auth_queries::UserRow, String, String), Box<Response>> {
    use koan_auth::second_factor::Purpose;
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let tickets = state.users.passkeys();
        // A challenge answers once, right or wrong: taken in one step, so
        // that requests racing each other cannot both answer it.
        let held = tickets
            .take(&ticket, Purpose::Passkey)
            .ok_or_else(|| refused(StatusCode::GONE, "try again"))?;
        let db = state
            .open_db()
            .map_err(|(status, msg)| Box::new((status, msg).into_response()))?;
        let user =
            koan_auth::second_factor::passkey_sign_in(&db.conn, &rp, &held.challenge, &assertion)
                .map_err(|e| internal("passkey sign-in", e))?
                .map(|id| auth_queries::get_user_by_id(&db.conn, id))
                .transpose()
                .map_err(|e| internal("user lookup", e))?
                .flatten()
                .ok_or_else(|| {
                    refused(
                        StatusCode::UNAUTHORIZED,
                        "that passkey does not sign in to this server",
                    )
                })?;
        state.users.signed_in(&user.username, from);
        let (access, refresh) = open_session(&state, &db.conn, &user)?;
        Ok((user, access, refresh))
    })
    .await
    .unwrap_or_else(|_| Err(Box::new(StatusCode::INTERNAL_SERVER_ERROR.into_response())))
}

/// A fresh access token and a stored refresh token for `user`.
pub(crate) fn open_session(
    state: &AuthRouteState,
    conn: &rusqlite::Connection,
    user: &auth_queries::UserRow,
) -> Result<(String, String), Box<Response>> {
    let access_token = auth::mint_access_token(
        &state.private_pem,
        user.id,
        &user.username,
        user.role,
        state.access_ttl_secs,
    )
    .map_err(|e| internal("mint token", e))?;
    let refresh_token_id = auth::random_token().map_err(|e| internal("refresh token", e))?;
    let refresh_expires = koan_core::clock::unix_secs() + state.refresh_ttl_secs as i64;
    auth_queries::store_refresh_token(conn, &refresh_token_id, user.id, refresh_expires)
        .map_err(|e| internal("store refresh token", e))?;
    // Clear out expired tokens; a failure here does not fail the sign-in.
    let _ = auth_queries::cleanup_expired_tokens(conn);
    Ok((access_token, refresh_token_id))
}

/// A session for `user_id` opened on the blocking pool, for a browser that
/// has just shown both factors: turning a second factor on ends every session
/// the account had, the one doing it included.
pub(crate) async fn session_for(state: &AuthRouteState, user_id: i64) -> Option<(String, String)> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let db = state.open_db().ok()?;
        let user = auth_queries::get_user_by_id(&db.conn, user_id).ok()??;
        open_session(&state, &db.conn, &user).ok()
    })
    .await
    .ok()
    .flatten()
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
    let signed = authenticate(
        &state,
        &req.username,
        &req.password,
        req.code.as_deref(),
        from,
    )
    .await;
    let (user, access_token, refresh_token_id) = match signed {
        Ok(SignedIn::Session(user, access, refresh)) => (user, access, refresh),
        // Asked for with the password again: a client without a page keeps
        // nothing between the two.
        Ok(SignedIn::NeedsSecond(_)) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "message": "this account needs a second factor: send `code` with a code from its authenticator app or a recovery code",
                    "second_factor": true,
                })),
            )
                .into_response();
        }
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
    let refresh_expires = koan_core::clock::unix_secs() + state.refresh_ttl_secs as i64;
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
    body: Option<Json<RefreshRequest>>,
) -> Response {
    // From the body alone: this answers with the tokens, and one read from
    // the cookie would hand page script what `HttpOnly` keeps from it. A
    // browser renews through `/auth/renew`, which answers with cookies.
    let supplied = body
        .and_then(|Json(req)| req.refresh_token)
        .filter(|t| !t.is_empty());
    let Some(supplied) = supplied else {
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
    use koan_auth::limit::LOGIN_MAX_PER_WINDOW;

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

    #[test]
    fn a_home_server_believes_only_a_tunnel_on_this_machine() {
        let from = |peer, forwarded| {
            let mut r = request_from(peer, forwarded);
            r.extensions_mut().insert(ForwardedFrom::Loopback);
            client_ip(&r)
        };
        // Another device on the network is itself, whatever it claims.
        assert_eq!(
            from("192.168.1.30", Some("203.0.113.9")),
            "192.168.1.30".parse::<IpAddr>().unwrap()
        );
        // A tunnel client here: the client it names.
        assert_eq!(
            from("127.0.0.1", Some("203.0.113.9")),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        // A tunnel client that names nobody is not this network.
        assert_eq!(
            from("127.0.0.1", None),
            IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
        );
    }

    #[tokio::test]
    async fn stalled_bodies_time_out_without_holding_a_permit() {
        use tower::ServiceExt as _;
        async fn read(_: axum::body::Bytes) -> StatusCode {
            StatusCode::OK
        }
        // With its state, as `auth_router` does: that is what builds each
        // route's layers once, rather than per request.
        let app = auth_perimeter(
            axum::Router::new().route("/auth/login", post(read)),
            std::time::Duration::from_secs(10),
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
        let held: Vec<_> = (0..4)
            .map(|_| tokio::spawn(app.clone().oneshot(stalled())))
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // More stalled bodies than permits, and a whole request still gets in.
        let ok = axum::http::Request::post("/auth/login")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(ok).await.unwrap().status(),
            StatusCode::OK
        );
        for h in held {
            assert_eq!(
                h.await.unwrap().unwrap().status(),
                StatusCode::REQUEST_TIMEOUT
            );
        }
        let big = axum::http::Request::post("/auth/login")
            .body(axum::body::Body::from(vec![b' '; AUTH_BODY_LIMIT + 1]))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(big).await.unwrap().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let broken = axum::http::Request::post("/auth/login")
            .body(axum::body::Body::from_stream(tokio_stream::once(Err::<
                axum::body::Bytes,
                _,
            >(
                std::io::Error::other("reset"),
            ))))
            .unwrap();
        assert_eq!(
            app.oneshot(broken).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn slow_work_is_shed_beyond_the_permits() {
        use tower::ServiceExt as _;
        async fn slow(_: axum::body::Bytes) -> StatusCode {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            StatusCode::OK
        }
        let app = auth_perimeter(
            axum::Router::new().route("/auth/login", post(slow)),
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(5),
        )
        .with_state(());
        let req = || {
            axum::http::Request::post("/auth/login")
                .body(axum::body::Body::from("{}"))
                .unwrap()
        };
        let held: Vec<_> = (0..2)
            .map(|_| tokio::spawn(app.clone().oneshot(req())))
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(
            app.clone().oneshot(req()).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        for h in held {
            assert_eq!(
                h.await.unwrap().unwrap().status(),
                StatusCode::REQUEST_TIMEOUT
            );
        }
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

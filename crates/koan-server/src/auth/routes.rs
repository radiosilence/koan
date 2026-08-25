//! Auth HTTP routes: login, refresh, logout.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

use koan_core::auth;
use koan_core::db::connection::Database;
use koan_core::db::queries::auth as auth_queries;

/// Name of the cookie carrying the refresh token. Scoped to `/auth/refresh` so
/// it is never attached to an API call, and `HttpOnly` so script cannot read it.
const REFRESH_COOKIE: &str = "koan_refresh";
const REFRESH_COOKIE_PATH: &str = "/auth/refresh";

/// Fixed-window per-IP cap on login attempts.
///
/// Argon2 is tuned to cost ~19MiB and real CPU per verification, which is
/// correct for resisting cracking and ruinous when anyone may trigger it at
/// will: a few hundred concurrent logins exhaust memory and starve every other
/// request. The window is coarse on purpose — it bounds cost, it is not a quota.
const LOGIN_WINDOW_SECS: u64 = 60;
const LOGIN_MAX_PER_WINDOW: u32 = 10;
/// Above this many tracked IPs, drop stale windows before inserting more.
const LOGIN_TRACKED_IPS_MAX: usize = 4096;

#[derive(Default)]
pub struct LoginRateLimiter {
    windows: Mutex<HashMap<IpAddr, (u64, u32)>>,
}

impl LoginRateLimiter {
    /// Returns false when `ip` has spent its allowance for the current window.
    fn allow(&self, ip: IpAddr) -> bool {
        let now = auth::now_unix();
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());

        if windows.len() > LOGIN_TRACKED_IPS_MAX {
            windows.retain(|_, (start, _)| now.saturating_sub(*start) < LOGIN_WINDOW_SECS);
        }

        let entry = windows.entry(ip).or_insert((now, 0));
        if now.saturating_sub(entry.0) >= LOGIN_WINDOW_SECS {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= LOGIN_MAX_PER_WINDOW
    }
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AuthRouteState {
    pub db_path: PathBuf,
    pub private_pem: Arc<Vec<u8>>,
    pub public_pem: Arc<Vec<u8>>,
    pub access_ttl_secs: u64,
    pub refresh_ttl_secs: u64,
    /// Mark cookies `Secure`. Only when clients actually reach koan over HTTPS —
    /// a browser discards a `Secure` cookie delivered over plain `http://`, so
    /// setting this on a LAN deployment silently breaks cookie auth entirely.
    pub cookie_secure: bool,
    pub login_limiter: Arc<LoginRateLimiter>,
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
}

/// Read the refresh token from the request body, falling back to the cookie so a
/// browser client never has to keep one in script-reachable storage.
fn refresh_token_from(body: Option<&str>, headers: &axum::http::HeaderMap) -> Option<String> {
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
async fn login_rate_limit(
    State(state): State<AuthRouteState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

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

impl AuthRouteState {
    fn open_db(&self) -> Result<Database, (StatusCode, String)> {
        Database::open(&self.db_path).map_err(|e| {
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
    axum::Router::new()
        .route(
            "/auth/login",
            post(login).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                login_rate_limit,
            )),
        )
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        // These routes are unauthenticated by definition and the work behind
        // them is deliberately expensive, so they get their own ceiling rather
        // than sharing the GraphQL one.
        .layer(tower::limit::ConcurrencyLimitLayer::new(2))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn login(State(state): State<AuthRouteState>, Json(req): Json<LoginRequest>) -> Response {
    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // Look up user.
    let user = match auth_queries::get_user_by_username(&db.conn, &req.username) {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(MessageResponse {
                    message: "invalid username or password".into(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("auth login db error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    // Argon2 blocks for milliseconds at a time; on the async workers that stalls
    // every other request the server is handling.
    let hash = user.password_hash.clone();
    let password = req.password.clone();
    let verified = tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false);

    if !verified {
        return (
            StatusCode::UNAUTHORIZED,
            Json(MessageResponse {
                message: "invalid username or password".into(),
            }),
        )
            .into_response();
    }

    // Mint access token.
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
            return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
        }
    };

    // Create refresh token.
    let refresh_token_id = match auth::random_token() {
        Ok(t) => t,
        Err(e) => {
            log::error!("auth refresh token generation error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
        }
    };
    let refresh_expires = auth::now_unix() as i64 + state.refresh_ttl_secs as i64;
    if let Err(e) =
        auth_queries::store_refresh_token(&db.conn, &refresh_token_id, user.id, refresh_expires)
    {
        log::error!("auth store refresh token error: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
    }

    // Housekeeping: clean up expired tokens on login (non-blocking).
    let _ = auth_queries::cleanup_expired_tokens(&db.conn);

    let cookies = [
        (SET_COOKIE, state.access_cookie(&access_token)),
        (SET_COOKIE, state.refresh_cookie(&refresh_token_id)),
    ];

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

async fn refresh(
    State(state): State<AuthRouteState>,
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

    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // Atomically consume (validate + revoke) the refresh token in a single
    // statement to prevent TOCTOU races during token rotation.
    let token = match auth_queries::consume_refresh_token(&db.conn, &supplied) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(MessageResponse {
                    message: "invalid or expired refresh token".into(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("auth refresh db error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    // Look up the user.
    let user = match auth_queries::get_user_by_id(&db.conn, token.user_id) {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(MessageResponse {
                    message: "user not found".into(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("auth refresh user lookup error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    // Mint new access token.
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
            return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
        }
    };

    // Issue new refresh token.
    let new_refresh_id = match auth::random_token() {
        Ok(t) => t,
        Err(e) => {
            log::error!("auth refresh token generation error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
        }
    };
    let refresh_expires = auth::now_unix() as i64 + state.refresh_ttl_secs as i64;
    if let Err(e) =
        auth_queries::store_refresh_token(&db.conn, &new_refresh_id, user.id, refresh_expires)
    {
        log::error!("auth store refresh token error: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
    }

    let cookies = [
        (SET_COOKIE, state.access_cookie(&access_token)),
        (SET_COOKIE, state.refresh_cookie(&new_refresh_id)),
    ];

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
    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    let supplied = body.and_then(|Json(req)| req.refresh_token);
    if let Some(token) = refresh_token_from(supplied.as_deref(), &headers) {
        let _ = auth_queries::revoke_refresh_token(&db.conn, &token);
    }

    let cookies = [
        (SET_COOKIE, state.cookie("koan_access", "", "/", 0)),
        (
            SET_COOKIE,
            state.cookie(REFRESH_COOKIE, "", REFRESH_COOKIE_PATH, 0),
        ),
    ];

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
        let limiter = LoginRateLimiter::default();
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        for _ in 0..LOGIN_MAX_PER_WINDOW {
            assert!(limiter.allow(ip));
        }
        assert!(!limiter.allow(ip));

        // Other callers are unaffected.
        assert!(limiter.allow("10.0.0.6".parse().unwrap()));
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
            db_path: PathBuf::from("/nonexistent"),
            private_pem: Arc::new(Vec::new()),
            public_pem: Arc::new(Vec::new()),
            access_ttl_secs: 900,
            refresh_ttl_secs: 60,
            cookie_secure,
            login_limiter: Arc::new(LoginRateLimiter::default()),
        };

        let plain = state(false).access_cookie("tok");
        assert!(plain.contains("SameSite=Lax"));
        assert!(plain.contains("HttpOnly"));
        assert!(!plain.contains("Secure"));

        assert!(state(true).access_cookie("tok").contains("; Secure"));

        // The refresh cookie never rides along on an API call.
        let refresh = state(false).refresh_cookie("tok");
        assert!(refresh.contains("Path=/auth/refresh"));
        assert!(refresh.contains("HttpOnly"));
    }
}

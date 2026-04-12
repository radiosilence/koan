//! Auth HTTP routes: login, refresh, logout.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use koan_core::auth;
use koan_core::db::connection::Database;
use koan_core::db::queries::auth as auth_queries;

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
}

impl AuthRouteState {
    fn open_db(&self) -> Result<Database, (StatusCode, String)> {
        Database::open(&self.db_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")))
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

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: String,
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
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
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

    // Verify password.
    if auth::verify_password(&req.password, &user.password_hash).is_err() {
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
    let refresh_token_id = Uuid::now_v7().to_string();
    let refresh_expires = auth::now_unix() as i64 + state.refresh_ttl_secs as i64;
    if let Err(e) =
        auth_queries::store_refresh_token(&db.conn, &refresh_token_id, user.id, refresh_expires)
    {
        log::error!("auth store refresh token error: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
    }

    // Housekeeping: clean up expired tokens on login (non-blocking).
    let _ = auth_queries::cleanup_expired_tokens(&db.conn);

    let resp = LoginResponse {
        access_token,
        refresh_token: refresh_token_id,
        token_type: "Bearer".into(),
        expires_in: state.access_ttl_secs,
        user: UserInfo {
            id: user.id,
            username: user.username,
            role: user.role.as_str().into(),
        },
    };

    (StatusCode::OK, Json(resp)).into_response()
}

async fn refresh(State(state): State<AuthRouteState>, Json(req): Json<RefreshRequest>) -> Response {
    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // Atomically consume (validate + revoke) the refresh token in a single
    // statement to prevent TOCTOU races during token rotation.
    let token = match auth_queries::consume_refresh_token(&db.conn, &req.refresh_token) {
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
    let new_refresh_id = Uuid::now_v7().to_string();
    let refresh_expires = auth::now_unix() as i64 + state.refresh_ttl_secs as i64;
    if let Err(e) =
        auth_queries::store_refresh_token(&db.conn, &new_refresh_id, user.id, refresh_expires)
    {
        log::error!("auth store refresh token error: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "token error").into_response();
    }

    let resp = RefreshResponse {
        access_token,
        refresh_token: new_refresh_id,
        token_type: "Bearer".into(),
        expires_in: state.access_ttl_secs,
    };

    (StatusCode::OK, Json(resp)).into_response()
}

async fn logout(State(state): State<AuthRouteState>, Json(req): Json<LogoutRequest>) -> Response {
    let db = match state.open_db() {
        Ok(db) => db,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    let _ = auth_queries::revoke_refresh_token(&db.conn, &req.refresh_token);

    (
        StatusCode::OK,
        Json(MessageResponse {
            message: "logged out".into(),
        }),
    )
        .into_response()
}

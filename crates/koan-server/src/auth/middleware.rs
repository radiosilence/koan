//! Axum middleware for JWT authentication.
//!
//! Extracts the `Authorization: Bearer <token>` header, validates the JWT,
//! and injects `AuthUser` into request extensions.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use koan_core::auth::{self, Role};

use super::AuthUser;

/// Shared state for the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    /// Ed25519 public key PEM for JWT verification.
    pub public_pem: Arc<Vec<u8>>,
    /// Whether auth is enforced.
    pub auth_enabled: bool,
    /// Process-scoped introspection key. Bypasses auth when matched.
    /// Generated randomly on server start, dies with the process.
    pub introspection_key: Option<Arc<String>>,
}

/// Axum middleware: validate JWT and inject `AuthUser`.
///
/// When `auth_enabled = false`, injects anonymous admin and passes through.
/// When `auth_enabled = true`, requires valid Bearer token.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    if !state.auth_enabled {
        request.extensions_mut().insert(AuthUser::anonymous_admin());
        return next.run(request).await;
    }

    // Check for introspection key (playground bypass).
    if let Some(ref expected_key) = state.introspection_key
        && let Some(provided) = request
            .headers()
            .get("X-Introspection-Key")
            .and_then(|v| v.to_str().ok())
        && provided == expected_key.as_str()
    {
        request.extensions_mut().insert(AuthUser::anonymous_admin());
        return next.run(request).await;
    }

    // Priority: 1. koan_access cookie  2. Authorization: Bearer
    //           3. ?token= query param
    let token = request
        .headers()
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .find_map(|c| c.trim().strip_prefix("koan_access=").map(String::from))
        })
        .or_else(|| {
            request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(String::from)
        })
        .or_else(|| {
            // Fall back to ?token= query parameter (for playground URLs).
            request.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|pair| pair.strip_prefix("token=").map(String::from))
            })
        });

    let Some(token) = token else {
        return (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", "Bearer")],
            "missing or invalid Authorization header",
        )
            .into_response();
    };

    match auth::validate_access_token(&state.public_pem, &token) {
        Ok(claims) => {
            let role = claims.role.parse().unwrap_or(Role::Readonly);
            let user = AuthUser {
                user_id: claims.sub,
                username: claims.username,
                role,
            };
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", "Bearer")],
            "invalid or expired token",
        )
            .into_response(),
    }
}

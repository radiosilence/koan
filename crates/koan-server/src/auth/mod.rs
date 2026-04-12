//! Authentication layer for the koan server.
//!
//! When `auth_enabled = true`:
//!   - All GraphQL/Subsonic requests must carry a valid JWT in `Authorization: Bearer <token>`
//!   - Auth routes (/auth/login, /auth/refresh, /auth/logout) are always accessible
//!
//! When `auth_enabled = false` (default):
//!   - All requests are treated as admin — no auth required. Same behavior as before this feature.

pub mod middleware;
pub mod routes;

use koan_core::auth::Role;

/// Authenticated user context injected into request extensions and GraphQL context.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
    pub role: Role,
}

impl AuthUser {
    /// Anonymous admin user for when auth is disabled.
    pub fn anonymous_admin() -> Self {
        Self {
            user_id: 0,
            username: "anonymous".into(),
            role: Role::Admin,
        }
    }
}

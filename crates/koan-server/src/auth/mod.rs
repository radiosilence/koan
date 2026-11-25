//! Authentication layer for the koan server.
//!
//! When `auth_enabled = true`:
//!   - GraphQL requests must carry a JWT, an API key or the introspection key
//!     (see `middleware` for where each is read from); the web UI takes a JWT
//!     only as the `koan_access` cookie
//!   - Auth routes (/auth/login, /auth/refresh, /auth/logout, and the web
//!     UI's /auth/resume, /auth/renew and /auth/signout) are always accessible
//!
//! When `auth_enabled = false` (opt-in, not the default):
//!   - All requests are treated as admin — no auth required.

pub mod middleware;
pub mod password;
pub mod routes;

use std::sync::Arc;

use parking_lot::RwLock;

use koan_core::auth::{Claims, Role};
use koan_core::db::pool::Pool;
use koan_core::db::queries::auth as auth_queries;

/// The server's signing keypair as PEM, private then public.
pub(crate) type Keypair = (Arc<Vec<u8>>, Arc<Vec<u8>>);

static SIGNING: RwLock<Option<Keypair>> = RwLock::new(None);

/// Set at each start to the keys sessions are signed with, so invite tokens
/// are signed and checked with the same pair on every path, whatever happens
/// to the files on disk while the server runs.
pub(crate) fn set_signing_keys(private: Arc<Vec<u8>>, public: Arc<Vec<u8>>) {
    *SIGNING.write() = Some((private, public));
}

/// The keys `set_signing_keys` was given; read from disk, once, where nothing
/// set them, as in tests.
pub(crate) fn signing_keys() -> Result<Keypair, koan_core::auth::AuthError> {
    if let Some(keys) = SIGNING.read().clone() {
        return Ok(keys);
    }
    let (private, public) = koan_core::auth::load_or_generate_keypair()?;
    Ok(SIGNING
        .write()
        .get_or_insert_with(|| (Arc::new(private), Arc::new(public)))
        .clone())
}

/// Authenticated user context injected into request extensions and GraphQL context.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
    pub role: Role,
}

/// The account a token names, as it stands now.
///
/// A token's claims hold for its whole lifetime, so taken at their word a role
/// change or a deletion would not reach GraphQL or the web UI until it
/// expired: time enough for a demoted admin to restore the role. `None` once
/// the account is gone, or when its id now belongs to another account.
pub(crate) async fn current_user(pool: &Arc<Pool>, claims: Claims) -> Option<AuthUser> {
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let db = pool.get().ok()?;
        let user = auth_queries::get_user_by_id(&db.conn, claims.sub).ok()??;
        (user.username == claims.username).then_some(AuthUser {
            user_id: user.id,
            username: user.username,
            role: user.role,
        })
    })
    .await
    .ok()
    .flatten()
}

/// The account an API key belongs to, as it stands now. `None` for a key
/// that is not live.
pub(crate) async fn key_user(pool: &Arc<Pool>, key: String) -> Option<AuthUser> {
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let db = pool.get().ok()?;
        let user =
            koan_core::db::queries::api_keys::authenticate_api_key(&db.conn, &key).ok()??;
        Some(AuthUser {
            user_id: user.id,
            username: user.username,
            role: user.role,
        })
    })
    .await
    .ok()
    .flatten()
}

/// What a socket opened with a credential is held under: see
/// `koan_auth::changes::Lease`.
pub(crate) use koan_auth::changes::Lease;

impl AuthUser {
    /// Anonymous admin user for when auth is disabled.
    pub fn anonymous_admin() -> Self {
        Self {
            user_id: 0,
            username: koan_core::auth::ANONYMOUS.into(),
            role: Role::Admin,
        }
    }
}

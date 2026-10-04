//! Authentication layer for the koan server.
//!
//! When `auth_enabled = true`:
//!   - GraphQL requests must carry a valid JWT (see `middleware` for where it is
//!     read from); the web UI takes it only as the `koan_access` cookie
//!   - Auth routes (/auth/login, /auth/refresh, /auth/logout) are always accessible
//!
//! When `auth_enabled = false` (opt-in, not the default):
//!   - All requests are treated as admin — no auth required.

pub mod middleware;
pub mod password;
pub mod routes;

use std::sync::{Arc, OnceLock};

use koan_core::auth::{Claims, Role};
use koan_core::db::pool::Pool;
use koan_core::db::queries::auth as auth_queries;

/// The server's signing keypair as PEM, private then public.
pub(crate) type Keypair = (Arc<Vec<u8>>, Arc<Vec<u8>>);

static SIGNING: OnceLock<Keypair> = OnceLock::new();

/// Set once at startup to the keys sessions are signed with, so invite tokens
/// are signed and checked with the same pair on every path, whatever happens
/// to the files on disk while the server runs.
pub(crate) fn set_signing_keys(private: Arc<Vec<u8>>, public: Arc<Vec<u8>>) {
    let _ = SIGNING.set((private, public));
}

/// The keys `set_signing_keys` was given; read from disk, once, where nothing
/// set them, as in tests.
pub(crate) fn signing_keys() -> Result<&'static Keypair, koan_core::auth::AuthError> {
    if let Some(keys) = SIGNING.get() {
        return Ok(keys);
    }
    let (private, public) = koan_core::auth::load_or_generate_keypair()?;
    Ok(SIGNING.get_or_init(|| (Arc::new(private), Arc::new(public))))
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

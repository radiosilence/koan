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

/// What a socket opened with a credential is held under: its account as of
/// `mark` (see `koan_core::auth::account_mark`), and for a token, when it
/// lapses.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lease {
    pub user_id: i64,
    pub mark: u64,
    /// Unix seconds.
    pub expires: Option<u64>,
}

impl Lease {
    /// Resolves when a socket held under this lease must close: its account
    /// changed, or its token lapsed. The client reconnects and authenticates
    /// again, so it holds no more than its credential now gives it.
    pub(crate) async fn ended(self) {
        let lapsed = async {
            match self.expires {
                Some(at) => {
                    let left = at.saturating_sub(koan_core::auth::now_unix());
                    tokio::time::sleep(std::time::Duration::from_secs(left)).await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = koan_core::auth::account_changed_since(self.user_id, self.mark) => {}
            _ = lapsed => {}
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_lease_ends_when_its_account_changes_and_no_other() {
        let lease = Lease {
            user_id: 9001,
            mark: koan_core::auth::account_mark(),
            expires: None,
        };
        koan_core::auth::account_changed(9002);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lease.ended())
                .await
                .is_err()
        );
        koan_core::auth::account_changed(9001);
        tokio::time::timeout(Duration::from_secs(1), lease.ended())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_lease_ends_when_its_token_lapses() {
        let lease = Lease {
            user_id: 9003,
            mark: koan_core::auth::account_mark(),
            expires: Some(koan_core::auth::now_unix()),
        };
        tokio::time::timeout(Duration::from_secs(1), lease.ended())
            .await
            .unwrap();
    }
}

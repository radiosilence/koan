//! Username and password checks: `koan-auth`'s verifier, over kōan's pool.
//!
//! One verifier serves every door a password comes through (Subsonic, the
//! JSON login and the web UI's form), so the ceiling on argon2 and the
//! per-username budget on failures hold across all of them. Subsonic token
//! auth (`t`/`s`) is not checked here: it needs the plaintext password, and
//! koan keeps only the hash.

use std::net::IpAddr;
use std::sync::Arc;

use koan_core::db::pool::Pool;
use koan_core::db::queries::auth::{self as auth_queries, UserRow};

#[cfg(test)]
pub(crate) use koan_auth::limit::FAILURES_PER_USERNAME_PER_MINUTE;
pub(crate) use koan_auth::limit::FailureLimiter;
pub use koan_auth::password::Refused;

pub struct PasswordVerifier {
    pool: Arc<Pool>,
    inner: koan_auth::password::Verifier,
}

impl PasswordVerifier {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self {
            pool,
            inner: koan_auth::password::Verifier::new(),
        }
    }

    /// Whether `username` has spent its failures for the minute, as far as a
    /// sign-in from `from` is concerned; never for a demo's account, whose
    /// password is public and which strangers would otherwise lock for
    /// everyone. The per-address limits still apply.
    pub(crate) fn spent(&self, username: &str, from: IpAddr) -> bool {
        !(crate::demo::enabled() && username == crate::demo::USERNAME)
            && self.inner.spent(username, from)
    }

    /// The account, as it stands, when the password is its.
    pub fn verify(&self, username: &str, password: &str) -> Result<UserRow, Refused> {
        let db = self.pool.get().map_err(|_| Refused::Wrong)?;
        let user =
            auth_queries::get_user_by_username(&db.conn, username).map_err(|_| Refused::Wrong)?;
        self.inner.verify(username, password, user)
    }
}

impl std::ops::Deref for PasswordVerifier {
    type Target = koan_auth::password::Verifier;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

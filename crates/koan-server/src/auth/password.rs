//! Username and password checks for transports that send them with every
//! request, such as Subsonic's `p=`.
//!
//! argon2 is deliberately slow, and a Subsonic client authenticates each call,
//! so a successful check is remembered for a while. The key is a digest of the
//! username, the password and the stored hash, so changing the password or
//! deleting the user ends it; the role is read afresh every time.
//!
//! Each successful check also seals the password for Subsonic token auth
//! (`koan_core::auth::seal_password`), so an account that has signed in once
//! by password can then use clients that only speak `t`/`s`.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use koan_core::auth::{self, Role};
use koan_core::db::pool::Pool;
use koan_core::db::queries::auth as auth_queries;
use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

const REMEMBER: Duration = Duration::from_secs(600);

pub struct PasswordVerifier {
    pool: Arc<Pool>,
    verified: Mutex<LruCache<[u8; 32], Instant>>,
    /// Seals passwords for token auth; `None` if it could not be loaded, which
    /// leaves password auth working and token auth refused.
    sealing: Option<[u8; 32]>,
}

impl PasswordVerifier {
    pub fn new(pool: Arc<Pool>) -> Self {
        let sealing = auth::subsonic_key()
            .inspect_err(|e| log::warn!("Subsonic token auth for accounts is off: {e}"))
            .ok();
        Self::with_key(pool, sealing)
    }

    pub fn with_key(pool: Arc<Pool>, sealing: Option<[u8; 32]>) -> Self {
        Self {
            pool,
            verified: Mutex::new(LruCache::new(NonZeroUsize::new(256).expect("non-zero"))),
            sealing,
        }
    }

    /// The user's id and role when `token` is `md5(password + salt)` for
    /// their password. Needs the sealed copy a password sign-in leaves; the opened
    /// password is then checked like any other, so a stale copy fails.
    pub fn verify_token(&self, username: &str, token: &str, salt: &str) -> Option<(i64, Role)> {
        use subtle::ConstantTimeEq;
        let key = self.sealing.as_ref()?;
        let sealed =
            auth_queries::sealed_password(&self.pool.get().ok()?.conn, username).ok()??;
        let password = auth::open_password(key, username, &sealed)?;
        let expected = format!("{:x}", md5::compute(format!("{password}{salt}")));
        if !bool::from(
            token
                .to_ascii_lowercase()
                .as_bytes()
                .ct_eq(expected.as_bytes()),
        ) {
            return None;
        }
        self.verify(username, &password)
    }

    /// Whether token auth could work for this user: a key and a sealed copy.
    pub fn has_sealed(&self, username: &str) -> bool {
        self.sealing.is_some()
            && self
                .pool
                .get()
                .ok()
                .and_then(|db| auth_queries::sealed_password(&db.conn, username).ok())
                .flatten()
                .is_some()
    }

    /// The user's id and role when the password is theirs.
    pub fn verify(&self, username: &str, password: &str) -> Option<(i64, Role)> {
        let db = self.pool.get().ok()?;
        let Some(user) = auth_queries::get_user_by_username(&db.conn, username).ok()? else {
            // Pay for a verify anyway, so response time doesn't say which
            // usernames exist.
            let _ = auth::verify_password(password, super::routes::dummy_password_hash());
            return None;
        };
        let key: [u8; 32] = Sha256::new()
            .chain_update(username)
            .chain_update([0])
            .chain_update(password)
            .chain_update([0])
            .chain_update(&user.password_hash)
            .finalize()
            .into();
        let fresh = self
            .verified
            .lock()
            .get(&key)
            .is_some_and(|at| at.elapsed() < REMEMBER);
        if !fresh {
            auth::verify_password(password, &user.password_hash).ok()?;
            self.verified.lock().put(key, Instant::now());
            if let Some(k) = &self.sealing
                && let Ok(sealed) = auth::seal_password(k, username, password)
            {
                let _ = auth_queries::set_sealed_password(&db.conn, username, &sealed);
            }
        }
        Some((user.id, user.role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::db::connection::Database;

    fn verifier() -> (PasswordVerifier, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(&path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();
        auth_queries::create_user(&db.conn, "mate", "hunter22", Role::Readonly).unwrap();
        (
            PasswordVerifier::with_key(Arc::new(Pool::new(path)), Some([7; 32])),
            dir,
        )
    }

    #[test]
    fn right_password_gives_the_users_role() {
        let (v, _dir) = verifier();
        assert_eq!(v.verify("mate", "hunter22"), Some((1, Role::Readonly)));
        // Remembered, and still answered from the database's role.
        assert_eq!(v.verify("mate", "hunter22"), Some((1, Role::Readonly)));
    }

    #[test]
    fn token_auth_works_once_a_password_sign_in_sealed_it() {
        let (v, _dir) = verifier();
        let token = |pw: &str, salt: &str| format!("{:x}", md5::compute(format!("{pw}{salt}")));
        assert_eq!(
            v.verify_token("mate", &token("hunter22", "abc"), "abc"),
            None
        );
        assert!(!v.has_sealed("mate"));
        v.verify("mate", "hunter22").unwrap();
        assert!(v.has_sealed("mate"));
        assert_eq!(
            v.verify_token("mate", &token("hunter22", "abc"), "abc"),
            Some((1, Role::Readonly))
        );
        assert_eq!(
            v.verify_token("mate", &token("hunter2", "abc"), "abc"),
            None
        );
        assert_eq!(
            v.verify_token("nobody", &token("hunter22", "abc"), "abc"),
            None
        );
    }

    #[test]
    fn a_password_changed_elsewhere_makes_the_sealed_copy_fail() {
        let (v, dir) = verifier();
        v.verify("mate", "hunter22").unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        auth_queries::update_password(&db.conn, "mate", "correct horse").unwrap();
        let token = format!("{:x}", md5::compute("hunter22salt"));
        assert_eq!(v.verify_token("mate", &token, "salt"), None);
    }

    #[test]
    fn a_sealed_password_opens_only_for_its_user_and_key() {
        let sealed = auth::seal_password(&[1; 32], "mate", "hunter22").unwrap();
        assert_eq!(
            auth::open_password(&[1; 32], "mate", &sealed).as_deref(),
            Some("hunter22")
        );
        assert_eq!(auth::open_password(&[1; 32], "owner", &sealed), None);
        assert_eq!(auth::open_password(&[2; 32], "mate", &sealed), None);
    }

    #[test]
    fn wrong_password_or_unknown_user_is_refused() {
        let (v, _dir) = verifier();
        assert_eq!(v.verify("mate", "hunter2"), None);
        assert_eq!(v.verify("nobody", "hunter22"), None);
    }

    #[test]
    fn a_changed_password_forgets_the_old_one() {
        let (v, dir) = verifier();
        assert!(v.verify("mate", "hunter22").is_some());
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        auth_queries::update_password(&db.conn, "mate", "correct horse").unwrap();
        assert_eq!(v.verify("mate", "hunter22"), None);
        assert_eq!(v.verify("mate", "correct horse"), Some((1, Role::Readonly)));
    }
}

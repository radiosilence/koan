//! Username and password checks for transports that send them with every
//! request, such as Subsonic's `p=`.
//!
//! argon2 is deliberately slow, and a Subsonic client authenticates each call,
//! so a successful check is remembered for a while. The key is a digest of the
//! username, the password and the stored hash, so changing the password or
//! deleting the user ends it; the role is read afresh every time.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use koan_core::auth::{self, Role};
use koan_core::db::connection::Database;
use koan_core::db::queries::auth as auth_queries;
use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

const REMEMBER: Duration = Duration::from_secs(600);

pub struct PasswordVerifier {
    db_path: PathBuf,
    verified: Mutex<LruCache<[u8; 32], Instant>>,
}

impl PasswordVerifier {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            verified: Mutex::new(LruCache::new(NonZeroUsize::new(256).expect("non-zero"))),
        }
    }

    /// The user's role when the password is theirs.
    pub fn verify(&self, username: &str, password: &str) -> Option<Role> {
        let db = Database::open(&self.db_path).ok()?;
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
        }
        Some(user.role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier() -> (PasswordVerifier, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(&path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();
        auth_queries::create_user(&db.conn, "mate", "hunter22", Role::Readonly).unwrap();
        (PasswordVerifier::new(path), dir)
    }

    #[test]
    fn right_password_gives_the_users_role() {
        let (v, _dir) = verifier();
        assert_eq!(v.verify("mate", "hunter22"), Some(Role::Readonly));
        // Remembered, and still answered from the database's role.
        assert_eq!(v.verify("mate", "hunter22"), Some(Role::Readonly));
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
        assert_eq!(v.verify("mate", "correct horse"), Some(Role::Readonly));
    }
}

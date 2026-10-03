//! Username and password checks for transports that send them with every
//! request, such as Subsonic's `p=`.
//!
//! argon2 is deliberately slow, and a Subsonic client authenticates each call,
//! so a successful check is remembered for a while. The key is a digest of the
//! username, the password and the stored hash, so changing the password or
//! deleting the user ends it; the role is read afresh every time.
//!
//! A check that misses the cache runs argon2, which costs ~19 MiB and a core
//! for tens of milliseconds, and anyone may ask for one — an unknown username
//! still pays, so response time does not say which exist. So only a few run at
//! once, and a request that finds them all busy is refused rather than queued.
//! Requests carrying the same credentials while one is being checked wait for
//! that check instead, which is what a client's burst of requests on first
//! contact looks like.
//!
//! Subsonic token auth (`t`/`s`) is not checked here: it needs the plaintext
//! password, and koan keeps only the hash.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use koan_core::auth::{self, Role};
use koan_core::db::pool::Pool;
use koan_core::db::queries::auth as auth_queries;
use lru::LruCache;
use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};

const REMEMBER: Duration = Duration::from_secs(600);

/// How long a request waits on another's check of the same credentials.
const WAIT_FOR_CHECK: Duration = Duration::from_secs(5);

/// argon2 checks allowed at once.
fn max_checks() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 8))
}

/// Why a password was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Not this account's password, or no such account.
    Wrong,
    /// Every argon2 slot was taken; nothing was checked.
    Busy,
}

/// A check in progress: its outcome once known, and a signal for those waiting.
#[derive(Default)]
struct Check {
    outcome: Mutex<Option<bool>>,
    done: Condvar,
}

pub struct PasswordVerifier {
    pool: Arc<Pool>,
    verified: Mutex<LruCache<[u8; 32], Instant>>,
    /// Checks running now, by the same key as `verified`.
    checking: Mutex<HashMap<[u8; 32], Arc<Check>>>,
    max_checks: usize,
}

impl PasswordVerifier {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self {
            pool,
            verified: Mutex::new(LruCache::new(NonZeroUsize::new(256).expect("non-zero"))),
            checking: Mutex::new(HashMap::new()),
            max_checks: max_checks(),
        }
    }

    /// The user's id and role when the password is theirs.
    pub fn verify(&self, username: &str, password: &str) -> Result<(i64, Role), Refused> {
        let db = self.pool.get().map_err(|_| Refused::Wrong)?;
        let user =
            auth_queries::get_user_by_username(&db.conn, username).map_err(|_| Refused::Wrong)?;
        // An unknown username is checked against the dummy hash, so response
        // time doesn't say which usernames exist.
        let hash = user.as_ref().map_or_else(
            || super::routes::dummy_password_hash(),
            |u| u.password_hash.as_str(),
        );
        let key: [u8; 32] = Sha256::new()
            .chain_update(username)
            .chain_update([0])
            .chain_update(password)
            .chain_update([0])
            .chain_update(hash)
            .finalize()
            .into();
        let fresh = self
            .verified
            .lock()
            .get(&key)
            .is_some_and(|at| at.elapsed() < REMEMBER);
        if !fresh {
            if !self.check(key, password, hash)? {
                return Err(Refused::Wrong);
            }
        }
        user.map(|u| (u.id, u.role)).ok_or(Refused::Wrong)
    }

    /// Run argon2 for `key`, or wait on the check already running for it.
    fn check(&self, key: [u8; 32], password: &str, hash: &str) -> Result<bool, Refused> {
        let (check, running) = {
            let mut checking = self.checking.lock();
            match checking.get(&key) {
                Some(check) => (check.clone(), true),
                None if checking.len() >= self.max_checks => return Err(Refused::Busy),
                None => {
                    let check = Arc::new(Check::default());
                    checking.insert(key, check.clone());
                    (check, false)
                }
            }
        };
        if running {
            let deadline = Instant::now() + WAIT_FOR_CHECK;
            let mut outcome = check.outcome.lock();
            while outcome.is_none() && !check.done.wait_until(&mut outcome, deadline).timed_out() {}
            return outcome.ok_or(Refused::Busy);
        }
        let ok = auth::verify_password(password, hash).is_ok();
        if ok {
            self.verified.lock().put(key, Instant::now());
        }
        *check.outcome.lock() = Some(ok);
        check.done.notify_all();
        self.checking.lock().remove(&key);
        Ok(ok)
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
        (PasswordVerifier::new(Arc::new(Pool::new(path))), dir)
    }

    #[test]
    fn right_password_gives_the_users_role() {
        let (v, _dir) = verifier();
        assert_eq!(v.verify("mate", "hunter22"), Ok((1, Role::Readonly)));
        // Remembered, and still answered from the database's role.
        assert_eq!(v.verify("mate", "hunter22"), Ok((1, Role::Readonly)));
    }

    #[test]
    fn wrong_password_or_unknown_user_is_refused() {
        let (v, _dir) = verifier();
        assert_eq!(v.verify("mate", "hunter2"), Err(Refused::Wrong));
        assert_eq!(v.verify("nobody", "hunter22"), Err(Refused::Wrong));
    }

    #[test]
    fn checks_beyond_the_ceiling_are_refused_without_running() {
        let (mut v, _dir) = verifier();
        v.max_checks = 1;
        assert!(v.verify("mate", "hunter22").is_ok());
        v.checking.lock().insert([0; 32], Arc::default());
        assert_eq!(v.verify("nobody", "guess"), Err(Refused::Busy));
        assert_eq!(v.verify("mate", "hunter2"), Err(Refused::Busy));
        // A remembered sign-in needs no check, so it still works.
        assert_eq!(v.verify("mate", "hunter22"), Ok((1, Role::Readonly)));
    }

    #[test]
    fn a_burst_of_one_sign_in_waits_for_a_single_check() {
        let (mut v, _dir) = verifier();
        v.max_checks = 1;
        let v = Arc::new(v);
        let burst: Vec<_> = (0..8)
            .map(|_| {
                let v = v.clone();
                std::thread::spawn(move || v.verify("mate", "hunter22"))
            })
            .collect();
        for t in burst {
            assert_eq!(t.join().unwrap(), Ok((1, Role::Readonly)));
        }
    }

    #[test]
    fn a_changed_password_forgets_the_old_one() {
        let (v, dir) = verifier();
        assert!(v.verify("mate", "hunter22").is_ok());
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        auth_queries::update_password(&db.conn, "mate", "correct horse").unwrap();
        assert_eq!(v.verify("mate", "hunter22"), Err(Refused::Wrong));
        assert_eq!(v.verify("mate", "correct horse"), Ok((1, Role::Readonly)));
    }
}

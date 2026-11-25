//! Passwords: argon2id, and the verifier every door that takes one goes
//! through.
//!
//! argon2 is deliberately slow, and some clients send the password with every
//! request (Subsonic's `p=`), so a successful check is remembered for a while.
//! The key is a digest of the username, the password and the stored hash, so a
//! changed password or a deleted account ends it; the role is read afresh
//! every time.
//!
//! A check that misses the cache runs argon2, which costs ~19 MiB and a core
//! for tens of milliseconds, and anyone may ask for one: an unknown username
//! still pays, so response time does not say which exist. So only a few run at
//! once, and a request that finds them all busy is refused rather than queued.
//! Requests carrying the same credentials while one is being checked wait for
//! that check instead, which is what a client's burst of requests on first
//! contact looks like.
//!
//! One verifier serves every door a password comes through, so the ceiling on
//! argon2 and the per-username budget on failures hold across all of them.
//!
//! The budget stops a guesser with many addresses, and on its own would let
//! anyone with two keep an account locked out. So the networks an account has
//! recently signed in from are remembered, and the budget does not apply to
//! them: an outsider can spend it, but not for the account's own people.

use std::collections::HashMap;
use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lru::LruCache;
use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};

use crate::AuthError;
use crate::limit::{FAILURES_PER_USERNAME_PER_MINUTE, FailureLimiter, network};
use crate::users::UserRow;

/// Hash a password with argon2id and a random salt.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    use argon2::Argon2;
    use argon2::password_hash::PasswordHasher;

    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

/// Verify a password against an argon2 hash.
pub fn verify_password(password: &str, hash: &str) -> Result<(), AuthError> {
    use argon2::Argon2;
    use argon2::password_hash::PasswordVerifier;
    use argon2::password_hash::phc::PasswordHash;

    let parsed = PasswordHash::new(hash).map_err(|e| AuthError::Hash(e.to_string()))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AuthError::InvalidPassword)
}

/// A hash with the same parameters as a real one, to check passwords for
/// unknown usernames against, so they cost what a real check does.
pub fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password("koan-dummy-password").unwrap_or_default())
}

const REMEMBER: Duration = Duration::from_secs(600);

/// How long a request waits on another's check of the same credentials.
const WAIT_FOR_CHECK: Duration = Duration::from_secs(5);

/// How long a network an account signed in from is spared its spent budget.
/// A browser that keeps its session renews it as it does.
const KNOWN_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

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

pub struct Verifier {
    verified: Mutex<LruCache<[u8; 32], Instant>>,
    /// Checks running now, by the same key as `verified`.
    checking: Mutex<HashMap<[u8; 32], Arc<Check>>>,
    max_checks: usize,
    /// Failed sign-ins by username, whatever the address, counted by the
    /// doors that take passwords. Only those: API keys and other random
    /// secrets are not worth guessing, so a flood of wrong passwords for an
    /// account never locks out the clients signed in with one.
    failures: FailureLimiter<String>,
    /// When each username last signed in from each network.
    known: Mutex<LruCache<(String, IpAddr), Instant>>,
    /// Credentials that just signed in, for a throttle outside the handler to
    /// read back once the request is answered; see [`Verifier::passed`].
    passes: Mutex<LruCache<[u8; 32], ()>>,
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Verifier {
    pub fn new() -> Self {
        Self {
            verified: Mutex::new(LruCache::new(NonZeroUsize::new(256).expect("non-zero"))),
            checking: Mutex::new(HashMap::new()),
            max_checks: max_checks(),
            failures: FailureLimiter::new(FAILURES_PER_USERNAME_PER_MINUTE),
            known: Mutex::new(LruCache::new(NonZeroUsize::new(4096).expect("non-zero"))),
            passes: Mutex::new(LruCache::new(NonZeroUsize::new(256).expect("non-zero"))),
        }
    }

    /// Whether `username` has spent its failures for the minute, as far as a
    /// sign-in from `from` is concerned: never from a network it recently
    /// signed in from.
    pub fn spent(&self, username: &str, from: IpAddr) -> bool {
        self.failures.exhausted(&username.to_owned())
            && !self
                .known
                .lock()
                .get(&(username.to_owned(), network(from)))
                .is_some_and(|at| at.elapsed() < KNOWN_FOR)
    }

    /// Whether `username` has spent its failures for the minute, from any
    /// network: for a password check that is not a sign-in, which the sparing
    /// of known networks would otherwise let a guesser on one of them repeat
    /// without limit.
    pub fn exhausted(&self, username: &str) -> bool {
        self.failures.exhausted(&username.to_owned())
    }

    /// Count a failed sign-in for `username`.
    pub fn failed(&self, username: &str) {
        self.failures.record(username.to_owned());
    }

    /// Remember that `username` signed in from `from`'s network.
    pub fn signed_in(&self, username: &str, from: IpAddr) {
        self.known
            .lock()
            .put((username.to_owned(), network(from)), Instant::now());
    }

    /// Note that the credential `digest` signed in, for a throttle around the
    /// handler, which knows the client's address where the handler does not,
    /// to take back with [`Verifier::took_pass`].
    pub fn passed(&self, digest: [u8; 32]) {
        self.passes.lock().put(digest, ());
    }

    /// Whether `digest` signed in since it was last asked.
    pub fn took_pass(&self, digest: &[u8; 32]) -> bool {
        self.passes.lock().pop(digest).is_some()
    }

    /// The account, as it stands, when `password` is its password. `user` is
    /// the account called `username`, as the caller read it, or `None` for
    /// no such account, which is checked against [`dummy_hash`] so that the
    /// time taken does not say which usernames exist.
    ///
    /// Blocks: argon2 holds the thread, and so does waiting on another
    /// request's check of the same credentials.
    pub fn verify(
        &self,
        username: &str,
        password: &str,
        user: Option<UserRow>,
    ) -> Result<UserRow, Refused> {
        let hash = match &user {
            Some(u) => u.password_hash.as_str(),
            None => dummy_hash(),
        };
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
        if !fresh && !self.check(key, password, hash)? {
            return Err(Refused::Wrong);
        }
        user.ok_or(Refused::Wrong)
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
        let _slot = Slot {
            checking: &self.checking,
            key,
            check: check.clone(),
        };
        let ok = verify_password(password, hash).is_ok();
        if ok {
            self.verified.lock().put(key, Instant::now());
        }
        *check.outcome.lock() = Some(ok);
        Ok(ok)
    }
}

/// A running check's place among those allowed at once, given back however
/// the check ends. A check that panicked and kept its place would hold it for
/// good, and enough of them would refuse every password as busy.
struct Slot<'a> {
    checking: &'a Mutex<HashMap<[u8; 32], Arc<Check>>>,
    key: [u8; 32],
    check: Arc<Check>,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.checking.lock().remove(&self.key);
        self.check.done.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use crate::users;

    #[test]
    fn password_hash_and_verify() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2"));
        verify_password("hunter2", &hash).unwrap();
        assert!(matches!(
            verify_password("wrong", &hash),
            Err(AuthError::InvalidPassword)
        ));
    }

    /// Hashed by argon2 0.5. Every password kōan stored was, so this is what a
    /// dependency bump must never stop accepting.
    #[test]
    fn password_verify_hash_from_argon2_0_5() {
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$M/zwWdjjbwOvNCjzP+5t5A$pflXrbL1iOYPBlbgtK59wr2PkBaH7UVLKoBisvJ+Yfk";
        verify_password("correct horse", hash).unwrap();
        assert!(matches!(
            verify_password("wrong horse", hash),
            Err(AuthError::InvalidPassword)
        ));
    }

    fn accounts() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        users::create_user(&conn, "mate", "hunter22", Role::Readonly).unwrap();
        conn
    }

    fn check_as(
        v: &Verifier,
        conn: &rusqlite::Connection,
        username: &str,
        password: &str,
    ) -> Result<(i64, Role), Refused> {
        let user = users::get_user_by_username(conn, username).unwrap();
        v.verify(username, password, user).map(|u| (u.id, u.role))
    }

    #[test]
    fn right_password_gives_the_users_role() {
        let (v, conn) = (Verifier::new(), accounts());
        assert_eq!(
            check_as(&v, &conn, "mate", "hunter22"),
            Ok((1, Role::Readonly))
        );
        // Remembered, and still answered from the account's role.
        assert_eq!(
            check_as(&v, &conn, "mate", "hunter22"),
            Ok((1, Role::Readonly))
        );
    }

    #[test]
    fn wrong_password_or_unknown_user_is_refused() {
        let (v, conn) = (Verifier::new(), accounts());
        assert_eq!(check_as(&v, &conn, "mate", "hunter2"), Err(Refused::Wrong));
        assert_eq!(
            check_as(&v, &conn, "nobody", "hunter22"),
            Err(Refused::Wrong)
        );
    }

    #[test]
    fn checks_beyond_the_ceiling_are_refused_without_running() {
        let (mut v, conn) = (Verifier::new(), accounts());
        v.max_checks = 1;
        assert!(check_as(&v, &conn, "mate", "hunter22").is_ok());
        v.checking.lock().insert([0; 32], Arc::default());
        assert_eq!(check_as(&v, &conn, "nobody", "guess"), Err(Refused::Busy));
        assert_eq!(check_as(&v, &conn, "mate", "hunter2"), Err(Refused::Busy));
        // A remembered sign-in needs no check, so it still works.
        assert_eq!(
            check_as(&v, &conn, "mate", "hunter22"),
            Ok((1, Role::Readonly))
        );
    }

    #[test]
    fn a_burst_of_one_sign_in_waits_for_a_single_check() {
        let mut v = Verifier::new();
        v.max_checks = 1;
        let v = Arc::new(v);
        let user = users::get_user_by_username(&accounts(), "mate").unwrap();
        let burst: Vec<_> = (0..8)
            .map(|_| {
                let (v, user) = (v.clone(), user.clone());
                std::thread::spawn(move || {
                    v.verify("mate", "hunter22", user).map(|u| (u.id, u.role))
                })
            })
            .collect();
        for t in burst {
            assert_eq!(t.join().unwrap(), Ok((1, Role::Readonly)));
        }
    }

    #[test]
    fn a_changed_password_forgets_the_old_one() {
        let (v, conn) = (Verifier::new(), accounts());
        assert!(check_as(&v, &conn, "mate", "hunter22").is_ok());
        users::update_password(&conn, "mate", "correct horse").unwrap();
        assert_eq!(check_as(&v, &conn, "mate", "hunter22"), Err(Refused::Wrong));
        assert_eq!(
            check_as(&v, &conn, "mate", "correct horse"),
            Ok((1, Role::Readonly))
        );
    }

    #[test]
    fn a_spent_budget_spares_the_accounts_own_networks() {
        let home: IpAddr = "198.51.100.7".parse().unwrap();
        let away: IpAddr = "203.0.113.9".parse().unwrap();
        let v = Verifier::new();
        v.signed_in("mate", home);
        for _ in 0..FAILURES_PER_USERNAME_PER_MINUTE {
            v.failed("mate");
        }
        assert!(v.spent("mate", away));
        assert!(!v.spent("mate", home));
        assert!(v.exhausted("mate"));
        assert!(!v.spent("someone else", away));
    }
}

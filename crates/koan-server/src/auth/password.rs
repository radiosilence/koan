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
//!
//! One verifier serves every door a password comes through — Subsonic, the
//! JSON login and the web UI's form — so the ceiling on argon2 and the
//! per-username budget on failures hold across all of them.
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

use koan_core::auth;
use koan_core::db::pool::Pool;
use koan_core::db::queries::auth::{self as auth_queries, UserRow};
use lru::LruCache;
use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};

const REMEMBER: Duration = Duration::from_secs(600);

/// How long a request waits on another's check of the same credentials.
const WAIT_FOR_CHECK: Duration = Duration::from_secs(5);

/// Failed password sign-ins allowed for one username in a minute, from every
/// address together. Per-address limits do not stop a guesser with many
/// addresses; this does, at a rate no person typing would reach.
pub(crate) const FAILURES_PER_USERNAME_PER_MINUTE: u32 = 60;
pub(crate) const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// How long a network an account signed in from is spared its spent budget.
/// A browser that keeps its session refreshes it every few minutes, which
/// renews this.
const KNOWN_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

/// Failures by key, in fixed windows of `FAILURE_WINDOW`.
pub(crate) struct FailureLimiter<K> {
    limit: u32,
    windows: Mutex<HashMap<K, (Instant, u32)>>,
}

impl<K: std::hash::Hash + Eq> FailureLimiter<K> {
    pub(crate) fn new(limit: u32) -> Self {
        Self {
            limit,
            windows: Default::default(),
        }
    }

    pub(crate) fn exhausted(&self, key: &K) -> bool {
        let windows = self.windows.lock();
        windows
            .get(key)
            .is_some_and(|(start, count)| start.elapsed() < FAILURE_WINDOW && *count >= self.limit)
    }

    pub(crate) fn record(&self, key: K) {
        let mut windows = self.windows.lock();
        if windows.len() > 4096 {
            windows.retain(|_, (start, _)| start.elapsed() < FAILURE_WINDOW);
        }
        let entry = windows.entry(key).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= FAILURE_WINDOW {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
    }
}

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
    /// Failed sign-ins by username, whatever the address, counted by the
    /// doors that take passwords. Only those: API keys and the Subsonic shared
    /// secret are random and not worth guessing, so a flood of wrong passwords
    /// for an account never locks out the apps signed in with either.
    failures: FailureLimiter<String>,
    /// When each username last signed in from each network (`routes::network`).
    known: Mutex<LruCache<(String, IpAddr), Instant>>,
    /// Subsonic credentials that just signed in, for the throttle to read back
    /// once the request is answered; see `passed`.
    passes: Mutex<LruCache<[u8; 32], ()>>,
}

impl PasswordVerifier {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self {
            pool,
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
    pub(crate) fn spent(&self, username: &str, from: IpAddr) -> bool {
        self.failures.exhausted(&username.to_owned())
            && !self
                .known
                .lock()
                .get(&(username.to_owned(), super::routes::network(from)))
                .is_some_and(|at| at.elapsed() < KNOWN_FOR)
    }

    /// Whether `username` has spent its failures for the minute, from any
    /// network: for a password check that is not a sign-in, which the sparing
    /// of known networks would otherwise let a guesser on one of them repeat
    /// without limit.
    pub(crate) fn exhausted(&self, username: &str) -> bool {
        self.failures.exhausted(&username.to_owned())
    }

    /// Count a failed sign-in for `username`.
    pub(crate) fn failed(&self, username: &str) {
        self.failures.record(username.to_owned());
    }

    /// Remember that `username` signed in from `from`'s network.
    pub(crate) fn signed_in(&self, username: &str, from: IpAddr) {
        self.known.lock().put(
            (username.to_owned(), super::routes::network(from)),
            Instant::now(),
        );
    }

    /// Note that the Subsonic credential `digest` signed in. The check runs
    /// inside the handler, which knows nothing of the client's address; the
    /// throttle around it does, and takes this back with `took_pass`.
    pub(crate) fn passed(&self, digest: [u8; 32]) {
        self.passes.lock().put(digest, ());
    }

    /// Whether `digest` signed in since it was last asked.
    pub(crate) fn took_pass(&self, digest: &[u8; 32]) -> bool {
        self.passes.lock().pop(digest).is_some()
    }

    /// The account, as it stands, when the password is its.
    pub fn verify(&self, username: &str, password: &str) -> Result<UserRow, Refused> {
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
    use koan_core::auth::Role;
    use koan_core::db::connection::Database;

    impl PasswordVerifier {
        fn check_as(&self, username: &str, password: &str) -> Result<(i64, Role), Refused> {
            self.verify(username, password).map(|u| (u.id, u.role))
        }
    }

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
        assert_eq!(v.check_as("mate", "hunter22"), Ok((1, Role::Readonly)));
        // Remembered, and still answered from the database's role.
        assert_eq!(v.check_as("mate", "hunter22"), Ok((1, Role::Readonly)));
    }

    #[test]
    fn wrong_password_or_unknown_user_is_refused() {
        let (v, _dir) = verifier();
        assert_eq!(v.check_as("mate", "hunter2"), Err(Refused::Wrong));
        assert_eq!(v.check_as("nobody", "hunter22"), Err(Refused::Wrong));
    }

    #[test]
    fn checks_beyond_the_ceiling_are_refused_without_running() {
        let (mut v, _dir) = verifier();
        v.max_checks = 1;
        assert!(v.check_as("mate", "hunter22").is_ok());
        v.checking.lock().insert([0; 32], Arc::default());
        assert_eq!(v.check_as("nobody", "guess"), Err(Refused::Busy));
        assert_eq!(v.check_as("mate", "hunter2"), Err(Refused::Busy));
        // A remembered sign-in needs no check, so it still works.
        assert_eq!(v.check_as("mate", "hunter22"), Ok((1, Role::Readonly)));
    }

    #[test]
    fn a_burst_of_one_sign_in_waits_for_a_single_check() {
        let (mut v, _dir) = verifier();
        v.max_checks = 1;
        let v = Arc::new(v);
        let burst: Vec<_> = (0..8)
            .map(|_| {
                let v = v.clone();
                std::thread::spawn(move || v.check_as("mate", "hunter22"))
            })
            .collect();
        for t in burst {
            assert_eq!(t.join().unwrap(), Ok((1, Role::Readonly)));
        }
    }

    #[test]
    fn a_changed_password_forgets_the_old_one() {
        let (v, dir) = verifier();
        assert!(v.check_as("mate", "hunter22").is_ok());
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        auth_queries::update_password(&db.conn, "mate", "correct horse").unwrap();
        assert_eq!(v.check_as("mate", "hunter22"), Err(Refused::Wrong));
        assert_eq!(v.check_as("mate", "correct horse"), Ok((1, Role::Readonly)));
    }
}

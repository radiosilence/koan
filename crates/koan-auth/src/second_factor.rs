//! A second factor for password sign-in: an authenticator app ([`crate::totp`]),
//! security keys and passkeys ([`crate::webauthn`]), and single-use recovery
//! codes for when both are lost.
//!
//! Only a password asks for one. API keys, app passwords, sessions and
//! invites already prove possession of something handed out, so turning a
//! second factor on ends every one of them the account has, as a new
//! password does: nothing signed in before outlives it. The service signs
//! the browser that turned it on in again, since it has just shown both.
//!
//! An account has a second factor while it has a confirmed authenticator app
//! or a security key. Removing the last of them turns it off, which ends the
//! account's sessions too and drops its recovery codes.
//!
//! Authenticator app secrets are sealed as app passwords are, under a key
//! derived from the server's signing key ([`totp_key`]) and bound to the
//! account, so a new signing key retires them ([`forget_totp`]). Recovery
//! codes are random and kept as their SHA-256.
//!
//! A per-minute budget alone would let someone holding the password guess
//! codes for ever, a few a minute. So wrong answers in a row are counted per
//! account, and past [`MAX_MISSES`] codes and keys are refused until a right
//! one (a passkey still signs in) or an admin turns two-step sign-in off, as
//! NIST SP 800-63B §5.2.2 asks.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use lru::LruCache;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::AuthError;
use crate::changes::account_changed;
use crate::webauthn::{self, Assertion, NewCredential, RelyingParty, WebAuthnError};

/// How many recovery codes an account is given at a time.
pub const RECOVERY_CODES: usize = 10;

/// Wrong second factors in a row after which an account's codes and keys are
/// refused. NIST SP 800-63B §5.2.2's ceiling: against three codes a step,
/// the odds of a guess getting through before it are about one in 3,000.
pub const MAX_MISSES: i64 = 100;

/// How a second factor was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Right,
    Wrong,
    /// [`MAX_MISSES`] wrong in a row: nothing was checked.
    Locked,
}

/// How many security keys one account may register.
pub const MAX_KEYS: i64 = 20;

/// The longest name a security key may have.
pub const MAX_NAME: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum SecondFactorError {
    #[error("this account already has an authenticator app; remove it first")]
    AlreadyOn,
    #[error("start setting up the authenticator app again: nothing is waiting to be confirmed")]
    NothingPending,
    #[error("that code is not right; check the app's clock and try the next one")]
    WrongCode,
    #[error("an account may have at most {MAX_KEYS} security keys; remove one first")]
    TooManyKeys,
    #[error("that security key is already registered")]
    KeyTaken,
    #[error(transparent)]
    WebAuthn(#[from] WebAuthnError),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Auth(#[from] AuthError),
}

/// The key authenticator app secrets are sealed under, derived from the
/// server's signing key: a copy of the database alone does not open them.
pub fn totp_key(private_pem: &[u8]) -> [u8; 32] {
    use ring::hkdf;

    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"koan").extract(private_pem);
    let mut key = [0u8; 32];
    prk.expand(&[b"totp secrets v1"], hkdf::HKDF_SHA256)
        .and_then(|okm| okm.fill(&mut key))
        .expect("HKDF-SHA256 yields 32 bytes");
    key
}

/// A security key or passkey, without its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// What stands behind an account's password.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Factors {
    /// A confirmed authenticator app.
    pub totp: bool,
    pub keys: Vec<KeyRow>,
    /// Recovery codes not yet used.
    pub recovery_left: i64,
}

impl Factors {
    pub fn enabled(&self) -> bool {
        self.totp || !self.keys.is_empty()
    }
}

/// Whether `user_id` has a second factor.
pub fn enabled(conn: &Connection, user_id: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM totp WHERE user_id = ?1 AND confirmed = 1)
             OR EXISTS(SELECT 1 FROM security_keys WHERE user_id = ?1)",
    )?
    .query_row([user_id], |r| r.get(0))
}

/// The accounts that have a second factor.
pub fn enabled_users(conn: &Connection) -> rusqlite::Result<std::collections::HashSet<i64>> {
    conn.prepare_cached(
        "SELECT user_id FROM totp WHERE confirmed = 1
         UNION SELECT user_id FROM security_keys",
    )?
    .query_map([], |r| r.get(0))?
    .collect()
}

/// Everything standing behind `user_id`'s password.
pub fn factors(conn: &Connection, user_id: i64) -> rusqlite::Result<Factors> {
    let totp = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM totp WHERE user_id = ?1 AND confirmed = 1)",
        [user_id],
        |r| r.get(0),
    )?;
    let recovery_left = conn.query_row(
        "SELECT COUNT(*) FROM recovery_codes WHERE user_id = ?1 AND used_at IS NULL",
        [user_id],
        |r| r.get(0),
    )?;
    Ok(Factors {
        totp,
        keys: keys(conn, user_id)?,
        recovery_left,
    })
}

/// What turning a second factor on does, inside the transaction that adds
/// the first one: every session, API key and app password ends, and the
/// account is given recovery codes, which are returned to be shown once.
fn turned_on(conn: &Connection, user_id: i64) -> Result<Vec<String>, SecondFactorError> {
    crate::sessions::revoke_all_user_tokens(conn, user_id)?;
    crate::api_keys::revoke_user_api_keys(conn, user_id)?;
    crate::app_passwords::revoke_user_app_passwords(conn, user_id)?;
    replace_recovery_codes(conn, user_id)
}

/// A factor added: the recovery codes when it was the first, to show once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub recovery_codes: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Authenticator apps
// ---------------------------------------------------------------------------

/// Start setting up an authenticator app: a new secret, kept unconfirmed
/// until a code from it is given to [`confirm_totp`]. Starting again replaces
/// it. Returns the secret, for [`crate::totp::key_uri`] and to be typed.
pub fn begin_totp(
    conn: &Connection,
    key: &[u8; 32],
    user_id: i64,
) -> Result<Vec<u8>, SecondFactorError> {
    let mut secret = [0u8; crate::totp::SECRET_LEN];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut secret)
        .map_err(|_| AuthError::Hash("rng failure".into()))?;
    let sealed = crate::app_passwords::seal(key, user_id, &secret)?;
    let written = conn.execute(
        "INSERT INTO totp (user_id, sealed, confirmed, last_step, created_at)
         VALUES (?1, ?2, 0, 0, ?3)
         ON CONFLICT (user_id) DO UPDATE SET sealed = ?2, created_at = ?3
         WHERE confirmed = 0",
        params![user_id, sealed, crate::clock::unix_secs()],
    )?;
    if written == 0 {
        return Err(SecondFactorError::AlreadyOn);
    }
    Ok(secret.to_vec())
}

/// The secret [`begin_totp`] made and is waiting on, to show again.
pub fn pending_totp(
    conn: &Connection,
    key: &[u8; 32],
    user_id: i64,
) -> rusqlite::Result<Option<Vec<u8>>> {
    let sealed: Option<Vec<u8>> = conn
        .query_row(
            "SELECT sealed FROM totp WHERE user_id = ?1 AND confirmed = 0",
            [user_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(sealed.and_then(|s| crate::app_passwords::open(key, user_id, &s)))
}

/// Confirm the authenticator app [`begin_totp`] started with a code from it.
pub fn confirm_totp(
    conn: &Connection,
    key: &[u8; 32],
    user_id: i64,
    code: &str,
) -> Result<Added, SecondFactorError> {
    let added = crate::sql::atomically(conn, || {
        let secret = pending_totp(conn, key, user_id)?.ok_or(SecondFactorError::NothingPending)?;
        let step = crate::totp::matching_step(&secret, code, crate::clock::unix_secs())
            .ok_or(SecondFactorError::WrongCode)?;
        let first = !enabled(conn, user_id)?;
        conn.execute(
            "UPDATE totp SET confirmed = 1, last_step = ?2 WHERE user_id = ?1",
            params![user_id, step],
        )?;
        Ok::<_, SecondFactorError>(Added {
            recovery_codes: if first {
                Some(turned_on(conn, user_id)?)
            } else {
                None
            },
        })
    })?;
    account_changed(user_id);
    Ok(added)
}

/// Remove the account's authenticator app, confirmed or not. Whether there
/// was one. When it was the last factor, the second factor is off: see
/// [`turn_off`].
pub fn remove_totp(conn: &Connection, user_id: i64) -> rusqlite::Result<bool> {
    removing(conn, user_id, |conn| {
        conn.execute("DELETE FROM totp WHERE user_id = ?1", [user_id])
    })
}

/// Check `code` against the account's authenticator app, refusing a step
/// already used. The step is written in the statement that checks it, so two
/// requests carrying one code cannot both pass.
fn check_totp(
    conn: &Connection,
    key: &[u8; 32],
    user_id: i64,
    code: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let sealed: Option<Vec<u8>> = conn
        .query_row(
            "SELECT sealed FROM totp WHERE user_id = ?1 AND confirmed = 1",
            [user_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(sealed) = sealed else {
        return Ok(false);
    };
    let Some(secret) = crate::app_passwords::open(key, user_id, &sealed) else {
        log::warn!(
            "account {user_id}: its authenticator app's secret does not open under this signing key; \
             a regenerated keypair retires them (second_factor::forget_totp)"
        );
        return Ok(false);
    };
    let Some(step) = crate::totp::matching_step(&secret, code, now) else {
        return Ok(false);
    };
    let n = conn.execute(
        "UPDATE totp SET last_step = ?2 WHERE user_id = ?1 AND confirmed = 1 AND last_step < ?2",
        params![user_id, step],
    )?;
    Ok(n == 1)
}

// ---------------------------------------------------------------------------
// Security keys and passkeys
// ---------------------------------------------------------------------------

/// `name` as a security key's name: trimmed, control characters dropped, and
/// refused when that leaves nothing or more than [`MAX_NAME`] characters.
pub fn key_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    (!name.is_empty() && name.chars().count() <= MAX_NAME).then_some(name)
}

/// The opaque handle WebAuthn knows the account by: its id, which names
/// nobody outside this server.
pub fn user_handle(user_id: i64) -> Vec<u8> {
    user_id.to_be_bytes().to_vec()
}

/// Register a security key checked by [`webauthn::register`].
pub fn add_key(
    conn: &Connection,
    user_id: i64,
    name: &str,
    credential: &NewCredential,
) -> Result<Added, SecondFactorError> {
    let added = crate::sql::atomically(conn, || {
        let first = !enabled(conn, user_id)?;
        let inserted = conn.execute(
            "INSERT INTO security_keys (user_id, name, credential_id, public_key, sign_count, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6
             WHERE (SELECT COUNT(*) FROM security_keys WHERE user_id = ?1) < ?7
             ON CONFLICT (credential_id) DO NOTHING",
            params![
                user_id,
                name,
                credential.id,
                credential.public_key,
                credential.sign_count,
                crate::clock::unix_secs(),
                MAX_KEYS
            ],
        )?;
        if inserted == 0 {
            let taken: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM security_keys WHERE credential_id = ?1)",
                [&credential.id],
                |r| r.get(0),
            )?;
            return Err(if taken {
                SecondFactorError::KeyTaken
            } else {
                SecondFactorError::TooManyKeys
            });
        }
        Ok(Added {
            recovery_codes: if first {
                Some(turned_on(conn, user_id)?)
            } else {
                None
            },
        })
    })?;
    account_changed(user_id);
    Ok(added)
}

/// The account's security keys, oldest first.
pub fn keys(conn: &Connection, user_id: i64) -> rusqlite::Result<Vec<KeyRow>> {
    conn.prepare_cached(
        "SELECT id, name, created_at, last_used_at FROM security_keys
         WHERE user_id = ?1 ORDER BY id",
    )?
    .query_map([user_id], |r| {
        Ok(KeyRow {
            id: r.get(0)?,
            name: r.get(1)?,
            created_at: r.get(2)?,
            last_used_at: r.get(3)?,
        })
    })?
    .collect()
}

/// The credential IDs of the account's security keys, for WebAuthn's options.
pub fn key_ids(conn: &Connection, user_id: i64) -> rusqlite::Result<Vec<Vec<u8>>> {
    conn.prepare_cached("SELECT credential_id FROM security_keys WHERE user_id = ?1 ORDER BY id")?
        .query_map([user_id], |r| r.get(0))?
        .collect()
}

/// Remove one of the account's security keys. Whether one went. When it was
/// the last factor, the second factor is off: see [`turn_off`].
pub fn remove_key(conn: &Connection, user_id: i64, id: i64) -> rusqlite::Result<bool> {
    removing(conn, user_id, |conn| {
        conn.execute(
            "DELETE FROM security_keys WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
    })
}

/// A security key's answer to a sign-in's challenge, checked against the
/// account's key that made it, whose counter moves on. Counted toward
/// [`MAX_MISSES`] as a code is.
pub fn check_assertion(
    conn: &Connection,
    rp: &RelyingParty,
    user_id: i64,
    challenge: &[u8],
    assertion: &Assertion,
) -> rusqlite::Result<Checked> {
    counted(conn, user_id, |conn| {
        check_key(conn, rp, user_id, challenge, assertion)
    })
}

fn check_key(
    conn: &Connection,
    rp: &RelyingParty,
    user_id: i64,
    challenge: &[u8],
    assertion: &Assertion,
) -> rusqlite::Result<bool> {
    let Ok(credential_id) = webauthn::assertion_id(assertion) else {
        return Ok(false);
    };
    let row: Option<(i64, Vec<u8>, i64)> = conn
        .query_row(
            "SELECT id, public_key, sign_count FROM security_keys
             WHERE user_id = ?1 AND credential_id = ?2",
            params![user_id, credential_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, public_key, count)) = row else {
        return Ok(false);
    };
    // A discoverable credential names its account too: it must be this one.
    match webauthn::assertion_user_handle(assertion) {
        Ok(None) => {}
        Ok(Some(handle)) if handle == user_handle(user_id) => {}
        _ => return Ok(false),
    }
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    match webauthn::verify(rp, challenge, assertion, &public_key, count, false) {
        Ok(now) => {
            // Conditional, as the TOTP step is: two requests carrying one
            // assertion cannot both pass on a counting key.
            let n = conn.execute(
                "UPDATE security_keys SET sign_count = ?3, last_used_at = ?4
                 WHERE id = ?1 AND sign_count = ?2",
                params![id, count, now, crate::clock::unix_secs()],
            )?;
            Ok(n == 1)
        }
        Err(e) => {
            log::info!("security key {id}: assertion refused: {e}");
            Ok(false)
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery codes
// ---------------------------------------------------------------------------

/// A recovery code as it is compared: lower case, without dashes or spaces.
fn normal(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Give the account a new set of recovery codes, replacing any it had.
/// Returned to be shown once: only their digests are kept.
pub fn replace_recovery_codes(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<String>, SecondFactorError> {
    let codes = (0..RECOVERY_CODES)
        .map(|_| crate::secret::typeable(15, 5))
        .collect::<Result<Vec<_>, _>>()?;
    crate::sql::atomically(conn, || {
        conn.execute("DELETE FROM recovery_codes WHERE user_id = ?1", [user_id])?;
        let mut insert =
            conn.prepare_cached("INSERT INTO recovery_codes (user_id, code_hash) VALUES (?1, ?2)")?;
        for code in &codes {
            insert.execute(params![user_id, crate::secret::sha256_hex(&normal(code))])?;
        }
        Ok::<_, rusqlite::Error>(())
    })?;
    Ok(codes)
}

/// Spend one of the account's recovery codes.
fn spend_recovery_code(conn: &Connection, user_id: i64, code: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE recovery_codes SET used_at = ?3
         WHERE user_id = ?1 AND code_hash = ?2 AND used_at IS NULL",
        params![
            user_id,
            crate::secret::sha256_hex(&normal(code)),
            crate::clock::unix_secs()
        ],
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Signing in, and turning it off
// ---------------------------------------------------------------------------

/// What a person typed as their second factor: a code from the authenticator
/// app or, longer, a recovery code. Either is spent by checking it, and
/// counted toward [`MAX_MISSES`] when wrong.
pub fn check_code(
    conn: &Connection,
    key: &[u8; 32],
    user_id: i64,
    code: &str,
) -> rusqlite::Result<Checked> {
    counted(conn, user_id, |conn| {
        let compact: String = code
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        if compact.len() == crate::totp::DIGITS as usize {
            check_totp(conn, key, user_id, &compact, crate::clock::unix_secs())
        } else {
            spend_recovery_code(conn, user_id, &compact)
        }
    })
}

/// Run `check` unless the account is locked, and count what it found: a
/// right answer clears the count, a wrong one adds to it.
fn counted(
    conn: &Connection,
    user_id: i64,
    check: impl FnOnce(&Connection) -> rusqlite::Result<bool>,
) -> rusqlite::Result<Checked> {
    let misses: i64 = conn
        .query_row(
            "SELECT count FROM second_factor_misses WHERE user_id = ?1",
            [user_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if misses >= MAX_MISSES {
        return Ok(Checked::Locked);
    }
    if check(conn)? {
        clear_misses(conn, user_id)?;
        Ok(Checked::Right)
    } else {
        conn.execute(
            "INSERT INTO second_factor_misses (user_id, count) VALUES (?1, 1)
             ON CONFLICT (user_id) DO UPDATE SET count = count + 1",
            [user_id],
        )?;
        Ok(Checked::Wrong)
    }
}

fn clear_misses(conn: &Connection, user_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM second_factor_misses WHERE user_id = ?1",
        [user_id],
    )?;
    Ok(())
}

/// Sign in with a passkey alone: the account whose key made `assertion`, when
/// the key verified the person and the assertion's user handle is that
/// account's. A passkey is something held and, unlocked by PIN or biometrics,
/// someone present, so it needs no password beside it.
pub fn passkey_sign_in(
    conn: &Connection,
    rp: &RelyingParty,
    challenge: &[u8],
    assertion: &Assertion,
) -> rusqlite::Result<Option<i64>> {
    let (Ok(credential_id), Ok(Some(handle))) = (
        webauthn::assertion_id(assertion),
        webauthn::assertion_user_handle(assertion),
    ) else {
        return Ok(None);
    };
    let row: Option<(i64, i64, Vec<u8>, i64)> = conn
        .query_row(
            "SELECT id, user_id, public_key, sign_count FROM security_keys WHERE credential_id = ?1",
            [&credential_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((id, user_id, public_key, count)) = row else {
        return Ok(None);
    };
    if handle != user_handle(user_id) {
        return Ok(None);
    }
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    match webauthn::verify(rp, challenge, assertion, &public_key, count, true) {
        Ok(now) => {
            let n = conn.execute(
                "UPDATE security_keys SET sign_count = ?3, last_used_at = ?4
                 WHERE id = ?1 AND sign_count = ?2",
                params![id, count, now, crate::clock::unix_secs()],
            )?;
            // The account's own passkey: whoever was guessing its codes was
            // not its owner, who need not wait on an admin.
            if n == 1 {
                clear_misses(conn, user_id)?;
            }
            Ok((n == 1).then_some(user_id))
        }
        Err(e) => {
            log::info!("security key {id}: passkey sign-in refused: {e}");
            Ok(None)
        }
    }
}

/// Remove a factor with `delete`, and when none is left, turn the second
/// factor off: see [`turn_off`].
fn removing(
    conn: &Connection,
    user_id: i64,
    delete: impl FnOnce(&Connection) -> rusqlite::Result<usize>,
) -> rusqlite::Result<bool> {
    let removed = crate::sql::atomically(conn, || {
        let was = enabled(conn, user_id)?;
        let removed = delete(conn)? > 0;
        if was && !enabled(conn, user_id)? {
            ended(conn, user_id)?;
        }
        Ok::<_, rusqlite::Error>(removed)
    })?;
    if removed {
        account_changed(user_id);
    }
    Ok(removed)
}

fn ended(conn: &Connection, user_id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM recovery_codes WHERE user_id = ?1", [user_id])?;
    clear_misses(conn, user_id)?;
    crate::sessions::revoke_all_user_tokens(conn, user_id)?;
    Ok(())
}

/// Turn the account's second factor off: its authenticator app, keys and
/// recovery codes go, and so do its sessions, so that a browser signed in by
/// whoever had the lost device is signed out with it. For an admin helping
/// someone who lost theirs. Whether it had one.
pub fn turn_off(conn: &Connection, user_id: i64) -> rusqlite::Result<bool> {
    let had = crate::sql::atomically(conn, || {
        let had = enabled(conn, user_id)?;
        conn.execute("DELETE FROM totp WHERE user_id = ?1", [user_id])?;
        conn.execute("DELETE FROM security_keys WHERE user_id = ?1", [user_id])?;
        ended(conn, user_id)?;
        Ok::<_, rusqlite::Error>(had)
    })?;
    account_changed(user_id);
    Ok(had)
}

/// Drop every authenticator app's secret, for a server that has regenerated
/// its signing key, under which they were sealed and which they no longer
/// open. Accounts left with no second factor have it turned off, as
/// [`turn_off`] does; those with keys keep them. Returns the accounts that
/// had an authenticator app.
pub fn forget_totp(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let users = crate::sql::atomically(conn, || {
        let users: Vec<i64> = conn
            .prepare("SELECT user_id FROM totp WHERE confirmed = 1 ORDER BY user_id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        conn.execute("DELETE FROM totp", [])?;
        for &user_id in &users {
            if !enabled(conn, user_id)? {
                ended(conn, user_id)?;
            }
        }
        Ok::<_, rusqlite::Error>(users)
    })?;
    for &user_id in &users {
        account_changed(user_id);
    }
    Ok(users)
}

// ---------------------------------------------------------------------------
// Ceremonies in progress
// ---------------------------------------------------------------------------

/// How long a sign-in waits for its second factor, or a security key for its
/// registration.
pub const TICKET_TTL: Duration = Duration::from_secs(300);

/// Wrong answers one ticket takes before it is dropped and the password must
/// be given again.
const TICKET_TRIES: u8 = 5;

/// What a ticket is for, so that one cannot stand in for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A password has passed and the second factor is awaited.
    SignIn,
    /// A security key is being registered.
    Register,
    /// A passkey is asked to sign in on its own: no account yet.
    Passkey,
}

/// A ceremony in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub user_id: i64,
    pub username: String,
    pub purpose: Purpose,
    /// The WebAuthn challenge, random and used once.
    pub challenge: [u8; 32],
    expires: Instant,
    tries: u8,
}

/// Ceremonies in progress, in memory, by an unguessable ticket the client
/// holds. A server that restarts forgets them, and a person gives their
/// password again.
pub struct Tickets {
    open: Mutex<LruCache<String, Ticket>>,
}

impl Default for Tickets {
    fn default() -> Self {
        Self {
            open: Mutex::new(LruCache::new(NonZeroUsize::new(4096).expect("non-zero"))),
        }
    }
}

impl Tickets {
    /// Open a ticket for `user_id`: the ticket, and the ceremony with its
    /// challenge.
    pub fn open(
        &self,
        user_id: i64,
        username: &str,
        purpose: Purpose,
    ) -> Result<(String, Ticket), AuthError> {
        let id = crate::secret::random_token()?;
        let mut challenge = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut challenge)
            .map_err(|_| AuthError::Hash("rng failure".into()))?;
        let ticket = Ticket {
            user_id,
            username: username.to_owned(),
            purpose,
            challenge,
            expires: Instant::now() + TICKET_TTL,
            tries: 0,
        };
        self.open.lock().put(id.clone(), ticket.clone());
        Ok((id, ticket))
    }

    /// The ceremony `id` names, while it lasts and is for `purpose`.
    pub fn get(&self, id: &str, purpose: Purpose) -> Option<Ticket> {
        let mut open = self.open.lock();
        let ticket = open.get(id)?.clone();
        if ticket.expires <= Instant::now() {
            open.pop(id);
            return None;
        }
        (ticket.purpose == purpose).then_some(ticket)
    }

    /// End the ceremony `id`: its challenge is used.
    pub fn close(&self, id: &str) {
        self.open.lock().pop(id);
    }

    /// Take the ceremony `id` to answer, while it lasts and is for
    /// `purpose`: until it is given back with [`Tickets::missed_answer`],
    /// no other request can answer the same challenge, so one answer cannot
    /// pass twice.
    pub fn take(&self, id: &str, purpose: Purpose) -> Option<Ticket> {
        let mut open = self.open.lock();
        let ticket = open.pop(id)?;
        (ticket.expires > Instant::now() && ticket.purpose == purpose).then_some(ticket)
    }

    /// Give back a ceremony [`Tickets::take`] took and answered wrongly,
    /// counted against its tries.
    pub fn missed_answer(&self, id: &str, mut ticket: Ticket) {
        ticket.tries += 1;
        if ticket.tries < TICKET_TRIES {
            self.give_back(id, ticket);
        }
    }

    /// Give back a ceremony [`Tickets::take`] took and could not answer, for
    /// the server's own failure: the person's answer was not judged.
    pub fn give_back(&self, id: &str, ticket: Ticket) {
        if ticket.expires > Instant::now() {
            self.open.lock().put(id.to_owned(), ticket);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use crate::schema::test_db;
    use crate::users::create_user;
    use crate::webauthn::testing::Authenticator;

    fn code_now(secret: &[u8]) -> String {
        let step = crate::totp::step(crate::clock::unix_secs());
        format!("{:06}", crate::totp::hotp(secret, step as u64))
    }

    fn rp() -> RelyingParty {
        RelyingParty::from_url("https://music.example.org", "kōan").unwrap()
    }

    #[test]
    fn an_authenticator_app_is_on_once_confirmed() {
        let conn = &test_db();
        let key = totp_key(b"a signing key");
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let secret = begin_totp(conn, &key, ann).unwrap();
        assert!(!enabled(conn, ann).unwrap());
        assert_eq!(pending_totp(conn, &key, ann).unwrap().unwrap(), secret);
        assert!(matches!(
            confirm_totp(conn, &key, ann, "000000"),
            Err(SecondFactorError::WrongCode)
        ));
        let added = confirm_totp(conn, &key, ann, &code_now(&secret)).unwrap();
        assert_eq!(added.recovery_codes.unwrap().len(), RECOVERY_CODES);
        assert!(enabled(conn, ann).unwrap());
        assert!(matches!(
            begin_totp(conn, &key, ann),
            Err(SecondFactorError::AlreadyOn)
        ));
    }

    #[test]
    fn turning_it_on_ends_what_signed_in_before() {
        let conn = &test_db();
        let key = totp_key(b"a signing key");
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let future = crate::clock::unix_secs() + 3600;
        crate::sessions::store_refresh_token(conn, "browser", ann, future).unwrap();
        let (_, api_key) = crate::api_keys::create_api_key(conn, ann, "phone").unwrap();
        let sealing = crate::app_passwords::app_password_key(b"a signing key");
        crate::app_passwords::create_app_password(conn, &sealing, ann, "app").unwrap();

        let secret = begin_totp(conn, &key, ann).unwrap();
        confirm_totp(conn, &key, ann, &code_now(&secret)).unwrap();
        assert!(
            crate::sessions::get_valid_refresh_token(conn, "browser")
                .unwrap()
                .is_none()
        );
        assert!(
            crate::api_keys::authenticate_api_key(conn, &api_key)
                .unwrap()
                .is_none()
        );
        assert!(
            crate::app_passwords::list_app_passwords(conn, ann)
                .unwrap()
                .is_empty()
        );

        // A second factor added later ends nothing.
        crate::sessions::store_refresh_token(conn, "after", ann, future).unwrap();
        let mut device = Authenticator::new();
        let made = webauthn::register(&rp(), b"c", &device.create(&rp(), b"c")).unwrap();
        let added = add_key(conn, ann, "yubikey", &made).unwrap();
        assert_eq!(added.recovery_codes, None);
        assert!(
            crate::sessions::get_valid_refresh_token(conn, "after")
                .unwrap()
                .is_some()
        );
        let assertion = device.get(&rp(), b"sign in");
        assert_eq!(
            check_assertion(conn, &rp(), ann, b"sign in", &assertion).unwrap(),
            Checked::Right
        );
        // The same assertion again is refused: the counter has moved on.
        assert_eq!(
            check_assertion(conn, &rp(), ann, b"sign in", &assertion).unwrap(),
            Checked::Wrong
        );
    }

    #[test]
    fn a_code_is_used_once() {
        let conn = &test_db();
        let key = totp_key(b"a signing key");
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let secret = begin_totp(conn, &key, ann).unwrap();
        let now = crate::clock::unix_secs();
        // Confirmed with the step before, so now's code is still fresh.
        conn.execute(
            "UPDATE totp SET confirmed = 1, last_step = ?1",
            [crate::totp::step(now) - 1],
        )
        .unwrap();
        let code = code_now(&secret);
        assert_eq!(check_code(conn, &key, ann, &code).unwrap(), Checked::Right);
        assert_eq!(check_code(conn, &key, ann, &code).unwrap(), Checked::Wrong);
        // Another server's key does not open the secret.
        let other = totp_key(b"another key");
        assert_eq!(
            check_code(conn, &other, ann, &code).unwrap(),
            Checked::Wrong
        );
    }

    #[test]
    fn a_recovery_code_is_spent_once_however_it_is_typed() {
        let conn = &test_db();
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let key = totp_key(b"a signing key");
        let codes = replace_recovery_codes(conn, ann).unwrap();
        assert_eq!(codes[0].len(), 17);
        let typed = codes[0].replace('-', " ").to_uppercase();
        assert_eq!(check_code(conn, &key, ann, &typed).unwrap(), Checked::Right);
        assert_eq!(
            check_code(conn, &key, ann, &codes[0]).unwrap(),
            Checked::Wrong
        );
        assert_eq!(
            check_code(conn, &key, ann, &codes[1]).unwrap(),
            Checked::Right
        );
        assert_eq!(factors(conn, ann).unwrap().recovery_left, 8);
    }

    #[test]
    fn removing_the_last_factor_turns_it_off_and_ends_sessions() {
        let conn = &test_db();
        let key = totp_key(b"a signing key");
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let secret = begin_totp(conn, &key, ann).unwrap();
        confirm_totp(conn, &key, ann, &code_now(&secret)).unwrap();
        let made =
            webauthn::register(&rp(), b"c", &Authenticator::new().create(&rp(), b"c")).unwrap();
        add_key(conn, ann, "yubikey", &made).unwrap();
        let future = crate::clock::unix_secs() + 3600;
        crate::sessions::store_refresh_token(conn, "browser", ann, future).unwrap();

        assert!(remove_totp(conn, ann).unwrap());
        assert!(enabled(conn, ann).unwrap());
        assert!(
            crate::sessions::get_valid_refresh_token(conn, "browser")
                .unwrap()
                .is_some()
        );
        let id = keys(conn, ann).unwrap()[0].id;
        assert!(remove_key(conn, ann, id).unwrap());
        assert_eq!(factors(conn, ann).unwrap(), Factors::default());
        assert!(
            crate::sessions::get_valid_refresh_token(conn, "browser")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_key_registers_once_and_an_admin_can_turn_it_all_off() {
        let conn = &test_db();
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let bob = create_user(conn, "bob", "b password", Role::User).unwrap();
        let made =
            webauthn::register(&rp(), b"c", &Authenticator::new().create(&rp(), b"c")).unwrap();
        add_key(conn, ann, "yubikey", &made).unwrap();
        assert!(matches!(
            add_key(conn, bob, "same key", &made),
            Err(SecondFactorError::KeyTaken)
        ));
        assert!(turn_off(conn, ann).unwrap());
        assert!(!enabled(conn, ann).unwrap());
        assert!(!turn_off(conn, ann).unwrap());
    }

    #[test]
    fn a_passkey_signs_in_its_own_account_alone() {
        let conn = &test_db();
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let mut device = Authenticator::new();
        let made = webauthn::register(&rp(), b"c", &device.create(&rp(), b"c")).unwrap();
        add_key(conn, ann, "phone", &made).unwrap();

        // No user handle, the wrong one, or no verification: refused.
        device.verifies = true;
        let anonymous = device.get(&rp(), b"s");
        assert_eq!(
            passkey_sign_in(conn, &rp(), b"s", &anonymous).unwrap(),
            None
        );
        device.handle = Some(user_handle(ann + 1));
        let other = device.get(&rp(), b"s");
        assert_eq!(passkey_sign_in(conn, &rp(), b"s", &other).unwrap(), None);
        device.handle = Some(user_handle(ann));
        device.verifies = false;
        let unverified = device.get(&rp(), b"s");
        assert_eq!(
            passkey_sign_in(conn, &rp(), b"s", &unverified).unwrap(),
            None
        );

        device.verifies = true;
        let good = device.get(&rp(), b"s");
        assert_eq!(
            passkey_sign_in(conn, &rp(), b"s", &good).unwrap(),
            Some(ann)
        );
        assert_eq!(passkey_sign_in(conn, &rp(), b"s", &good).unwrap(), None);
    }

    #[test]
    fn wrong_answers_in_a_row_lock_codes_until_a_right_one_or_turning_it_off() {
        let conn = &test_db();
        let key = totp_key(b"a signing key");
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let codes = replace_recovery_codes(conn, ann).unwrap();
        for _ in 0..MAX_MISSES - 1 {
            assert_eq!(
                check_code(conn, &key, ann, "000000").unwrap(),
                Checked::Wrong
            );
        }
        // A right answer before the limit clears the count.
        assert_eq!(
            check_code(conn, &key, ann, &codes[0]).unwrap(),
            Checked::Right
        );
        for _ in 0..MAX_MISSES {
            assert_eq!(
                check_code(conn, &key, ann, "000000").unwrap(),
                Checked::Wrong
            );
        }
        // Locked: even a right code is not checked, nor spent.
        assert_eq!(
            check_code(conn, &key, ann, &codes[1]).unwrap(),
            Checked::Locked
        );
        assert_eq!(factors(conn, ann).unwrap().recovery_left, 9);
        // An admin turning it off clears the lock with everything else.
        turn_off(conn, ann).unwrap();
        let codes = replace_recovery_codes(conn, ann).unwrap();
        assert_eq!(
            check_code(conn, &key, ann, &codes[0]).unwrap(),
            Checked::Right
        );
    }

    #[test]
    fn a_new_signing_key_retires_authenticator_apps() {
        let conn = &test_db();
        let key = totp_key(b"a signing key");
        let ann = create_user(conn, "ann", "a password", Role::User).unwrap();
        let bob = create_user(conn, "bob", "b password", Role::User).unwrap();
        for id in [ann, bob] {
            let secret = begin_totp(conn, &key, id).unwrap();
            confirm_totp(conn, &key, id, &code_now(&secret)).unwrap();
        }
        let made =
            webauthn::register(&rp(), b"c", &Authenticator::new().create(&rp(), b"c")).unwrap();
        add_key(conn, bob, "yubikey", &made).unwrap();
        assert_eq!(forget_totp(conn).unwrap(), [ann, bob]);
        // Ann had nothing else: off. Bob keeps his key, and his codes.
        assert_eq!(factors(conn, ann).unwrap(), Factors::default());
        let bob_now = factors(conn, bob).unwrap();
        assert!(!bob_now.totp && bob_now.keys.len() == 1 && bob_now.recovery_left == 10);
    }

    #[test]
    fn a_ticket_is_for_one_purpose_and_a_few_tries() {
        let tickets = Tickets::default();
        let (id, ticket) = tickets.open(1, "ann", Purpose::SignIn).unwrap();
        assert_eq!(tickets.get(&id, Purpose::SignIn), Some(ticket));
        assert_eq!(tickets.get(&id, Purpose::Register), None);
        for _ in 0..TICKET_TRIES {
            let taken = tickets.take(&id, Purpose::SignIn).unwrap();
            tickets.missed_answer(&id, taken);
        }
        assert_eq!(tickets.get(&id, Purpose::SignIn), None);
        // Taken, it cannot be taken again until given back.
        let (id, _) = tickets.open(1, "ann", Purpose::SignIn).unwrap();
        let taken = tickets.take(&id, Purpose::SignIn).unwrap();
        assert_eq!(tickets.take(&id, Purpose::SignIn), None);
        tickets.missed_answer(&id, taken);
        assert!(tickets.take(&id, Purpose::SignIn).is_some());
        let (id, _) = tickets.open(1, "ann", Purpose::SignIn).unwrap();
        tickets.close(&id);
        assert_eq!(tickets.get(&id, Purpose::SignIn), None);
    }
}

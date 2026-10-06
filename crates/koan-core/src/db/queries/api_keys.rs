//! Subsonic API keys (OpenSubsonic `apiKeyAuthentication`).
//!
//! A key stands in for a username and password: it acts as its user, at the
//! user's current role, until revoked. Only `sha256(key)` is stored, so the key
//! itself is shown once, when it is made.

use rusqlite::{Connection, OptionalExtension, params};
use subtle::ConstantTimeEq;

use crate::auth::{self, Role};

use super::auth::UserRow;

/// `last_used_at` is written at most this often per key, so a client paging
/// through a library is not a database write per request.
const TOUCH_INTERVAL_SECS: i64 = 60;

#[derive(Debug, Clone)]
pub struct ApiKeyRow {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Make a key for `user_id`. Returns its row id and the key, which is not
/// recoverable afterwards.
pub fn create_api_key(
    conn: &Connection,
    user_id: i64,
    name: &str,
) -> Result<(i64, String), rusqlite::Error> {
    let key =
        auth::random_api_key().map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
    conn.execute(
        "INSERT INTO api_keys (user_id, name, key_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            user_id,
            name,
            auth::sha256_hex(&key),
            auth::now_unix() as i64
        ],
    )?;
    Ok((conn.last_insert_rowid(), key))
}

/// Make a key for `user_id` in place of any it already has named `name`: what
/// a device signing in again is given, so a reinstall or a sign-out leaves no
/// key behind that nothing holds any more.
pub fn replace_api_key(
    conn: &Connection,
    user_id: i64,
    name: &str,
) -> Result<(i64, String), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    let replaced = tx.execute(
        "DELETE FROM api_keys WHERE user_id = ?1 AND name = ?2",
        params![user_id, name],
    )?;
    let made = create_api_key(&tx, user_id, name)?;
    tx.commit()?;
    if replaced > 0 {
        auth::account_changed(user_id);
    }
    Ok(made)
}

/// Keys, oldest first — every user's, or one user's.
pub fn list_api_keys(
    conn: &Connection,
    user_id: Option<i64>,
) -> Result<Vec<ApiKeyRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT k.id, k.user_id, u.username, k.name, k.created_at, k.last_used_at
         FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE ?1 IS NULL OR k.user_id = ?1
         ORDER BY k.id",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(ApiKeyRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            name: row.get(3)?,
            created_at: row.get(4)?,
            last_used_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Revoke a key. With `user_id`, only that user's key can go. Returns whether
/// one did.
pub fn revoke_api_key(
    conn: &Connection,
    id: i64,
    user_id: Option<i64>,
) -> Result<bool, rusqlite::Error> {
    let owner: Option<i64> = conn
        .query_row(
            "DELETE FROM api_keys WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2) RETURNING user_id",
            params![id, user_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(owner) = owner {
        auth::account_changed(owner);
    }
    Ok(owner.is_some())
}

/// Revoke the key `key` itself: for a client giving up a key it holds, signed
/// in with that key. Returns whether one went.
pub fn revoke_api_key_value(conn: &Connection, key: &str) -> Result<bool, rusqlite::Error> {
    let owner: Option<i64> = conn
        .query_row(
            "DELETE FROM api_keys WHERE key_hash = ?1 RETURNING user_id",
            params![auth::sha256_hex(key)],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(owner) = owner {
        auth::account_changed(owner);
    }
    Ok(owner.is_some())
}

/// The row id of the key `key`, if it is a live key: which of a user's keys a
/// request signed in with.
pub fn id_of(conn: &Connection, key: &str) -> Result<Option<i64>, rusqlite::Error> {
    conn.query_row(
        "SELECT id FROM api_keys WHERE key_hash = ?1",
        params![auth::sha256_hex(key)],
        |row| row.get(0),
    )
    .optional()
}

/// Revoke every key a user has. Returns how many went.
pub fn revoke_user_api_keys(conn: &Connection, user_id: i64) -> Result<usize, rusqlite::Error> {
    let n = conn.execute("DELETE FROM api_keys WHERE user_id = ?1", params![user_id])?;
    if n > 0 {
        auth::account_changed(user_id);
    }
    Ok(n)
}

/// The user a key belongs to, if it is a live key.
///
/// Every stored hash is compared, in constant time, rather than looking the
/// hash up by index: an index lookup takes longer the more of the hash it
/// matched. The table holds a handful of rows per user.
pub fn authenticate_api_key(
    conn: &Connection,
    key: &str,
) -> Result<Option<UserRow>, rusqlite::Error> {
    let given = auth::sha256_hex(key);
    let mut found = None;
    {
        let mut stmt = conn.prepare_cached("SELECT id, key_hash, last_used_at FROM api_keys")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, hash, last_used) = row?;
            if bool::from(hash.as_bytes().ct_eq(given.as_bytes())) {
                found = Some((id, last_used));
            }
        }
    }
    let Some((id, last_used)) = found else {
        return Ok(None);
    };

    // Decided here rather than in the UPDATE's WHERE: an UPDATE takes the
    // write lock even when it matches nothing. A busy lock skips the stamp
    // rather than holding up the request.
    let now = auth::now_unix() as i64;
    if last_used.is_none_or(|t| t <= now - TOUCH_INTERVAL_SECS)
        && let Err(e) = crate::db::connection::without_waiting(conn, |conn| {
            conn.execute(
                "UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2",
                params![now, id],
            )
        })
    {
        log::debug!("api key {id}: last use not recorded: {e}");
    }

    let mut stmt = conn.prepare_cached(
        "SELECT u.id, u.username, u.password_hash, u.role, u.created_at
         FROM api_keys k JOIN users u ON u.id = k.user_id WHERE k.id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        let role: String = row.get(3)?;
        Ok(UserRow {
            id: row.get(0)?,
            username: row.get(1)?,
            password_hash: row.get(2)?,
            role: role.parse().unwrap_or(Role::Readonly),
            created_at: row.get(4)?,
        })
    })?;
    rows.next().transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::auth::{create_user, delete_user, update_role};

    fn test_db() -> (Database, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let db = Database::open(&tmp.path().join("test.db")).unwrap();
        (db, tmp)
    }

    #[test]
    fn key_signs_in_as_its_user_at_their_current_role() {
        let (db, _tmp) = test_db();
        let uid = create_user(&db.conn, "alice", "pw", Role::User).unwrap();
        let (_, key) = create_api_key(&db.conn, uid, "phone").unwrap();
        assert_eq!(key.len(), 43);

        let user = authenticate_api_key(&db.conn, &key).unwrap().unwrap();
        assert_eq!((user.username.as_str(), user.role), ("alice", Role::User));

        update_role(&db.conn, "alice", Role::Readonly).unwrap();
        let user = authenticate_api_key(&db.conn, &key).unwrap().unwrap();
        assert_eq!(user.role, Role::Readonly);

        assert!(authenticate_api_key(&db.conn, "nope").unwrap().is_none());
    }

    #[test]
    fn only_the_hash_is_stored_and_use_is_recorded() {
        let (db, _tmp) = test_db();
        let uid = create_user(&db.conn, "alice", "pw", Role::User).unwrap();
        let (_, key) = create_api_key(&db.conn, uid, "phone").unwrap();
        let stored: String = db
            .conn
            .query_row("SELECT key_hash FROM api_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, auth::sha256_hex(&key));

        assert!(
            list_api_keys(&db.conn, None).unwrap()[0]
                .last_used_at
                .is_none()
        );
        authenticate_api_key(&db.conn, &key).unwrap();
        assert!(
            list_api_keys(&db.conn, None).unwrap()[0]
                .last_used_at
                .is_some()
        );
    }

    /// Signing in must not wait for a scan or sync to finish writing: the
    /// stamp is skipped while the lock is held, and not attempted while fresh.
    #[test]
    fn a_held_write_lock_does_not_hold_up_sign_in() {
        let (db, tmp) = test_db();
        let uid = create_user(&db.conn, "alice", "pw", Role::User).unwrap();
        let (_, key) = create_api_key(&db.conn, uid, "phone").unwrap();

        let writer = Database::open_existing(&tmp.path().join("test.db")).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        assert!(authenticate_api_key(&db.conn, &key).unwrap().is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        writer.conn.execute_batch("ROLLBACK").unwrap();

        authenticate_api_key(&db.conn, &key).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        assert!(authenticate_api_key(&db.conn, &key).unwrap().is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        writer.conn.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn revoke_is_scoped_and_users_take_their_keys_with_them() {
        let (db, _tmp) = test_db();
        let alice = create_user(&db.conn, "alice", "pw", Role::User).unwrap();
        let bob = create_user(&db.conn, "bob", "pw", Role::User).unwrap();
        let (a, _) = create_api_key(&db.conn, alice, "a").unwrap();
        let (_, bob_key) = create_api_key(&db.conn, bob, "b").unwrap();

        assert!(!revoke_api_key(&db.conn, a, Some(bob)).unwrap());
        assert!(revoke_api_key(&db.conn, a, Some(alice)).unwrap());
        assert_eq!(list_api_keys(&db.conn, Some(alice)).unwrap().len(), 0);

        delete_user(&db.conn, bob).unwrap();
        assert!(authenticate_api_key(&db.conn, &bob_key).unwrap().is_none());
        assert!(list_api_keys(&db.conn, None).unwrap().is_empty());
    }

    #[test]
    fn replacing_a_devices_key_ends_what_the_old_one_signed() {
        use std::future::Future;
        use std::task::{Context, Waker};
        let changed_since = |user_id, mark| {
            let fut = std::pin::pin!(auth::account_changed_since(user_id, mark));
            fut.poll(&mut Context::from_waker(Waker::noop())).is_ready()
        };
        let (db, _tmp) = test_db();
        let alice = create_user(&db.conn, "alice", "pw", Role::User).unwrap();

        let (_, old) = replace_api_key(&db.conn, alice, "phone").unwrap();
        let mark = auth::account_mark();
        let (_, new) = replace_api_key(&db.conn, alice, "phone").unwrap();
        assert!(changed_since(alice, mark));
        assert!(authenticate_api_key(&db.conn, &old).unwrap().is_none());
        assert!(authenticate_api_key(&db.conn, &new).unwrap().is_some());
    }
}

/// A device's public key, as the account's devices are sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKey {
    pub device: String,
    /// Base64 of the device's 32-byte Ed25519 public key.
    pub key: String,
    /// The account it belongs to, for a device shared with `username`;
    /// `None` for its own.
    pub owner: Option<String>,
}

/// Record that the API key `raw_key` signed in `device`, which proves itself
/// with `public_key`. Whether it was kept.
///
/// A key row is bound to the first device it records, and a device id to the
/// one standing key row of its account that claimed it. Otherwise a thief
/// holding one device's key could link as another device's id and have its
/// own public key published in that device's place. A device signing in again
/// is given a new key in place of its old one (`replace_api_key`), so the
/// rightful claim is never refused; a new keypair on the same key replaces
/// the old public key.
pub fn set_device_key(
    conn: &Connection,
    raw_key: &str,
    device: &str,
    public_key: &str,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE api_keys SET device = ?1, device_key = ?2
          WHERE key_hash = ?3
            AND (device IS NULL OR device = ?1)
            AND NOT EXISTS (SELECT 1 FROM api_keys other
                             WHERE other.user_id = api_keys.user_id
                               AND other.device = ?1
                               AND other.id != api_keys.id)",
        params![device, public_key, auth::sha256_hex(raw_key)],
    )?;
    Ok(changed > 0)
}

/// The public keys `username`'s devices prove themselves with, and those of
/// the devices other accounts share with `username`: the newest key each
/// device registered under a key still standing.
pub fn device_keys(conn: &Connection, username: &str) -> rusqlite::Result<Vec<DeviceKey>> {
    let mut stmt = conn.prepare_cached(
        "SELECT k.device, k.device_key, u.username
           FROM api_keys k JOIN users u ON u.id = k.user_id
          WHERE k.device IS NOT NULL AND k.device_key IS NOT NULL
            AND (u.username = ?1
                 OR EXISTS (SELECT 1 FROM link_grants g
                             WHERE g.grantee = ?1 AND g.owner = u.username
                               AND g.device = k.device))
          ORDER BY k.created_at, k.id",
    )?;
    let rows = stmt.query_map([username], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    // Oldest first, so a later key for the same device replaces it.
    let mut keys: Vec<DeviceKey> = Vec::new();
    for row in rows {
        let (device, key, owner) = row?;
        let owner = (owner != username).then_some(owner);
        keys.retain(|k| !(k.device == device && k.owner == owner));
        keys.push(DeviceKey { device, key, owner });
    }
    Ok(keys)
}

#[cfg(test)]
mod device_key_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        for name in ["jo", "kim"] {
            crate::db::queries::auth::create_user(&conn, name, "pw", auth::Role::User).unwrap();
        }
        conn
    }

    fn user(conn: &Connection, name: &str) -> i64 {
        crate::db::queries::auth::get_user_by_username(conn, name)
            .unwrap()
            .unwrap()
            .id
    }

    #[test]
    fn a_key_records_its_device_and_is_published_until_revoked() {
        let conn = db();
        let jo = user(&conn, "jo");
        let (_, phone) = create_api_key(&conn, jo, "phone").unwrap();
        let (mac_id, mac) = create_api_key(&conn, jo, "mac").unwrap();
        assert!(set_device_key(&conn, &phone, "dev-phone", "PHONE1").unwrap());
        assert!(set_device_key(&conn, &mac, "dev-mac", "MAC").unwrap());
        assert!(!set_device_key(&conn, "not a key", "dev-x", "X").unwrap());
        // Signing in again on the phone: its key replaced, a new keypair.
        let (_, phone2) = replace_api_key(&conn, jo, "phone").unwrap();
        assert!(set_device_key(&conn, &phone2, "dev-phone", "PHONE2").unwrap());

        let keys = device_keys(&conn, "jo").unwrap();
        let of = |d: &str| keys.iter().find(|k| k.device == d).map(|k| k.key.as_str());
        assert_eq!(of("dev-phone"), Some("PHONE2"));
        assert_eq!(of("dev-mac"), Some("MAC"));
        assert!(keys.iter().all(|k| k.owner.is_none()));
        assert!(device_keys(&conn, "kim").unwrap().is_empty());

        revoke_api_key(&conn, mac_id, None).unwrap();
        let keys = device_keys(&conn, "jo").unwrap();
        assert_eq!(keys.len(), 1, "the Mac's key went with its API key");
    }

    /// The phone's key, stolen, cannot take the Mac's place: a device id
    /// belongs to the key row that claimed it, and a key row to its device.
    #[test]
    fn a_key_cannot_claim_another_devices_id() {
        let conn = db();
        let jo = user(&conn, "jo");
        let (_, phone) = create_api_key(&conn, jo, "phone").unwrap();
        let (_, mac) = create_api_key(&conn, jo, "mac").unwrap();
        assert!(set_device_key(&conn, &mac, "dev-mac", "MAC").unwrap());
        assert!(set_device_key(&conn, &phone, "dev-phone", "PHONE").unwrap());
        assert!(!set_device_key(&conn, &phone, "dev-mac", "THIEF").unwrap());
        assert!(!set_device_key(&conn, &phone, "dev-new", "THIEF").unwrap());
        // Its own id, with a new keypair, is still its to change.
        assert!(set_device_key(&conn, &phone, "dev-phone", "PHONE2").unwrap());
        let keys = device_keys(&conn, "jo").unwrap();
        let of = |d: &str| keys.iter().find(|k| k.device == d).map(|k| k.key.as_str());
        assert_eq!(of("dev-mac"), Some("MAC"));
        assert_eq!(of("dev-phone"), Some("PHONE2"));
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn a_shared_device_is_published_to_its_grantee_with_its_owner() {
        let conn = db();
        let jo = user(&conn, "jo");
        let (_, phone) = create_api_key(&conn, jo, "phone").unwrap();
        let (_, mac) = create_api_key(&conn, jo, "mac").unwrap();
        set_device_key(&conn, &phone, "dev-phone", "PHONE").unwrap();
        set_device_key(&conn, &mac, "dev-mac", "MAC").unwrap();
        conn.execute(
            "INSERT INTO link_grants (device, owner, grantee, created_at)
             VALUES ('dev-phone', 'jo', 'kim', 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            device_keys(&conn, "kim").unwrap(),
            vec![DeviceKey {
                device: "dev-phone".into(),
                key: "PHONE".into(),
                owner: Some("jo".into()),
            }]
        );
    }
}

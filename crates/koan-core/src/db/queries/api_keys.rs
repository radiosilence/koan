//! Subsonic API keys (OpenSubsonic `apiKeyAuthentication`).
//!
//! A key stands in for a username and password: it acts as its user, at the
//! user's current role, until revoked. Only `sha256(key)` is stored, so the key
//! itself is shown once, when it is made.

use rusqlite::{Connection, params};
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
    let n = conn.execute(
        "DELETE FROM api_keys WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)",
        params![id, user_id],
    )?;
    Ok(n > 0)
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
        let mut stmt = conn.prepare("SELECT id, key_hash FROM api_keys")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, hash) = row?;
            if bool::from(hash.as_bytes().ct_eq(given.as_bytes())) {
                found = Some(id);
            }
        }
    }
    let Some(id) = found else {
        return Ok(None);
    };

    let now = auth::now_unix() as i64;
    conn.execute(
        "UPDATE api_keys SET last_used_at = ?1
         WHERE id = ?2 AND (last_used_at IS NULL OR last_used_at <= ?3)",
        params![now, id, now - TOUCH_INTERVAL_SECS],
    )?;

    let mut stmt = conn.prepare(
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
}

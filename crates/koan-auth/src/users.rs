//! Accounts as rows: a username, an argon2 hash of the password and a role.
//!
//! The functions here change one row and announce it ([`crate::changes`]);
//! the rules about who may be made, demoted or removed are [`crate::accounts`]'.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::Role;
use crate::changes::account_changed;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub created_at: Option<String>,
}

pub(crate) const USER_COLUMNS: &str = "id, username, password_hash, role, created_at";

/// A row selected as [`USER_COLUMNS`], from column `at` on. A role this build
/// does not know reads as the least it can be.
pub(crate) fn user_row(row: &Row, at: usize) -> rusqlite::Result<UserRow> {
    let role: String = row.get(at + 3)?;
    Ok(UserRow {
        id: row.get(at)?,
        username: row.get(at + 1)?,
        password_hash: row.get(at + 2)?,
        role: role.parse().unwrap_or(Role::Readonly),
        created_at: row.get(at + 4)?,
    })
}

/// Create an account. Returns its id. Ids are never reused, so nothing made
/// for a deleted account names a later one.
pub fn create_user(
    conn: &Connection,
    username: &str,
    password: &str,
    role: Role,
) -> Result<i64, rusqlite::Error> {
    let hash = crate::password::hash_password(password)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
    conn.execute(
        "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
        params![username, hash, role.as_str()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_user_by_username(
    conn: &Connection,
    username: &str,
) -> Result<Option<UserRow>, rusqlite::Error> {
    conn.prepare_cached(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username = ?1"
    ))?
    .query_row(params![username], |r| user_row(r, 0))
    .optional()
}

pub fn get_user_by_id(conn: &Connection, user_id: i64) -> Result<Option<UserRow>, rusqlite::Error> {
    conn.prepare_cached(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"))?
        .query_row(params![user_id], |r| user_row(r, 0))
        .optional()
}

/// Every account, oldest first.
pub fn list_users(conn: &Connection) -> Result<Vec<UserRow>, rusqlite::Error> {
    conn.prepare_cached(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY id"))?
        .query_map([], |r| user_row(r, 0))?
        .collect()
}

/// Delete an account by id, with its sessions, keys and app passwords.
/// Whether there was one.
///
/// They are deleted here rather than left to the tables' `ON DELETE
/// CASCADE`, which SQLite applies only on a connection with `foreign_keys`
/// on: a service that leaves it off would otherwise keep a deleted account's
/// sessions, some of which are read without the account.
pub fn delete_user(conn: &Connection, user_id: i64) -> Result<bool, rusqlite::Error> {
    let count = crate::sql::atomically(conn, || {
        for table in ["refresh_tokens", "api_keys", "app_passwords"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE user_id = ?1"),
                params![user_id],
            )?;
        }
        conn.execute("DELETE FROM users WHERE id = ?1", params![user_id])
    })?;
    if count > 0 {
        account_changed(user_id);
    }
    Ok(count > 0)
}

/// Give an account a new password, and end every session, API key and app
/// password it has: a reset is how an admin shuts out whoever else had the
/// password, and a credential made with it would otherwise outlast it.
///
/// One transaction: a reset that fails part way leaves the old password and
/// everything it signed in, not a new password beside credentials it was
/// meant to end.
pub fn update_password(
    conn: &Connection,
    username: &str,
    new_password: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let hash = crate::password::hash_password(new_password)?;
    let updated = crate::sql::atomically(conn, || {
        let updated: Option<i64> = conn
            .query_row(
                "UPDATE users SET password_hash = ?1 WHERE username = ?2 RETURNING id",
                params![hash, username],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = updated {
            crate::sessions::revoke_all_user_tokens(conn, id)?;
            crate::api_keys::revoke_user_api_keys(conn, id)?;
            crate::app_passwords::revoke_user_app_passwords(conn, id)?;
        }
        Ok::<_, rusqlite::Error>(updated)
    })?;
    if let Some(id) = updated {
        account_changed(id);
    }
    Ok(updated.is_some())
}

/// Change an account's role. Whether there was such an account.
pub fn update_role(conn: &Connection, username: &str, role: Role) -> Result<bool, rusqlite::Error> {
    let updated: Option<i64> = conn
        .query_row(
            "UPDATE users SET role = ?1 WHERE username = ?2 RETURNING id",
            params![role.as_str(), username],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = updated {
        account_changed(id);
    }
    Ok(updated.is_some())
}

/// Whether any account exists: a server with none is waiting for its first
/// admin.
pub fn has_users(conn: &Connection) -> Result<bool, rusqlite::Error> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM users)", [], |row| row.get(0))
}

pub fn admin_count(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM users WHERE role = 'admin'",
        [],
        |row| row.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::test_db;

    #[test]
    fn create_and_get_user() {
        let conn = test_db();
        let id = create_user(&conn, "alice", "password123", Role::Admin).unwrap();
        assert!(id > 0);
        let user = get_user_by_username(&conn, "alice").unwrap().unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, Role::Admin);
        assert!(user.password_hash.starts_with("$argon2"));
        assert_eq!(get_user_by_id(&conn, id).unwrap(), Some(user));
        assert_eq!(get_user_by_id(&conn, id + 1).unwrap(), None);
    }

    #[test]
    fn duplicate_username_rejected() {
        let conn = test_db();
        create_user(&conn, "bob", "pass1", Role::User).unwrap();
        assert!(create_user(&conn, "bob", "pass2", Role::User).is_err());
    }

    #[test]
    fn list_and_delete_users() {
        let conn = test_db();
        let id1 = create_user(&conn, "user1", "pass", Role::Admin).unwrap();
        create_user(&conn, "user2", "pass", Role::User).unwrap();
        assert_eq!(list_users(&conn).unwrap().len(), 2);
        assert!(delete_user(&conn, id1).unwrap());
        assert!(!delete_user(&conn, id1).unwrap());
        let users = list_users(&conn).unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "user2");
    }

    #[test]
    fn has_users_empty_and_populated() {
        let conn = test_db();
        assert!(!has_users(&conn).unwrap());
        create_user(&conn, "first", "pass", Role::Admin).unwrap();
        assert!(has_users(&conn).unwrap());
        assert_eq!(admin_count(&conn).unwrap(), 1);
    }

    #[test]
    fn a_new_password_ends_every_session_and_key() {
        let conn = test_db();
        let id = create_user(&conn, "ann", "old password", Role::User).unwrap();
        let future = crate::clock::unix_secs() + 3600;
        crate::sessions::store_refresh_token(&conn, "browser", id, future).unwrap();
        let (_, key) = crate::api_keys::create_api_key(&conn, id, "phone").unwrap();
        assert!(update_password(&conn, "ann", "new password").unwrap());
        assert!(
            crate::sessions::get_valid_refresh_token(&conn, "browser")
                .unwrap()
                .is_none()
        );
        assert!(
            crate::api_keys::authenticate_api_key(&conn, &key)
                .unwrap()
                .is_none()
        );
        assert!(!update_password(&conn, "nobody", "new password").unwrap());
    }

    #[test]
    fn a_reset_that_fails_part_way_changes_nothing() {
        let conn = test_db();
        let id = create_user(&conn, "ann", "old password", Role::User).unwrap();
        let future = crate::clock::unix_secs() + 3600;
        crate::sessions::store_refresh_token(&conn, "browser", id, future).unwrap();
        let (_, key) = crate::api_keys::create_api_key(&conn, id, "phone").unwrap();
        // A write that fails after the password's, as one held up by another
        // connection's lock would.
        conn.execute_batch(
            "CREATE TRIGGER keep_keys BEFORE DELETE ON api_keys
             BEGIN SELECT RAISE(ABORT, 'held'); END;",
        )
        .unwrap();
        assert!(update_password(&conn, "ann", "new password").is_err());
        let ann = get_user_by_username(&conn, "ann").unwrap().unwrap();
        crate::password::verify_password("old password", &ann.password_hash).unwrap();
        assert!(
            crate::sessions::get_valid_refresh_token(&conn, "browser")
                .unwrap()
                .is_some()
        );
        assert!(
            crate::api_keys::authenticate_api_key(&conn, &key)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_deleted_account_takes_its_credentials_without_foreign_keys() {
        // As SQLite opens a connection unless told otherwise: no cascades.
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        let id = create_user(&conn, "ann", "password", Role::User).unwrap();
        let future = crate::clock::unix_secs() + 3600;
        crate::sessions::store_refresh_token(&conn, "browser", id, future).unwrap();
        let (_, key) = crate::api_keys::create_api_key(&conn, id, "phone").unwrap();
        let sealing = crate::app_passwords::app_password_key(b"a signing key");
        crate::app_passwords::create_app_password(&conn, &sealing, id, "app").unwrap();

        assert!(delete_user(&conn, id).unwrap());
        // Read without the account, so a row left behind would still spend.
        assert!(
            crate::sessions::consume_refresh_token(&conn, "browser")
                .unwrap()
                .is_none()
        );
        assert!(
            crate::api_keys::authenticate_api_key(&conn, &key)
                .unwrap()
                .is_none()
        );
        let left: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM refresh_tokens) + (SELECT COUNT(*) FROM api_keys)
                      + (SELECT COUNT(*) FROM app_passwords)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }
}

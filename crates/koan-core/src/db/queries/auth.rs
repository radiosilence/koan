//! Auth queries: user CRUD, refresh token management.

use rusqlite::{Connection, params};

use crate::auth::{self, Role};

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenRow {
    pub id: String,
    pub user_id: i64,
    pub expires_at: i64,
    pub revoked: bool,
    pub created_at: Option<String>,
}

// ---------------------------------------------------------------------------
// User CRUD
// ---------------------------------------------------------------------------

/// Create a new user. Returns the user ID.
pub fn create_user(
    conn: &Connection,
    username: &str,
    password: &str,
    role: Role,
) -> Result<i64, rusqlite::Error> {
    let hash = auth::hash_password(password)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
    conn.execute(
        "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
        params![username, hash, role.as_str()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get a user by username.
pub fn get_user_by_username(
    conn: &Connection,
    username: &str,
) -> Result<Option<UserRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, username, password_hash, role, created_at FROM users WHERE username = ?1",
    )?;
    let mut rows = stmt.query_map(params![username], |row| {
        let role_str: String = row.get(3)?;
        Ok(UserRow {
            id: row.get(0)?,
            username: row.get(1)?,
            password_hash: row.get(2)?,
            role: role_str.parse().unwrap_or(Role::Readonly),
            created_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(user)) => Ok(Some(user)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Get a user by ID.
pub fn get_user_by_id(conn: &Connection, user_id: i64) -> Result<Option<UserRow>, rusqlite::Error> {
    let mut stmt = conn
        .prepare("SELECT id, username, password_hash, role, created_at FROM users WHERE id = ?1")?;
    let mut rows = stmt.query_map(params![user_id], |row| {
        let role_str: String = row.get(3)?;
        Ok(UserRow {
            id: row.get(0)?,
            username: row.get(1)?,
            password_hash: row.get(2)?,
            role: role_str.parse().unwrap_or(Role::Readonly),
            created_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(user)) => Ok(Some(user)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// List all users (no password hashes).
pub fn list_users(conn: &Connection) -> Result<Vec<UserRow>, rusqlite::Error> {
    let mut stmt = conn
        .prepare("SELECT id, username, password_hash, role, created_at FROM users ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        let role_str: String = row.get(3)?;
        Ok(UserRow {
            id: row.get(0)?,
            username: row.get(1)?,
            password_hash: row.get(2)?,
            role: role_str.parse().unwrap_or(Role::Readonly),
            created_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Delete a user by ID. Returns true if a row was deleted.
pub fn delete_user(conn: &Connection, user_id: i64) -> Result<bool, rusqlite::Error> {
    let count = conn.execute("DELETE FROM users WHERE id = ?1", params![user_id])?;
    Ok(count > 0)
}

/// Update a user's password. Revokes all their refresh tokens.
pub fn update_password(
    conn: &Connection,
    username: &str,
    new_password: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let hash = crate::auth::hash_password(new_password)?;
    let updated = conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE username = ?2",
        params![hash, username],
    )?;
    if updated > 0 {
        // Revoke all existing tokens for this user.
        if let Some(user) = get_user_by_username(conn, username)? {
            revoke_all_user_tokens(conn, user.id)?;
        }
    }
    Ok(updated > 0)
}

/// Update a user's role.
pub fn update_role(
    conn: &Connection,
    username: &str,
    role: crate::auth::Role,
) -> Result<bool, rusqlite::Error> {
    let updated = conn.execute(
        "UPDATE users SET role = ?1 WHERE username = ?2",
        params![role.as_str(), username],
    )?;
    Ok(updated > 0)
}

/// Check if any users exist (for first-run detection).
pub fn has_users(conn: &Connection) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
    Ok(count > 0)
}

/// Count users with admin role.
pub fn admin_count(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM users WHERE role = 'admin'",
        [],
        |row| row.get(0),
    )
}

// ---------------------------------------------------------------------------
// Refresh tokens
// ---------------------------------------------------------------------------

/// Store a refresh token.
pub fn store_refresh_token(
    conn: &Connection,
    token_id: &str,
    user_id: i64,
    expires_at: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO refresh_tokens (id, user_id, expires_at) VALUES (?1, ?2, ?3)",
        params![token_id, user_id, expires_at],
    )?;
    Ok(())
}

/// Look up a refresh token. Returns None if not found, expired, or revoked.
pub fn get_valid_refresh_token(
    conn: &Connection,
    token_id: &str,
) -> Result<Option<RefreshTokenRow>, rusqlite::Error> {
    let now = auth::now_unix() as i64;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, expires_at, revoked, created_at
         FROM refresh_tokens
         WHERE id = ?1 AND revoked = 0 AND expires_at > ?2",
    )?;
    let mut rows = stmt.query_map(params![token_id, now], |row| {
        Ok(RefreshTokenRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            expires_at: row.get(2)?,
            revoked: row.get::<_, i32>(3)? != 0,
            created_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(token)) => Ok(Some(token)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Atomically consume a valid refresh token: revoke it and return the row in one
/// statement. Returns `None` if the token doesn't exist, is already revoked, or
/// has expired. This prevents TOCTOU races in refresh-token rotation.
pub fn consume_refresh_token(
    conn: &Connection,
    token_id: &str,
) -> Result<Option<RefreshTokenRow>, rusqlite::Error> {
    let now = auth::now_unix() as i64;
    let mut stmt = conn.prepare(
        "UPDATE refresh_tokens SET revoked = 1
         WHERE id = ?1 AND revoked = 0 AND expires_at > ?2
         RETURNING id, user_id, expires_at, revoked, created_at",
    )?;
    let mut rows = stmt.query_map(params![token_id, now], |row| {
        Ok(RefreshTokenRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            expires_at: row.get(2)?,
            revoked: row.get::<_, i32>(3)? != 0,
            created_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(token)) => Ok(Some(token)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Revoke a single refresh token (logout).
pub fn revoke_refresh_token(conn: &Connection, token_id: &str) -> Result<bool, rusqlite::Error> {
    let count = conn.execute(
        "UPDATE refresh_tokens SET revoked = 1 WHERE id = ?1",
        params![token_id],
    )?;
    Ok(count > 0)
}

/// Revoke all refresh tokens for a user (password change, account delete).
pub fn revoke_all_user_tokens(conn: &Connection, user_id: i64) -> Result<usize, rusqlite::Error> {
    let count = conn.execute(
        "UPDATE refresh_tokens SET revoked = 1 WHERE user_id = ?1 AND revoked = 0",
        params![user_id],
    )?;
    Ok(count)
}

/// Clean up expired/revoked refresh tokens (housekeeping).
pub fn cleanup_expired_tokens(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let now = auth::now_unix() as i64;
    let count = conn.execute(
        "DELETE FROM refresh_tokens WHERE revoked = 1 OR expires_at <= ?1",
        params![now],
    )?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db, tmp)
    }

    #[test]
    fn create_and_get_user() {
        let (db, _tmp) = test_db();
        let id = create_user(&db.conn, "alice", "password123", Role::Admin).unwrap();
        assert!(id > 0);

        let user = get_user_by_username(&db.conn, "alice").unwrap().unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, Role::Admin);
        assert!(user.password_hash.starts_with("$argon2"));
    }

    #[test]
    fn duplicate_username_rejected() {
        let (db, _tmp) = test_db();
        create_user(&db.conn, "bob", "pass1", Role::User).unwrap();
        let result = create_user(&db.conn, "bob", "pass2", Role::User);
        assert!(result.is_err());
    }

    #[test]
    fn list_and_delete_users() {
        let (db, _tmp) = test_db();
        let id1 = create_user(&db.conn, "user1", "pass", Role::Admin).unwrap();
        create_user(&db.conn, "user2", "pass", Role::User).unwrap();

        let users = list_users(&db.conn).unwrap();
        assert_eq!(users.len(), 2);

        assert!(delete_user(&db.conn, id1).unwrap());
        let users = list_users(&db.conn).unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "user2");
    }

    #[test]
    fn has_users_empty_and_populated() {
        let (db, _tmp) = test_db();
        assert!(!has_users(&db.conn).unwrap());
        create_user(&db.conn, "first", "pass", Role::Admin).unwrap();
        assert!(has_users(&db.conn).unwrap());
    }

    #[test]
    fn refresh_token_lifecycle() {
        let (db, _tmp) = test_db();
        let uid = create_user(&db.conn, "user", "pass", Role::User).unwrap();

        let future_ts = auth::now_unix() as i64 + 86400;
        store_refresh_token(&db.conn, "tok-123", uid, future_ts).unwrap();

        // Valid lookup.
        let tok = get_valid_refresh_token(&db.conn, "tok-123")
            .unwrap()
            .unwrap();
        assert_eq!(tok.user_id, uid);

        // Revoke.
        assert!(revoke_refresh_token(&db.conn, "tok-123").unwrap());
        assert!(
            get_valid_refresh_token(&db.conn, "tok-123")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn expired_token_not_returned() {
        let (db, _tmp) = test_db();
        let uid = create_user(&db.conn, "user", "pass", Role::User).unwrap();

        // Already expired.
        store_refresh_token(&db.conn, "tok-old", uid, 0).unwrap();
        assert!(
            get_valid_refresh_token(&db.conn, "tok-old")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cleanup_removes_expired_and_revoked() {
        let (db, _tmp) = test_db();
        let uid = create_user(&db.conn, "user", "pass", Role::User).unwrap();

        let future = auth::now_unix() as i64 + 86400;
        store_refresh_token(&db.conn, "active", uid, future).unwrap();
        store_refresh_token(&db.conn, "expired", uid, 0).unwrap();
        store_refresh_token(&db.conn, "revoked", uid, future).unwrap();
        revoke_refresh_token(&db.conn, "revoked").unwrap();

        let cleaned = cleanup_expired_tokens(&db.conn).unwrap();
        assert_eq!(cleaned, 2);

        // Active token still there.
        assert!(
            get_valid_refresh_token(&db.conn, "active")
                .unwrap()
                .is_some()
        );
    }
}

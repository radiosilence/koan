//! Subsonic app passwords: generated passwords for clients that only sign in
//! with Subsonic token auth (`t = md5(password + salt)`).
//!
//! Checking a token needs the password itself, which koan never keeps for an
//! account. An app password is the exception made for it: random, made for one
//! client, shown once, and stored sealed under a key derived from the server's
//! signing key (`auth::app_password_key`). It acts as its user, at the user's
//! current role, until revoked, and it is never the account's real password.

use rusqlite::{Connection, params};

use crate::auth;

use super::auth::{UserRow, get_user_by_username};

/// `last_used_at` is written at most this often per password, so a client
/// paging through a library is not a database write per request.
const TOUCH_INTERVAL_SECS: i64 = 60;

#[derive(Debug, Clone)]
pub struct AppPasswordRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Make an app password for `user_id`. Returns its row id and the password,
/// which is not shown again.
pub fn create_app_password(
    conn: &Connection,
    key: &[u8; 32],
    user_id: i64,
    name: &str,
) -> Result<(i64, String), rusqlite::Error> {
    let fail = |e: auth::AuthError| rusqlite::Error::ToSqlConversionFailure(e.into());
    let password = auth::random_app_password().map_err(fail)?;
    let sealed = auth::seal_app_password(key, user_id, &password).map_err(fail)?;
    conn.execute(
        "INSERT INTO app_passwords (user_id, name, sealed, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![user_id, name, sealed, auth::now_unix() as i64],
    )?;
    Ok((conn.last_insert_rowid(), password))
}

/// A user's app passwords, oldest first, without the passwords.
pub fn list_app_passwords(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<AppPasswordRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, name, created_at, last_used_at FROM app_passwords
         WHERE user_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(AppPasswordRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            name: row.get(2)?,
            created_at: row.get(3)?,
            last_used_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Revoke one of `user_id`'s app passwords. Returns whether one went.
pub fn revoke_app_password(
    conn: &Connection,
    id: i64,
    user_id: i64,
) -> Result<bool, rusqlite::Error> {
    let n = conn.execute(
        "DELETE FROM app_passwords WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    Ok(n > 0)
}

/// Revoke every app password a user has. Returns how many went.
pub fn revoke_user_app_passwords(
    conn: &Connection,
    user_id: i64,
) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "DELETE FROM app_passwords WHERE user_id = ?1",
        params![user_id],
    )
}

/// How a sign-in against `username`'s app passwords went.
pub enum AppPasswordAuth {
    /// One of them matched.
    Matched(UserRow),
    /// The account has app passwords, and none matched.
    Wrong,
    /// No such account, or it has no app passwords.
    NoneMade,
}

/// Check a credential against each of `username`'s app passwords with
/// `matches`, which is given the password: a token check for `t`/`s`, a
/// comparison for `p`. Every password is tried, so the time taken does not say
/// which one matched.
pub fn authenticate_app_password(
    conn: &Connection,
    key: &[u8; 32],
    username: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<AppPasswordAuth, rusqlite::Error> {
    let Some(user) = get_user_by_username(conn, username)? else {
        return Ok(AppPasswordAuth::NoneMade);
    };
    let mut any = false;
    let mut found = None;
    {
        let mut stmt = conn.prepare_cached(
            "SELECT id, sealed, last_used_at FROM app_passwords WHERE user_id = ?1",
        )?;
        let rows = stmt.query_map(params![user.id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, sealed, last_used) = row?;
            any = true;
            if let Some(password) = auth::open_app_password(key, user.id, &sealed)
                && matches(&password)
            {
                found = Some((id, last_used));
            }
        }
    }
    let Some((id, last_used)) = found else {
        return Ok(if any {
            AppPasswordAuth::Wrong
        } else {
            AppPasswordAuth::NoneMade
        });
    };

    // A busy lock skips the stamp rather than holding up the request.
    let now = auth::now_unix() as i64;
    if last_used.is_none_or(|t| t <= now - TOUCH_INTERVAL_SECS)
        && let Err(e) = crate::db::connection::without_waiting(conn, |conn| {
            conn.execute(
                "UPDATE app_passwords SET last_used_at = ?1 WHERE id = ?2",
                params![now, id],
            )
        })
    {
        log::debug!("app password {id}: last use not recorded: {e}");
    }
    Ok(AppPasswordAuth::Matched(user))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;
    use crate::db::connection::Database;
    use crate::db::queries::auth::create_user;

    fn test_db() -> (Database, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let db = Database::open(&tmp.path().join("test.db")).unwrap();
        (db, tmp)
    }

    #[test]
    fn an_app_password_signs_its_user_in_and_nobody_else() {
        let (db, _tmp) = test_db();
        let conn = &db.conn;
        let key = auth::app_password_key(b"a signing key");
        let alice = create_user(conn, "alice", "a password", Role::User).unwrap();
        create_user(conn, "bob", "b password", Role::User).unwrap();
        let (_, password) = create_app_password(conn, &key, alice, "arpeggi").unwrap();
        assert_eq!(password.len(), 23);
        assert_ne!(password, "a password");

        let ok = authenticate_app_password(conn, &key, "alice", |p| p == password).unwrap();
        assert!(matches!(ok, AppPasswordAuth::Matched(u) if u.username == "alice"));
        let wrong = authenticate_app_password(conn, &key, "alice", |p| p == "guess").unwrap();
        assert!(matches!(wrong, AppPasswordAuth::Wrong));
        let bob = authenticate_app_password(conn, &key, "bob", |p| p == password).unwrap();
        assert!(matches!(bob, AppPasswordAuth::NoneMade));
        // Another server's key does not open it.
        let other = auth::app_password_key(b"another key");
        let elsewhere =
            authenticate_app_password(conn, &other, "alice", |p| p == password).unwrap();
        assert!(matches!(elsewhere, AppPasswordAuth::Wrong));
    }

    #[test]
    fn a_revoked_app_password_no_longer_signs_in() {
        let (db, _tmp) = test_db();
        let conn = &db.conn;
        let key = auth::app_password_key(b"a signing key");
        let alice = create_user(conn, "alice", "a password", Role::User).unwrap();
        let (id, password) = create_app_password(conn, &key, alice, "arpeggi").unwrap();
        assert_eq!(list_app_passwords(conn, alice).unwrap().len(), 1);
        assert!(revoke_app_password(conn, id, alice).unwrap());
        let after = authenticate_app_password(conn, &key, "alice", |p| p == password).unwrap();
        assert!(matches!(after, AppPasswordAuth::NoneMade));
    }

    #[test]
    fn a_sealed_password_moved_to_another_account_does_not_open() {
        let key = auth::app_password_key(b"a signing key");
        let sealed = auth::seal_app_password(&key, 1, "secret").unwrap();
        assert_eq!(
            auth::open_app_password(&key, 1, &sealed).as_deref(),
            Some("secret")
        );
        assert_eq!(auth::open_app_password(&key, 2, &sealed), None);
    }
}

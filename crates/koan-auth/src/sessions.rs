//! Sessions: an opaque secret a client holds, stored only as its SHA-256, so
//! a read of the database yields nothing that signs in.
//!
//! A client presents its secret as it is, as a browser does on every request
//! with a session cookie ([`holder`]), or trades it for a new one each time
//! it is used ([`consume_refresh_token`]), as kōan's apps do for their access
//! tokens. The table is `refresh_tokens`, after the second.
//!
//! A session can belong to an OAuth grant: one client's connection, given
//! when an account approved it. A grant's tokens are spent once each, and a
//! spent one coming back means a copy is in other hands
//! ([`revoke_replayed_grant`]).

use rusqlite::{Connection, OptionalExtension, params};

use crate::secret::sha256_hex;
use crate::users::{USER_COLUMNS, UserRow, user_row};

#[derive(Debug, Clone)]
pub struct RefreshTokenRow {
    /// The secret's SHA-256, hex encoded.
    pub id: String,
    pub user_id: i64,
    pub expires_at: i64,
    pub revoked: bool,
    pub created_at: Option<String>,
    /// The OAuth grant the token descends from, and the client it was granted
    /// to; `None` for app and browser sessions.
    pub grant: Option<OAuthGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrant {
    pub id: String,
    pub client_name: String,
}

const TOKEN_COLUMNS: &str = "id, user_id, expires_at, revoked, created_at, grant_id, client_name";

fn token_row(row: &rusqlite::Row) -> rusqlite::Result<RefreshTokenRow> {
    let grant_id: Option<String> = row.get(5)?;
    let client_name: Option<String> = row.get(6)?;
    Ok(RefreshTokenRow {
        id: row.get(0)?,
        user_id: row.get(1)?,
        expires_at: row.get(2)?,
        revoked: row.get::<_, i32>(3)? != 0,
        created_at: row.get(4)?,
        grant: grant_id.map(|id| OAuthGrant {
            id,
            client_name: client_name.unwrap_or_default(),
        }),
    })
}

/// Store a session for `user_id` until `expires_at` (unix seconds).
pub fn store_refresh_token(
    conn: &Connection,
    token_id: &str,
    user_id: i64,
    expires_at: i64,
) -> Result<(), rusqlite::Error> {
    store_grant_token(conn, token_id, user_id, expires_at, None)
}

/// Store a session belonging to an OAuth grant, or to none.
pub fn store_grant_token(
    conn: &Connection,
    token_id: &str,
    user_id: i64,
    expires_at: i64,
    grant: Option<&OAuthGrant>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO refresh_tokens (id, user_id, expires_at, grant_id, client_name)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            sha256_hex(token_id),
            user_id,
            expires_at,
            grant.map(|g| &g.id),
            grant.map(|g| &g.client_name),
        ],
    )?;
    Ok(())
}

/// A session, unless it is unknown, expired or revoked.
pub fn get_valid_refresh_token(
    conn: &Connection,
    token_id: &str,
) -> Result<Option<RefreshTokenRow>, rusqlite::Error> {
    conn.prepare_cached(&format!(
        "SELECT {TOKEN_COLUMNS} FROM refresh_tokens
         WHERE id = ?1 AND revoked = 0 AND expires_at > ?2"
    ))?
    .query_row(
        params![sha256_hex(token_id), crate::clock::unix_secs()],
        token_row,
    )
    .optional()
}

/// The account a presented session belongs to, as it stands now, while the
/// session lasts. A grant's tokens are not sessions a client presents, so
/// they answer `None`: a token handed to an OAuth client cannot stand in for
/// a browser's session.
pub fn holder(conn: &Connection, token_id: &str) -> Result<Option<UserRow>, rusqlite::Error> {
    let columns = USER_COLUMNS
        .split(", ")
        .map(|c| format!("u.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    conn.prepare_cached(&format!(
        "SELECT {columns} FROM refresh_tokens t JOIN users u ON u.id = t.user_id
         WHERE t.id = ?1 AND t.revoked = 0 AND t.expires_at > ?2 AND t.grant_id IS NULL"
    ))?
    .query_row(
        params![sha256_hex(token_id), crate::clock::unix_secs()],
        |r| user_row(r, 0),
    )
    .optional()
}

/// Spend a session for its replacement: revoke it and return the row in one
/// statement, so two requests racing with the same token cannot both spend
/// it. `None` if it is unknown, already spent, or expired.
pub fn consume_refresh_token(
    conn: &Connection,
    token_id: &str,
) -> Result<Option<RefreshTokenRow>, rusqlite::Error> {
    conn.prepare_cached(&format!(
        "UPDATE refresh_tokens SET revoked = 1, used_at = ?2
         WHERE id = ?1 AND revoked = 0 AND expires_at > ?2
         RETURNING {TOKEN_COLUMNS}"
    ))?
    .query_row(
        params![sha256_hex(token_id), crate::clock::unix_secs()],
        token_row,
    )
    .optional()
}

/// End one session (signing out). Whether there was one.
pub fn revoke_refresh_token(conn: &Connection, token_id: &str) -> Result<bool, rusqlite::Error> {
    let count = conn.execute(
        "UPDATE refresh_tokens SET revoked = 1 WHERE id = ?1",
        params![sha256_hex(token_id)],
    )?;
    Ok(count > 0)
}

/// End every session of an account (a new password, a deleted account).
pub fn revoke_all_user_tokens(conn: &Connection, user_id: i64) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE refresh_tokens SET revoked = 1 WHERE user_id = ?1 AND revoked = 0",
        params![user_id],
    )
}

/// A token of an OAuth grant spent once already, more than `grace_secs` ago,
/// is a copy in someone else's hands: revoke the whole grant, so whichever
/// side refreshed first loses it too. The grace covers a client retrying a
/// refresh whose answer it never received. Returns how many tokens were
/// revoked.
///
/// OAuth grants only: browser tabs and app tasks share one session and may
/// race a refresh, which is not theft.
pub fn revoke_replayed_grant(
    conn: &Connection,
    token_id: &str,
    grace_secs: i64,
) -> Result<usize, rusqlite::Error> {
    let cutoff = crate::clock::unix_secs() - grace_secs;
    conn.execute(
        "UPDATE refresh_tokens SET revoked = 1
         WHERE revoked = 0 AND grant_id = (
           SELECT grant_id FROM refresh_tokens
           WHERE id = ?1 AND revoked = 1 AND grant_id IS NOT NULL AND used_at < ?2)",
        params![sha256_hex(token_id), cutoff],
    )
}

/// Revoke every token of an OAuth grant.
pub fn revoke_grant(conn: &Connection, grant_id: &str) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE refresh_tokens SET revoked = 1 WHERE grant_id = ?1 AND revoked = 0",
        params![grant_id],
    )
}

/// Delete expired and revoked sessions. A spent token of an OAuth grant
/// stays until it would have expired, so [`revoke_replayed_grant`] can still
/// recognise it.
pub fn cleanup_expired_tokens(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "DELETE FROM refresh_tokens
         WHERE expires_at <= ?1 OR (revoked = 1 AND (grant_id IS NULL OR used_at IS NULL))",
        params![crate::clock::unix_secs()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use crate::schema::test_db;
    use crate::users::create_user;

    fn later() -> i64 {
        crate::clock::unix_secs() + 86400
    }

    #[test]
    fn refresh_token_lifecycle() {
        let conn = test_db();
        let uid = create_user(&conn, "user", "pass", Role::User).unwrap();
        store_refresh_token(&conn, "tok-123", uid, later()).unwrap();
        let tok = get_valid_refresh_token(&conn, "tok-123").unwrap().unwrap();
        assert_eq!(tok.user_id, uid);
        assert_ne!(tok.id, "tok-123");
        assert!(revoke_refresh_token(&conn, "tok-123").unwrap());
        assert!(get_valid_refresh_token(&conn, "tok-123").unwrap().is_none());
    }

    #[test]
    fn expired_token_not_returned() {
        let conn = test_db();
        let uid = create_user(&conn, "user", "pass", Role::User).unwrap();
        store_refresh_token(&conn, "tok-old", uid, 0).unwrap();
        assert!(get_valid_refresh_token(&conn, "tok-old").unwrap().is_none());
        assert!(holder(&conn, "tok-old").unwrap().is_none());
    }

    #[test]
    fn a_session_is_held_by_its_account_as_it_now_stands() {
        let conn = test_db();
        let uid = create_user(&conn, "ann", "pass", Role::User).unwrap();
        store_refresh_token(&conn, "cookie", uid, later()).unwrap();
        assert_eq!(holder(&conn, "cookie").unwrap().unwrap().role, Role::User);
        crate::users::update_role(&conn, "ann", Role::Readonly).unwrap();
        assert_eq!(
            holder(&conn, "cookie").unwrap().unwrap().role,
            Role::Readonly
        );
        assert!(holder(&conn, "not-a-session").unwrap().is_none());
        crate::users::delete_user(&conn, uid).unwrap();
        assert!(holder(&conn, "cookie").unwrap().is_none());
    }

    #[test]
    fn a_grants_token_is_no_session() {
        let conn = test_db();
        let uid = create_user(&conn, "ann", "pass", Role::User).unwrap();
        let grant = OAuthGrant {
            id: "g".into(),
            client_name: "client".into(),
        };
        store_grant_token(&conn, "granted", uid, later(), Some(&grant)).unwrap();
        assert!(holder(&conn, "granted").unwrap().is_none());
    }

    #[test]
    fn a_spent_grant_token_coming_back_revokes_the_grant() {
        let conn = test_db();
        let uid = create_user(&conn, "user", "pass", Role::User).unwrap();
        let grant = OAuthGrant {
            id: "g1".into(),
            client_name: "Claude".into(),
        };
        store_grant_token(&conn, "first", uid, later(), Some(&grant)).unwrap();
        let spent = consume_refresh_token(&conn, "first").unwrap().unwrap();
        assert_eq!(spent.grant.as_ref(), Some(&grant));
        store_grant_token(&conn, "second", uid, later(), Some(&grant)).unwrap();
        // An app session, spent the same way, is not a grant.
        store_refresh_token(&conn, "app", uid, later()).unwrap();
        consume_refresh_token(&conn, "app").unwrap().unwrap();

        // Within the grace: a retry, nothing revoked.
        assert_eq!(revoke_replayed_grant(&conn, "first", 30).unwrap(), 0);
        assert_eq!(revoke_replayed_grant(&conn, "app", -1).unwrap(), 0);
        // Spent and kept, so cleanup leaves it to be recognised.
        cleanup_expired_tokens(&conn).unwrap();
        assert_eq!(revoke_replayed_grant(&conn, "first", -1).unwrap(), 1);
        assert!(get_valid_refresh_token(&conn, "second").unwrap().is_none());
    }

    #[test]
    fn cleanup_removes_expired_and_revoked() {
        let conn = test_db();
        let uid = create_user(&conn, "user", "pass", Role::User).unwrap();
        store_refresh_token(&conn, "active", uid, later()).unwrap();
        store_refresh_token(&conn, "expired", uid, 0).unwrap();
        store_refresh_token(&conn, "revoked", uid, later()).unwrap();
        revoke_refresh_token(&conn, "revoked").unwrap();
        assert_eq!(cleanup_expired_tokens(&conn).unwrap(), 2);
        assert!(get_valid_refresh_token(&conn, "active").unwrap().is_some());
    }
}

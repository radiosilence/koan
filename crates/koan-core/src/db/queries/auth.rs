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
    /// The OAuth grant the token descends from, and the client it was granted
    /// to; `None` for app and web sessions.
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

// ---------------------------------------------------------------------------
// Whose data
// ---------------------------------------------------------------------------

/// The user a caller with no account acts as: the macOS app, the TUI, a server
/// with auth disabled, the Subsonic shared secret.
///
/// Favourites, playlists and play history are per user. An install with no
/// admin account keeps them under this id; once there is one, the first admin
/// owns them and this id [resolves](resolve_user) to theirs, so a single-user
/// server and a local library behave the same.
pub const LOCAL_USER: i64 = 0;

/// The first admin account, which answers for [`LOCAL_USER`].
pub fn first_admin(conn: &Connection) -> Result<Option<i64>, rusqlite::Error> {
    conn.prepare_cached("SELECT MIN(id) FROM users WHERE role = 'admin'")?
        .query_row([], |r| r.get(0))
}

/// The id whose rows `user` reads and writes: `user` itself for an account,
/// the first admin (or [`LOCAL_USER`] while there is none) for the implicit user.
pub fn resolve_user(conn: &Connection, user: i64) -> Result<i64, rusqlite::Error> {
    if user != LOCAL_USER {
        return Ok(user);
    }
    Ok(first_admin(conn)?.unwrap_or(LOCAL_USER))
}

/// Whether `user` is the one [`LOCAL_USER`] resolves to: whose favourites and
/// playlists this koan syncs with an upstream server.
pub fn is_local_user(conn: &Connection, user: i64) -> Result<bool, rusqlite::Error> {
    Ok(resolve_user(conn, user)? == resolve_user(conn, LOCAL_USER)?)
}

/// Hand the implicit user's rows to the first admin, once there is one.
///
/// Where a row would duplicate one the admin already has, theirs is kept.
pub fn adopt_local_rows(conn: &Connection) -> Result<(), rusqlite::Error> {
    let Some(admin) = first_admin(conn)? else {
        return Ok(());
    };
    for table in [
        "favourites",
        "favourite_albums",
        "favourite_artists",
        "track_ratings",
        "album_ratings",
        "artist_ratings",
        "play_history",
        "playlists",
        "shares",
    ] {
        // Runs on every open: a read, so it takes no write lock when there is
        // nothing to hand over.
        let pending: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE user_id = ?1)"),
            params![LOCAL_USER],
            |r| r.get(0),
        )?;
        if !pending {
            continue;
        }
        conn.execute(
            &format!("UPDATE OR IGNORE {table} SET user_id = ?1 WHERE user_id = ?2"),
            params![admin, LOCAL_USER],
        )?;
        conn.execute(
            &format!("DELETE FROM {table} WHERE user_id = ?1"),
            params![LOCAL_USER],
        )?;
    }
    Ok(())
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
    let id = conn.last_insert_rowid();
    adopt_local_rows(conn)?;
    Ok(id)
}

/// Get a user by username.
pub fn get_user_by_username(
    conn: &Connection,
    username: &str,
) -> Result<Option<UserRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
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

/// Update a user's password. Revokes all their refresh tokens and API keys: a
/// reset is how an admin shuts out whoever else had the password, and a key
/// made with it would otherwise outlast it.
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
            super::api_keys::revoke_user_api_keys(conn, user.id)?;
            super::app_passwords::revoke_user_app_passwords(conn, user.id)?;
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
    adopt_local_rows(conn)?;
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

/// Store a refresh token. Only `sha256(token)` is persisted — the raw token is
/// a bearer credential and read access to the database must not yield one.
pub fn store_refresh_token(
    conn: &Connection,
    token_id: &str,
    user_id: i64,
    expires_at: i64,
) -> Result<(), rusqlite::Error> {
    store_grant_token(conn, token_id, user_id, expires_at, None)
}

/// Store a refresh token belonging to an OAuth grant, or to none.
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
            auth::sha256_hex(token_id),
            user_id,
            expires_at,
            grant.map(|g| &g.id),
            grant.map(|g| &g.client_name),
        ],
    )?;
    Ok(())
}

/// Look up a refresh token. Returns None if not found, expired, or revoked.
pub fn get_valid_refresh_token(
    conn: &Connection,
    token_id: &str,
) -> Result<Option<RefreshTokenRow>, rusqlite::Error> {
    let now = auth::now_unix() as i64;
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {TOKEN_COLUMNS} FROM refresh_tokens
         WHERE id = ?1 AND revoked = 0 AND expires_at > ?2"
    ))?;
    let mut rows = stmt.query_map(params![auth::sha256_hex(token_id), now], token_row)?;
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
    let mut stmt = conn.prepare(&format!(
        "UPDATE refresh_tokens SET revoked = 1, used_at = ?2
         WHERE id = ?1 AND revoked = 0 AND expires_at > ?2
         RETURNING {TOKEN_COLUMNS}"
    ))?;
    let mut rows = stmt.query_map(params![auth::sha256_hex(token_id), now], token_row)?;
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
        params![auth::sha256_hex(token_id)],
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

/// A refresh token of an OAuth grant spent once already, more than `grace_secs`
/// ago, is a copy in someone else's hands: revoke the whole grant, so whichever
/// side refreshed first loses it too. The grace covers a client retrying a
/// refresh whose answer it never received. Returns how many tokens were revoked.
///
/// OAuth grants only: browser tabs and app tasks share one session and may
/// race a refresh, which is not theft.
pub fn revoke_replayed_grant(
    conn: &Connection,
    token_id: &str,
    grace_secs: i64,
) -> Result<usize, rusqlite::Error> {
    let cutoff = auth::now_unix() as i64 - grace_secs;
    conn.execute(
        "UPDATE refresh_tokens SET revoked = 1
         WHERE revoked = 0 AND grant_id = (
           SELECT grant_id FROM refresh_tokens
           WHERE id = ?1 AND revoked = 1 AND grant_id IS NOT NULL AND used_at < ?2)",
        params![auth::sha256_hex(token_id), cutoff],
    )
}

/// Revoke every refresh token of an OAuth grant.
pub fn revoke_grant(conn: &Connection, grant_id: &str) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE refresh_tokens SET revoked = 1 WHERE grant_id = ?1 AND revoked = 0",
        params![grant_id],
    )
}

/// Clean up expired/revoked refresh tokens (housekeeping). A spent token of an
/// OAuth grant stays until it would have expired, so `revoke_replayed_grant`
/// can still recognise it.
pub fn cleanup_expired_tokens(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let now = auth::now_unix() as i64;
    let count = conn.execute(
        "DELETE FROM refresh_tokens
         WHERE expires_at <= ?1 OR (revoked = 1 AND (grant_id IS NULL OR used_at IS NULL))",
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
    fn a_spent_grant_token_coming_back_revokes_the_grant() {
        let (db, _tmp) = test_db();
        let uid = create_user(&db.conn, "user", "pass", Role::User).unwrap();
        let future = auth::now_unix() as i64 + 86400;
        let grant = OAuthGrant {
            id: "g1".into(),
            client_name: "Claude".into(),
        };
        store_grant_token(&db.conn, "first", uid, future, Some(&grant)).unwrap();
        let spent = consume_refresh_token(&db.conn, "first").unwrap().unwrap();
        assert_eq!(spent.grant.as_ref(), Some(&grant));
        store_grant_token(&db.conn, "second", uid, future, Some(&grant)).unwrap();
        // An app session, spent the same way, is not a grant.
        store_refresh_token(&db.conn, "app", uid, future).unwrap();
        consume_refresh_token(&db.conn, "app").unwrap().unwrap();

        // Within the grace: a retry, nothing revoked.
        assert_eq!(revoke_replayed_grant(&db.conn, "first", 30).unwrap(), 0);
        assert_eq!(revoke_replayed_grant(&db.conn, "app", -1).unwrap(), 0);
        // Spent and kept, so cleanup leaves it to be recognised.
        cleanup_expired_tokens(&db.conn).unwrap();
        assert_eq!(revoke_replayed_grant(&db.conn, "first", -1).unwrap(), 1);
        assert!(
            get_valid_refresh_token(&db.conn, "second")
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

    // -- Per-user data ------------------------------------------------------

    use crate::db::queries::{self, sample_meta, upsert_track};

    fn count(db: &Database, sql: &str) -> i64 {
        db.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn two_users_star_the_same_track_independently() {
        let (db, _tmp) = test_db();
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();
        let mate = create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let track = upsert_track(&db.conn, &sample_meta("Scatology", "Coil", "Scatology")).unwrap();
        let album = queries::get_track_row(&db.conn, track)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();

        queries::add_favourite(&db.conn, admin, track).unwrap();
        queries::add_favourite(&db.conn, mate, track).unwrap();
        queries::remove_favourite(&db.conn, admin, track).unwrap();

        assert!(
            queries::load_favourites(&db.conn, admin)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::load_favourites(&db.conn, mate)
                .unwrap()
                .contains(&track)
        );
        assert!(queries::toggle_favourite_album(&db.conn, mate, album).unwrap());
        assert!(queries::toggle_favourite_album(&db.conn, admin, album).unwrap());
        assert_eq!(count(&db, "SELECT COUNT(*) FROM favourite_albums"), 2);
    }

    #[test]
    fn the_local_user_is_the_first_admin_once_there_is_one() {
        let (db, _tmp) = test_db();
        let track = upsert_track(&db.conn, &sample_meta("T", "A", "B")).unwrap();
        queries::add_favourite(&db.conn, LOCAL_USER, track).unwrap();
        queries::record_play(&db.conn, LOCAL_USER, track, None).unwrap();
        let list = queries::create_playlist(&db.conn, LOCAL_USER, "Mine", None).unwrap();
        assert_eq!(resolve_user(&db.conn, LOCAL_USER).unwrap(), LOCAL_USER);

        create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        assert_eq!(resolve_user(&db.conn, LOCAL_USER).unwrap(), LOCAL_USER);
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();

        assert_eq!(resolve_user(&db.conn, LOCAL_USER).unwrap(), admin);
        assert!(
            queries::load_favourites(&db.conn, admin)
                .unwrap()
                .contains(&track)
        );
        assert_eq!(queries::play_count(&db.conn, admin, track).unwrap(), 1);
        assert_eq!(
            queries::get_playlist(&db.conn, list)
                .unwrap()
                .unwrap()
                .user_id,
            admin
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM favourites WHERE user_id = 0"),
            0
        );
    }

    #[test]
    fn playlists_are_the_owners_plus_everyones_public_ones() {
        let (db, _tmp) = test_db();
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();
        let mate = create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let private = queries::create_playlist(&db.conn, admin, "Private", None).unwrap();
        let public = queries::create_playlist(&db.conn, admin, "Public", None).unwrap();
        db.conn
            .execute("UPDATE playlists SET public = 1 WHERE id = ?1", [public])
            .unwrap();
        let own = queries::create_playlist(&db.conn, mate, "Mate's", None).unwrap();

        let ids = |user| -> Vec<i64> {
            let mut ids: Vec<i64> = queries::list_playlists(&db.conn, user)
                .unwrap()
                .into_iter()
                .map(|p| p.id)
                .collect();
            ids.sort_unstable();
            ids
        };
        assert_eq!(ids(mate), vec![public, own]);
        assert_eq!(ids(admin), vec![private, public]);
        // The implicit user is the first admin.
        assert_eq!(ids(LOCAL_USER), vec![private, public]);

        let row = queries::get_playlist(&db.conn, public).unwrap().unwrap();
        assert!(row.readable_by(mate) && !row.editable_by(mate));
        assert_eq!(row.owner.as_deref(), Some("owner"));
        let row = queries::get_playlist(&db.conn, private).unwrap().unwrap();
        assert!(!row.readable_by(mate));
    }

    #[test]
    fn deleting_an_account_takes_its_data_with_it() {
        let (db, _tmp) = test_db();
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();
        let mate = create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let track = upsert_track(&db.conn, &sample_meta("T", "A", "B")).unwrap();
        let row = queries::get_track_row(&db.conn, track).unwrap().unwrap();
        for user in [admin, mate] {
            queries::add_favourite(&db.conn, user, track).unwrap();
            queries::set_favourite_album(&db.conn, user, row.album_id.unwrap(), true).unwrap();
            queries::set_favourite_artist(&db.conn, user, row.artist_id.unwrap(), true).unwrap();
            for (kind, id) in [
                (queries::RatingKind::Track, track),
                (queries::RatingKind::Album, row.album_id.unwrap()),
                (queries::RatingKind::Artist, row.artist_id.unwrap()),
            ] {
                queries::set_rating(&db.conn, user, kind, id, 4).unwrap();
            }
            queries::record_play(&db.conn, user, track, None).unwrap();
            queries::create_playlist(&db.conn, user, "List", None).unwrap();
            queries::shares::create_share(
                &db.conn,
                user,
                queries::shares::Slice::TRACKS,
                &[track],
                None,
                0,
                None,
            )
            .unwrap();
        }

        assert!(delete_user(&db.conn, mate).unwrap());

        for table in [
            "favourites",
            "favourite_albums",
            "favourite_artists",
            "track_ratings",
            "album_ratings",
            "artist_ratings",
            "play_history",
            "playlists",
            "shares",
        ] {
            assert_eq!(
                count(
                    &db,
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_id = {mate}")
                ),
                0,
                "{table} kept the deleted account's rows"
            );
            assert_eq!(
                count(
                    &db,
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_id = {admin}")
                ),
                1,
                "{table} lost another account's rows"
            );
        }
    }
}
